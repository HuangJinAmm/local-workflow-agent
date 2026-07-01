//! Core types for the agent module.
//!
//! These are the same wire-format types chat-ai uses internally — `Message`,
//! `ContentBlock`, `FileSource`, `Tool`, `ToolDefinition`. They intentionally
//! mirror the Anthropic Messages API JSON shape (so they serialise straight
//! to the request body) and are kept separate from the library's richer
//! `core::types::*` so the UI code ported from chat-ai needs no changes.
//!
//! `Agent` (in `client.rs`) is responsible for translating these into the
//! library's `ProviderRequest` / `Message` / `ContentBlock` types before
//! calling into `local_workflow_agent::api::AnthropicProvider`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A tool that can be executed by the agent.
///
/// Mirrors `chat-ai`'s `Tool` shape — just enough metadata for the API
/// request. Execution is done through the library's `tools::Tool` trait
/// from inside `Agent::chat_step`.
#[derive(Clone)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Message in a conversation with the LLM.
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

/// Content block within a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
    #[serde(rename = "document")]
    Document { source: FileSource },
}

/// File source for referencing uploaded files.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum FileSource {
    #[serde(rename = "file")]
    File { file_id: String },
}

/// Tool definition for the LLM API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}
