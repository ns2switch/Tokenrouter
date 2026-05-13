use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::entities::{
    ApiKey, DashboardMetrics, IdempotencyRecord, PricingConfig, Provider, RecentTransactionsPage,
    Tier,
};

/// Snapshot of in-memory runtime metrics persisted to DynamoDB
/// and merged back on restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeMetricsSnapshot {
    pub requests_total: u64,
    pub status_counts: Vec<(u16, u64)>,
    pub model_counts: Vec<(String, u64)>,
    pub ttft_buckets: Vec<(u64, u64)>,
    pub ttft_sum_ms: u64,
    pub duration_buckets: Vec<(u64, u64)>,
    pub duration_sum_ms: u64,
    pub throughput_tps_x100: u64,
}

#[async_trait]
pub trait ApiKeyPort: Send + Sync {
    async fn find_api_key_by_hash(&self, hash: &str) -> anyhow::Result<Option<ApiKey>>;
    async fn find_api_key_by_id(&self, id: &str) -> anyhow::Result<Option<ApiKey>>;
    async fn list_api_keys(&self) -> anyhow::Result<Vec<ApiKey>>;
    async fn create_api_key(
        &self,
        key_hash: &str,
        tier: &Tier,
        credit_limit: Option<f64>,
    ) -> anyhow::Result<ApiKey>;
    async fn set_key_active(&self, key_hash: &str, active: bool) -> anyhow::Result<()>;
    async fn update_key_credit_limit(
        &self,
        key_hash: &str,
        credit_limit: Option<f64>,
    ) -> anyhow::Result<()>;
    async fn delete_api_key(&self, key_hash: &str) -> anyhow::Result<()>;
}

#[async_trait]
#[allow(dead_code)]
pub trait ProviderPort: Send + Sync {
    async fn all_providers(&self) -> anyhow::Result<Vec<Provider>>;
    async fn provider_by_id(&self, id: &str) -> anyhow::Result<Option<Provider>>;
    #[allow(clippy::too_many_arguments)]
    async fn upsert_provider(
        &self,
        id: &str,
        name: &str,
        base_url: &str,
        api_key: &str,
        quantization: &str,
        context_length: i64,
        max_output_length: i64,
        datacenter_country: &str,
    ) -> anyhow::Result<()>;
    async fn delete_provider(&self, id: &str) -> anyhow::Result<()>;
}

#[async_trait]
pub trait PricingPort: Send + Sync {
    async fn pricing_for_model(&self, model: &str) -> anyhow::Result<Option<PricingConfig>>;
    async fn all_pricing(&self) -> anyhow::Result<Vec<PricingConfig>>;
    #[allow(clippy::too_many_arguments)]
    async fn upsert_pricing(
        &self,
        model_name: &str,
        provider_id: &str,
        provider_cost_input_per_1m: f64,
        provider_cost_output_per_1m: f64,
        bit_price_per_1m: f64,
        node_price_per_1m: f64,
        cluster_price_per_1m: f64,
        min_tier: &super::entities::Tier,
    ) -> anyhow::Result<()>;
    async fn delete_pricing(&self, model_name: &str) -> anyhow::Result<()>;
}

#[async_trait]
pub trait TransactionPort: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    async fn record_transaction(
        &self,
        api_key: &ApiKey,
        model_name: &str,
        input_tokens: i64,
        output_tokens: i64,
        cost_basis: f64,
        revenue_generated: f64,
        idempotency_key: &str,
        request_hash: &str,
        idempotency_ttl_seconds: i64,
        response_body: &str,
    ) -> anyhow::Result<()>;

    async fn store_idempotency_record(
        &self,
        idempotency_key: &str,
        request_hash: &str,
        ttl_seconds: i64,
        response_body: &str,
    ) -> anyhow::Result<()>;

    async fn get_idempotency_record(
        &self,
        idempotency_key: &str,
    ) -> anyhow::Result<Option<IdempotencyRecord>>;

    async fn recent_transactions_page(
        &self,
        limit: i64,
        cursor: Option<String>,
    ) -> anyhow::Result<RecentTransactionsPage>;

    async fn recent_transactions_page_backward(
        &self,
        limit: i64,
        before_sk: &str,
    ) -> anyhow::Result<RecentTransactionsPage>;
}

#[async_trait]
pub trait MetricsPort: Send + Sync {
    async fn dashboard_metrics(&self) -> anyhow::Result<DashboardMetrics>;
}

#[async_trait]
pub trait RuntimeMetricsPort: Send + Sync {
    async fn save_runtime_metrics(&self, snapshot: &RuntimeMetricsSnapshot) -> anyhow::Result<()>;
    async fn load_runtime_metrics(&self) -> anyhow::Result<Option<RuntimeMetricsSnapshot>>;
}

#[async_trait]
pub trait HealthPort: Send + Sync {
    async fn check_idempotency_ttl_enabled(&self) -> anyhow::Result<bool>;
    async fn write_probe(&self) -> anyhow::Result<()>;
    async fn query_dead_letters(&self, limit: i64) -> anyhow::Result<Vec<DeadLetterEntry>>;
}

/// A failed transaction written to the dead-letter table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadLetterEntry {
    pub id: String,
    pub idempotency_key: String,
    pub api_key_id: String,
    pub model_name: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_basis: f64,
    pub revenue_generated: f64,
    pub error: String,
    pub timestamp: String,
}

/// Combined repository trait used by AppState.
/// Any type implementing all individual ports automatically implements this.
pub trait Repository:
    ApiKeyPort
    + ProviderPort
    + PricingPort
    + TransactionPort
    + MetricsPort
    + RuntimeMetricsPort
    + HealthPort
    + Send
    + Sync
{
}

impl<T> Repository for T where
    T: ApiKeyPort
        + ProviderPort
        + PricingPort
        + TransactionPort
        + MetricsPort
        + RuntimeMetricsPort
        + HealthPort
        + Send
        + Sync
{
}
