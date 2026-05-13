use anyhow::Result;
use aws_sdk_dynamodb::types::{
    AttributeDefinition, AttributeValue, KeySchemaElement, KeyType, ScalarAttributeType,
    TimeToLiveSpecification,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::env;
use tokenrouter::domain::entities::Tier;
use tokenrouter::domain::ports::{
    ApiKeyPort, HealthPort, MetricsPort, PricingPort, ProviderPort, RuntimeMetricsPort,
    RuntimeMetricsSnapshot, TransactionPort,
};
use tokenrouter::infrastructure::dynamodb::DynamoDbStore;
use tokenrouter::provider_registry::ProviderRegistry;

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

fn hash_key(raw: &str, secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hasher.update(b":");
    hasher.update(raw.as_bytes());
    hex_encode(&hasher.finalize())
}

async fn make_client() -> aws_sdk_dynamodb::Client {
    let config = aws_config::load_from_env().await;
    aws_sdk_dynamodb::Client::new(&config)
}

fn av_s(v: &str) -> AttributeValue {
    AttributeValue::S(v.to_string())
}

fn av_n(v: &str) -> AttributeValue {
    AttributeValue::N(v.to_string())
}

fn dynamodb_available() -> bool {
    env::var("AWS_ENDPOINT_URL_DYNAMODB").is_ok()
}

struct TestTables {
    api_keys: String,
    providers: String,
    pricing: String,
    transactions: String,
    timeline: String,
    idempotency: String,
    metrics: String,
}

async fn setup_tables(client: &aws_sdk_dynamodb::Client) -> TestTables {
    let suffix = format!(
        "_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );

    let tables = TestTables {
        api_keys: format!("test_api_keys{}", suffix),
        providers: format!("test_providers{}", suffix),
        pricing: format!("test_pricing{}", suffix),
        transactions: format!("test_transactions{}", suffix),
        timeline: format!("test_timeline{}", suffix),
        idempotency: format!("test_idempotency{}", suffix),
        metrics: format!("test_metrics{}", suffix),
    };

    create_table(
        client,
        &tables.api_keys,
        &[("key_hash", "S")],
        &[("key_hash", "HASH")],
    )
    .await;
    create_table(client, &tables.providers, &[("id", "S")], &[("id", "HASH")]).await;
    create_table(
        client,
        &tables.pricing,
        &[("model_name", "S")],
        &[("model_name", "HASH")],
    )
    .await;
    create_table(
        client,
        &tables.transactions,
        &[("id", "S")],
        &[("id", "HASH")],
    )
    .await;
    create_table(
        client,
        &tables.timeline,
        &[("timeline_pk", "S"), ("timeline_sk", "S")],
        &[("timeline_pk", "HASH"), ("timeline_sk", "RANGE")],
    )
    .await;
    create_table(
        client,
        &tables.idempotency,
        &[("idempotency_key", "S")],
        &[("idempotency_key", "HASH")],
    )
    .await;
    create_table(
        client,
        &tables.metrics,
        &[("metric_id", "S")],
        &[("metric_id", "HASH")],
    )
    .await;

    let _ = client
        .update_time_to_live()
        .table_name(&tables.idempotency)
        .time_to_live_specification(
            TimeToLiveSpecification::builder()
                .enabled(true)
                .attribute_name("expires_at")
                .build()
                .unwrap(),
        )
        .send()
        .await;

    tables
}

async fn create_table(
    client: &aws_sdk_dynamodb::Client,
    name: &str,
    attrs: &[(&str, &str)],
    keys: &[(&str, &str)],
) {
    let attr_defs: Vec<AttributeDefinition> = attrs
        .iter()
        .map(|(n, t)| {
            AttributeDefinition::builder()
                .attribute_name(*n)
                .attribute_type(ScalarAttributeType::from(*t))
                .build()
                .unwrap()
        })
        .collect();

    let key_schema: Vec<KeySchemaElement> = keys
        .iter()
        .map(|(n, t)| {
            KeySchemaElement::builder()
                .attribute_name(*n)
                .key_type(KeyType::from(*t))
                .build()
                .unwrap()
        })
        .collect();

    let _ = client
        .create_table()
        .table_name(name)
        .set_attribute_definitions(Some(attr_defs))
        .set_key_schema(Some(key_schema))
        .billing_mode(aws_sdk_dynamodb::types::BillingMode::PayPerRequest)
        .send()
        .await;
}

async fn store_from(tables: &TestTables, shard_count: u16) -> DynamoDbStore {
    DynamoDbStore::connect(
        tables.api_keys.clone(),
        tables.providers.clone(),
        tables.pricing.clone(),
        tables.transactions.clone(),
        tables.timeline.clone(),
        tables.idempotency.clone(),
        tables.metrics.clone(),
        Some(format!("{}_dead_letter", tables.transactions)),
        shard_count,
    )
    .await
    .unwrap()
}

async fn drop_tables(client: &aws_sdk_dynamodb::Client, tables: &TestTables) {
    for name in [
        &tables.api_keys,
        &tables.providers,
        &tables.pricing,
        &tables.transactions,
        &tables.timeline,
        &tables.idempotency,
        &tables.metrics,
    ] {
        let _ = client.delete_table().table_name(name.clone()).send().await;
    }
    let _ = client
        .delete_table()
        .table_name(format!("{}_dead_letter", tables.transactions))
        .send()
        .await;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_store_find_api_key() -> Result<()> {
    if !dynamodb_available() {
        return Ok(());
    }
    let client = make_client().await;
    let tables = setup_tables(&client).await;

    let store = store_from(&tables, 1).await;
    let hashed = hash_key("nonexistent", "test-secret");
    let key = store.find_api_key_by_hash(&hashed).await?;
    assert!(key.is_none());

    drop_tables(&client, &tables).await;
    Ok(())
}

#[tokio::test]
async fn test_store_pricing_operations() -> Result<()> {
    if !dynamodb_available() {
        return Ok(());
    }
    let client = make_client().await;
    let tables = setup_tables(&client).await;

    let store = store_from(&tables, 1).await;
    store
        .upsert_pricing("test-model", "default", 1.0, 2.0, 3.0, 4.0, 5.0, &Tier::Bit)
        .await?;

    let p = store.pricing_for_model("test-model").await?;
    assert!(p.is_some());
    let p = p.unwrap();
    assert_eq!(p.provider_id, "default");
    assert_eq!(p.provider_cost_input_per_1m, 1.0);
    assert_eq!(p.provider_cost_output_per_1m, 2.0);
    assert_eq!(p.bit_price_per_1m, 3.0);
    assert_eq!(p.node_price_per_1m, 4.0);
    assert_eq!(p.cluster_price_per_1m, 5.0);

    let all = store.all_pricing().await?;
    assert!(!all.is_empty());

    drop_tables(&client, &tables).await;
    Ok(())
}

#[tokio::test]
async fn test_store_write_probe() -> Result<()> {
    if !dynamodb_available() {
        return Ok(());
    }
    let client = make_client().await;
    let tables = setup_tables(&client).await;

    let store = store_from(&tables, 1).await;
    store.write_probe().await?;

    drop_tables(&client, &tables).await;
    Ok(())
}

#[tokio::test]
async fn test_store_runtime_metrics_persistence() -> Result<()> {
    if !dynamodb_available() {
        return Ok(());
    }
    let client = make_client().await;
    let tables = setup_tables(&client).await;

    let store = store_from(&tables, 1).await;
    let snap = RuntimeMetricsSnapshot {
        requests_total: 42,
        status_counts: vec![(200, 30), (429, 12)],
        model_counts: vec![("gpt-4".to_string(), 20), ("llama-3".to_string(), 22)],
        ttft_buckets: vec![(100, 10), (500, 5)],
        ttft_sum_ms: 3500,
        duration_buckets: vec![(1000, 15)],
        duration_sum_ms: 15000,
        throughput_tps_x100: 5000,
    };

    store.save_runtime_metrics(&snap).await?;
    let loaded = store.load_runtime_metrics().await?;
    assert!(loaded.is_some());
    let loaded = loaded.unwrap();
    assert_eq!(loaded.requests_total, 42);
    assert_eq!(loaded.status_counts.len(), 2);
    assert_eq!(loaded.model_counts.len(), 2);

    drop_tables(&client, &tables).await;
    Ok(())
}

#[tokio::test]
async fn test_store_transaction_flow() -> Result<()> {
    if !dynamodb_available() {
        return Ok(());
    }
    let client = make_client().await;
    let tables = setup_tables(&client).await;

    let store = store_from(&tables, 1).await;

    let mut ak_item = HashMap::new();
    let kh = hash_key("sk_test_flow", "secret");
    ak_item.insert("key_hash".to_string(), av_s(&kh));
    ak_item.insert("id".to_string(), av_s("key-flow-1"));
    ak_item.insert("tier".to_string(), av_s("bit"));
    ak_item.insert("balance_accumulated".to_string(), av_n("0"));
    ak_item.insert("created_at".to_string(), av_s("2026-01-01T00:00:00Z"));
    client
        .put_item()
        .table_name(&tables.api_keys)
        .set_item(Some(ak_item))
        .send()
        .await?;

    store
        .upsert_pricing("test-model", "default", 0.5, 1.5, 1.0, 0.8, 0.6, &Tier::Bit)
        .await?;

    let key = store.find_api_key_by_hash(&kh).await?.unwrap();
    assert_eq!(key.tier, Tier::Bit);

    store
        .record_transaction(
            &key,
            "test-model",
            100,
            50,
            0.00005,
            0.0001,
            "idem-flow-1",
            "hash-flow-1",
            3600,
            "",
        )
        .await?;

    let page = store.recent_transactions_page(10, None).await?;
    assert_eq!(page.transactions.len(), 1);
    assert_eq!(page.transactions[0].model_name, "test-model");

    let m = store.dashboard_metrics().await?;
    assert_eq!(m.tx_count, 1);
    assert!(m.total_margin > 0.0);

    drop_tables(&client, &tables).await;
    Ok(())
}

#[tokio::test]
async fn test_store_idempotency_conflict_same_payload() -> Result<()> {
    if !dynamodb_available() {
        return Ok(());
    }
    let client = make_client().await;
    let tables = setup_tables(&client).await;

    let store = store_from(&tables, 1).await;

    let mut ak_item = HashMap::new();
    let kh = hash_key("sk_test_idem", "secret");
    ak_item.insert("key_hash".to_string(), av_s(&kh));
    ak_item.insert("id".to_string(), av_s("key-idem-1"));
    ak_item.insert("tier".to_string(), av_s("bit"));
    ak_item.insert("balance_accumulated".to_string(), av_n("0"));
    ak_item.insert("created_at".to_string(), av_s("2026-01-01T00:00:00Z"));
    client
        .put_item()
        .table_name(&tables.api_keys)
        .set_item(Some(ak_item))
        .send()
        .await?;

    store
        .upsert_pricing("test-model", "default", 0.5, 1.5, 1.0, 0.8, 0.6, &Tier::Bit)
        .await?;
    let key = store.find_api_key_by_hash(&kh).await?.unwrap();

    store
        .record_transaction(
            &key,
            "test-model",
            10,
            5,
            0.001,
            0.002,
            "idem-same",
            "hash-same",
            3600,
            r#"{"ok":true}"#,
        )
        .await?;

    let result = store
        .record_transaction(
            &key,
            "test-model",
            10,
            5,
            0.001,
            0.002,
            "idem-same",
            "hash-same",
            3600,
            r#"{"ok":true}"#,
        )
        .await;
    assert!(result.is_err());
    let err = result.err().unwrap().to_string();
    assert!(
        err.contains("ConditionalCheckFailed")
            || err.contains("TransactionCanceledException")
            || err.contains("Transaction")
            || err.contains("service")
            || err.contains("conflict"),
        "expected conflict error, got: {err}"
    );

    drop_tables(&client, &tables).await;
    Ok(())
}

#[tokio::test]
async fn test_store_multi_shard_transactions() -> Result<()> {
    if !dynamodb_available() {
        return Ok(());
    }
    let client = make_client().await;
    let tables = setup_tables(&client).await;

    let store = store_from(&tables, 4).await;
    store
        .upsert_pricing("test-model", "default", 0.5, 1.5, 1.0, 0.8, 0.6, &Tier::Bit)
        .await?;

    for i in 0..4 {
        let kh = hash_key(&format!("sk_shard_{}", i), "secret");
        let mut ak_item = HashMap::new();
        ak_item.insert("key_hash".to_string(), av_s(&kh));
        ak_item.insert("id".to_string(), av_s(&format!("key-shard-{}", i)));
        ak_item.insert("tier".to_string(), av_s("bit"));
        ak_item.insert("balance_accumulated".to_string(), av_n("0"));
        ak_item.insert("created_at".to_string(), av_s("2026-01-01T00:00:00Z"));
        client
            .put_item()
            .table_name(&tables.api_keys)
            .set_item(Some(ak_item))
            .send()
            .await?;

        let key = store.find_api_key_by_hash(&kh).await?.unwrap();
        store
            .record_transaction(
                &key,
                "test-model",
                100,
                50,
                0.001,
                0.002,
                &format!("idem-shard-{}", i),
                &format!("hash-{}", i),
                3600,
                "",
            )
            .await?;
    }

    let page = store.recent_transactions_page(10, None).await?;
    assert_eq!(page.transactions.len(), 4);

    drop_tables(&client, &tables).await;
    Ok(())
}

#[tokio::test]
async fn test_store_ttl_check() -> Result<()> {
    if !dynamodb_available() {
        return Ok(());
    }
    let client = make_client().await;
    let tables = setup_tables(&client).await;

    let store = store_from(&tables, 1).await;
    let ttl = store.check_idempotency_ttl_enabled().await?;
    assert!(ttl);

    drop_tables(&client, &tables).await;
    Ok(())
}

#[tokio::test]
async fn test_store_paginated_transactions() -> Result<()> {
    if !dynamodb_available() {
        return Ok(());
    }
    let client = make_client().await;
    let tables = setup_tables(&client).await;

    let store = store_from(&tables, 1).await;

    let kh = hash_key("sk_paginated", "secret");
    let mut ak_item = HashMap::new();
    ak_item.insert("key_hash".to_string(), av_s(&kh));
    ak_item.insert("id".to_string(), av_s("key-paginated"));
    ak_item.insert("tier".to_string(), av_s("bit"));
    ak_item.insert("balance_accumulated".to_string(), av_n("0"));
    ak_item.insert("created_at".to_string(), av_s("2026-01-01T00:00:00Z"));
    client
        .put_item()
        .table_name(&tables.api_keys)
        .set_item(Some(ak_item))
        .send()
        .await?;

    store
        .upsert_pricing("test-model", "default", 0.5, 1.5, 1.0, 0.8, 0.6, &Tier::Bit)
        .await?;
    let key = store.find_api_key_by_hash(&kh).await?.unwrap();

    for i in 0..5 {
        store
            .record_transaction(
                &key,
                "test-model",
                10,
                5,
                0.001,
                0.002,
                &format!("idem-pag-{}", i),
                &format!("hash-{}", i),
                3600,
                "",
            )
            .await?;
    }

    let page1 = store.recent_transactions_page(2, None).await?;
    assert_eq!(page1.transactions.len(), 2);
    assert!(page1.next_cursor.is_some());

    drop_tables(&client, &tables).await;
    Ok(())
}

#[tokio::test]
async fn test_store_provider_operations() -> Result<()> {
    if !dynamodb_available() {
        return Ok(());
    }
    let client = make_client().await;
    let tables = setup_tables(&client).await;

    let store = store_from(&tables, 1).await;

    store
        .upsert_provider(
            "test-prov",
            "Test Provider",
            "https://api.test.example.com",
            "sk-test-key",
            "fp16",
            128000,
            8192,
            "US",
        )
        .await?;

    let prov = store.provider_by_id("test-prov").await?;
    assert!(prov.is_some());
    let prov = prov.unwrap();
    assert_eq!(prov.name, "Test Provider");
    assert_eq!(prov.base_url, "https://api.test.example.com");
    assert_eq!(prov.api_key, "sk-test-key");

    let created_at_original = prov.created_at;

    store
        .upsert_provider(
            "test-prov",
            "Updated Name",
            "https://api.test.example.com",
            "sk-test-key",
            "fp16",
            128000,
            8192,
            "US",
        )
        .await?;

    let updated = store.provider_by_id("test-prov").await?;
    assert!(updated.is_some());
    let updated = updated.unwrap();
    assert_eq!(updated.name, "Updated Name");
    assert_eq!(
        updated.created_at, created_at_original,
        "created_at should be preserved on update"
    );

    let all = store.all_providers().await?;
    assert!(!all.is_empty());

    store.delete_provider("test-prov").await?;
    let deleted = store.provider_by_id("test-prov").await?;
    assert!(deleted.is_none());

    drop_tables(&client, &tables).await;
    Ok(())
}

#[tokio::test]
async fn test_provider_registry_resolve_unknown_provider_fails() -> Result<()> {
    if !dynamodb_available() {
        return Ok(());
    }
    let client = make_client().await;
    let tables = setup_tables(&client).await;

    let store = store_from(&tables, 1).await;

    let default = tokenrouter::domain::entities::Provider {
        id: "default".to_string(),
        name: "Default".to_string(),
        base_url: "https://default.example.com/v1".to_string(),
        api_key: "sk-default".to_string(),
        quantization: "fp16".to_string(),
        context_length: 128000,
        max_output_length: 8192,
        datacenter_country: "US".to_string(),
        created_at: chrono::Utc::now(),
    };
    let registry = ProviderRegistry::new(default);

    store
        .upsert_pricing(
            "test-model",
            "nonexistent-provider",
            1.0,
            1.0,
            2.0,
            3.0,
            4.0,
            &Tier::Bit,
        )
        .await?;

    let result = registry.resolve_for_model(&store, "test-model").await;
    assert!(
        result.is_err(),
        "resolve_for_model should fail for unknown provider"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("nonexistent-provider"),
        "error should mention the provider ID: {err}"
    );

    drop_tables(&client, &tables).await;
    Ok(())
}

#[tokio::test]
async fn test_store_dead_letter_query() -> Result<()> {
    if !dynamodb_available() {
        return Ok(());
    }
    let client = make_client().await;
    let tables = setup_tables(&client).await;

    let store = store_from(&tables, 1).await;

    let entries = store.query_dead_letters(10).await?;
    assert!(entries.is_empty());

    drop_tables(&client, &tables).await;
    Ok(())
}
