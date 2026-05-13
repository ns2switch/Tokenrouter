use crate::interfaces::dto::OpenAiError;
use crate::interfaces::dto::OpenAiErrorBody;
use crate::provider_registry::ProviderRegistry;
use crate::security::hash_api_key;
use crate::state::AppState;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

pub async fn embeddings(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(payload): axum::extract::Json<Value>,
) -> Response {
    let request_id = uuid::Uuid::new_v4().to_string();
    tracing::Span::current().record("request_id", &*request_id);
    let mut resp = do_embeddings(state.clone(), headers, payload).await;
    resp.headers_mut().insert(
        HeaderName::from_static("x-request-id"),
        HeaderValue::from_str(&request_id).unwrap_or(HeaderValue::from_static("unknown")),
    );
    let remaining = state.inflight_limit.available_permits().to_string();
    resp.headers_mut().insert(
        HeaderName::from_static("x-ratelimit-remaining-requests"),
        HeaderValue::from_str(&remaining).unwrap_or(HeaderValue::from_static("0")),
    );
    resp
}

async fn do_embeddings(state: Arc<AppState>, headers: HeaderMap, payload: Value) -> Response {
    let _permit = match state.inflight_limit.clone().try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            return error_response_with(
                StatusCode::TOO_MANY_REQUESTS,
                "server is busy, retry later",
                None,
                Some("rate_limit_exceeded"),
            )
        }
    };

    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            error_response_with(
                StatusCode::UNAUTHORIZED,
                "missing authorization header",
                None,
                Some("invalid_api_key"),
            )
        });
    let user_key = match auth {
        Ok(v) => match v.strip_prefix("Bearer ") {
            Some(k) => k,
            None => {
                return error_response_with(
                    StatusCode::UNAUTHORIZED,
                    "invalid auth scheme",
                    None,
                    Some("invalid_api_key"),
                )
            }
        },
        Err(resp) => return resp,
    };

    let user_key_hash = hash_api_key(user_key, &state.api_key_hash_secret);

    let _per_key = match state.per_key_limiter.acquire_guard(&user_key_hash) {
        Some(g) => g,
        None => {
            return error_response_with(
                StatusCode::TOO_MANY_REQUESTS,
                "too many requests for this API key",
                None,
                Some("rate_limit_exceeded"),
            )
        }
    };

    match state.repo.find_api_key_by_hash(&user_key_hash).await {
        Ok(Some(key)) => {
            tracing::Span::current().record("key_id", &*key.id);
        }
        Ok(None) => {
            return error_response_with(
                StatusCode::UNAUTHORIZED,
                "api key not found",
                None,
                Some("invalid_api_key"),
            )
        }
        Err(_e) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal server error")
        }
    }

    let model = payload
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("default");
    let provider = match state
        .providers
        .resolve_for_model(state.repo.as_ref(), model)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(%e, "failed to resolve provider for embeddings");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal server error");
        }
    };
    let upstream_url = ProviderRegistry::endpoint_url(&provider, "/v1/embeddings");

    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let payload_json = serde_json::to_string(&payload).unwrap_or_default();
    let request_hash = crate::security::hash_payload(&payload_json, &state.api_key_hash_secret);

    if let Ok(Some(rec)) = state.repo.get_idempotency_record(&idempotency_key).await {
        if rec.request_hash == request_hash {
            if let Some(body) = rec.response_body {
                if let Ok(replayed) = serde_json::from_str::<Value>(&body) {
                    let mut resp = (StatusCode::OK, axum::Json(replayed)).into_response();
                    let remaining = state.inflight_limit.available_permits().to_string();
                    resp.headers_mut().insert(
                        HeaderName::from_static("x-ratelimit-remaining-requests"),
                        HeaderValue::from_str(&remaining).unwrap_or(HeaderValue::from_static("0")),
                    );
                    return resp;
                }
            }
            return error_response(StatusCode::CONFLICT, "duplicate idempotency key");
        }
        return error_response(
            StatusCode::CONFLICT,
            "idempotency key reused with different payload",
        );
    }

    let upstream_timeout = Duration::from_secs(state.upstream_timeout_seconds);
    let resp = match timeout(
        upstream_timeout,
        state
            .client
            .post(&upstream_url)
            .bearer_auth(&provider.api_key)
            .json(&payload)
            .send(),
    )
    .await
    {
        Ok(Ok(r)) => r,
        Ok(Err(_e)) => return error_response(StatusCode::BAD_GATEWAY, "upstream request failed"),
        Err(_) => return error_response(StatusCode::GATEWAY_TIMEOUT, "upstream request timed out"),
    };

    let status = resp.status();
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(_e) => {
            return error_response(StatusCode::BAD_GATEWAY, "failed to read upstream response")
        }
    };

    if !status.is_success() {
        let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        let msg = body
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("upstream request failed");
        return error_response(status, msg);
    }

    let body_json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let response_body_str = serde_json::to_string(&body_json).unwrap_or_default();
    let _ = state
        .repo
        .store_idempotency_record(
            &idempotency_key,
            &request_hash,
            state.idempotency_ttl_seconds,
            &response_body_str,
        )
        .await;
    (status, axum::Json(body_json)).into_response()
}

fn error_response(status: StatusCode, msg: &str) -> Response {
    error_response_with(status, msg, None, None)
}

fn error_response_with(
    status: StatusCode,
    msg: &str,
    param: Option<&str>,
    code: Option<&str>,
) -> Response {
    (
        status,
        axum::Json(OpenAiError {
            error: OpenAiErrorBody {
                message: msg.to_string(),
                kind: "gateway_error".to_string(),
                param: param.map(|s| s.to_string()),
                code: code.map(|s| s.to_string()),
            },
        }),
    )
        .into_response()
}
