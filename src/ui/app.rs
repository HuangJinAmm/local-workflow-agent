// ui::app — top-level application state, owned by the GPUI root.
// Holds the tokio runtime, registry, storage, and in-flight turn tokens.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::RwLock;
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;

use crate::api::registry::ProviderRegistry;
use crate::core::sqlite_storage::SqliteSessionStore;
use crate::tools::{all_tools, Tool};

use super::model::SessionId;
use super::settings::Settings;
use super::storage::MessageStore;

pub struct AppState {
    pub runtime: Arc<Runtime>,
    pub providers: Arc<ProviderRegistry>,
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

        let providers = Arc::new(ProviderRegistry::new());
        let tools: Vec<Box<dyn Tool>> = all_tools();
        let settings = Arc::new(RwLock::new(Settings::default()));

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
}

/// Resolve the default data directory. Honours `LWA_DATA_DIR` for tests / sandboxes.
fn default_data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("LWA_DATA_DIR") {
        return PathBuf::from(p);
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".local-workflow-agent")
}
