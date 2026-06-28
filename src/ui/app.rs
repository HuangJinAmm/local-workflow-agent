// ui::app — top-level application state, owned by the GPUI root.
// Holds the tokio runtime, registry, storage, and in-flight turn tokens.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;

use crate::api::client::ClientConfig;
use crate::api::providers::{AnthropicProvider, OpenAiProvider};
use crate::api::registry::ProviderRegistry;
use crate::core::provider_id::ProviderId;
use crate::core::sqlite_storage::SqliteSessionStore;
use crate::tools::{all_tools, Tool};

use super::model::SessionId;
use super::settings::persistence;
use super::settings::Settings;
use super::storage::MessageStore;

pub struct AppState {
    pub runtime: Arc<Runtime>,
    pub providers: Arc<RwLock<ProviderRegistry>>,
    pub storage: Arc<SqliteSessionStore>,
    pub messages: Arc<MessageStore>,
    pub tools: Arc<Vec<Box<dyn Tool>>>,
    pub settings: Arc<RwLock<Settings>>,
    pub inflight: Arc<RwLock<HashMap<SessionId, CancellationToken>>>,
    pub attachments_dir: PathBuf,
    /// Reserved for future bash-cwd scoping; not read yet.
    pub working_dir: PathBuf,
}

impl AppState {
    pub fn new(working_dir: PathBuf) -> anyhow::Result<Self> {
        Self::with_data_dir(working_dir, default_data_dir())
    }

    pub fn with_data_dir(working_dir: PathBuf, data_dir: PathBuf) -> anyhow::Result<Self> {
        let runtime = Runtime::new()?;
        std::fs::create_dir_all(&data_dir)?;
        let attachments_dir = data_dir.join("attachments");
        std::fs::create_dir_all(&attachments_dir)?;

        let db_path = data_dir.join("agent.db");
        let messages = Arc::new(MessageStore::open(&db_path)?);
        let storage = Arc::new(SqliteSessionStore::open(&db_path)?);

        // Load persisted settings (provider, model, API keys, theme, tool policy).
        // Falls back to `Settings::default()` on missing/malformed file.
        let settings = Arc::new(RwLock::new(persistence::load(&data_dir)));
        let providers = Arc::new(RwLock::new(build_registry_from_settings(
            settings.read().clone(),
        )));
        let tools: Vec<Box<dyn Tool>> = all_tools();

        // Best-effort orphan sweep (failure logged, not fatal).
        if let Err(e) = messages.sweep_attachments(&attachments_dir) {
            tracing::warn!(?e, "attachment sweep failed");
        }

        Ok(Self {
            runtime: Arc::new(runtime),
            providers,
            storage,
            messages,
            tools: Arc::new(tools),
            settings,
            inflight: Arc::new(RwLock::new(HashMap::new())),
            attachments_dir,
            working_dir,
        })
    }

    pub fn cancel_turn(&self, session_id: &SessionId) {
        if let Some(token) = self.inflight.write().remove(session_id) {
            token.cancel();
        }
    }

    pub fn begin_turn(&self, session_id: SessionId) -> CancellationToken {
        let token = CancellationToken::new();
        self.inflight.write().insert(session_id, token.clone());
        token
    }

    /// Update the API key for `provider_id` in settings, persist to disk,
    /// and rebuild the in-memory `ProviderRegistry` so the next turn picks
    /// up the new key. If `key` is `None` or empty, the provider is removed.
    pub fn update_api_key(&self, provider_id: &str, key: Option<String>) -> anyhow::Result<()> {
        let cleaned = key.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        {
            let mut s = self.settings.write();
            match provider_id {
                "anthropic" => s.anthropic_api_key = cleaned.clone(),
                "openai" => s.openai_api_key = cleaned.clone(),
                _ => {}
            }
            persistence::save(&self.attachments_dir.parent().unwrap_or(&PathBuf::from(".")), &s)
                .map_err(|e| anyhow::anyhow!("save settings: {e}"))?;
        }
        self.reload_providers_from_settings();
        Ok(())
    }

    /// Switch the default provider + model. Persists the new selection.
    pub fn set_default_model(&self, provider: &str, model: &str) -> anyhow::Result<()> {
        {
            let mut s = self.settings.write();
            s.default_provider = provider.to_string();
            s.default_model = model.to_string();
            persistence::save(&self.attachments_dir.parent().unwrap_or(&PathBuf::from(".")), &s)
                .map_err(|e| anyhow::anyhow!("save settings: {e}"))?;
        }
        self.reload_providers_from_settings();
        Ok(())
    }

    /// Rebuild the in-memory `ProviderRegistry` from the current settings.
    /// Local-only providers are always re-attached; remote providers are
    /// only re-attached when an API key is present.
    pub fn reload_providers_from_settings(&self) {
        let snapshot = self.settings.read().clone();
        *self.providers.write() = build_registry_from_settings(snapshot);
    }
}

/// Build a `ProviderRegistry` from a `Settings` snapshot. Mirrors the
/// `from_environment_with_auth_store` builder but pulls keys from the
/// settings struct (which itself was seeded from env vars + persistence).
fn build_registry_from_settings(s: Settings) -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();

    // Anthropic — always register so the UI can surface a clear
    // "missing API key" error at stream time. Use the empty key if none.
    let anthropic_key = s
        .anthropic_api_key
        .clone()
        .unwrap_or_default();
    let anthropic = AnthropicProvider::from_config(ClientConfig {
        api_key: anthropic_key,
        ..ClientConfig::default()
    });
    registry.register(Arc::new(anthropic));

    // OpenAI — only register when a key is present.
    if let Some(key) = s.openai_api_key.clone() {
        if !key.trim().is_empty() {
            registry.register(Arc::new(OpenAiProvider::new(key)));
        }
    }

    // Always include local-only providers (no key required).
    registry.with_available_providers();

    // Set the configured default provider if it exists in the registry.
    let default_id = ProviderId::new(&s.default_provider);
    if registry.get(&default_id).is_some() {
        registry.set_default(default_id);
    } else {
        registry.set_default(ProviderId::new(ProviderId::ANTHROPIC));
    }

    registry
}

/// Resolve the default data directory. Honours `LWA_DATA_DIR` for tests / sandboxes.
fn default_data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("LWA_DATA_DIR") {
        return PathBuf::from(p);
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".local-workflow-agent")
}
