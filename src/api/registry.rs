// registry.rs — Registry of all available LLM providers.
//
// Holds an `Arc<dyn LlmProvider>` for each registered provider and exposes
// lookup, health-check, and default-provider helpers.

use std::collections::HashMap;
use std::sync::Arc;

use crate::core::ProviderId;

use super::client::ClientConfig;
use super::provider::LlmProvider;
use super::provider_types::ProviderStatus;
use super::providers::{
    AnthropicProvider,
    OpenAiProvider,
};

fn normalize_openai_compat_base(override_base: &str) -> String {
    let trimmed = override_base.trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        trimmed.to_string()
    } else {
        format!("{}/v1", trimmed)
    }
}

fn normalize_openai_base(override_base: &str) -> String {
    let trimmed = override_base.trim_end_matches('/');
    if trimmed.ends_with("/v1") {
        trimmed.trim_end_matches("/v1").to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn resolve_provider_api_base(
    config: &crate::core::config::Config,
    provider_id: &str,
) -> Option<String> {
    let base = config.resolve_provider_api_base(provider_id)?;
    if provider_id == "openai" {
        Some(normalize_openai_base(&base))
    } else if super::providers::openai_compat_providers::provider_for_id(provider_id).is_some() {
        Some(normalize_openai_compat_base(&base))
    } else {
        Some(base)
    }
}

/// Registry of all available LLM providers.
/// Holds `Arc<dyn LlmProvider>` for each registered provider.
pub struct ProviderRegistry {
    providers: HashMap<ProviderId, Arc<dyn LlmProvider>>,
    default_provider_id: ProviderId,
}

fn provider_from_key(provider_id: &str, key: String) -> Option<Arc<dyn LlmProvider>> {
    use super::providers::openai_compat_providers as p;

    if let Some(provider) = p::provider_for_id(provider_id) {
        return Some(Arc::new(provider.with_api_key(key)));
    }

    match provider_id {
        "anthropic" => Some(Arc::new(AnthropicProvider::from_config(
            ClientConfig { api_key: key, ..Default::default() },
        ))),
        "openai" => Some(Arc::new(OpenAiProvider::new(key))),
        "custom-openai" => Some(Arc::new(p::custom_openai().with_api_key(key))),
        _ => None,
    }
}

pub fn provider_from_config(
    config: &crate::core::config::Config,
    provider_id: &str,
) -> Option<Arc<dyn LlmProvider>> {
    let provider_cfg = config.provider_configs.get(provider_id);
    if provider_cfg.is_some_and(|provider| !provider.enabled) {
        return None;
    }

    let api_key = config.resolve_provider_api_key(provider_id);
    let api_base = resolve_provider_api_base(config, provider_id).filter(|base| !base.is_empty());

    use super::providers;

    match provider_id {
        "anthropic" => None,
        "openai" => {
            let mut provider = OpenAiProvider::new(api_key.unwrap_or_default());
            if let Some(base) = api_base {
                provider = provider.with_base_url(base);
            }
            Some(Arc::new(provider))
        }
        "ollama" => {
            let mut provider = providers::ollama();
            if let Some(base) = api_base {
                provider = provider.with_base_url(base);
            }
            Some(Arc::new(provider))
        }
        "lmstudio" | "lm-studio" => {
            let mut provider = providers::lm_studio();
            if let Some(base) = api_base {
                provider = provider.with_base_url(base);
            }
            Some(Arc::new(provider))
        }
        "llamacpp" | "llama-cpp" | "llama-server" => {
            let mut provider = providers::llama_cpp();
            if let Some(base) = api_base {
                provider = provider.with_base_url(base);
            }
            Some(Arc::new(provider))
        }
        "deepseek" => {
            let mut provider = providers::deepseek();
            if let Some(key) = api_key {
                provider = provider.with_api_key(key);
            }
            if let Some(base) = api_base {
                provider = provider.with_base_url(base);
            }
            Some(Arc::new(provider))
        }
        "groq" => {
            let mut provider = providers::groq();
            if let Some(key) = api_key {
                provider = provider.with_api_key(key);
            }
            if let Some(base) = api_base {
                provider = provider.with_base_url(base);
            }
            Some(Arc::new(provider))
        }
        "xai" => {
            let mut provider = providers::xai();
            if let Some(key) = api_key {
                provider = provider.with_api_key(key);
            }
            if let Some(base) = api_base {
                provider = provider.with_base_url(base);
            }
            Some(Arc::new(provider))
        }
        "openrouter" => {
            let mut provider = providers::openrouter();
            if let Some(key) = api_key {
                provider = provider.with_api_key(key);
            }
            if let Some(base) = api_base {
                provider = provider.with_base_url(base);
            }
            Some(Arc::new(provider))
        }
        _ => api_key.and_then(|key| provider_from_key(provider_id, key)),
    }
}

pub fn runtime_provider_for(provider_id: &str) -> Option<Arc<dyn LlmProvider>> {
    use super::providers::openai_compat_providers as p;

    // Local providers never require an API key — build them directly so that
    // the auth-store bypass below doesn't silently drop them.
    // Accept both the hyphenated canonical IDs ("llama-cpp", "lm-studio") and
    // the non-hyphenated aliases ("llamacpp", "lmstudio") used throughout the
    // TUI / connect dialog.
    match provider_id {
        "ollama" => return Some(Arc::new(p::ollama())),
        "lmstudio" | "lm-studio" => return Some(Arc::new(p::lm_studio())),
        // "llama-server" is the binary name for the modern llama.cpp server.
        "llamacpp" | "llama-cpp" | "llama-server" => return Some(Arc::new(p::llama_cpp())),
        _ => {}
    }

    let auth_store = crate::core::AuthStore::load();
    let key = auth_store.api_key_for(provider_id)?;
    if key.is_empty() {
        return None;
    }
    provider_from_key(provider_id, key)
}

impl ProviderRegistry {
    /// Create an empty registry with Anthropic as the default provider ID.
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
            default_provider_id: ProviderId::new(ProviderId::ANTHROPIC),
        }
    }

    /// Register a provider. Returns `&mut self` for builder chaining.
    pub fn register(&mut self, provider: Arc<dyn LlmProvider>) -> &mut Self {
        let id = provider.id().clone();
        self.providers.insert(id, provider);
        self
    }

    /// Set the default provider by ID.
    ///
    /// # Panics
    /// Panics if no provider with that ID has been registered.
    pub fn set_default(&mut self, id: ProviderId) -> &mut Self {
        assert!(
            self.providers.contains_key(&id),
            "set_default: provider '{}' is not registered",
            id,
        );
        self.default_provider_id = id;
        self
    }

    /// Get a provider by ID.
    pub fn get(&self, id: &ProviderId) -> Option<&Arc<dyn LlmProvider>> {
        self.providers.get(id)
    }

    /// Get the default provider.
    pub fn default_provider(&self) -> Option<&Arc<dyn LlmProvider>> {
        self.providers.get(&self.default_provider_id)
    }

    /// Get the default provider ID.
    pub fn default_provider_id(&self) -> &ProviderId {
        &self.default_provider_id
    }

    /// List all registered provider IDs.
    pub fn provider_ids(&self) -> Vec<&ProviderId> {
        self.providers.keys().collect()
    }

    /// Check health of all providers sequentially.
    /// Returns `(provider_id, status)` pairs.
    pub async fn check_all_health(&self) -> Vec<(ProviderId, ProviderStatus)> {
        let mut results = Vec::new();
        for (id, provider) in &self.providers {
            let status = provider
                .health_check()
                .await
                .unwrap_or(ProviderStatus::Unavailable {
                    reason: "health check failed".to_string(),
                });
            results.push((id.clone(), status));
        }
        results
    }

    /// Convenience: build a registry with just Anthropic registered as the
    /// default provider.  Takes the same [`ClientConfig`] that
    /// [`AnthropicClient`] takes.
    ///
    /// [`AnthropicClient`]: crate::client::AnthropicClient
    pub fn with_anthropic(config: ClientConfig) -> Self {
        let mut registry = Self::new();
        let provider = Arc::new(AnthropicProvider::from_config(config));
        registry.register(provider);
        registry
    }

    pub fn from_config(
        config: &crate::core::config::Config,
        anthropic_config: ClientConfig,
    ) -> Self {
        let mut registry = Self::from_environment_with_auth_store(anthropic_config);
        let active_provider = config.selected_provider_id();

        let mut configured_provider_ids: Vec<String> = config
            .provider_configs
            .keys()
            .cloned()
            .collect();
        if configured_provider_ids.iter().all(|id| id != active_provider) {
            configured_provider_ids.push(active_provider.to_string());
        }

        for provider_id in configured_provider_ids {
            if let Some(provider) = provider_from_config(config, &provider_id) {
                registry.register(provider);
            }
        }

        let default_provider_id = ProviderId::new(active_provider);
        if registry.get(&default_provider_id).is_some() {
            registry.set_default(default_provider_id);
        }

        registry
    }

    /// Register [`OpenAiProvider`] if `OPENAI_API_KEY` is set in the
    /// environment.  Returns `&mut self` for builder chaining.
    pub fn with_openai_if_key_set(&mut self) -> &mut Self {
        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            let provider = Arc::new(OpenAiProvider::new(key));
            self.register(provider);
        }
        self
    }

    /// Build a registry with **all** providers that have credentials configured
    /// in the environment.  Anthropic is always the default provider.
    ///
    /// This is the recommended constructor for production use.
    pub fn from_environment(anthropic_config: ClientConfig) -> Self {
        let mut registry = Self::with_anthropic(anthropic_config);
        registry
            .with_openai_if_key_set()
            .with_available_providers();
        registry
    }

    /// Build a registry that checks **both** environment variables and the
    /// persistent [`AuthStore`] (`~/.claurst/auth.json`) for credentials.
    ///
    /// This ensures that API keys stored via `/connect` or `claurst auth` are
    /// picked up at startup, not just env vars.  Falls back to
    /// `from_environment` for providers that only support env-var config, and
    /// adds any extra providers that have keys in the auth store.
    ///
    /// [`AuthStore`]: crate::core::AuthStore
    pub fn from_environment_with_auth_store(anthropic_config: ClientConfig) -> Self {
        // Start with env-based registration.
        let mut registry = Self::from_environment(anthropic_config);

        // Now check the auth store for providers that weren't registered from
        // env vars.
        let auth_store = crate::core::AuthStore::load();

        for (provider_id, _cred) in &auth_store.credentials {
            let pid = crate::core::ProviderId::new(provider_id.as_str());
            // Skip if already registered from env vars.
            if registry.get(&pid).is_some() {
                continue;
            }
            // Try to get a usable key from the auth store.
            if let Some(key) = auth_store.api_key_for(provider_id) {
                if key.is_empty() {
                    continue;
                }
                let provider = provider_from_key(provider_id, key);
                if let Some(p) = provider {
                    registry.register(p);
                }
            }
        }

        registry
    }

    /// Register all providers that have environment variable credentials set.
    ///
    /// Local providers (Ollama, LM Studio, llama.cpp) are always registered
    /// regardless of credentials — `health_check()` will report them as
    /// unavailable if the server is not running.
    ///
    /// Remote API-key providers are only registered when their respective
    /// environment variables are set (non-empty).
    ///
    /// Returns `&mut self` for builder chaining.
    pub fn with_available_providers(&mut self) -> &mut Self {
        use super::providers::openai_compat_providers as p;

        // Local providers — always try to register.
        self.register(Arc::new(p::ollama()));
        self.register(Arc::new(p::lm_studio()));
        self.register(Arc::new(p::llama_cpp()));

        // Remote providers — only register when an API key is present.
        if std::env::var("DEEPSEEK_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::deepseek()));
        }
        if std::env::var("GROQ_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::groq()));
        }
        if std::env::var("XAI_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::xai()));
        }
        if std::env::var("OPENROUTER_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::openrouter()));
        }
        if std::env::var("TOGETHER_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::together_ai()));
        }
        if std::env::var("PERPLEXITY_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::perplexity()));
        }
        if std::env::var("CEREBRAS_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::cerebras()));
        }
        if std::env::var("DEEPINFRA_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::deepinfra()));
        }
        if std::env::var("VENICE_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::venice()));
        }
        if std::env::var("DASHSCOPE_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::qwen()));
        }
        if std::env::var("MISTRAL_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::mistral()));
        }
        if std::env::var("SAMBANOVA_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::sambanova()));
        }
        if std::env::var("HF_TOKEN").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::huggingface()));
        }
        if std::env::var("NVIDIA_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::nvidia()));
        }
        if std::env::var("SILICONFLOW_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::siliconflow()));
        }
        if std::env::var("MOONSHOT_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::moonshot()));
        }
        if std::env::var("ZHIPU_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::zhipu()));
        }
        if std::env::var("ZAI_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::zai()));
        }
        if std::env::var("NEBIUS_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::nebius()));
        }
        if std::env::var("NOVITA_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::novita()));
        }
        if std::env::var("OVHCLOUD_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::ovhcloud()));
        }
        if std::env::var("SCALEWAY_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::scaleway()));
        }
        if std::env::var("VULTR_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::vultr_ai()));
        }
        if std::env::var("BASETEN_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::baseten()));
        }
        if std::env::var("FRIENDLI_TOKEN").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::friendli()));
        }
        if std::env::var("UPSTAGE_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::upstage()));
        }
        if std::env::var("STEPFUN_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::stepfun()));
        }
        if std::env::var("FIREWORKS_API_KEY").map(|v| !v.is_empty()).unwrap_or(false) {
            self.register(Arc::new(p::fireworks()));
        }
        self
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}
