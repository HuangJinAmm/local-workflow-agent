//! Message types for agent communication and UI display.
//!
//! Ported from `chat-ai/src/services/agent/messages.rs` — these are the
//! channel types flowing between the GPUI foreground (`ChatAI`) and the
//! background agent task (`handle_outgoing` / `handle_incoming`).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ============================================================================
// Agent Communication Types
// ============================================================================

/// Messages sent from UI to Agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentRequest {
    /// Start a new chat with a user message and optional files
    Chat {
        content: String,
        files: Vec<std::path::PathBuf>,
    },
    /// Cancel the in-flight turn (if any).
    Cancel,
    /// Change the working directory used for tool execution.
    SetWorkingDir(std::path::PathBuf),
    /// Clear conversation history
    ClearHistory,
    /// Change the LLM provider (e.g. "anthropic", "openai", "deepseek",
    /// "ollama"). Rebuilds the underlying provider implementation.
    SetProvider(String),
    /// Change the LLM model
    SetModel(String),
    /// Update the Anthropic API key (rebuilds the underlying provider)
    SetApiKey(String),
    /// Update the API base URL (rebuilds the underlying provider)
    SetBaseUrl(String),
    /// Update the API key + base URL atomically (rebuilds the provider once)
    SetApiConfig {
        api_key: String,
        base_url: String,
    },
}

/// Messages sent from Agent to UI
pub enum AgentResponse {
    /// A streaming event from the current turn (text delta, tool call, done, …).
    TurnEvent(crate::agent::TurnEvent),
    /// A tool permission request that needs the user's approval via the GUI modal.
    PermissionRequest(crate::ui::permission_modal::PermissionRequest),
    /// A question from the `AskUserQuestion` tool that needs the user's answer.
    UserQuestion(crate::tools::UserQuestionEvent),
    /// Agent encountered an error
    Error(String),
}

/// Data for a tool call that needs to be executed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallData {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// Data for a tool result being returned to the agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultData {
    pub tool_use_id: String,
    pub content: String,
    pub is_error: bool,
}

// ============================================================================
// UI Message Types
// ============================================================================

/// Role of a message in the UI conversation display
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    ToolCall,
    ToolResult,
}

/// A message in the UI conversation display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiMessage {
    pub role: MessageRole,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub metadata: Option<MessageMetadata>,
}

/// Additional metadata for messages
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageMetadata {
    pub tool_name: Option<String>,
    pub is_error: bool,
    pub tool_input: Option<Value>,
}

impl UiMessage {
    /// Create a new user message
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
            timestamp: Utc::now(),
            metadata: None,
        }
    }

    /// Create a new assistant message
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
            timestamp: Utc::now(),
            metadata: None,
        }
    }

    /// Create a new system message (used for status / tool-call annotations).
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: content.into(),
            timestamp: Utc::now(),
            metadata: None,
        }
    }

    /// Create a new tool call message
    #[allow(dead_code)]
    pub fn tool_call(tool_name: String, tool_input: Value) -> Self {
        Self {
            role: MessageRole::ToolCall,
            content: format!("Calling {}", tool_name),
            timestamp: Utc::now(),
            metadata: Some(MessageMetadata {
                tool_name: Some(tool_name),
                is_error: false,
                tool_input: Some(tool_input),
            }),
        }
    }

    /// Create an error message
    pub fn error(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: format!("❌ Error: {}", content.into()),
            timestamp: Utc::now(),
            metadata: Some(MessageMetadata {
                tool_name: None,
                is_error: true,
                tool_input: None,
            }),
        }
    }
}
