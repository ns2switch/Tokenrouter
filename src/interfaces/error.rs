use crate::interfaces::dto::OpenAiError;
use crate::interfaces::dto::OpenAiErrorBody;
use crate::runtime_metrics::RuntimeMetrics;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use std::sync::Arc;

/// Scoped guard that decrements the inflight request counter on drop.
/// Ensures the counter is always decremented even if the handler panics.
pub struct ScopedInflightGuard {
    metrics: Arc<RuntimeMetrics>,
}

impl ScopedInflightGuard {
    pub fn new(metrics: Arc<RuntimeMetrics>) -> Self {
        metrics.inc_inflight();
        Self { metrics }
    }
}

impl Drop for ScopedInflightGuard {
    fn drop(&mut self) {
        self.metrics.dec_inflight();
    }
}

/// Structured error for the chat completions gateway.
/// Carries status, message, and optional OpenAI-compatible param/code.
pub struct GateError {
    pub status: StatusCode,
    pub message: String,
    pub param: Option<String>,
    pub code: Option<String>,
}

impl GateError {
    pub fn into_response(self) -> Response {
        let body = axum::Json(OpenAiError {
            error: OpenAiErrorBody {
                message: self.message,
                kind: "gateway_error".to_string(),
                param: self.param,
                code: self.code,
            },
        });
        (self.status, body).into_response()
    }
}

/// Returns a 500 GateError with a sanitised message. The real error is logged
/// via tracing so operators can investigate.
pub fn internal_gate_error(err: impl std::fmt::Display) -> GateError {
    tracing::error!(%err, "internal error");
    GateError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: "internal server error".to_string(),
        param: None,
        code: None,
    }
}

/// Strips non-printable characters and truncates an upstream error body so
/// that internal details are not leaked to API consumers. JSON error bodies
/// are parsed and only their `error.message` field is returned.
pub fn sanitise_upstream_error(text: &str) -> String {
    const LIMIT: usize = 256;
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(msg) = v
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|v| v.as_str())
        {
            return msg.to_string();
        }
    }
    let printable: String = text
        .chars()
        .filter(|c| c.is_ascii_graphic() || c.is_ascii_whitespace())
        .take(LIMIT)
        .collect();
    if printable.len() < text.len() {
        format!("{printable}...")
    } else {
        printable
    }
}
