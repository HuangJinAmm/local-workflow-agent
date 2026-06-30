//! GPUI-based chat UI for the local-workflow-agent.
//!
//! The UI shell (TitleBar / message list / input form / model select / file
//! attachments) is ported from the permissively-licensed
//! [duanebester/chat-ai](https://github.com/duanebester/chat-ai) project (MIT).
//! The agent layer underneath is replaced with this crate's `local_workflow_agent`
//! library — `api::providers::AnthropicProvider` plus the `tools` registry — so
//! that the chat can stream responses and execute tool calls through the
//! same code paths the CLI uses.

pub mod ask_modal;
pub mod assets;
pub mod chat;
pub mod handler;
pub mod permission_modal;
pub mod services;
pub mod settings;
pub mod settings_panel;
pub mod theme;
pub mod window;

pub use ask_modal::AskModal;
pub use assets::Assets;
pub use chat::ChatAI;
pub use permission_modal::{PermissionModal, PermissionRequest};
pub use settings::Settings;
pub use settings_panel::SettingsPanel;
