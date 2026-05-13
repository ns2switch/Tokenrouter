use crate::interfaces::error::{
    internal_gate_error, sanitise_upstream_error, GateError, ScopedInflightGuard,
};
use crate::provider_registry::ProviderRegistry;
use crate::security::{hash_api_key, hash_payload};
use crate::state::AppState;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::Value;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::OwnedSemaphorePermit;
use tokio::time::{timeout, Duration, Instant};
use uuid::Uuid;

pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Json(payload): axum::extract::Json<Value>,
) -> Response {
    let request_id = Uuid::new_v4().to_string();
    tracing::Span::current().record("request_id", &*request_id);
    let model = payload
        .get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let mut resp = match do_chat_completions(state.clone(), headers, payload).await {
        Ok(resp) => resp,
        Err(err) => err.into_response(),
    };
    let status = resp.status().as_u16();
    tracing::info!(
        event = "request_completed",
        request_id = %request_id,
        model = model.as_deref().unwrap_or("unknown"),
        status = status,
    );
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

async fn do_chat_completions(
    state: Arc<AppState>,
    headers: HeaderMap,
    mut payload: Value,
) -> Result<Response, GateError> {
    let req_started = Instant::now();
    let _inflight_guard = ScopedInflightGuard::new(state.runtime_metrics.clone());
    let permit = state
        .inflight_limit
        .clone()
        .try_acquire_owned()
        .map_err(|_| GateError {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: "server is busy, retry later".to_string(),
            param: None,
            code: Some("rate_limit_exceeded".to_string()),
        })?;

    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(GateError {
            status: StatusCode::UNAUTHORIZED,
            message: "missing authorization header".to_string(),
            param: None,
            code: Some("invalid_api_key".to_string()),
        })?;
    let user_key = auth.strip_prefix("Bearer ").ok_or(GateError {
        status: StatusCode::UNAUTHORIZED,
        message: "invalid auth scheme".to_string(),
        param: None,
        code: Some("invalid_api_key".to_string()),
    })?;
    let user_key_hash = hash_api_key(user_key, &state.api_key_hash_secret);

    let _per_key = state
        .per_key_limiter
        .acquire_guard(&user_key_hash)
        .ok_or(GateError {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: "too many requests for this API key".to_string(),
            param: None,
            code: Some("rate_limit_exceeded".to_string()),
        })?;

    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let payload_json = serde_json::to_string(&payload).map_err(internal_err)?;
    let request_hash = hash_payload(&payload_json, &state.api_key_hash_secret);

    let model = payload
        .get("model")
        .and_then(|v| v.as_str())
        .ok_or(GateError {
            status: StatusCode::BAD_REQUEST,
            message: "missing model".to_string(),
            param: Some("model".to_string()),
            code: Some("invalid_value".to_string()),
        })?
        .to_string();
    let messages_arr = payload.get("messages").and_then(|v| v.as_array());
    const MAX_MESSAGES: usize = 512;
    match messages_arr {
        Some(arr) if arr.is_empty() => {
            return Err(GateError {
                status: StatusCode::BAD_REQUEST,
                message: "messages must be a non-empty array".to_string(),
                param: Some("messages".to_string()),
                code: Some("invalid_value".to_string()),
            });
        }
        Some(arr) if arr.len() > MAX_MESSAGES => {
            return Err(GateError {
                status: StatusCode::BAD_REQUEST,
                message: format!("messages array exceeds maximum of {MAX_MESSAGES}"),
                param: Some("messages".to_string()),
                code: Some("invalid_value".to_string()),
            });
        }
        Some(_) => {}
        None => {
            return Err(GateError {
                status: StatusCode::BAD_REQUEST,
                message: "missing required field: messages".to_string(),
                param: Some("messages".to_string()),
                code: Some("invalid_value".to_string()),
            });
        }
    }
    let stream = payload
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let include_usage = payload
        .get("stream_options")
        .and_then(|v| v.get("include_usage"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let api_key = state
        .repo
        .find_api_key_by_hash(&user_key_hash)
        .await
        .map_err(internal_err)?
        .ok_or(GateError {
            status: StatusCode::UNAUTHORIZED,
            message: "api key not found".to_string(),
            param: None,
            code: Some("invalid_api_key".to_string()),
        })?;

    tracing::Span::current().record("key_id", &*api_key.id);

    if state.billing.credit_exhausted(&api_key) {
        return Err(GateError {
            status: StatusCode::PAYMENT_REQUIRED,
            message: "credit limit exhausted".to_string(),
            param: None,
            code: Some("insufficient_quota".to_string()),
        });
    }

    let pricing = state
        .repo
        .pricing_for_model(&model)
        .await
        .map_err(internal_err)?
        .ok_or(GateError {
            status: StatusCode::BAD_REQUEST,
            message: format!("pricing not configured for model {model}"),
            param: Some("model".to_string()),
            code: Some("model_not_found".to_string()),
        })?;

    if api_key.tier < pricing.min_tier {
        return Err(GateError {
            status: StatusCode::FORBIDDEN,
            message: format!(
                "model {model} requires at least {} tier",
                format!("{:?}", pricing.min_tier).to_lowercase()
            ),
            param: Some("model".to_string()),
            code: Some("tier_insufficient".to_string()),
        });
    }

    let input_text = extract_input_text(&payload);
    let mut input_tokens = state.billing.count_tokens(&input_text) as i64;

    let image_tokens = estimate_image_tokens(&payload);
    input_tokens += image_tokens;

    if let Some(tools) = payload.get("tools") {
        let tools_json = serde_json::to_string(tools).unwrap_or_default();
        input_tokens += state.billing.count_tokens(&tools_json) as i64;
    }

    let sell_price_per_1m = state.billing.effective_sell_price(&pricing, &api_key.tier);

    // Check idempotency before upstream call
    if let Ok(Some(rec)) = state.repo.get_idempotency_record(&idempotency_key).await {
        if rec.request_hash == request_hash {
            if let Some(body) = rec.response_body {
                if let Ok(replayed) = serde_json::from_str::<Value>(&body) {
                    if stream {
                        let content_text = extract_message_text(
                            replayed
                                .get("choices")
                                .and_then(|v| v.as_array())
                                .and_then(|a| a.first())
                                .and_then(|x| x.get("message")),
                        );
                        let chunk = sse_content_chunk(&model, &content_text);
                        let full = format!(
                            "{}{}data: [DONE]\n\n",
                            chunk,
                            if include_usage {
                                let out_tok = state.billing.count_tokens(&content_text) as i64;
                                sse_usage_chunk(input_tokens, out_tok)
                            } else {
                                String::new()
                            }
                        );
                        let mut h = HeaderMap::new();
                        h.insert(
                            axum::http::header::CONTENT_TYPE,
                            HeaderValue::from_static("text/event-stream"),
                        );
                        return Ok((h, axum::body::Body::from(full)).into_response());
                    }
                    return Ok((StatusCode::OK, axum::Json(replayed)).into_response());
                }
            }
            return Err(GateError {
                status: StatusCode::CONFLICT,
                message: "duplicate idempotency key".to_string(),
                param: None,
                code: Some("idempotency_key_reused".to_string()),
            });
        }
        return Err(GateError {
            status: StatusCode::CONFLICT,
            message: "idempotency key reused with different payload".to_string(),
            param: Some("idempotency-key".to_string()),
            code: Some("idempotency_key_reused".to_string()),
        });
    }

    let provider = state
        .providers
        .resolve_for_model(state.repo.as_ref(), &model)
        .await
        .map_err(|e| {
            tracing::error!(%e, "failed to resolve provider for model {model}");
            GateError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: "failed to resolve provider".to_string(),
                param: None,
                code: Some("provider_error".to_string()),
            }
        })?;
    let upstream_url = ProviderRegistry::endpoint_url(&provider, "/v1/chat/completions");

    clamp_max_output_tokens(&mut payload, state.max_output_tokens);

    let cache_key = format!("{}:{model}:{request_hash}", provider.id);
    if let Some((cached_body, cached_in, cached_out)) = state.request_cache.get(&cache_key) {
        if stream {
            let content = cached_body.clone();
            let include = include_usage;
            let model_copy = model.clone();
            let metrics = Arc::clone(&state.runtime_metrics);
            let started = req_started;
            let boxed: Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>> = Box::pin(
                async_stream::try_stream! {
                    yield Bytes::from(sse_content_chunk(&model_copy, &content));
                    if include {
                        yield Bytes::from(sse_usage_chunk(cached_in, cached_out));
                    }
                    log_request_metrics(&metrics, &model_copy, started, None, cached_in, cached_out, StatusCode::OK.as_u16());
                    yield Bytes::from_static(b"data: [DONE]\n\n");
                },
            );
            let mut out_headers = HeaderMap::new();
            out_headers.insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("text/event-stream"),
            );
            return Ok((out_headers, axum::body::Body::from_stream(boxed)).into_response());
        }
        if let Ok(replayed) = serde_json::from_str::<Value>(&cached_body) {
            let mut resp = (StatusCode::OK, axum::Json(replayed)).into_response();
            resp.headers_mut().insert(
                HeaderName::from_static("x-ratelimit-remaining-requests"),
                HeaderValue::from_str(&state.inflight_limit.available_permits().to_string())
                    .unwrap_or(HeaderValue::from_static("0")),
            );
            log_request_metrics(
                &state.runtime_metrics,
                &model,
                req_started,
                None,
                cached_in,
                cached_out,
                StatusCode::OK.as_u16(),
            );
            return Ok(resp);
        }
    }

    let upstream_resp = call_upstream(&state.client, &upstream_url, &provider.api_key, &payload)
        .await
        .map_err(internal_err)?;

    if !upstream_resp.status().is_success() {
        let status = map_upstream_status(upstream_resp.status().as_u16());
        let text = upstream_resp
            .text()
            .await
            .unwrap_or_else(|_| "upstream error".to_string());
        let message = sanitise_upstream_error(&text);
        return Err(GateError {
            status,
            message,
            param: None,
            code: None,
        });
    }

    if stream {
        let mut out_headers = HeaderMap::new();
        out_headers.insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        let stream = stream_and_bill(
            upstream_resp.bytes_stream(),
            Arc::clone(&state.repo),
            Arc::clone(&state.runtime_metrics),
            Arc::clone(&state.request_cache),
            cache_key,
            api_key,
            model,
            input_tokens,
            pricing.provider_cost_input_per_1m,
            pricing.provider_cost_output_per_1m,
            sell_price_per_1m,
            idempotency_key,
            state.idempotency_ttl_seconds,
            req_started,
            permit,
            request_hash,
            state.billing.clone(),
            include_usage,
            state.max_streaming_seconds,
        );
        return Ok((out_headers, axum::body::Body::from_stream(stream)).into_response());
    }

    // Non-streaming path
    let bytes = upstream_resp.bytes().await.map_err(internal_err)?;
    let mut body_json: Value = serde_json::from_slice(&bytes).map_err(internal_err)?;
    if has_error_finish_reason(&body_json) {
        return Err(GateError {
            status: StatusCode::BAD_GATEWAY,
            message: "upstream returned finish_reason=error".to_string(),
            param: None,
            code: None,
        });
    }

    let output_text = extract_message_text(
        body_json
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|x| x.get("message")),
    );
    let output_tokens = state.billing.count_tokens(&output_text) as i64;

    let cost_basis = state
        .billing
        .provider_cost(input_tokens, output_tokens, &pricing);
    let total_tokens = input_tokens + output_tokens;
    let revenue_generated = state.billing.cost(total_tokens, sell_price_per_1m);
    let response_body_str = serde_json::to_string(&body_json).unwrap_or_default();

    if let Err(e) = state
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
        .await
    {
        let msg = e.to_string();
        if msg.contains("TransactionCanceledException") || msg.contains("ConditionalCheckFailed") {
            return match state.repo.get_idempotency_record(&idempotency_key).await {
                Ok(Some(rec)) if rec.request_hash == request_hash => {
                    if let Some(body) = rec.response_body {
                        if let Ok(replayed) = serde_json::from_str::<Value>(&body) {
                            return Ok((StatusCode::OK, axum::Json(replayed)).into_response());
                        }
                    }
                    Err(GateError {
                        status: StatusCode::CONFLICT,
                        message: "duplicate idempotency key".to_string(),
                        param: None,
                        code: Some("idempotency_key_reused".to_string()),
                    })
                }
                Ok(Some(_)) => Err(GateError {
                    status: StatusCode::CONFLICT,
                    message: "idempotency key reused with different payload".to_string(),
                    param: Some("idempotency-key".to_string()),
                    code: Some("idempotency_key_reused".to_string()),
                }),
                _ => Err(GateError {
                    status: StatusCode::CONFLICT,
                    message: "idempotency conflict".to_string(),
                    param: None,
                    code: Some("idempotency_key_reused".to_string()),
                }),
            };
        }
        return Err(internal_err(msg));
    }

    log_request_metrics(
        &state.runtime_metrics,
        &model,
        req_started,
        None,
        input_tokens,
        output_tokens,
        StatusCode::OK.as_u16(),
    );
    drop(permit);

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

    state
        .request_cache
        .put(&cache_key, &response_body_str, input_tokens, output_tokens);

    Ok((StatusCode::OK, axum::Json(body_json)).into_response())
}

#[allow(clippy::too_many_arguments)]
fn stream_and_bill<S>(
    mut upstream_stream: S,
    repo: Arc<dyn crate::domain::ports::Repository>,
    runtime_metrics: Arc<crate::runtime_metrics::RuntimeMetrics>,
    request_cache: Arc<crate::request_cache::RequestCache>,
    cache_key: String,
    api_key: crate::domain::entities::ApiKey,
    model_name: String,
    input_tokens: i64,
    cost_input_per_1m: f64,
    cost_output_per_1m: f64,
    sell_price_per_1m: f64,
    idempotency_key: String,
    idempotency_ttl_seconds: i64,
    req_started: Instant,
    _permit: OwnedSemaphorePermit,
    request_hash: String,
    billing_service: crate::application::billing::BillingService,
    include_usage: bool,
    max_streaming_seconds: u64,
) -> Pin<Box<dyn Stream<Item = Result<Bytes, std::io::Error>> + Send>>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Unpin + Send + 'static,
{
    let mut full_output = String::new();
    let mut done_seen = false;
    let mut error_seen = false;
    let keepalive_every = Duration::from_secs(15);
    let max_duration = Duration::from_secs(max_streaming_seconds);
    let mut first_token_at: Option<Instant> = None;

    let stream = async_stream::try_stream! {
        loop {
            if req_started.elapsed() > max_duration {
                break;
            }
            let maybe_chunk = match timeout(keepalive_every, upstream_stream.next()).await {
                Ok(v) => v,
                Err(_) => {
                    yield Bytes::from_static(b": keep-alive\n\n");
                    continue;
                }
            };

            let Some(chunk_result) = maybe_chunk else { break; };
            let chunk = chunk_result.map_err(to_io)?;
            let text = String::from_utf8_lossy(&chunk);
            if first_token_at.is_none() {
                first_token_at = Some(Instant::now());
            }

            for line in text.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    if data.trim() == "[DONE]" {
                        done_seen = true;
                        break;
                    }
                    if let Ok(v) = serde_json::from_str::<Value>(data) {
                        let first_choice = v
                            .get("choices")
                            .and_then(|c| c.as_array())
                            .and_then(|a| a.first());
                        if first_choice
                            .and_then(|c| c.get("finish_reason"))
                            .and_then(|r| r.as_str())
                            == Some("error")
                        {
                            error_seen = true;
                        }
                        if let Some(delta) = first_choice.and_then(|c| c.get("delta"))
                        {
                            if let Some(content) =
                                delta.get("content").and_then(|v| v.as_str())
                            {
                                full_output.push_str(content);
                            }
                            if let Some(tool_calls) =
                                delta.get("tool_calls").and_then(|v| v.as_array())
                            {
                                for tc in tool_calls {
                                    if let Some(func) = tc.get("function") {
                                        if let Some(name) =
                                            func.get("name").and_then(|v| v.as_str())
                                        {
                                            full_output.push_str(name);
                                        }
                                        if let Some(args) =
                                            func.get("arguments").and_then(|v| v.as_str())
                                        {
                                            full_output.push_str(args);
                                        }
                                    }
                                }
                            }
                            if let Some(func_call) = delta.get("function_call") {
                                if let Some(name) =
                                    func_call.get("name").and_then(|v| v.as_str())
                                {
                                    full_output.push_str(name);
                                }
                                if let Some(args) =
                                    func_call.get("arguments").and_then(|v| v.as_str())
                                {
                                    full_output.push_str(args);
                                }
                            }
                        }
                    }
                }
            }

            if !done_seen {
                yield chunk;
            } else {
                break;
            }
        }

        let output_tokens = billing_service.count_tokens(&full_output) as i64;
        let cost_basis = (input_tokens as f64 / 1_000_000.0) * cost_input_per_1m
            + (output_tokens as f64 / 1_000_000.0) * cost_output_per_1m;
        let total_tokens = input_tokens + output_tokens;
        let revenue_generated = (total_tokens as f64 / 1_000_000.0) * sell_price_per_1m;

        let status_code = if error_seen {
            StatusCode::BAD_GATEWAY.as_u16()
        } else {
            StatusCode::OK.as_u16()
        };

        if !error_seen {
            if let Err(e) = repo
                .record_transaction(
                    &api_key,
                    &model_name,
                    input_tokens,
                    output_tokens,
                    cost_basis,
                    revenue_generated,
                    &idempotency_key,
                    &request_hash,
                    idempotency_ttl_seconds,
                    &full_output,
                )
                .await
            {
                let msg = e.to_string();
                if msg.contains("TransactionCanceledException")
                    || msg.contains("ConditionalCheckFailed")
                {
                    if let Ok(Some(rec)) = repo.get_idempotency_record(&idempotency_key).await {
                        if rec.request_hash == request_hash {
                            if let Some(stored_text) = rec.response_body {
                                let chunk = sse_content_chunk(&model_name, &stored_text);
                                yield Bytes::from(chunk);
                                if include_usage {
                                    let replay_out =
                                        billing_service.count_tokens(&stored_text) as i64;
                                    yield Bytes::from(sse_usage_chunk(input_tokens, replay_out));
                                }
                            }
                        }
                    }
                    log_request_metrics(
                        runtime_metrics.as_ref(),
                        &model_name,
                        req_started,
                        first_token_at,
                        input_tokens,
                        output_tokens,
                        StatusCode::OK.as_u16(),
                    );
                    yield Bytes::from_static(b"data: [DONE]\n\n");
                    return;
                }
                Err(to_io(msg))?;
            }
        }

        log_request_metrics(
            runtime_metrics.as_ref(),
            &model_name,
            req_started,
            first_token_at,
            input_tokens,
            output_tokens,
            status_code,
        );
        if !error_seen {
            request_cache.put(&cache_key, &full_output, input_tokens, output_tokens);
        }
        if include_usage {
            yield Bytes::from(sse_usage_chunk(input_tokens, output_tokens));
        }
        yield Bytes::from_static(b"data: [DONE]\n\n");
    };

    Box::pin(stream)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn clamp_max_output_tokens(payload: &mut Value, max_tokens: i64) {
    if let Some(obj) = payload.as_object_mut() {
        if let Some(mt) = obj.get_mut("max_tokens") {
            if let Some(n) = mt.as_i64() {
                if n > max_tokens {
                    *mt = serde_json::json!(max_tokens);
                }
            }
        }
        if let Some(mct) = obj.get_mut("max_completion_tokens") {
            if let Some(n) = mct.as_i64() {
                if n > max_tokens {
                    *mct = serde_json::json!(max_tokens);
                }
            }
        }
    }
}

fn extract_input_text(payload: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(messages) = payload.get("messages").and_then(|v| v.as_array()) {
        for m in messages {
            let role = m.get("role").and_then(|v| v.as_str()).unwrap_or("user");
            let content = message_content_to_text(m.get("content"));
            parts.push(format!("{role}: {content}"));
        }
    }
    parts.join("\n")
}

fn message_content_to_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|part| {
                part.get("text")
                    .or_else(|| part.get("content"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .collect::<Vec<_>>()
            .join(" "),
        Some(v) => v.to_string(),
        None => String::new(),
    }
}

fn estimate_image_tokens(payload: &Value) -> i64 {
    const LOW_DETAIL_TOKENS: i64 = 85;
    const HIGH_DETAIL_BASE: i64 = 85;
    const HIGH_DETAIL_TILE_TOKENS: i64 = 170;
    const HIGH_DETAIL_DEFAULT_TILES: i64 = 6;

    let messages = match payload.get("messages").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return 0,
    };

    let mut total = 0i64;
    for msg in messages {
        let content = match msg.get("content") {
            Some(Value::Array(arr)) => arr,
            _ => continue,
        };
        for part in content {
            let part_type = part.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if part_type != "image_url" {
                continue;
            }
            let detail = part
                .get("image_url")
                .and_then(|iu| iu.get("detail"))
                .and_then(|v| v.as_str())
                .unwrap_or("auto");

            match detail {
                "low" => total += LOW_DETAIL_TOKENS,
                "high" => {
                    total += HIGH_DETAIL_BASE + HIGH_DETAIL_TILE_TOKENS * HIGH_DETAIL_DEFAULT_TILES
                }
                _ => total += LOW_DETAIL_TOKENS,
            }
        }
    }
    total
}

fn extract_message_text(message: Option<&Value>) -> String {
    let mut parts = Vec::new();
    let msg = match message {
        Some(m) => m,
        None => return String::new(),
    };
    if let Some(content) = msg.get("content").and_then(|v| v.as_str()) {
        parts.push(content.to_string());
    }
    if let Some(tool_calls) = msg.get("tool_calls").and_then(|v| v.as_array()) {
        for tc in tool_calls {
            if let Some(func) = tc.get("function") {
                if let Some(name) = func.get("name").and_then(|v| v.as_str()) {
                    parts.push(name.to_string());
                }
                if let Some(args) = func.get("arguments").and_then(|v| v.as_str()) {
                    parts.push(args.to_string());
                }
            }
        }
    }
    if let Some(func_call) = msg.get("function_call") {
        if let Some(name) = func_call.get("name").and_then(|v| v.as_str()) {
            parts.push(name.to_string());
        }
        if let Some(args) = func_call.get("arguments").and_then(|v| v.as_str()) {
            parts.push(args.to_string());
        }
    }
    parts.join(" ")
}

fn sse_content_chunk(model: &str, content: &str) -> String {
    let body = serde_json::json!({
        "choices": [{"delta": {"content": content}, "finish_reason": "stop"}],
        "model": model
    });
    format!("data: {body}\n\n")
}

fn sse_usage_chunk(prompt_tokens: i64, completion_tokens: i64) -> String {
    let body = serde_json::json!({
        "choices": [],
        "usage": {
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": prompt_tokens + completion_tokens,
        }
    });
    format!("data: {body}\n\n")
}

async fn call_upstream(
    client: &reqwest::Client,
    url: &str,
    upstream_api_key: &str,
    payload: &Value,
) -> Result<reqwest::Response, reqwest::Error> {
    let retry_delays = [Duration::from_millis(100), Duration::from_millis(300)];

    for delay in retry_delays.iter() {
        let resp = client
            .post(url)
            .bearer_auth(upstream_api_key)
            .json(payload)
            .send()
            .await?;
        let status = resp.status().as_u16();
        if status < 500 {
            return Ok(resp);
        }
        let _ = resp.bytes().await;
        tokio::time::sleep(*delay).await;
    }
    client
        .post(url)
        .bearer_auth(upstream_api_key)
        .json(payload)
        .send()
        .await
}

fn has_error_finish_reason(body: &Value) -> bool {
    body.get("choices")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|c| c.get("finish_reason"))
        .and_then(|v| v.as_str())
        .map(|r| r.eq_ignore_ascii_case("error"))
        .unwrap_or(false)
}

fn map_upstream_status(code: u16) -> StatusCode {
    match code {
        400 => StatusCode::BAD_REQUEST,
        401 => StatusCode::UNAUTHORIZED,
        402 => StatusCode::PAYMENT_REQUIRED,
        403 => StatusCode::FORBIDDEN,
        404 => StatusCode::NOT_FOUND,
        413 => StatusCode::PAYLOAD_TOO_LARGE,
        429 => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::BAD_GATEWAY,
    }
}

fn log_request_metrics(
    runtime_metrics: &crate::runtime_metrics::RuntimeMetrics,
    model: &str,
    started: Instant,
    first_token_at: Option<Instant>,
    input_tokens: i64,
    output_tokens: i64,
    status_code: u16,
) {
    let total_secs = started.elapsed().as_secs_f64().max(0.000_001);
    let ttft_ms = first_token_at
        .map(|t| t.duration_since(started).as_millis() as u64)
        .unwrap_or(0);
    let throughput_tps = output_tokens as f64 / total_secs;
    runtime_metrics.record_request(
        model,
        status_code,
        (total_secs * 1000.0) as u64,
        ttft_ms,
        throughput_tps,
    );
    tracing::info!(
        event = "provider_request_metrics",
        model = model,
        status_code = status_code,
        input_tokens = input_tokens,
        output_tokens = output_tokens,
        total_duration_ms = (total_secs * 1000.0) as u64,
        ttft_ms = ttft_ms,
        throughput_tps = throughput_tps,
    );
}

fn internal_err<E: std::fmt::Display>(err: E) -> GateError {
    internal_gate_error(err)
}

fn to_io<E: std::fmt::Display>(err: E) -> std::io::Error {
    std::io::Error::other(err.to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_provider(url: &str) -> crate::domain::entities::Provider {
        crate::domain::entities::Provider {
            id: "test".to_string(),
            name: "Test".to_string(),
            base_url: url.to_string(),
            api_key: "key".to_string(),
            quantization: "fp16".to_string(),
            context_length: 128000,
            max_output_length: 8192,
            datacenter_country: "US".to_string(),
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn upstream_url_strips_v1() {
        assert_eq!(
            super::ProviderRegistry::endpoint_url(
                &test_provider("https://api.example.com/v1"),
                "/v1/chat/completions"
            ),
            "https://api.example.com/v1/chat/completions"
        );
    }

    #[test]
    fn upstream_url_no_v1() {
        assert_eq!(
            super::ProviderRegistry::endpoint_url(
                &test_provider("https://api.example.com"),
                "/v1/chat/completions"
            ),
            "https://api.example.com/v1/chat/completions"
        );
    }

    #[test]
    fn upstream_url_trailing_slash() {
        assert_eq!(
            super::ProviderRegistry::endpoint_url(
                &test_provider("https://api.example.com/v1/"),
                "/v1/chat/completions"
            ),
            "https://api.example.com/v1/chat/completions"
        );
    }

    #[test]
    fn map_upstream_status_passthrough() {
        assert_eq!(map_upstream_status(400), StatusCode::BAD_REQUEST);
        assert_eq!(map_upstream_status(401), StatusCode::UNAUTHORIZED);
        assert_eq!(map_upstream_status(429), StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn map_upstream_status_5xx_becomes_502() {
        assert_eq!(map_upstream_status(500), StatusCode::BAD_GATEWAY);
        assert_eq!(map_upstream_status(503), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn has_error_finish_reason_true() {
        let body = json!({"choices": [{"finish_reason": "error"}]});
        assert!(has_error_finish_reason(&body));
    }

    #[test]
    fn has_error_finish_reason_false() {
        assert!(!has_error_finish_reason(
            &json!({"choices": [{"finish_reason": "stop"}]})
        ));
        assert!(!has_error_finish_reason(&json!({"choices": []})));
    }

    #[test]
    fn extract_input_text_string_content() {
        let p = json!({"messages": [{"role": "user", "content": "hello"}]});
        assert_eq!(extract_input_text(&p), "user: hello");
    }

    #[test]
    fn extract_input_text_array_content() {
        let p = json!({
            "messages": [{"role": "user", "content": [{"type": "text", "text": "hi"}]}]
        });
        assert!(extract_input_text(&p).contains("hi"));
    }

    #[test]
    fn message_content_to_text_none() {
        assert_eq!(message_content_to_text(None), "");
    }

    #[test]
    fn sse_content_chunk_valid() {
        let chunk = sse_content_chunk("llama-3", "hello");
        assert!(chunk.starts_with("data: "));
        assert!(chunk.ends_with("\n\n"));
        let json_part = chunk.trim_start_matches("data: ").trim_end();
        let v: Value = serde_json::from_str(json_part).unwrap();
        assert_eq!(
            v["choices"][0]["delta"]["content"].as_str().unwrap(),
            "hello"
        );
    }

    #[test]
    fn sse_content_chunk_escapes_newlines() {
        let chunk = sse_content_chunk("m", "line1\nline2");
        let json_part = chunk.trim_start_matches("data: ").trim_end();
        let v: Value = serde_json::from_str(json_part).unwrap();
        assert_eq!(
            v["choices"][0]["delta"]["content"].as_str().unwrap(),
            "line1\nline2"
        );
    }

    #[test]
    fn extract_message_text_content_only() {
        let msg = json!({"content": "hello"});
        assert_eq!(extract_message_text(Some(&msg)), "hello");
    }

    #[test]
    fn extract_message_text_tool_calls_only() {
        let msg = json!({
            "tool_calls": [{
                "function": {
                    "name": "get_weather",
                    "arguments": "{\"location\":\"NYC\"}"
                }
            }]
        });
        assert_eq!(
            extract_message_text(Some(&msg)),
            "get_weather {\"location\":\"NYC\"}"
        );
    }

    #[test]
    fn extract_message_text_content_and_tool_calls() {
        let msg = json!({
            "content": "Let me check",
            "tool_calls": [{
                "function": {"name": "search", "arguments": "{}"}
            }]
        });
        assert_eq!(extract_message_text(Some(&msg)), "Let me check search {}");
    }

    #[test]
    fn extract_message_text_none() {
        assert_eq!(extract_message_text(None), "");
    }

    #[test]
    fn extract_message_text_function_call() {
        let msg = json!({
            "function_call": {
                "name": "get_weather",
                "arguments": "{\"location\":\"NYC\"}"
            }
        });
        assert_eq!(
            extract_message_text(Some(&msg)),
            "get_weather {\"location\":\"NYC\"}"
        );
    }

    #[test]
    fn sse_usage_chunk_format() {
        let chunk = sse_usage_chunk(100, 50);
        assert!(chunk.starts_with("data: "));
        assert!(chunk.ends_with("\n\n"));
        let json_part = chunk.trim_start_matches("data: ").trim_end();
        let v: Value = serde_json::from_str(json_part).unwrap();
        assert_eq!(v["choices"].as_array().unwrap().len(), 0);
        assert_eq!(v["usage"]["prompt_tokens"].as_i64().unwrap(), 100);
        assert_eq!(v["usage"]["completion_tokens"].as_i64().unwrap(), 50);
        assert_eq!(v["usage"]["total_tokens"].as_i64().unwrap(), 150);
    }

    #[test]
    fn extract_input_text_tool_role() {
        let p = json!({
            "messages": [
                {"role": "system", "content": "You are helpful"},
                {"role": "user", "content": "What is the weather?"},
                {"role": "assistant", "content": null, "tool_calls": [{"function": {"name": "get_weather", "arguments": "{}"}}]},
                {"role": "tool", "content": "Sunny, 72F", "tool_call_id": "call_1"}
            ]
        });
        let text = extract_input_text(&p);
        assert!(text.contains("tool: Sunny, 72F"));
        assert!(text.contains("system: You are helpful"));
    }

    #[test]
    fn extract_input_text_empty_messages() {
        let p = json!({"messages": []});
        assert_eq!(extract_input_text(&p), "");
    }

    #[test]
    fn extract_message_text_empty_object() {
        let msg = json!({});
        assert_eq!(extract_message_text(Some(&msg)), "");
    }

    #[test]
    fn message_content_to_text_image_only() {
        let content =
            json!([{"type": "image_url", "image_url": {"url": "http://example.com/img.png"}}]);
        assert_eq!(message_content_to_text(Some(&content)), "");
    }
}
