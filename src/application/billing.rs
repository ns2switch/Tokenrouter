use crate::domain::entities::{PricingConfig, Tier};
use std::sync::Arc;
use tiktoken_rs::CoreBPE;

/// Pure billing logic: token counting, price selection, cost calculation.
/// Stateless except for the cached tokeniser (expensive to build).
#[derive(Clone)]
pub struct BillingService {
    bpe: Arc<CoreBPE>,
}

impl BillingService {
    pub fn new() -> anyhow::Result<Self> {
        let bpe = tiktoken_rs::cl100k_base().map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(Self { bpe: Arc::new(bpe) })
    }

    pub fn count_tokens(&self, text: &str) -> usize {
        self.bpe.encode_with_special_tokens(text).len()
    }

    pub fn effective_sell_price(&self, pricing: &PricingConfig, tier: &Tier) -> f64 {
        match tier {
            Tier::Bit => pricing.bit_price_per_1m,
            Tier::Node => pricing.node_price_per_1m,
            Tier::Cluster => pricing.cluster_price_per_1m,
        }
    }

    pub fn cost(&self, tokens: i64, price_per_1m: f64) -> f64 {
        (tokens as f64 / 1_000_000.0) * price_per_1m
    }

    pub fn provider_cost(
        &self,
        input_tokens: i64,
        output_tokens: i64,
        pricing: &PricingConfig,
    ) -> f64 {
        self.cost(input_tokens, pricing.provider_cost_input_per_1m)
            + self.cost(output_tokens, pricing.provider_cost_output_per_1m)
    }

    /// Returns true if the key has a credit limit and has already spent it all.
    pub fn credit_exhausted(&self, key: &crate::domain::entities::ApiKey) -> bool {
        match key.credit_limit {
            Some(limit) => key.balance_accumulated >= limit,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::entities::{PricingConfig, Tier};
    use chrono::Utc;

    fn pricing(bit: f64, node: f64, cluster: f64) -> PricingConfig {
        PricingConfig {
            model_name: "m".into(),
            provider_id: "default".to_string(),
            provider_cost_input_per_1m: 0.5,
            provider_cost_output_per_1m: 1.5,
            bit_price_per_1m: bit,
            node_price_per_1m: node,
            cluster_price_per_1m: cluster,
            min_tier: Tier::Bit,
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn provider_cost_separate_io() {
        let svc = BillingService::new().unwrap();
        let p = pricing(1.0, 2.0, 3.0);
        // 1000 input tokens at $0.50/1M + 500 output tokens at $1.50/1M
        let cost = svc.provider_cost(1000, 500, &p);
        assert!((cost - (0.0005 + 0.00075)).abs() < 0.0001);
    }

    #[test]
    fn effective_price_selects_correct_tier() {
        let svc = BillingService::new().unwrap();
        let p = pricing(1.0, 2.0, 3.0);
        assert_eq!(svc.effective_sell_price(&p, &Tier::Bit), 1.0);
        assert_eq!(svc.effective_sell_price(&p, &Tier::Node), 2.0);
        assert_eq!(svc.effective_sell_price(&p, &Tier::Cluster), 3.0);
    }

    #[test]
    fn cost_calculation() {
        let svc = BillingService::new().unwrap();
        // 1M tokens at $1.00/1M = $1.00
        assert!((svc.cost(1_000_000, 1.0) - 1.0).abs() < f64::EPSILON);
        // 500k tokens at $2.00/1M = $1.00
        assert!((svc.cost(500_000, 2.0) - 1.0).abs() < f64::EPSILON);
        // 0 tokens = $0
        assert_eq!(svc.cost(0, 10.0), 0.0);
    }

    #[test]
    fn count_tokens_nonempty() {
        let svc = BillingService::new().unwrap();
        assert!(svc.count_tokens("hello world") > 0);
    }

    #[test]
    fn count_tokens_empty() {
        let svc = BillingService::new().unwrap();
        assert_eq!(svc.count_tokens(""), 0);
    }
}
