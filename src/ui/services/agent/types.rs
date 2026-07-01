//! Core types for the agent module.
//!
//! `Message` and `Tool` are UI-specific data carriers (the `Message` enum
//! shape differs from the library's `core::types::Message` struct, and `Tool`
//! is a UI-side data bag while the library has a `tools::Tool` trait). They
//! are kept here.
//!
//! The remaining types (`ContentBlock`, `ToolDefinition`) are re-exported
//! from the library's `core::types` so there is a single source of truth —
//! the translation layer in `client.rs` no longer needs to map between two
//! parallel definitions.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// Re-export library types — single source of truth.
pub use crate::core::types::{ContentBlock, ToolDefinition};

/// A tool that can be executed by the agent.
///
/// Mirrors `chat-ai`'s `Tool` shape — just enough metadata for the API
/// request. Execution is done through the library's `tools::Tool` trait
/// from inside `run_turn`.
#[derive(Clone)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Message in a conversation with the LLM.
///
/// UI enum form — the library's `core::types::Message` is a struct with a
/// `MessageContent` enum; the UI keeps its own enum shape because the
/// serialization path and chat-ai history differ. `client.rs::message_to_lib`
/// handles the enum → struct translation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    User {
        role: String,
        content: Vec<ContentBlock>,
    },
    Assistant {
        role: String,
        content: Vec<ContentBlock>,
    },
}
