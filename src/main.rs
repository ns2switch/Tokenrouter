mod application;
mod domain;
mod infrastructure;
mod interfaces;
mod per_key_limiter;
mod provider_registry;
mod request_cache;
mod runtime_metrics;
mod security;
mod state;

use anyhow::Context;
use application::billing::BillingService;
use axum::body::Body;
use axum::extract::DefaultBodyLimit;
use axum::http::header::{HeaderName, HeaderValue, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS};
use axum::http::{Request, StatusCode};
use axum::middleware;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use chrono::Utc;
use domain::entities::Provider;
use domain::ports::RuntimeMetricsPort;
use infrastructure::dynamodb::DynamoDbStore;
use interfaces::admin::{
    cache_stats, create_key, dashboard_json, dead_letters, delete_key, delete_pricing,
    delete_provider, health, list_keys, list_providers, login_submit, logout, metrics_json,
    pricing_json, public_health, refresh_providers, toggle_key, transactions_json,
    update_key_limit, upsert_pricing_json, upsert_provider,
};
use interfaces::admin_auth::require_admin_bearer;
use interfaces::audio::{transcriptions, translations};
use interfaces::completions::completions;
use interfaces::embeddings::embeddings;
use interfaces::images::generations;
use interfaces::metrics::metrics;
use interfaces::provider::{list_models, list_models_openai, upstream_models};
use interfaces::proxy::chat_completions;
use lambda_http::Error as LambdaError;
use provider_registry::ProviderRegistry;
use reqwest::Client;
use state::AppState;
use std::env;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let nim_base_url = env::var("NIM_BASE_URL").context("NIM_BASE_URL is required")?;
    let nim_api_key = env::var("NIM_API_KEY").context("NIM_API_KEY is required")?;
    let api_key_hash_secret =
        env::var("API_KEY_HASH_SECRET").context("API_KEY_HASH_SECRET is required")?;
    let idempotency_ttl_seconds = env::var("IDEMPOTENCY_TTL_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(86_400i64);
    let max_inflight_requests = env::var("MAX_INFLIGHT_REQUESTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200usize);
    let timeline_shard_count = env::var("TIMELINE_SHARD_COUNT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1u16);
    let max_output_tokens = env::var("MAX_OUTPUT_TOKENS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(16_384i64);
    let max_inflight_per_key = env::var("MAX_INFLIGHT_PER_KEY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10u64);
    let upstream_timeout = env::var("UPSTREAM_TIMEOUT_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300u64);
    let cache_ttl = env::var("REQUEST_CACHE_TTL_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3600u64);
    let cache_max = env::var("REQUEST_CACHE_MAX_ENTRIES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000usize);
    let cache_max_response = env::var("REQUEST_CACHE_MAX_RESPONSE_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(65536usize);
    let max_streaming_seconds = env::var("MAX_STREAMING_DURATION_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(600u64);

    let default_provider = Provider {
        id: "default".to_string(),
        name: env::var("PROVIDER_NAME_PREFIX").unwrap_or_else(|_| "TokenRouter".into()),
        base_url: nim_base_url,
        api_key: nim_api_key,
        quantization: env::var("PROVIDER_QUANTIZATION").unwrap_or_else(|_| "fp16".into()),
        context_length: env::var("PROVIDER_CONTEXT_LENGTH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(128_000),
        max_output_length: env::var("PROVIDER_MAX_OUTPUT_LENGTH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8_192),
        datacenter_country: env::var("PROVIDER_DATACENTER_COUNTRY").unwrap_or_else(|_| "US".into()),
        created_at: Utc::now(),
    };

    let dead_letter = env::var("DDB_DEAD_LETTER_TABLE")
        .ok()
        .filter(|s| !s.is_empty());
    let store = DynamoDbStore::connect(
        env::var("DDB_API_KEYS_TABLE").unwrap_or_else(|_| "api_keys".into()),
        env::var("DDB_PROVIDERS_TABLE").unwrap_or_else(|_| "providers".into()),
        env::var("DDB_PRICING_TABLE").unwrap_or_else(|_| "pricing_config".into()),
        env::var("DDB_TRANSACTIONS_TABLE").unwrap_or_else(|_| "transactions".into()),
        env::var("DDB_TRANSACTIONS_TIMELINE_TABLE")
            .unwrap_or_else(|_| "transactions_timeline".into()),
        env::var("DDB_IDEMPOTENCY_TABLE").unwrap_or_else(|_| "idempotency_keys".into()),
        env::var("DDB_METRICS_TABLE").unwrap_or_else(|_| "metrics_global".into()),
        dead_letter,
        timeline_shard_count,
    )
    .await?;
    store.init_schema().await?;

    let providers = Arc::new(ProviderRegistry::new(default_provider));
    providers.refresh(&store).await?;

    let runtime_metrics = Arc::new(runtime_metrics::RuntimeMetrics::new());
    match store.load_runtime_metrics().await {
        Ok(Some(snap)) => {
            runtime_metrics.merge_snapshot(&snap);
            tracing::info!("loaded persisted runtime metrics");
        }
        Ok(None) => tracing::info!("no persisted runtime metrics found"),
        Err(e) => tracing::warn!("failed to load runtime metrics: {e}"),
    }

    let metrics_flush_interval = env::var("METRICS_FLUSH_INTERVAL_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300u64);
    let flush_repo = Arc::new(store.clone());
    let flush_metrics = runtime_metrics.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(metrics_flush_interval));
        loop {
            interval.tick().await;
            let snap = flush_metrics.snapshot();
            if let Err(e) = flush_repo.save_runtime_metrics(&snap).await {
                tracing::warn!("failed to flush runtime metrics: {e}");
            }
        }
    });

    let app_state = Arc::new(AppState {
        repo: Arc::new(store),
        billing: BillingService::new()?,
        providers,
        api_key_hash_secret,
        idempotency_ttl_seconds,
        inflight_limit: Arc::new(Semaphore::new(max_inflight_requests)),
        per_key_limiter: Arc::new(per_key_limiter::PerKeyLimiter::new(max_inflight_per_key)),
        request_cache: Arc::new(request_cache::RequestCache::new(
            cache_max,
            cache_max_response,
            cache_ttl,
        )),
        runtime_metrics,
        max_output_tokens,
        max_streaming_seconds,
        client: Client::builder()
            .timeout(Duration::from_secs(upstream_timeout))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .context("failed to build HTTP client")?,
    });

    let admin_router = Router::new()
        .route("/health", get(health))
        .route("/dashboard", get(dashboard_json))
        .route("/pricing", post(upsert_pricing_json))
        .route("/pricing", get(pricing_json))
        .route("/transactions", get(transactions_json))
        .route("/metrics", get(metrics_json))
        .route("/cache", get(cache_stats))
        .route("/dead-letter", get(dead_letters))
        .route("/keys", get(list_keys))
        .route("/keys", post(create_key))
        .route("/keys/toggle", post(toggle_key))
        .route("/keys/limit", post(update_key_limit))
        .route("/keys/delete", post(delete_key))
        .route("/pricing/delete", post(delete_pricing))
        .route("/models/upstream", get(upstream_models))
        .route("/providers", get(list_providers))
        .route("/providers", post(upsert_provider))
        .route("/providers/delete", post(delete_provider))
        .route("/providers/refresh", post(refresh_providers))
        .route_layer(middleware::from_fn(require_admin_bearer));

    async fn spa_fallback(req: Request<Body>) -> Response<Body> {
        let path = req.uri().path();
        if !path.starts_with('/')
            || path.contains("..")
            || path.contains("//")
            || path.contains('\\')
        {
            return not_found();
        }
        let safe = format!("frontend/dist{path}");
        if let Ok(content) = tokio::fs::read(&safe).await {
            let file = std::path::Path::new(&safe);
            let ct = content_type(file.extension().and_then(|e| e.to_str()).unwrap_or(""));
            return Response::builder()
                .status(StatusCode::OK)
                .header("content-type", ct)
                .body(Body::from(content))
                .expect("file response builder");
        }
        let dist = std::path::Path::new("frontend/dist");
        match dist.canonicalize() {
            Ok(_dist_canon) => match tokio::fs::read("frontend/dist/index.html").await {
                Ok(content) => Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "text/html")
                    .body(Body::from(content))
                    .expect("index.html response builder"),
                Err(_) => not_found(),
            },
            Err(_) => not_found(),
        }
    }

    fn not_found() -> Response<Body> {
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("not found"))
            .expect("static response builder")
    }

    fn content_type(ext: &str) -> &'static str {
        match ext {
            "html" => "text/html",
            "js" => "application/javascript",
            "css" => "text/css",
            "svg" => "image/svg+xml",
            "png" => "image/png",
            "json" => "application/json",
            _ => "application/octet-stream",
        }
    }

    let app = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/completions", post(completions))
        .route("/v1/embeddings", post(embeddings))
        .route("/v1/audio/transcriptions", post(transcriptions))
        .route("/v1/audio/translations", post(translations))
        .route("/v1/images/generations", post(generations))
        .route("/v1/models", get(list_models))
        .route("/v1/openai/models", get(list_models_openai))
        .route("/health", get(public_health))
        .route(
            "/metrics",
            get(metrics).route_layer(middleware::from_fn(require_admin_bearer)),
        )
        .route("/admin/login", post(login_submit))
        .route("/admin/logout", get(logout))
        .nest("/admin", admin_router)
        .fallback(get(spa_fallback))
        .with_state(app_state)
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(CompressionLayer::new())
        .layer(DefaultBodyLimit::max(8 * 1024 * 1024))
        .layer(SetResponseHeaderLayer::if_not_present(
            X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("cross-origin-resource-policy"),
            HeaderValue::from_static("cross-origin"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("content-security-policy"),
            HeaderValue::from_static("default-src 'self'"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("strict-transport-security"),
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        ))
        .layer(TraceLayer::new_for_http());

    let run_mode = env::var("RUN_MODE").unwrap_or_else(|_| "lambda".to_string());
    if run_mode.eq_ignore_ascii_case("server") {
        let listen_addr = env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
        let listener = tokio::net::TcpListener::bind(&listen_addr).await?;
        tracing::info!("listening on {}", listen_addr);
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                tokio::signal::ctrl_c().await.ok();
                tracing::info!("shutting down gracefully");
            })
            .await?;
        return Ok(());
    }

    run_lambda(app)
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    Ok(())
}

async fn run_lambda(app: Router) -> Result<(), LambdaError> {
    lambda_http::run(app).await
}
