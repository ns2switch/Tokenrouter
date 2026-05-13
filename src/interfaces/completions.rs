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

const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(60);

pub async fn completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(payload): axum::extract::Json<Value>,
) -> Response {
    let request_id = uuid::Uuid::new_v4().to_string();
    tracing::Span::current().record("request_id", &*request_id);
    let mut resp = do_completions(state.clone(), headers, payload).await;
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

async fn do_completions(state: Arc<AppState>, headers: HeaderMap, mut payload: Value) -> Response {
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

    let api_key = match state.repo.find_api_key_by_hash(&user_key_hash).await {
        Ok(Some(key)) => key,
        Ok(None) => {
            return error_response_with(
                StatusCode::UNAUTHORIZED,
                "api key not found",
                None,
                Some("invalid_api_key"),
            )
        }
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };

    tracing::Span::current().record("key_id", &*api_key.id);

    if state.billing.credit_exhausted(&api_key) {
        return error_response_with(
            StatusCode::PAYMENT_REQUIRED,
            "credit limit exhausted",
            None,
            Some("insufficient_quota"),
        );
    }

    let model = match payload.get("model").and_then(|v| v.as_str()) {
        Some(m) => m.to_string(),
        None => {
            return error_response_with(
                StatusCode::BAD_REQUEST,
                "missing model",
                Some("model"),
                Some("invalid_value"),
            )
        }
    };

    let pricing = match state.repo.pricing_for_model(&model).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return error_response_with(
                StatusCode::BAD_REQUEST,
                &format!("pricing not configured for model {model}"),
                Some("model"),
                Some("model_not_found"),
            )
        }
        Err(_e) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal server error")
        }
    };

    if api_key.tier < pricing.min_tier {
        return error_response_with(
            StatusCode::FORBIDDEN,
            &format!(
                "model {model} requires at least {} tier",
                format!("{:?}", pricing.min_tier).to_lowercase()
            ),
            Some("model"),
            Some("tier_insufficient"),
        );
    }

    let prompt_text = match payload.get("prompt") {
        Some(Value::String(s)) => s.as_str(),
        Some(Value::Array(_)) => "",
        Some(_) => {
            return error_response_with(
                StatusCode::BAD_REQUEST,
                "prompt must be a string or array",
                Some("prompt"),
                Some("invalid_value"),
            )
        }
        None => {
            return error_response_with(
                StatusCode::BAD_REQUEST,
                "missing required field: prompt",
                Some("prompt"),
                Some("invalid_value"),
            )
        }
    };
    let input_tokens = state.billing.count_tokens(prompt_text) as i64;

    let sell_price_per_1m = state.billing.effective_sell_price(&pricing, &api_key.tier);

    let provider = match state
        .providers
        .resolve_for_model(state.repo.as_ref(), &model)
        .await
    {
        Ok(p) => p,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let upstream_url = ProviderRegistry::endpoint_url(&provider, "/v1/completions");

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
            return error_response_with(
                StatusCode::CONFLICT,
                "duplicate idempotency key",
                None,
                Some("idempotency_key_reused"),
            );
        }
        return error_response_with(
            StatusCode::CONFLICT,
            "idempotency key reused with different payload",
            Some("idempotency-key"),
            Some("idempotency_key_reused"),
        );
    }

    if let Some(obj) = payload.as_object_mut() {
        let cap = state.max_output_tokens;
        if let Some(mt) = obj.get_mut("max_tokens") {
            if let Some(n) = mt.as_i64() {
                if n > cap {
                    *mt = serde_json::json!(cap);
                }
            }
        }
        if let Some(mct) = obj.get_mut("max_completion_tokens") {
            if let Some(n) = mct.as_i64() {
                if n > cap {
                    *mct = serde_json::json!(cap);
                }
            }
        }
    }

    let resp = match timeout(
        UPSTREAM_TIMEOUT,
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

    let mut body_json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);

    let output_text = body_json
        .get("choices")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let output_tokens = state.billing.count_tokens(&output_text) as i64;

    let total_tokens = input_tokens + output_tokens;
    let cost_basis = state
        .billing
        .provider_cost(input_tokens, output_tokens, &pricing);
    let revenue_generated = state.billing.cost(total_tokens, sell_price_per_1m);
    let response_body_str = serde_json::to_string(&body_json).unwrap_or_default();

    let _ = state
        .repo
        .record_transaction(
            &api_key,
            &model,
            input_tokens,
            output_tokens,
            cost_basis,
            revenue_generated,
            &idempotency_key,
            &request_hash,
            state.idempotency_ttl_seconds,
            &response_body_str,
        )
        .await;

    if body_json.get("usage").is_none() {
        if let Some(obj) = body_json.as_object_mut() {
            obj.insert(
                "usage".to_string(),
                serde_json::json!({
                    "prompt_tokens": input_tokens,
                    "completion_tokens": output_tokens,
                    "total_tokens": total_tokens,
                }),
            );
        }
    }

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

#[cfg(test)]
mod tests {
    use crate::domain::entities::Provider;
    use crate::provider_registry::ProviderRegistry;
    use chrono::Utc;

    fn test_provider(url: &str) -> Provider {
        Provider {
            id: "test".to_string(),
            name: "Test".to_string(),
            base_url: url.to_string(),
            api_key: "key".to_string(),
            quantization: "fp16".to_string(),
            context_length: 128000,
            max_output_length: 8192,
            datacenter_country: "US".to_string(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn upstream_url_strips_v1() {
        assert_eq!(
            ProviderRegistry::endpoint_url(
                &test_provider("https://api.example.com/v1"),
                "/v1/completions"
            ),
            "https://api.example.com/v1/completions"
        );
    }

    #[test]
    fn upstream_url_no_v1() {
        assert_eq!(
            ProviderRegistry::endpoint_url(
                &test_provider("https://api.example.com"),
                "/v1/completions"
            ),
            "https://api.example.com/v1/completions"
        );
    }

    #[test]
    fn upstream_url_trailing_slash() {
        assert_eq!(
            ProviderRegistry::endpoint_url(
                &test_provider("https://api.example.com/v1/"),
                "/v1/completions"
            ),
            "https://api.example.com/v1/completions"
        );
    }
}
