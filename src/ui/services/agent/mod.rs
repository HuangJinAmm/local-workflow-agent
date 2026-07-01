//! Agent module for the chat UI.
//!
//! Re-exports the UI ↔ agent channel types (`AgentRequest`, `AgentResponse`,
//! `UiMessage`, …) and the `Agent` wrapper that bridges them onto the
//! `local_workflow_agent` library's `LlmProvider` trait.
//!
//! File uploads are handled by the library's `api::uploads` module; the UI
//! handler calls it directly.

mod client;
mod messages;
mod permission_handler;
mod types;

// Re-export main client types.
pub use client::{Agent, AgentBuilder};

// Re-export message types.
pub use messages::{
    AgentRequest, AgentResponse, MessageMetadata, MessageRole, ToolCallData, ToolResultData,
    UiMessage,
};

// Re-export core types — thin aliases over the library's own types so the
// chat view does not need to know about `local_workflow_agent::core` paths.
pub use types::{Message, Tool};
// `ContentBlock` and `ToolDefinition` come from `core::types` via `types.rs`.
pub use crate::core::types::{ContentBlock, ToolDefinition};

// Re-export the GUI permission handler.
pub use permission_handler::{GuiPermissionHandler, PermissionRequest as GuiPermissionRequest};
