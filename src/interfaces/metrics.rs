use crate::state::AppState;
use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;
use std::sync::Arc;

pub async fn metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let body = state.runtime_metrics.render_prometheus();
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], body)
}
