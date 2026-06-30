//! Agent module for the chat UI.
//!
//! Re-exports the UI ↔ agent channel types (`AgentRequest`, `AgentResponse`,
//! `UiMessage`, …) and the `Agent` wrapper that bridges them onto the
//! `local_workflow_agent` library's `LlmProvider` trait.
//!
//! File uploads are still performed via the standalone `upload_file` helper,
//! which talks to Anthropic's Files API directly.

mod client;
mod files;
mod messages;
mod permission_handler;
mod types;

// Re-export main client types.
pub use client::{Agent, AgentBuilder, PROVIDER_PRESETS};

// Re-export files API.
pub use files::upload_file;

// Re-export message types.
pub use messages::{
    AgentRequest, AgentResponse, MessageMetadata, MessageRole, ToolCallData, ToolResultData,
    UiMessage,
};

// Re-export core types — thin aliases over the library's own types so the
// chat view does not need to know about `local_workflow_agent::core` paths.
pub use types::{ContentBlock, FileSource, Message, Tool, ToolDefinition};

// Re-export the GUI permission handler. The GUI-side `PermissionRequest` is
// re-exported as `GuiPermissionRequest` to avoid clashing with the library's
// own `core::permissions::PermissionRequest`.
pub use permission_handler::{GuiPermissionHandler, PermissionRequest as GuiPermissionRequest};
