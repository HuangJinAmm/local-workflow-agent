//! Agent client bridging the chat UI onto the `local_workflow_agent` library.
//!
//! Holds a `local_workflow_agent` `LlmProvider` (boxed behind `Arc<dyn …>`)
//! plus the in-memory conversation transcript. The transcript is stored in
//! chat-ai's wire types (`super::types::Message`) and translated into the
//! library's `core::types::Message` immediately before each request via
//! [`Agent::build_provider_request`].
//!
//! The actual multi-round tool-use loop is driven by
//! [`crate::agent::run_turn`], which the UI handler invokes with the
//! `ProviderRequest` produced here and the library's full `all_tools()`
//! registry. Streaming `TurnEvent`s flow back to the UI through the
//! `AgentResponse` channel.

use anyhow::Result;
use std::env;
use std::sync::Arc;

use crate::api::client::{AnthropicClient, ClientConfig};
use crate::api::provider::LlmProvider;
use crate::api::provider_types::{ProviderRequest, SystemPrompt};
use crate::api::providers::{AnthropicProvider, OpenAiProvider, OpenAiCompatProvider};
use crate::core::types::{
    ContentBlock as LibContentBlock, Message as LibMessage, MessageContent as LibMessageContent,
    Role as LibRole, ToolDefinition as LibToolDefinition, ToolResultContent,
};

use super::types::{ContentBlock, FileSource, Message, Tool, ToolDefinition};

/// Available provider presets exposed in the settings panel.
///
/// The first field is the stable provider id (sent to the background agent
/// as `AgentRequest::SetProvider`); the second is a human-readable label
/// shown in the dropdown.
pub const PROVIDER_PRESETS: &[(&str, &str)] = &[
    ("anthropic", "Anthropic (Claude)"),
    ("openai", "OpenAI"),
    ("deepseek", "DeepSeek"),
    ("moonshot", "Moonshot (Kimi)"),
    ("qwen", "Qwen (通义千问)"),
    ("zhipu", "Zhipu (智谱 GLM)"),
    ("zai", "Z.AI"),
    ("siliconflow", "SiliconFlow (硅基流动)"),
    ("groq", "Groq"),
    ("mistral", "Mistral"),
    ("openrouter", "OpenRouter"),
    ("ollama", "Ollama (本地)"),
    ("lmstudio", "LM Studio (本地)"),
];

/// Agent that can converse with an LLM and execute tools.
///
/// Holds a `local_workflow_agent` `LlmProvider` (boxed behind `Arc<dyn …>`)
/// plus the in-memory conversation transcript. The transcript is stored in
/// chat-ai's wire types (`super::types::Message`) and translated into the
/// library's `core::types::Message` immediately before each request.
#[derive(Clone)]
pub struct Agent {
    model: String,
    system_prompt: String,
    tools: Vec<Tool>,
    conversation: Vec<Message>,
    max_tokens: u32,
    /// Boxed provider — the concrete type depends on `provider_kind`.
    provider: Arc<dyn LlmProvider>,
    /// Current provider id, e.g. `"anthropic"`, `"openai"`, `"deepseek"`,
    /// `"ollama"`. Used to rebuild the provider when settings change.
    provider_kind: String,
    /// Current API key + base URL — kept so the provider can be rebuilt
    /// when the user changes either field in the settings panel.
    api_key: String,
    base_url: String,
}

impl Agent {
    /// Create a new agent with the given tools.
    pub fn new(tools: Vec<Tool>) -> Result<Self> {
        Agent::builder().build(tools)
    }

    /// Create a new agent with custom configuration.
    pub fn builder() -> AgentBuilder {
        AgentBuilder::default()
    }

    /// Default system prompt.
    fn default_system_prompt() -> String {
        "You are a helpful AI assistant with access to tools that can help you complete tasks. \
        When you need to use a tool, respond with the appropriate tool call. \
        Be concise and helpful in your responses."
            .to_string()
    }

    /// Set the system prompt.
    pub fn set_system_prompt(&mut self, prompt: String) {
        self.system_prompt = prompt;
    }

    /// Set the model.
    pub fn set_model(&mut self, model: String) {
        self.model = model;
    }

    /// Set max tokens.
    pub fn set_max_tokens(&mut self, max_tokens: u32) {
        self.max_tokens = max_tokens;
    }

    /// Update the provider kind (e.g. "anthropic", "openai", "deepseek",
    /// "ollama"). Rebuilds the underlying provider implementation. Clears
    /// the conversation since different providers use different message
    /// formats / tool-call conventions.
    pub fn set_provider(&mut self, provider: String) -> Result<()> {
        self.provider_kind = provider;
        self.rebuild_provider()?;
        // Different providers may use different tool-call id conventions
        // and message shapes — start fresh.
        self.conversation.clear();
        Ok(())
    }

    /// Update the API key — rebuilds the underlying provider.
    pub fn set_api_key(&mut self, api_key: String) -> Result<()> {
        self.api_key = api_key;
        self.rebuild_provider()
    }

    /// Update the API base URL — rebuilds the underlying provider.
    pub fn set_base_url(&mut self, base_url: String) -> Result<()> {
        self.base_url = base_url;
        self.rebuild_provider()
    }

    /// Update both the API key and base URL atomically — rebuilds the
    /// provider only once (cheaper than two separate updates).
    pub fn set_api_config(&mut self, api_key: String, base_url: String) -> Result<()> {
        self.api_key = api_key;
        self.base_url = base_url;
        self.rebuild_provider()
    }

    /// Reconstruct the underlying `LlmProvider` from the current
    /// `provider_kind` + `api_key` + `base_url`. Called after any change to
    /// those fields.
    fn rebuild_provider(&mut self) -> Result<()> {
        let kind = self.provider_kind.as_str();
        let provider: Arc<dyn LlmProvider> = match kind {
            "anthropic" => {
                let client = AnthropicClient::new(ClientConfig {
                    api_key: self.api_key.clone(),
                    api_base: self.base_url.clone(),
                    ..Default::default()
                })?;
                Arc::new(AnthropicProvider::new(Arc::new(client)))
            }
            "openai" => {
                let mut p = OpenAiProvider::new(self.api_key.clone());
                if !self.base_url.trim().is_empty()
                    && self.base_url != crate::core::constants::ANTHROPIC_API_BASE
                {
                    p = p.with_base_url(self.base_url.clone());
                }
                Arc::new(p)
            }
            // Everything else: route through the OpenAI-compatible adapter.
            // First try the built-in factory (deepseek, groq, mistral,
            // ollama, lmstudio, zhipu, zai, moonshot, qwen, siliconflow,
            // openrouter, …) which sets the right base URL + quirks.
            other => {
                let compat = if let Some(built) =
                    crate::api::providers::provider_for_id(other)
                {
                    built
                } else {
                    // Fallback: a generic OpenAI-compatible adapter pointed
                    // at the user-supplied base URL.
                    OpenAiCompatProvider::new(
                        "custom-openai",
                        "Custom OpenAI-Compatible",
                        self.base_url.clone(),
                    )
                };
                let compat = if !self.api_key.trim().is_empty() {
                    compat.with_api_key(self.api_key.clone())
                } else {
                    compat
                };
                let compat = if !self.base_url.trim().is_empty()
                    && self.base_url != crate::core::constants::ANTHROPIC_API_BASE
                {
                    compat.with_base_url(self.base_url.clone())
                } else {
                    compat
                };
                Arc::new(compat)
            }
        };
        self.provider = provider;
        Ok(())
    }

    /// Current provider id (e.g. "anthropic").
    pub fn provider_kind(&self) -> &str {
        &self.provider_kind
    }

    /// Current API key (for file uploads and provider rebuilds).
    pub fn api_key(&self) -> String {
        self.api_key.clone()
    }

    /// Clone of the underlying `LlmProvider` — passed to `run_turn` which
    /// needs an `Arc<dyn LlmProvider>`.
    pub fn provider_arc(&self) -> Arc<dyn LlmProvider> {
        self.provider.clone()
    }

    /// Current API base URL (for the settings panel to display).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Current model name (for the settings panel to display).
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Add a user message to the conversation.
    pub fn add_user_message(&mut self, content: String) {
        self.conversation.push(Message::User {
            role: "user".to_string(),
            content: vec![ContentBlock::Text { text: content }],
        });
    }

    /// Get all tool definitions in a format suitable for the LLM.
    pub fn get_tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|tool| ToolDefinition {
                name: tool.name.clone(),
                description: tool.description.clone(),
                input_schema: tool.input_schema.clone(),
            })
            .collect()
    }

    /// Get the current conversation history.
    #[allow(dead_code)]
    pub fn get_conversation(&self) -> &[Message] {
        &self.conversation
    }

    /// Clear the conversation history.
    pub fn clear_conversation(&mut self) {
        self.conversation.clear();
    }

    /// Build a `ProviderRequest` from the current transcript + system prompt.
    ///
    /// Adds `user_content` (chat-ai wire `ContentBlock`s) to the in-memory
    /// conversation as a new user message, then translates the full transcript
    /// into the library's `Message`/`ContentBlock` types and packs them into a
    /// `ProviderRequest` ready for `run_turn`.
    pub fn build_provider_request(
        &mut self,
        user_content: Vec<ContentBlock>,
    ) -> Result<ProviderRequest, anyhow::Error> {
        // Add user message to local transcript.
        self.conversation.push(Message::User {
            role: "user".to_string(),
            content: user_content,
        });

        let messages: Vec<LibMessage> = self.conversation.iter().map(message_to_lib).collect();

        let tools: Vec<LibToolDefinition> = self
            .tools
            .iter()
            .map(|t| LibToolDefinition {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.input_schema.clone(),
            })
            .collect();

        Ok(ProviderRequest {
            model: self.model.clone(),
            messages,
            system_prompt: Some(SystemPrompt::Text(self.system_prompt.clone())),
            tools,
            max_tokens: self.max_tokens,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: Vec::new(),
            thinking: None,
            provider_options: serde_json::Value::Object(Default::default()),
        })
    }
}

// ---------------------------------------------------------------------------
// Translation helpers — chat-ai wire types ↔ library types
// ---------------------------------------------------------------------------

fn message_to_lib(msg: &Message) -> LibMessage {
    match msg {
        Message::User { content, .. } => {
            let blocks: Vec<LibContentBlock> =
                content.iter().cloned().map(content_block_to_lib).collect();
            LibMessage {
                role: LibRole::User,
                content: LibMessageContent::Blocks(blocks),
                uuid: None,
                cost: None,
                snapshot_patch: None,
            }
        }
        Message::Assistant { content, .. } => {
            let blocks: Vec<LibContentBlock> =
                content.iter().cloned().map(content_block_to_lib).collect();
            LibMessage {
                role: LibRole::Assistant,
                content: LibMessageContent::Blocks(blocks),
                uuid: None,
                cost: None,
                snapshot_patch: None,
            }
        }
    }
}

fn content_block_to_lib(block: ContentBlock) -> LibContentBlock {
    match block {
        ContentBlock::Text { text } => LibContentBlock::Text { text },
        ContentBlock::ToolUse { id, name, input } => {
            LibContentBlock::ToolUse { id, name, input }
        }
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => LibContentBlock::ToolResult {
            tool_use_id,
            content: ToolResultContent::Text(content),
            is_error,
        },
        // The library's `Document` block supports base64 / URL sources only,
        // not Anthropic Files-API `file_id`s. To preserve the API shape we
        // emit a Text block carrying the JSON-serialized source so the request
        // at least round-trips; the chat UI does not currently exercise this
        // path anyway.
        ContentBlock::Document { source: FileSource::File { file_id } } => {
            LibContentBlock::Text {
                text: format!(
                    "[Attached file: id={} (Anthropic Files API)]",
                    file_id
                ),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Builder for creating agents with custom configuration.
pub struct AgentBuilder {
    provider: String,
    api_key: Option<String>,
    base_url: Option<String>,
    model: String,
    system_prompt: String,
    max_tokens: u32,
}

impl Default for AgentBuilder {
    fn default() -> Self {
        Self {
            provider: "anthropic".to_string(),
            api_key: None,
            base_url: None,
            model: "claude-haiku-4-5-20251001".to_string(),
            system_prompt: Agent::default_system_prompt(),
            max_tokens: 4096,
        }
    }
}

#[allow(dead_code)]
impl AgentBuilder {
    pub fn provider(mut self, provider: String) -> Self {
        self.provider = provider;
        self
    }

    pub fn api_key(mut self, api_key: String) -> Self {
        self.api_key = Some(api_key);
        self
    }

    pub fn base_url(mut self, base_url: String) -> Self {
        self.base_url = Some(base_url);
        self
    }

    pub fn model(mut self, model: String) -> Self {
        self.model = model;
        self
    }

    pub fn system_prompt(mut self, prompt: String) -> Self {
        self.system_prompt = prompt;
        self
    }

    pub fn max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    pub fn build(self, tools: Vec<Tool>) -> Result<Agent> {
        let api_key = match self.api_key {
            Some(key) => key,
            None => env::var("ANTHROPIC_API_KEY").unwrap_or_default(),
        };

        // Use the provided base URL, or fall back to the library default
        // (which reads `ANTHROPIC_API_BASE` from `core::constants`).
        let base_url = self.base_url.unwrap_or_else(|| {
            crate::core::constants::ANTHROPIC_API_BASE.to_string()
        });

        // If no API key is configured we still construct an agent — it will
        // simply surface an error on the first `run_turn` call. This lets
        // the GUI boot before the user has entered their key in the
        // settings panel.
        let mut agent = Agent {
            model: self.model,
            system_prompt: self.system_prompt,
            tools,
            conversation: Vec::new(),
            max_tokens: self.max_tokens,
            provider: Arc::new(AnthropicProvider::new(Arc::new(AnthropicClient::new(
                ClientConfig::default(),
            )?))),
            provider_kind: self.provider,
            api_key,
            base_url,
        };
        // Now rebuild the provider for real based on the requested kind.
        // This swaps in an OpenAI/OpenAI-compat provider if `provider_kind`
        // isn't "anthropic".
        agent.rebuild_provider()?;

        Ok(agent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_builder() {
        let agent = Agent::builder()
            .api_key("test-key".to_string())
            .model("claude-sonnet-4-5-20250929".to_string())
            .system_prompt("You are a test assistant".to_string())
            .max_tokens(2048)
            .build(vec![]);

        assert!(agent.is_ok());
    }
}
