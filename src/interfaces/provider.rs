use crate::domain::entities::PricingConfig;
use crate::interfaces::dto::{
    OpenAiModel, OpenAiModelsResponse, ProviderDatacenter, ProviderModel, ProviderModelsResponse,
    ProviderOpenRouter, ProviderPricing,
};
use crate::provider_registry::ProviderRegistry;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Deserialize, Default)]
pub struct ModelsQuery {
    pub format: Option<String>,
}

pub async fn list_models(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ModelsQuery>,
) -> Response {
    if q.format.as_deref() == Some("openai") {
        return list_models_openai_inner(state).await.into_response();
    }

    let pricing = state.repo.all_pricing().await.unwrap_or_default();
    let pricing_map: HashMap<String, &PricingConfig> =
        pricing.iter().map(|p| (p.model_name.clone(), p)).collect();

    let mut data: Vec<ProviderModel> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let created = chrono::Utc::now().timestamp();

    let all_providers = state.providers.all_cached();
    for provider in &all_providers {
        let upstream_models = fetch_upstream_models(&state, provider).await;

        for um in upstream_models {
            let model_id = um.id.clone();
            let short_name = model_id
                .strip_prefix(&format!("{}/", provider.name))
                .unwrap_or(&model_id)
                .to_string();
            seen.insert(short_name.clone());

            let (sell_price, modalities) = if let Some(p) = pricing_map.get(&short_name) {
                (p.bit_price_per_1m, vec!["text".to_string()])
            } else {
                (0.0, vec!["text".to_string()])
            };

            data.push(ProviderModel {
                id: model_id,
                name: format!("{}/{}", provider.name, &short_name),
                created,
                input_modalities: modalities.clone(),
                output_modalities: modalities,
                quantization: provider.quantization.clone(),
                context_length: provider.context_length,
                max_output_length: provider.max_output_length,
                pricing: ProviderPricing {
                    prompt: format_token_price(sell_price),
                    completion: format_token_price(sell_price),
                    image: "0".to_string(),
                    request: "0".to_string(),
                    input_cache_read: "0".to_string(),
                },
                supported_sampling_parameters: vec![
                    "temperature".to_string(),
                    "top_p".to_string(),
                    "stop".to_string(),
                    "max_tokens".to_string(),
                ],
                supported_features: vec![
                    "tools".to_string(),
                    "json_mode".to_string(),
                    "structured_outputs".to_string(),
                    "reasoning".to_string(),
                ],
                description: None,
                deprecation_date: None,
                openrouter: Some(ProviderOpenRouter {
                    slug: format!("{}/{}", provider.name.to_lowercase(), &short_name),
                }),
                datacenters: Some(vec![ProviderDatacenter {
                    country_code: provider.datacenter_country.clone(),
                }]),
            });
        }
    }

    for p in &pricing {
        if !seen.contains(&p.model_name) {
            let default_provider = state.providers.get_default();
            data.push(ProviderModel {
                id: format!("{}/{}", default_provider.name, p.model_name),
                name: format!("{}/{}", default_provider.name, p.model_name),
                created,
                input_modalities: vec!["text".to_string()],
                output_modalities: vec!["text".to_string()],
                quantization: default_provider.quantization.clone(),
                context_length: default_provider.context_length,
                max_output_length: default_provider.max_output_length,
                pricing: ProviderPricing {
                    prompt: format_token_price(p.bit_price_per_1m),
                    completion: format_token_price(p.bit_price_per_1m),
                    image: "0".to_string(),
                    request: "0".to_string(),
                    input_cache_read: "0".to_string(),
                },
                supported_sampling_parameters: vec![
                    "temperature".to_string(),
                    "top_p".to_string(),
                    "stop".to_string(),
                    "max_tokens".to_string(),
                ],
                supported_features: vec![
                    "tools".to_string(),
                    "json_mode".to_string(),
                    "structured_outputs".to_string(),
                    "reasoning".to_string(),
                ],
                description: None,
                deprecation_date: None,
                openrouter: Some(ProviderOpenRouter {
                    slug: format!("{}/{}", default_provider.name.to_lowercase(), p.model_name),
                }),
                datacenters: Some(vec![ProviderDatacenter {
                    country_code: default_provider.datacenter_country.clone(),
                }]),
            });
        }
    }

    axum::Json(ProviderModelsResponse { data }).into_response()
}

async fn fetch_upstream_models(
    state: &AppState,
    provider: &crate::domain::entities::Provider,
) -> Vec<OpenAiModel> {
    let url = ProviderRegistry::endpoint_url(provider, "/v1/models");
    match state
        .client
        .get(&url)
        .bearer_auth(&provider.api_key)
        .send()
        .await
    {
        Ok(resp) => {
            if let Ok(json) = resp.json::<OpenAiModelsResponse>().await {
                return json.data;
            }
        }
        Err(e) => {
            tracing::warn!(%e, provider_id=%provider.id, "failed to fetch upstream models, falling back to local only");
        }
    }
    vec![]
}

pub async fn list_models_openai(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    list_models_openai_inner(state).await
}

async fn list_models_openai_inner(state: Arc<AppState>) -> impl IntoResponse {
    let pricing = state.repo.all_pricing().await.unwrap_or_default();
    let created = chrono::Utc::now().timestamp();

    let pricing_names: std::collections::HashSet<String> =
        pricing.iter().map(|p| p.model_name.clone()).collect();
    let mut data: Vec<OpenAiModel> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let all_providers = state.providers.all_cached();
    for provider in &all_providers {
        let upstream_models = fetch_upstream_models(&state, provider).await;

        for um in upstream_models {
            seen.insert(um.id.clone());
            data.push(OpenAiModel {
                id: um.id,
                object: "model".to_string(),
                created,
                owned_by: provider.name.clone(),
            });
        }
    }

    let default_provider = state.providers.get_default();
    for p in &pricing {
        let full_name = format!("{}/{}", default_provider.name, p.model_name);
        if !seen.contains(&full_name) {
            data.push(OpenAiModel {
                id: full_name,
                object: "model".to_string(),
                created,
                owned_by: default_provider.name.clone(),
            });
        }
    }

    if data.is_empty() && pricing_names.is_empty() {
        data.push(OpenAiModel {
            id: "nogpu".to_string(),
            object: "model".to_string(),
            created,
            owned_by: default_provider.name.clone(),
        });
    }

    axum::Json(OpenAiModelsResponse {
        object: "list".to_string(),
        data,
    })
}

pub async fn upstream_models(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let default_provider = state.providers.get_default();
    let models = fetch_upstream_models(&state, &default_provider).await;
    axum::Json(serde_json::json!({
        "count": models.len(),
        "models": models.iter().map(|m| &m.id).collect::<Vec<_>>(),
    }))
}

fn format_token_price(price_per_1m: f64) -> String {
    format!("{:.10}", price_per_1m / 1_000_000.0)
}
