use crate::application::billing::BillingService;
use crate::domain::ports::Repository;
use crate::per_key_limiter::PerKeyLimiter;
use crate::provider_registry::ProviderRegistry;
use crate::request_cache::RequestCache;
use crate::runtime_metrics::RuntimeMetrics;
use reqwest::Client;
use std::sync::Arc;
use tokio::sync::Semaphore;

pub struct AppState {
    pub repo: Arc<dyn Repository>,
    pub billing: BillingService,
    pub providers: Arc<ProviderRegistry>,
    pub api_key_hash_secret: String,
    pub idempotency_ttl_seconds: i64,
    pub inflight_limit: Arc<Semaphore>,
    pub per_key_limiter: Arc<PerKeyLimiter>,
    pub request_cache: Arc<RequestCache>,
    pub runtime_metrics: Arc<RuntimeMetrics>,
    pub client: Client,
    pub max_output_tokens: i64,
    pub max_streaming_seconds: u64,
    pub upstream_timeout_seconds: u64,
}
