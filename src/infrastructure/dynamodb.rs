use anyhow::anyhow;
use async_trait::async_trait;
use aws_sdk_dynamodb::types::{AttributeValue, Put, TransactWriteItem, Update};
use aws_sdk_dynamodb::Client as DynamoDbClient;
use chrono::{DateTime, Utc};
use std::cmp::Reverse;
use std::collections::HashMap;
use uuid::Uuid;

use crate::domain::entities::{
    ApiKey, DashboardMetrics, IdempotencyRecord, PricingConfig, Provider, RecentTransactionsPage,
    Tier, Transaction,
};
use crate::domain::ports::{
    ApiKeyPort, DeadLetterEntry, HealthPort, MetricsPort, PricingPort, ProviderPort,
    RuntimeMetricsPort, RuntimeMetricsSnapshot, TransactionPort,
};

#[derive(Clone)]
pub struct DynamoDbStore {
    client: DynamoDbClient,
    api_keys_table: String,
    providers_table: String,
    pricing_table: String,
    transactions_table: String,
    transactions_timeline_table: String,
    idempotency_table: String,
    metrics_table: String,
    dead_letter_table: Option<String>,
    timeline_shard_count: u16,
}

impl DynamoDbStore {
    #[allow(clippy::too_many_arguments)]
    pub async fn connect(
        api_keys_table: String,
        providers_table: String,
        pricing_table: String,
        transactions_table: String,
        transactions_timeline_table: String,
        idempotency_table: String,
        metrics_table: String,
        dead_letter_table: Option<String>,
        timeline_shard_count: u16,
    ) -> anyhow::Result<Self> {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        let client = DynamoDbClient::new(&config);
        let store = Self {
            client,
            api_keys_table,
            providers_table,
            pricing_table,
            transactions_table,
            transactions_timeline_table,
            idempotency_table,
            metrics_table,
            dead_letter_table,
            timeline_shard_count: timeline_shard_count.max(1),
        };
        store.validate_tables().await?;
        Ok(store)
    }

    async fn validate_tables(&self) -> anyhow::Result<()> {
        let tables = [
            &self.api_keys_table,
            &self.providers_table,
            &self.pricing_table,
            &self.transactions_table,
            &self.transactions_timeline_table,
            &self.idempotency_table,
            &self.metrics_table,
        ];
        let mut missing = Vec::new();
        for table in &tables {
            if let Err(e) = self.client.describe_table().table_name(*table).send().await {
                tracing::error!("table {table} not found: {e}");
                missing.push(table.as_str().to_string());
            }
        }
        if !missing.is_empty() {
            return Err(anyhow!(
                "missing DynamoDB tables: {}. Run bootstrap script first.",
                missing.join(", ")
            ));
        }
        Ok(())
    }

    pub async fn init_schema(&self) -> anyhow::Result<()> {
        Ok(())
    }

    fn timeline_pk(&self, api_key_id: &str) -> String {
        let mut h: u64 = 0;
        for b in api_key_id.bytes() {
            h = h.wrapping_mul(31).wrapping_add(b as u64);
        }
        let shard = (h % self.timeline_shard_count as u64) + 1;
        format!("shard-{shard}")
    }

    fn all_timeline_pks(&self) -> Vec<String> {
        if self.timeline_shard_count <= 1 {
            return vec!["global".to_string()];
        }
        (1..=self.timeline_shard_count)
            .map(|s| format!("shard-{s}"))
            .collect()
    }
}

#[async_trait]
impl ApiKeyPort for DynamoDbStore {
    async fn find_api_key_by_hash(&self, hash: &str) -> anyhow::Result<Option<ApiKey>> {
        let mut key = HashMap::new();
        key.insert("key_hash".to_string(), av_s(hash));

        let out = self
            .client
            .get_item()
            .table_name(&self.api_keys_table)
            .set_key(Some(key))
            .send()
            .await?;

        out.item()
            .map(parse_api_key)
            .transpose()
            .map(|opt| opt.filter(|k| k.active))
    }

    async fn list_api_keys(&self) -> anyhow::Result<Vec<ApiKey>> {
        let mut items = Vec::new();
        let mut start_key: Option<HashMap<String, AttributeValue>> = None;

        loop {
            let mut req = self.client.scan().table_name(&self.api_keys_table);
            if let Some(ref sk) = start_key {
                req = req.set_exclusive_start_key(Some(sk.clone()));
            }
            let out = req.send().await?;

            for item in out.items() {
                if let Ok(key) = parse_api_key(item) {
                    items.push(key);
                }
            }
            start_key = out.last_evaluated_key().cloned();
            if start_key.is_none() {
                break;
            }
        }

        Ok(items)
    }

    async fn create_api_key(
        &self,
        key_hash: &str,
        tier: &Tier,
        credit_limit: Option<f64>,
    ) -> anyhow::Result<ApiKey> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let tier_str = match tier {
            Tier::Bit => "bit",
            Tier::Node => "node",
            Tier::Cluster => "cluster",
        };

        let mut item = HashMap::new();
        item.insert("key_hash".to_string(), av_s(key_hash));
        item.insert("id".to_string(), av_s(&id));
        item.insert("tier".to_string(), av_s(tier_str));
        item.insert("balance_accumulated".to_string(), av_n("0"));
        item.insert("active".to_string(), AttributeValue::Bool(true));
        item.insert("created_at".to_string(), av_s(&now));
        if let Some(limit) = credit_limit {
            item.insert("credit_limit".to_string(), av_n(&limit.to_string()));
        }

        self.client
            .put_item()
            .table_name(&self.api_keys_table)
            .set_item(Some(item))
            .send()
            .await?;

        Ok(ApiKey {
            id,
            key_hash: key_hash.to_string(),
            tier: tier.clone(),
            balance_accumulated: 0.0,
            credit_limit,
            active: true,
            created_at: Utc::now(),
        })
    }

    async fn set_key_active(&self, key_hash: &str, active: bool) -> anyhow::Result<()> {
        let mut key = HashMap::new();
        key.insert("key_hash".to_string(), av_s(key_hash));

        let mut expr_vals = HashMap::new();
        expr_vals.insert(":active".to_string(), AttributeValue::Bool(active));

        self.client
            .update_item()
            .table_name(&self.api_keys_table)
            .set_key(Some(key))
            .update_expression("SET active = :active")
            .set_expression_attribute_values(Some(expr_vals))
            .send()
            .await?;
        Ok(())
    }

    async fn update_key_credit_limit(
        &self,
        key_hash: &str,
        credit_limit: Option<f64>,
    ) -> anyhow::Result<()> {
        let mut key = HashMap::new();
        key.insert("key_hash".to_string(), av_s(key_hash));

        if let Some(limit) = credit_limit {
            let mut expr_vals = HashMap::new();
            expr_vals.insert(":limit".to_string(), av_n(&limit.to_string()));
            self.client
                .update_item()
                .table_name(&self.api_keys_table)
                .set_key(Some(key))
                .update_expression("SET credit_limit = :limit")
                .set_expression_attribute_values(Some(expr_vals))
                .send()
                .await?;
        } else {
            self.client
                .update_item()
                .table_name(&self.api_keys_table)
                .set_key(Some(key))
                .update_expression("REMOVE credit_limit")
                .send()
                .await?;
        }
        Ok(())
    }

    async fn delete_api_key(&self, key_hash: &str) -> anyhow::Result<()> {
        let mut key = HashMap::new();
        key.insert("key_hash".to_string(), av_s(key_hash));
        self.client
            .delete_item()
            .table_name(&self.api_keys_table)
            .set_key(Some(key))
            .send()
            .await?;
        Ok(())
    }
}

#[async_trait]
impl PricingPort for DynamoDbStore {
    async fn pricing_for_model(&self, model: &str) -> anyhow::Result<Option<PricingConfig>> {
        let mut key = HashMap::new();
        key.insert("model_name".to_string(), av_s(model));

        let out = self
            .client
            .get_item()
            .table_name(&self.pricing_table)
            .set_key(Some(key))
            .send()
            .await?;

        out.item().map(parse_pricing).transpose()
    }

    async fn all_pricing(&self) -> anyhow::Result<Vec<PricingConfig>> {
        let mut items = Vec::new();
        let mut start_key: Option<HashMap<String, AttributeValue>> = None;

        loop {
            let mut req = self.client.scan().table_name(&self.pricing_table);
            if let Some(ref sk) = start_key {
                req = req.set_exclusive_start_key(Some(sk.clone()));
            }
            let out = req.send().await?;

            for item in out.items() {
                items.push(parse_pricing(item)?);
            }
            start_key = out.last_evaluated_key().cloned();
            if start_key.is_none() {
                break;
            }
        }

        items.sort_by(|a, b| a.model_name.cmp(&b.model_name));
        Ok(items)
    }

    async fn upsert_pricing(
        &self,
        model_name: &str,
        provider_id: &str,
        provider_cost_input_per_1m: f64,
        provider_cost_output_per_1m: f64,
        bit_price_per_1m: f64,
        node_price_per_1m: f64,
        cluster_price_per_1m: f64,
        min_tier: &Tier,
    ) -> anyhow::Result<()> {
        let mut item = HashMap::new();
        item.insert("model_name".to_string(), av_s(model_name));
        item.insert("provider_id".to_string(), av_s(provider_id));
        item.insert(
            "provider_cost_input_per_1m".to_string(),
            av_n(&provider_cost_input_per_1m.to_string()),
        );
        item.insert(
            "provider_cost_output_per_1m".to_string(),
            av_n(&provider_cost_output_per_1m.to_string()),
        );
        item.insert(
            "bit_price_per_1m".to_string(),
            av_n(&bit_price_per_1m.to_string()),
        );
        item.insert(
            "node_price_per_1m".to_string(),
            av_n(&node_price_per_1m.to_string()),
        );
        item.insert(
            "cluster_price_per_1m".to_string(),
            av_n(&cluster_price_per_1m.to_string()),
        );
        item.insert(
            "min_tier".to_string(),
            av_s(match min_tier {
                Tier::Bit => "bit",
                Tier::Node => "node",
                Tier::Cluster => "cluster",
            }),
        );
        item.insert(
            "updated_at".to_string(),
            av_s(&Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        );

        self.client
            .put_item()
            .table_name(&self.pricing_table)
            .set_item(Some(item))
            .send()
            .await?;
        Ok(())
    }

    async fn delete_pricing(&self, model_name: &str) -> anyhow::Result<()> {
        let mut key = HashMap::new();
        key.insert("model_name".to_string(), av_s(model_name));
        self.client
            .delete_item()
            .table_name(&self.pricing_table)
            .set_key(Some(key))
            .send()
            .await?;
        Ok(())
    }
}

#[async_trait]
impl ProviderPort for DynamoDbStore {
    async fn all_providers(&self) -> anyhow::Result<Vec<Provider>> {
        let mut items = Vec::new();
        let mut start_key: Option<HashMap<String, AttributeValue>> = None;

        loop {
            let mut req = self.client.scan().table_name(&self.providers_table);
            if let Some(ref sk) = start_key {
                req = req.set_exclusive_start_key(Some(sk.clone()));
            }
            let out = req.send().await?;

            for item in out.items() {
                items.push(parse_provider(item)?);
            }
            start_key = out.last_evaluated_key().cloned();
            if start_key.is_none() {
                break;
            }
        }

        items.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(items)
    }

    async fn provider_by_id(&self, id: &str) -> anyhow::Result<Option<Provider>> {
        let mut key = HashMap::new();
        key.insert("id".to_string(), av_s(id));

        let out = self
            .client
            .get_item()
            .table_name(&self.providers_table)
            .set_key(Some(key))
            .send()
            .await?;

        out.item().map(parse_provider).transpose()
    }

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
    ) -> anyhow::Result<()> {
        let mut item = HashMap::new();
        item.insert("id".to_string(), av_s(id));
        item.insert("name".to_string(), av_s(name));
        item.insert("base_url".to_string(), av_s(base_url));
        item.insert("api_key".to_string(), av_s(api_key));
        item.insert("quantization".to_string(), av_s(quantization));
        item.insert(
            "context_length".to_string(),
            av_n(&context_length.to_string()),
        );
        item.insert(
            "max_output_length".to_string(),
            av_n(&max_output_length.to_string()),
        );
        item.insert("datacenter_country".to_string(), av_s(datacenter_country));

        let created_at = match self.provider_by_id(id).await? {
            Some(existing) => existing
                .created_at
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            None => Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        };
        item.insert("created_at".to_string(), av_s(&created_at));

        self.client
            .put_item()
            .table_name(&self.providers_table)
            .set_item(Some(item))
            .send()
            .await?;
        Ok(())
    }

    async fn delete_provider(&self, id: &str) -> anyhow::Result<()> {
        let mut key = HashMap::new();
        key.insert("id".to_string(), av_s(id));
        self.client
            .delete_item()
            .table_name(&self.providers_table)
            .set_key(Some(key))
            .send()
            .await?;
        Ok(())
    }
}

#[async_trait]
impl TransactionPort for DynamoDbStore {
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
    ) -> anyhow::Result<()> {
        let tx_id = Uuid::new_v4().to_string();
        let now_dt = Utc::now();
        let now = now_dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let expires_at = now_dt.timestamp() + idempotency_ttl_seconds.max(60);

        let mut idem_item = HashMap::new();
        idem_item.insert("idempotency_key".to_string(), av_s(idempotency_key));
        idem_item.insert("transaction_id".to_string(), av_s(&tx_id));
        idem_item.insert("created_at".to_string(), av_s(&now));
        idem_item.insert("request_hash".to_string(), av_s(request_hash));
        idem_item.insert("expires_at".to_string(), av_n(&expires_at.to_string()));
        if !response_body.is_empty() {
            idem_item.insert("response_body".to_string(), av_s(response_body));
        }
        let put_idem = TransactWriteItem::builder()
            .put(
                Put::builder()
                    .table_name(&self.idempotency_table)
                    .set_item(Some(idem_item))
                    .condition_expression("attribute_not_exists(idempotency_key)")
                    .build()?,
            )
            .build();

        let mut tx_item = HashMap::new();
        tx_item.insert("id".to_string(), av_s(&tx_id));
        tx_item.insert("idempotency_key".to_string(), av_s(idempotency_key));
        tx_item.insert("api_key_id".to_string(), av_s(&api_key.id));
        tx_item.insert("model_name".to_string(), av_s(model_name));
        tx_item.insert("input_tokens".to_string(), av_n(&input_tokens.to_string()));
        tx_item.insert(
            "output_tokens".to_string(),
            av_n(&output_tokens.to_string()),
        );
        tx_item.insert("cost_basis".to_string(), av_n(&cost_basis.to_string()));
        tx_item.insert(
            "revenue_generated".to_string(),
            av_n(&revenue_generated.to_string()),
        );
        tx_item.insert("timestamp".to_string(), av_s(&now));
        let put_tx = TransactWriteItem::builder()
            .put(
                Put::builder()
                    .table_name(&self.transactions_table)
                    .set_item(Some(tx_item))
                    .build()?,
            )
            .build();

        let tl_pk = self.timeline_pk(&api_key.id);
        let mut tl_item = HashMap::new();
        tl_item.insert("timeline_pk".to_string(), av_s(&tl_pk));
        tl_item.insert(
            "timeline_sk".to_string(),
            av_s(&format!("{}#{}", now_dt.timestamp_millis(), tx_id)),
        );
        tl_item.insert("id".to_string(), av_s(&tx_id));
        tl_item.insert("idempotency_key".to_string(), av_s(idempotency_key));
        tl_item.insert("api_key_id".to_string(), av_s(&api_key.id));
        tl_item.insert("model_name".to_string(), av_s(model_name));
        tl_item.insert("input_tokens".to_string(), av_n(&input_tokens.to_string()));
        tl_item.insert(
            "output_tokens".to_string(),
            av_n(&output_tokens.to_string()),
        );
        tl_item.insert("cost_basis".to_string(), av_n(&cost_basis.to_string()));
        tl_item.insert(
            "revenue_generated".to_string(),
            av_n(&revenue_generated.to_string()),
        );
        tl_item.insert("timestamp".to_string(), av_s(&now));
        let put_timeline = TransactWriteItem::builder()
            .put(
                Put::builder()
                    .table_name(&self.transactions_timeline_table)
                    .set_item(Some(tl_item))
                    .build()?,
            )
            .build();

        let mut ak_key = HashMap::new();
        ak_key.insert("key_hash".to_string(), av_s(&api_key.key_hash));
        let mut ak_expr = HashMap::new();
        ak_expr.insert(":zero".to_string(), av_n("0"));
        ak_expr.insert(":rev".to_string(), av_n(&revenue_generated.to_string()));
        let update_api = TransactWriteItem::builder()
            .update(
                Update::builder()
                    .table_name(&self.api_keys_table)
                    .set_key(Some(ak_key))
                    .update_expression(
                        "SET balance_accumulated = if_not_exists(balance_accumulated, :zero) + :rev",
                    )
                    .set_expression_attribute_values(Some(ak_expr))
                    .build()?,
            )
            .build();

        let mut m_key = HashMap::new();
        m_key.insert("metric_id".to_string(), av_s("global"));
        let mut m_expr = HashMap::new();
        m_expr.insert(":z".to_string(), av_n("0"));
        m_expr.insert(":one".to_string(), av_n("1"));
        m_expr.insert(":in".to_string(), av_n(&input_tokens.to_string()));
        m_expr.insert(":out".to_string(), av_n(&output_tokens.to_string()));
        m_expr.insert(":cost".to_string(), av_n(&cost_basis.to_string()));
        m_expr.insert(":rev".to_string(), av_n(&revenue_generated.to_string()));
        m_expr.insert(":ts".to_string(), av_s(&now));
        let update_metrics = TransactWriteItem::builder()
            .update(
                Update::builder()
                    .table_name(&self.metrics_table)
                    .set_key(Some(m_key))
                    .update_expression(
                        "SET tx_count = if_not_exists(tx_count, :z) + :one, \
                         total_input_tokens = if_not_exists(total_input_tokens, :z) + :in, \
                         total_output_tokens = if_not_exists(total_output_tokens, :z) + :out, \
                         total_cost = if_not_exists(total_cost, :z) + :cost, \
                         total_revenue = if_not_exists(total_revenue, :z) + :rev, \
                         updated_at = :ts",
                    )
                    .set_expression_attribute_values(Some(m_expr))
                    .build()?,
            )
            .build();

        let result = self
            .client
            .transact_write_items()
            .set_transact_items(Some(vec![
                put_idem,
                put_tx,
                put_timeline,
                update_api,
                update_metrics,
            ]))
            .send()
            .await;

        if let Err(e) = &result {
            if let Some(ref dl_table) = self.dead_letter_table {
                let mut dl_item = HashMap::new();
                dl_item.insert("id".to_string(), av_s(&tx_id));
                dl_item.insert("idempotency_key".to_string(), av_s(idempotency_key));
                dl_item.insert("api_key_id".to_string(), av_s(&api_key.id));
                dl_item.insert("model_name".to_string(), av_s(model_name));
                dl_item.insert("input_tokens".to_string(), av_n(&input_tokens.to_string()));
                dl_item.insert(
                    "output_tokens".to_string(),
                    av_n(&output_tokens.to_string()),
                );
                dl_item.insert("cost_basis".to_string(), av_n(&cost_basis.to_string()));
                dl_item.insert(
                    "revenue_generated".to_string(),
                    av_n(&revenue_generated.to_string()),
                );
                dl_item.insert("request_hash".to_string(), av_s(request_hash));
                dl_item.insert("error".to_string(), av_s(&format!("{e}")));
                dl_item.insert("timestamp".to_string(), av_s(&now));
                let _ = self
                    .client
                    .put_item()
                    .table_name(dl_table)
                    .set_item(Some(dl_item))
                    .send()
                    .await;
            }
        }

        result?;
        Ok(())
    }

    async fn get_idempotency_record(
        &self,
        idempotency_key: &str,
    ) -> anyhow::Result<Option<IdempotencyRecord>> {
        let mut key = HashMap::new();
        key.insert("idempotency_key".to_string(), av_s(idempotency_key));

        let out = self
            .client
            .get_item()
            .table_name(&self.idempotency_table)
            .set_key(Some(key))
            .send()
            .await?;

        let Some(item) = out.item() else {
            return Ok(None);
        };
        Ok(Some(IdempotencyRecord {
            request_hash: attr_s(item, "request_hash").unwrap_or_default(),
            response_body: item
                .get("response_body")
                .and_then(|v| v.as_s().ok().cloned()),
        }))
    }

    async fn recent_transactions_page(
        &self,
        limit: i64,
        cursor: Option<String>,
    ) -> anyhow::Result<RecentTransactionsPage> {
        let pks = self.all_timeline_pks();
        let limit = limit.max(1);
        let per_shard_limit = ((limit as usize).div_ceil(pks.len())).max(1) as i32;
        let mut all_rows = Vec::new();
        let mut last_sk: Option<String> = None;

        let (cursor_shard, cursor_sk) = parse_cursor(cursor.as_deref());

        for pk in &pks {
            let shard_idx = pk
                .strip_prefix("shard-")
                .and_then(|s| s.parse::<u16>().ok());
            if let Some(ref target) = cursor_shard {
                if shard_idx != Some(*target) && shard_idx.is_some() {
                    continue;
                }
            }

            let mut eav = HashMap::new();
            eav.insert(":pk".to_string(), av_s(pk));
            let mut esk = None;
            if let Some(sk) = cursor_sk.as_deref().or(cursor.as_deref()) {
                if cursor_shard.is_none() || shard_idx == cursor_shard {
                    let mut m = HashMap::new();
                    m.insert("timeline_pk".to_string(), av_s(pk));
                    m.insert("timeline_sk".to_string(), av_s(sk));
                    esk = Some(m);
                }
            }

            let mut req = self
                .client
                .query()
                .table_name(&self.transactions_timeline_table)
                .key_condition_expression("timeline_pk = :pk")
                .set_expression_attribute_values(Some(eav))
                .scan_index_forward(false)
                .limit(per_shard_limit);
            if let Some(ek) = esk {
                req = req.set_exclusive_start_key(Some(ek));
            }
            let out = req.send().await?;

            for item in out.items() {
                all_rows.push(parse_transaction(item)?);
            }
            if let Some(lek) = out.last_evaluated_key() {
                if let Some(sk) = lek.get("timeline_sk").and_then(|v| v.as_s().ok().cloned()) {
                    last_sk = Some(match shard_idx {
                        Some(n) => format!("shard{n}|{sk}"),
                        None => sk,
                    });
                }
            }
        }

        all_rows.sort_by_key(|a| Reverse(a.timestamp));
        all_rows.truncate(limit as usize);

        let prev_cursor = cursor.filter(|_| !all_rows.is_empty());

        Ok(RecentTransactionsPage {
            transactions: all_rows,
            next_cursor: last_sk,
            prev_cursor,
        })
    }

    async fn recent_transactions_page_backward(
        &self,
        limit: i64,
        before_sk: &str,
    ) -> anyhow::Result<RecentTransactionsPage> {
        let pks = self.all_timeline_pks();
        let limit = limit.max(1);
        let per_shard_limit = ((limit as usize).div_ceil(pks.len())).max(1) as i32;
        let mut all_rows = Vec::new();
        let mut next_sk: Option<String> = None;

        let (cursor_shard, cursor_sk) = parse_cursor(Some(before_sk));

        for pk in &pks {
            let shard_idx = pk
                .strip_prefix("shard-")
                .and_then(|s| s.parse::<u16>().ok());
            if let Some(ref target) = cursor_shard {
                if shard_idx != Some(*target) && shard_idx.is_some() {
                    continue;
                }
            }

            let mut eav = HashMap::new();
            eav.insert(":pk".to_string(), av_s(pk));

            let before = cursor_sk.as_deref().unwrap_or(before_sk);
            let mut esk = HashMap::new();
            esk.insert("timeline_pk".to_string(), av_s(pk));
            esk.insert("timeline_sk".to_string(), av_s(before));

            let out = self
                .client
                .query()
                .table_name(&self.transactions_timeline_table)
                .key_condition_expression("timeline_pk = :pk")
                .set_expression_attribute_values(Some(eav))
                .scan_index_forward(true)
                .limit(per_shard_limit)
                .set_exclusive_start_key(Some(esk))
                .send()
                .await?;

            for item in out.items() {
                all_rows.push(parse_transaction(item)?);
            }
            if next_sk.is_none() {
                if let Some(lek) = out.last_evaluated_key() {
                    if let Some(sk) = lek.get("timeline_sk").and_then(|v| v.as_s().ok().cloned()) {
                        next_sk = Some(match shard_idx {
                            Some(n) => format!("shard{n}|{sk}"),
                            None => sk,
                        });
                    }
                }
            }
        }

        all_rows.sort_by_key(|a| Reverse(a.timestamp));
        all_rows.truncate(limit as usize);
        all_rows.reverse();

        Ok(RecentTransactionsPage {
            transactions: all_rows,
            next_cursor: next_sk,
            prev_cursor: None,
        })
    }
}

#[async_trait]
impl MetricsPort for DynamoDbStore {
    async fn dashboard_metrics(&self) -> anyhow::Result<DashboardMetrics> {
        let mut key = HashMap::new();
        key.insert("metric_id".to_string(), av_s("global"));

        let out = self
            .client
            .get_item()
            .table_name(&self.metrics_table)
            .set_key(Some(key))
            .send()
            .await?;

        if let Some(item) = out.item() {
            let total_revenue = attr_n_default(item, "total_revenue", 0.0)?;
            let total_cost = attr_n_default(item, "total_cost", 0.0)?;
            return Ok(DashboardMetrics {
                total_margin: total_revenue - total_cost,
                total_input_tokens: attr_i64_default(item, "total_input_tokens", 0)?,
                total_output_tokens: attr_i64_default(item, "total_output_tokens", 0)?,
                tx_count: attr_i64_default(item, "tx_count", 0)?,
            });
        }

        Ok(DashboardMetrics {
            total_margin: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            tx_count: 0,
        })
    }
}

#[async_trait]
impl RuntimeMetricsPort for DynamoDbStore {
    async fn save_runtime_metrics(&self, snapshot: &RuntimeMetricsSnapshot) -> anyhow::Result<()> {
        let json = serde_json::to_string(snapshot)?;
        let mut item = HashMap::new();
        item.insert("metric_id".to_string(), av_s("runtime_metrics"));
        item.insert("snapshot".to_string(), av_s(&json));
        item.insert(
            "updated_at".to_string(),
            av_s(&Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        );
        self.client
            .put_item()
            .table_name(&self.metrics_table)
            .set_item(Some(item))
            .send()
            .await?;
        Ok(())
    }

    async fn load_runtime_metrics(&self) -> anyhow::Result<Option<RuntimeMetricsSnapshot>> {
        let mut key = HashMap::new();
        key.insert("metric_id".to_string(), av_s("runtime_metrics"));
        let out = self
            .client
            .get_item()
            .table_name(&self.metrics_table)
            .set_key(Some(key))
            .send()
            .await?;
        match out.item() {
            Some(item) => {
                let json = attr_s(item, "snapshot")?;
                let snap: RuntimeMetricsSnapshot = serde_json::from_str(&json)?;
                Ok(Some(snap))
            }
            None => Ok(None),
        }
    }
}

#[async_trait]
impl HealthPort for DynamoDbStore {
    async fn check_idempotency_ttl_enabled(&self) -> anyhow::Result<bool> {
        let out = self
            .client
            .describe_time_to_live()
            .table_name(&self.idempotency_table)
            .send()
            .await?;
        Ok(out
            .time_to_live_description()
            .and_then(|d| d.time_to_live_status())
            .map(|s| s.as_str() == "ENABLED")
            .unwrap_or(false))
    }

    async fn write_probe(&self) -> anyhow::Result<()> {
        let now = Utc::now();
        let mut item = HashMap::new();
        item.insert("metric_id".to_string(), av_s("health_probe"));
        item.insert(
            "probe_ts".to_string(),
            av_s(&now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        );
        self.client
            .put_item()
            .table_name(&self.metrics_table)
            .set_item(Some(item))
            .send()
            .await?;
        Ok(())
    }

    async fn query_dead_letters(&self, limit: i64) -> anyhow::Result<Vec<DeadLetterEntry>> {
        let Some(ref dl_table) = self.dead_letter_table else {
            return Ok(vec![]);
        };
        let limit_i32 = limit.clamp(1, 100) as i32;
        let out = self
            .client
            .scan()
            .table_name(dl_table)
            .limit(limit_i32)
            .send()
            .await?;
        let items = out.items();
        items
            .iter()
            .map(|item| {
                Ok(DeadLetterEntry {
                    id: attr_s(item, "id").unwrap_or_default(),
                    idempotency_key: attr_s(item, "idempotency_key").unwrap_or_default(),
                    api_key_id: attr_s(item, "api_key_id").unwrap_or_default(),
                    model_name: attr_s(item, "model_name").unwrap_or_default(),
                    input_tokens: attr_n(item, "input_tokens").unwrap_or(0.0) as i64,
                    output_tokens: attr_n(item, "output_tokens").unwrap_or(0.0) as i64,
                    cost_basis: attr_n(item, "cost_basis").unwrap_or(0.0),
                    revenue_generated: attr_n(item, "revenue_generated").unwrap_or(0.0),
                    error: attr_s(item, "error").unwrap_or_default(),
                    timestamp: attr_s(item, "timestamp").unwrap_or_default(),
                })
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn av_s(v: &str) -> AttributeValue {
    AttributeValue::S(v.to_string())
}

fn av_n(v: &str) -> AttributeValue {
    AttributeValue::N(v.to_string())
}

fn parse_api_key(item: &HashMap<String, AttributeValue>) -> anyhow::Result<ApiKey> {
    let tier = match attr_s(item, "tier")?.as_str() {
        "bit" => Tier::Bit,
        "node" => Tier::Node,
        "cluster" => Tier::Cluster,
        other => return Err(anyhow!("invalid tier: {other}")),
    };
    Ok(ApiKey {
        id: attr_s(item, "id")?,
        key_hash: attr_s(item, "key_hash")?,
        tier,
        balance_accumulated: attr_n(item, "balance_accumulated")?,
        credit_limit: item
            .get("credit_limit")
            .and_then(|v| v.as_n().ok())
            .map(|s| {
                s.parse::<f64>()
                    .map_err(|e| anyhow!("invalid credit_limit: {e}"))
            })
            .transpose()?,
        active: item
            .get("active")
            .and_then(|v| v.as_bool().ok().copied())
            .unwrap_or(true),
        created_at: parse_ts(&attr_s(item, "created_at")?)?,
    })
}

fn parse_pricing(item: &HashMap<String, AttributeValue>) -> anyhow::Result<PricingConfig> {
    let default_cost = item
        .get("provider_cost_per_1m")
        .and_then(|v| v.as_n().ok())
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);
    let min_tier = match item
        .get("min_tier")
        .and_then(|v| v.as_s().ok())
        .map(|s| s.as_str())
    {
        Some("node") => Tier::Node,
        Some("cluster") => Tier::Cluster,
        _ => Tier::Bit,
    };
    Ok(PricingConfig {
        model_name: attr_s(item, "model_name")?,
        provider_id: item
            .get("provider_id")
            .and_then(|v| v.as_s().ok().cloned())
            .unwrap_or_else(|| "default".to_string()),
        provider_cost_input_per_1m: item
            .get("provider_cost_input_per_1m")
            .and_then(|v| v.as_n().ok())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(default_cost),
        provider_cost_output_per_1m: item
            .get("provider_cost_output_per_1m")
            .and_then(|v| v.as_n().ok())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(default_cost),
        bit_price_per_1m: attr_n(item, "bit_price_per_1m")?,
        node_price_per_1m: attr_n(item, "node_price_per_1m")?,
        cluster_price_per_1m: attr_n(item, "cluster_price_per_1m")?,
        min_tier,
        updated_at: parse_ts(&attr_s(item, "updated_at")?)?,
    })
}

fn parse_provider(item: &HashMap<String, AttributeValue>) -> anyhow::Result<Provider> {
    Ok(Provider {
        id: attr_s(item, "id")?,
        name: attr_s(item, "name").unwrap_or_default(),
        base_url: attr_s(item, "base_url")?,
        api_key: attr_s(item, "api_key")?,
        quantization: attr_s(item, "quantization").unwrap_or_else(|_| "fp16".to_string()),
        context_length: item
            .get("context_length")
            .and_then(|v| v.as_n().ok())
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(128000),
        max_output_length: item
            .get("max_output_length")
            .and_then(|v| v.as_n().ok())
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(8192),
        datacenter_country: attr_s(item, "datacenter_country").unwrap_or_else(|_| "US".to_string()),
        created_at: parse_ts(
            &item
                .get("created_at")
                .and_then(|v| v.as_s().ok().cloned())
                .unwrap_or_else(|| Utc::now().to_rfc3339()),
        )?,
    })
}

fn parse_transaction(item: &HashMap<String, AttributeValue>) -> anyhow::Result<Transaction> {
    Ok(Transaction {
        id: attr_s(item, "id")?,
        api_key_id: attr_s(item, "api_key_id")?,
        model_name: attr_s(item, "model_name")?,
        input_tokens: attr_i64(item, "input_tokens")?,
        output_tokens: attr_i64(item, "output_tokens")?,
        cost_basis: attr_n(item, "cost_basis")?,
        revenue_generated: attr_n(item, "revenue_generated")?,
        timestamp: parse_ts(&attr_s(item, "timestamp")?)?,
    })
}

fn parse_ts(value: &str) -> anyhow::Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)
        .map_err(|e| anyhow!("invalid timestamp {value}: {e}"))?
        .with_timezone(&Utc))
}

fn attr_s(item: &HashMap<String, AttributeValue>, key: &str) -> anyhow::Result<String> {
    item.get(key)
        .and_then(|v| v.as_s().ok().cloned())
        .ok_or_else(|| anyhow!("missing string attribute: {key}"))
}

fn attr_n(item: &HashMap<String, AttributeValue>, key: &str) -> anyhow::Result<f64> {
    item.get(key)
        .and_then(|v| v.as_n().ok())
        .ok_or_else(|| anyhow!("missing number attribute: {key}"))?
        .parse::<f64>()
        .map_err(|e| anyhow!("invalid number for {key}: {e}"))
}

fn attr_n_default(
    item: &HashMap<String, AttributeValue>,
    key: &str,
    default: f64,
) -> anyhow::Result<f64> {
    match item.get(key).and_then(|v| v.as_n().ok()) {
        Some(v) => v
            .parse::<f64>()
            .map_err(|e| anyhow!("invalid number for {key}: {e}")),
        None => Ok(default),
    }
}

fn attr_i64(item: &HashMap<String, AttributeValue>, key: &str) -> anyhow::Result<i64> {
    item.get(key)
        .and_then(|v| v.as_n().ok())
        .ok_or_else(|| anyhow!("missing number attribute: {key}"))?
        .parse::<i64>()
        .map_err(|e| anyhow!("invalid integer for {key}: {e}"))
}

fn attr_i64_default(
    item: &HashMap<String, AttributeValue>,
    key: &str,
    default: i64,
) -> anyhow::Result<i64> {
    match item.get(key).and_then(|v| v.as_n().ok()) {
        Some(v) => v
            .parse::<i64>()
            .map_err(|e| anyhow!("invalid integer for {key}: {e}")),
        None => Ok(default),
    }
}

fn parse_cursor(cursor: Option<&str>) -> (Option<u16>, Option<String>) {
    let Some(c) = cursor else {
        return (None, None);
    };
    if let Some(rest) = c.strip_prefix("shard") {
        if let Some(pipe_pos) = rest.find('|') {
            let shard = rest[..pipe_pos].parse::<u16>().ok();
            let sk = rest[pipe_pos + 1..].to_string();
            return (shard, Some(sk));
        }
    }
    (None, Some(c.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tx_item(sk: &str) -> HashMap<String, AttributeValue> {
        let mut m = HashMap::new();
        m.insert("timeline_sk".to_string(), av_s(sk));
        m.insert("id".to_string(), av_s("tx-1"));
        m.insert("idempotency_key".to_string(), av_s("ik-1"));
        m.insert("api_key_id".to_string(), av_s("ak-1"));
        m.insert("model_name".to_string(), av_s("llama-3"));
        m.insert("input_tokens".to_string(), av_n("10"));
        m.insert("output_tokens".to_string(), av_n("20"));
        m.insert("cost_basis".to_string(), av_n("0.001"));
        m.insert("revenue_generated".to_string(), av_n("0.002"));
        m.insert("timestamp".to_string(), av_s("2025-01-01T00:00:00Z"));
        m
    }

    #[test]
    fn parse_transaction_roundtrip() {
        let item = make_tx_item("1700000000000#uuid");
        let tx = parse_transaction(&item).unwrap();
        assert_eq!(tx.model_name, "llama-3");
        assert_eq!(tx.input_tokens, 10);
        assert_eq!(tx.output_tokens, 20);
    }

    #[test]
    fn parse_transaction_missing_field_errors() {
        let mut item = make_tx_item("sk");
        item.remove("model_name");
        assert!(parse_transaction(&item).is_err());
    }

    #[test]
    fn attr_s_missing_returns_error() {
        let item: HashMap<String, AttributeValue> = HashMap::new();
        assert!(attr_s(&item, "key").is_err());
    }

    #[test]
    fn attr_n_invalid_returns_error() {
        let mut item = HashMap::new();
        item.insert(
            "x".to_string(),
            AttributeValue::N("NaN-notanumber".to_string()),
        );
        assert!(attr_n(&item, "x").is_err());
    }

    #[test]
    fn attr_n_default_uses_default_when_missing() {
        let item: HashMap<String, AttributeValue> = HashMap::new();
        assert_eq!(attr_n_default(&item, "missing", 3.14).unwrap(), 3.14);
    }
}
