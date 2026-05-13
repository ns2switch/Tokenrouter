use crate::domain::entities::{DashboardMetrics, Tier};
use crate::interfaces::dto::{
    CreateKeyRequest, CreateKeyResponse, DashboardQuery, DeleteProviderQuery, HealthCheck,
    HealthResponse, KeyListItem, PricingForm, ProviderForm,
};
use crate::provider_registry::ProviderRegistry;
use crate::security::hash_api_key;
use crate::state::AppState;
use axum::extract::{Json, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use std::sync::Arc;
use uuid::Uuid;

use crate::interfaces::admin_auth::{constant_time_eq, load_admin_tokens, store_session};
use axum::http::header::SET_COOKIE;

pub async fn dashboard_json(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let metrics = match state.repo.dashboard_metrics().await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(%e, "failed to load dashboard metrics");
            DashboardMetrics {
                total_margin: 0.0,
                total_input_tokens: 0,
                total_output_tokens: 0,
                tx_count: 0,
            }
        }
    };
    let pricing = state.repo.all_pricing().await.unwrap_or_else(|e| {
        tracing::error!(%e, "failed to load pricing");
        vec![]
    });
    let inflight = state.runtime_metrics.inflight();
    axum::Json(serde_json::json!({
        "metrics": metrics,
        "pricing_count": pricing.len(),
        "inflight_requests": inflight,
    }))
}

pub async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut checks = Vec::new();

    let ttl_res = state.repo.check_idempotency_ttl_enabled().await;
    let ttl_enabled = ttl_res.as_ref().copied().unwrap_or(false);
    checks.push(HealthCheck {
        name: "idempotency_ttl".to_string(),
        ok: ttl_enabled,
        detail: match ttl_res {
            Ok(v) => format!("ttl_enabled={v}"),
            Err(_e) => "failed to check ttl".to_string(),
        },
    });

    let secret_present = !state.api_key_hash_secret.trim().is_empty();
    if !secret_present {
        checks.push(HealthCheck {
            name: "api_key_hash_secret".to_string(),
            ok: false,
            detail: "missing".to_string(),
        });
    }

    let write_res = state.repo.write_probe().await;
    let write_ok = write_res.is_ok();
    checks.push(HealthCheck {
        name: "dynamodb_write_probe".to_string(),
        ok: write_ok,
        detail: write_res
            .map(|_| "ok".to_string())
            .unwrap_or_else(|_e| "write probe failed".to_string()),
    });

    let (upstream_ok, provider_results) = check_upstream_connectivity(&state).await;

    let providers = state.providers.all_cached();
    for (i, p) in providers.iter().enumerate() {
        let provider_ok = provider_results.get(i).copied().unwrap_or(false);
        checks.push(HealthCheck {
            name: format!("upstream_{}", p.id),
            ok: provider_ok,
            detail: if provider_ok {
                "reachable".to_string()
            } else {
                "unreachable".to_string()
            },
        });
    }

    checks.push(HealthCheck {
        name: "upstream_connectivity".to_string(),
        ok: upstream_ok,
        detail: if upstream_ok {
            "all reachable".to_string()
        } else {
            "one or more unreachable".to_string()
        },
    });

    let ok = ttl_enabled && secret_present && write_ok && upstream_ok;
    let inflight = state.runtime_metrics.inflight();
    let payload = HealthResponse {
        ok,
        ttl_enabled,
        dynamodb_write_ok: write_ok,
        upstream_ok,
        inflight_requests: inflight,
        checks,
    };

    let status = if ok {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    };
    (status, axum::Json(payload))
}

pub async fn transactions_json(
    State(state): State<Arc<AppState>>,
    Query(q): Query<DashboardQuery>,
) -> impl IntoResponse {
    let empty = crate::domain::entities::RecentTransactionsPage {
        transactions: vec![],
        next_cursor: None,
        prev_cursor: None,
    };
    let page = if let Some(prev_sk) = q.prev {
        state
            .repo
            .recent_transactions_page_backward(100, &prev_sk)
            .await
            .unwrap_or(empty)
    } else {
        state
            .repo
            .recent_transactions_page(100, q.cursor)
            .await
            .unwrap_or(empty)
    };
    axum::Json(page)
}

pub async fn pricing_json(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let pricing = state.repo.all_pricing().await.unwrap_or_default();
    axum::Json(pricing)
}

pub async fn metrics_json(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let snap = state.runtime_metrics.snapshot();
    let inflight = state.runtime_metrics.inflight();
    axum::Json(serde_json::json!({
        "inflight_requests": inflight,
        "snapshot": snap,
    }))
}

pub async fn list_keys(State(state): State<Arc<AppState>>) -> Response {
    match state.repo.list_api_keys().await {
        Ok(keys) => {
            let items: Vec<KeyListItem> = keys
                .into_iter()
                .map(|k| KeyListItem {
                    id: k.id,
                    tier: format!("{:?}", k.tier).to_lowercase(),
                    balance_accumulated: k.balance_accumulated,
                    credit_limit: k.credit_limit,
                    active: k.active,
                    created_at: k.created_at.to_rfc3339(),
                })
                .collect();
            axum::Json(items).into_response()
        }
        Err(e) => {
            tracing::error!(%e, "failed to list api keys");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({"error": "internal server error"})),
            )
                .into_response()
        }
    }
}

pub async fn create_key(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateKeyRequest>,
) -> Response {
    let tier = match req.tier.as_str() {
        "bit" => Tier::Bit,
        "node" => Tier::Node,
        "cluster" => Tier::Cluster,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({"error": "invalid tier"})),
            )
                .into_response()
        }
    };

    let raw_key = format!("sk-{}", Uuid::new_v4().to_string().replace('-', ""));
    let key_hash = hash_api_key(&raw_key, &state.api_key_hash_secret);

    match state
        .repo
        .create_api_key(&key_hash, &tier, req.credit_limit)
        .await
    {
        Ok(api_key) => {
            let resp = CreateKeyResponse {
                id: api_key.id,
                raw_key,
                tier: req.tier,
            };
            (StatusCode::CREATED, axum::Json(resp)).into_response()
        }
        Err(e) => {
            tracing::error!(%e, "failed to create api key");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({"error": "internal server error"})),
            )
                .into_response()
        }
    }
}

pub async fn login_submit(
    State(_state): State<Arc<AppState>>,
    Json(form): Json<LoginForm>,
) -> Response {
    let tokens = load_admin_tokens();
    if tokens.is_empty() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({"error": "no admin tokens configured"})),
        )
            .into_response();
    }
    if !tokens
        .iter()
        .any(|t| constant_time_eq(t.as_bytes(), form.token.as_bytes()))
    {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({"error": "invalid token"})),
        )
            .into_response();
    }

    let session_id = store_session(&form.token);
    let cookie = format!(
        "admin_token={}; HttpOnly; Secure; SameSite=Lax; Path=/admin; Max-Age=86400",
        session_id
    );

    let mut resp = Redirect::to("/admin/dashboard").into_response();
    if let Ok(val) = cookie.parse() {
        resp.headers_mut().insert(SET_COOKIE, val);
    }
    resp
}

#[derive(serde::Deserialize)]
pub struct LoginForm {
    token: String,
}

#[derive(serde::Deserialize)]
pub struct ToggleKeyQuery {
    id: String,
    active: Option<bool>,
}

pub async fn toggle_key(
    State(state): State<Arc<AppState>>,
    Json(q): Json<ToggleKeyQuery>,
) -> Response {
    let keys = match state.repo.list_api_keys().await {
        Ok(k) => k,
        Err(e) => {
            tracing::error!(%e, "failed to list keys for toggle");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({"error": "internal server error"})),
            )
                .into_response();
        }
    };

    let target = match keys.iter().find(|k| k.id == q.id) {
        Some(k) => k,
        None => {
            return (
                StatusCode::NOT_FOUND,
                axum::Json(serde_json::json!({"error": "key not found"})),
            )
                .into_response();
        }
    };

    let active = q.active.unwrap_or(!target.active);

    if let Err(e) = state.repo.set_key_active(&target.key_hash, active).await {
        tracing::error!(%e, key_id=%target.id, "failed to toggle key");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({"error": "internal server error"})),
        )
            .into_response();
    }

    axum::Json(serde_json::json!({"id": target.id, "active": active})).into_response()
}

#[derive(serde::Deserialize)]
pub struct UpdateKeyQuery {
    id: String,
    credit_limit: Option<String>,
}

fn parse_credit_limit(raw: &Option<String>) -> Option<f64> {
    match raw {
        Some(s) if s.is_empty() => None,
        Some(s) => s.parse::<f64>().ok(),
        None => None,
    }
}

pub async fn update_key_limit(
    State(state): State<Arc<AppState>>,
    Json(q): Json<UpdateKeyQuery>,
) -> Response {
    let keys = match state.repo.list_api_keys().await {
        Ok(k) => k,
        Err(e) => {
            tracing::error!(%e, "failed to list keys for credit update");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({"error": "internal server error"})),
            )
                .into_response();
        }
    };

    let target = match keys.iter().find(|k| k.id == q.id) {
        Some(k) => k,
        None => {
            return (
                StatusCode::NOT_FOUND,
                axum::Json(serde_json::json!({"error": "key not found"})),
            )
                .into_response();
        }
    };

    let limit = match parse_credit_limit(&q.credit_limit) {
        Some(v) => Some(v),
        None if q.credit_limit.as_ref().is_none_or(|s| s.is_empty()) => None,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({"error": "invalid credit_limit"})),
            )
                .into_response();
        }
    };

    if let Err(e) = state
        .repo
        .update_key_credit_limit(&target.key_hash, limit)
        .await
    {
        tracing::error!(%e, key_id=%target.id, "failed to update key credit_limit");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({"error": "internal server error"})),
        )
            .into_response();
    }

    axum::Json(serde_json::json!({
        "id": target.id,
        "credit_limit": parse_credit_limit(&q.credit_limit)
    }))
    .into_response()
}

pub async fn delete_key(
    State(state): State<Arc<AppState>>,
    Json(q): Json<ToggleKeyQuery>,
) -> Response {
    let keys = match state.repo.list_api_keys().await {
        Ok(k) => k,
        Err(e) => {
            tracing::error!(%e, "failed to list keys for deletion");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({"error": "internal server error"})),
            )
                .into_response();
        }
    };

    let target = match keys.iter().find(|k| k.id == q.id) {
        Some(k) => k,
        None => {
            return (
                StatusCode::NOT_FOUND,
                axum::Json(serde_json::json!({"error": "key not found"})),
            )
                .into_response();
        }
    };

    if let Err(e) = state.repo.delete_api_key(&target.key_hash).await {
        tracing::error!(%e, key_id=%target.id, "failed to delete api key");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({"error": "internal server error"})),
        )
            .into_response();
    }

    axum::Json(serde_json::json!({"deleted": target.id})).into_response()
}

pub async fn logout() -> Response {
    let cookie_str = "admin_token=; HttpOnly; SameSite=Lax; Path=/admin; Max-Age=0";
    let mut resp = Redirect::to("/").into_response();
    if let Ok(cookie) = cookie_str.parse() {
        resp.headers_mut().insert(SET_COOKIE, cookie);
    }
    resp
}

#[derive(serde::Deserialize)]
pub struct DeletePricingQuery {
    model: String,
}

pub async fn delete_pricing(
    State(state): State<Arc<AppState>>,
    Json(q): Json<DeletePricingQuery>,
) -> Response {
    if let Err(e) = state.repo.delete_pricing(&q.model).await {
        tracing::error!(%e, model=%q.model, "failed to delete pricing");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({"error": "internal server error"})),
        )
            .into_response();
    }
    axum::Json(serde_json::json!({"deleted": q.model})).into_response()
}

pub async fn upsert_pricing_json(
    State(state): State<Arc<AppState>>,
    Json(form): Json<PricingForm>,
) -> Response {
    if form.provider_cost_input_per_1m.is_sign_negative()
        || form.provider_cost_input_per_1m.is_nan()
        || form.provider_cost_output_per_1m.is_sign_negative()
        || form.provider_cost_output_per_1m.is_nan()
        || form.bit_price_per_1m.is_sign_negative()
        || form.bit_price_per_1m.is_nan()
        || form.node_price_per_1m.is_sign_negative()
        || form.node_price_per_1m.is_nan()
        || form.cluster_price_per_1m.is_sign_negative()
        || form.cluster_price_per_1m.is_nan()
    {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({"error": "pricing values must be non-negative numbers"})),
        )
            .into_response();
    }
    let min_tier = match form.min_tier.as_str() {
        "node" => Tier::Node,
        "cluster" => Tier::Cluster,
        _ => Tier::Bit,
    };
    let provider_id = form.provider_id.as_deref().unwrap_or("default");
    if state.providers.get(provider_id).is_none() {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({"error": format!("unknown provider_id: {provider_id}")})),
        )
            .into_response();
    }
    if let Err(e) = state
        .repo
        .upsert_pricing(
            &form.model_name,
            provider_id,
            form.provider_cost_input_per_1m,
            form.provider_cost_output_per_1m,
            form.bit_price_per_1m,
            form.node_price_per_1m,
            form.cluster_price_per_1m,
            &min_tier,
        )
        .await
    {
        tracing::error!(%e, model=%form.model_name, "failed to upsert pricing");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({"error": "internal server error"})),
        )
            .into_response();
    }
    axum::Json(serde_json::json!({"updated": form.model_name})).into_response()
}

async fn check_provider_connectivity(
    state: &AppState,
    provider: &crate::domain::entities::Provider,
) -> bool {
    let url = ProviderRegistry::endpoint_url(provider, "/v1/models");
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        state.client.get(&url).bearer_auth(&provider.api_key).send(),
    )
    .await;
    matches!(result, Ok(Ok(resp)) if resp.status().is_success())
}

async fn check_upstream_connectivity(state: &AppState) -> (bool, Vec<bool>) {
    let providers = state.providers.all_cached();
    let mut results = Vec::with_capacity(providers.len());
    for p in &providers {
        results.push(check_provider_connectivity(state, p).await);
    }
    let all_ok = results.iter().all(|&ok| ok);
    (all_ok, results)
}

pub async fn public_health() -> impl IntoResponse {
    axum::Json(serde_json::json!({"status": "ok"}))
}

pub async fn cache_stats(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.request_cache.stats())
}

pub async fn list_providers(State(state): State<Arc<AppState>>) -> Response {
    match state.repo.all_providers().await {
        Ok(providers) => axum::Json(serde_json::json!({
            "count": providers.len(),
            "providers": providers,
        }))
        .into_response(),
        Err(e) => {
            tracing::error!(%e, "failed to list providers");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({"error": "failed to list providers"})),
            )
                .into_response()
        }
    }
}

pub async fn upsert_provider(
    State(state): State<Arc<AppState>>,
    Json(form): Json<ProviderForm>,
) -> Response {
    if form.id.is_empty() || form.base_url.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({"error": "id and base_url are required"})),
        )
            .into_response();
    }

    let api_key = if form.api_key.is_empty() {
        match state.repo.provider_by_id(&form.id).await {
            Ok(Some(existing)) => existing.api_key,
            Ok(None) => {
                return (
                    StatusCode::BAD_REQUEST,
                    axum::Json(
                        serde_json::json!({"error": "api_key is required for new providers"}),
                    ),
                )
                    .into_response();
            }
            Err(e) => {
                tracing::error!(%e, provider_id=%form.id, "failed to look up existing provider");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    axum::Json(serde_json::json!({"error": "internal server error"})),
                )
                    .into_response();
            }
        }
    } else {
        form.api_key
    };
    let quantization = form.quantization.as_deref().unwrap_or("fp16");
    let context_length = form.context_length.unwrap_or(128_000);
    let max_output_length = form.max_output_length.unwrap_or(8_192);
    let datacenter_country = form.datacenter_country.as_deref().unwrap_or("US");

    if let Err(e) = state
        .repo
        .upsert_provider(
            &form.id,
            &form.name,
            &form.base_url,
            &api_key,
            quantization,
            context_length,
            max_output_length,
            datacenter_country,
        )
        .await
    {
        tracing::error!(%e, provider_id=%form.id, "failed to upsert provider");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({"error": "failed to upsert provider"})),
        )
            .into_response();
    }

    let _ = state.providers.refresh(state.repo.as_ref()).await;

    axum::Json(serde_json::json!({"updated": form.id})).into_response()
}

pub async fn delete_provider(
    State(state): State<Arc<AppState>>,
    Json(q): Json<DeleteProviderQuery>,
) -> Response {
    if q.id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(serde_json::json!({"error": "id is required"})),
        )
            .into_response();
    }
    if let Err(e) = state.repo.delete_provider(&q.id).await {
        tracing::error!(%e, provider_id=%q.id, "failed to delete provider");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({"error": "failed to delete provider"})),
        )
            .into_response();
    }

    let _ = state.providers.refresh(state.repo.as_ref()).await;

    axum::Json(serde_json::json!({"deleted": q.id})).into_response()
}

pub async fn refresh_providers(State(state): State<Arc<AppState>>) -> Response {
    match state.providers.refresh(state.repo.as_ref()).await {
        Ok(()) => axum::Json(serde_json::json!({"refreshed": true})).into_response(),
        Err(e) => {
            tracing::error!(%e, "failed to refresh provider cache");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({"error": "failed to refresh providers"})),
            )
                .into_response()
        }
    }
}

pub async fn dead_letters(State(state): State<Arc<AppState>>) -> Response {
    match state.repo.query_dead_letters(100).await {
        Ok(entries) => axum::Json(serde_json::json!({
            "count": entries.len(),
            "entries": entries,
        }))
        .into_response(),
        Err(e) => {
            tracing::error!(%e, "failed to query dead letter table");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({"error": "failed to query dead letters"})),
            )
                .into_response()
        }
    }
}
