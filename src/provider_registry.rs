use crate::domain::entities::Provider;
use crate::domain::ports::{PricingPort, ProviderPort};
use std::collections::HashMap;
use std::sync::RwLock;

pub struct ProviderRegistry {
    default_provider: Provider,
    cache: RwLock<HashMap<String, Provider>>,
}

impl ProviderRegistry {
    pub fn new(default_provider: Provider) -> Self {
        Self {
            default_provider,
            cache: RwLock::new(HashMap::new()),
        }
    }

    pub async fn refresh(&self, repo: &dyn ProviderPort) -> anyhow::Result<()> {
        let providers = repo.all_providers().await?;
        let mut cache = self.cache.write().expect("provider cache poisoned");
        cache.clear();
        for p in providers {
            cache.insert(p.id.clone(), p);
        }
        Ok(())
    }

    pub fn get(&self, provider_id: &str) -> Option<Provider> {
        let cache = self.cache.read().expect("provider cache poisoned");
        if let Some(p) = cache.get(provider_id) {
            return Some(p.clone());
        }
        if provider_id == "default" {
            return Some(self.default_provider.clone());
        }
        tracing::warn!(
            provider_id = provider_id,
            "provider not in cache, returning None"
        );
        None
    }

    pub fn get_default(&self) -> Provider {
        self.default_provider.clone()
    }

    pub fn all_cached(&self) -> Vec<Provider> {
        let cache = self.cache.read().expect("provider cache poisoned");
        let mut providers: Vec<Provider> = cache.values().cloned().collect();
        providers.push(self.default_provider.clone());
        providers
    }

    pub async fn resolve_for_model(
        &self,
        repo: &dyn PricingPort,
        model: &str,
    ) -> anyhow::Result<Provider> {
        let provider_id = match repo.pricing_for_model(model).await? {
            Some(p) => p.provider_id,
            None => "default".to_string(),
        };
        self.get(&provider_id)
            .ok_or_else(|| anyhow::anyhow!("provider {provider_id} not found for model {model}"))
    }

    pub fn endpoint_url(provider: &Provider, path: &str) -> String {
        let base = provider.base_url.trim_end_matches('/');
        if let Some(rest) = base.strip_suffix("/v1") {
            format!("{rest}{path}")
        } else {
            format!("{base}{path}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_provider(id: &str) -> Provider {
        Provider {
            id: id.to_string(),
            name: format!("Provider {id}"),
            base_url: format!("https://{id}.example.com/v1"),
            api_key: "sk-test".to_string(),
            quantization: "fp16".to_string(),
            context_length: 128000,
            max_output_length: 8192,
            datacenter_country: "US".to_string(),
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn get_default_always_returns_some() {
        let reg = ProviderRegistry::new(make_provider("default"));
        assert!(reg.get("default").is_some());
    }

    #[test]
    fn get_unknown_returns_none() {
        let reg = ProviderRegistry::new(make_provider("default"));
        assert!(reg.get("custom-gpu").is_none());
    }

    #[test]
    fn get_default_shortcut_works() {
        let def = make_provider("default");
        let reg = ProviderRegistry::new(def.clone());
        assert_eq!(reg.get_default().id, def.id);
        assert_eq!(reg.get_default().name, def.name);
    }

    #[test]
    fn all_cached_includes_default() {
        let def = make_provider("default");
        let reg = ProviderRegistry::new(def.clone());
        let all = reg.all_cached();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "default");
    }

    #[test]
    fn get_cached_provider_after_insert() {
        let reg = ProviderRegistry::new(make_provider("default"));
        let p = make_provider("other");
        {
            let mut cache = reg.cache.write().unwrap();
            cache.insert("other".to_string(), p.clone());
        }
        let found = reg.get("other");
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, "other");
    }
}
