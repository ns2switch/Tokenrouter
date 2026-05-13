use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// OpenAI error format
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct OpenAiError {
    pub error: OpenAiErrorBody,
}

#[derive(Debug, Serialize)]
pub struct OpenAiErrorBody {
    pub message: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

// ---------------------------------------------------------------------------
// Provider (OpenRouter-compatible) model list
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ProviderModelsResponse {
    pub data: Vec<ProviderModel>,
}

#[derive(Debug, Serialize)]
pub struct ProviderModel {
    pub id: String,
    pub name: String,
    pub created: i64,
    pub input_modalities: Vec<String>,
    pub output_modalities: Vec<String>,
    pub quantization: String,
    pub context_length: i64,
    pub max_output_length: i64,
    pub pricing: ProviderPricing,
    pub supported_sampling_parameters: Vec<String>,
    pub supported_features: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deprecation_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openrouter: Option<ProviderOpenRouter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datacenters: Option<Vec<ProviderDatacenter>>,
}

#[derive(Debug, Serialize)]
pub struct ProviderPricing {
    pub prompt: String,
    pub completion: String,
    pub image: String,
    pub request: String,
    pub input_cache_read: String,
}

#[derive(Debug, Serialize)]
pub struct ProviderOpenRouter {
    pub slug: String,
}

#[derive(Debug, Serialize)]
pub struct ProviderDatacenter {
    pub country_code: String,
}

// ---------------------------------------------------------------------------
// OpenAI-compatible model list
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenAiModelsResponse {
    pub object: String,
    pub data: Vec<OpenAiModel>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OpenAiModel {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub owned_by: String,
}

// ---------------------------------------------------------------------------
// Admin forms / queries
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct PricingForm {
    pub model_name: String,
    pub provider_id: Option<String>,
    pub provider_cost_input_per_1m: f64,
    pub provider_cost_output_per_1m: f64,
    pub bit_price_per_1m: f64,
    pub node_price_per_1m: f64,
    pub cluster_price_per_1m: f64,
    #[serde(default)]
    pub min_tier: String,
}

#[derive(Debug, Deserialize)]
pub struct DashboardQuery {
    pub cursor: Option<String>,
    pub prev: Option<String>,
}

// ---------------------------------------------------------------------------
// Health check response
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub ok: bool,
    pub ttl_enabled: bool,
    pub dynamodb_write_ok: bool,
    pub upstream_ok: bool,
    pub inflight_requests: i64,
    pub checks: Vec<HealthCheck>,
}

#[derive(Debug, Serialize)]
pub struct HealthCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

// ---------------------------------------------------------------------------
// API key management
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateKeyRequest {
    pub tier: String,
    pub credit_limit: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct CreateKeyResponse {
    pub id: String,
    pub raw_key: String,
    pub tier: String,
}

#[derive(Debug, Serialize)]
pub struct KeyListItem {
    pub id: String,
    pub tier: String,
    pub balance_accumulated: f64,
    pub credit_limit: Option<f64>,
    pub active: bool,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// Provider management
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ProviderForm {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub quantization: Option<String>,
    pub context_length: Option<i64>,
    pub max_output_length: Option<i64>,
    pub datacenter_country: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteProviderQuery {
    pub id: String,
}
