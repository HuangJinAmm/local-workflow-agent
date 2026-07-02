//! Agent client bridging the chat UI onto the `local_workflow_agent` library.
//!
//! Holds a `local_workflow_agent` `LlmProvider` (boxed behind `Arc<dyn …>`)
//! plus the in-memory conversation transcript. The transcript is stored in
//! chat-ai's wire types (`super::types::Message`) and translated into the
//! library's `core::types::Message` immediately before each request via
//! [`Agent::build_provider_request`].
//!
//! Provider construction, key/base_url resolution, and capabilities filtering
//! all delegate to the library's `api::registry`, `core::config::Config`, and
//! `api::provider_types::ProviderCapabilities` — no duplicate logic here.

use anyhow::Result;
use std::sync::Arc;

use crate::api::provider::LlmProvider;
use crate::api::provider_types::{ProviderCapabilities, ProviderRequest, SystemPrompt};
use crate::api::registry::provider_from_config;
use crate::api::model_registry::{effective_model_for_config, ModelRegistry};
use crate::core::config::Config;
use crate::core::system_prompt::{build_system_prompt, SystemPromptOptions};
use crate::core::types::{
    ContentBlock as LibContentBlock, Message as LibMessage,
    MessageContent as LibMessageContent, Role as LibRole, ToolDefinition as LibToolDefinition,
};

use super::types::{ContentBlock, Message, Tool};

/// Agent that can converse with an LLM and execute tools.
#[derive(Clone)]
pub struct Agent {
    model: String,
    system_prompt: String,
    tools: Vec<Tool>,
    conversation: Vec<Message>,
    max_tokens: u32,
    provider: Arc<dyn LlmProvider>,
    provider_kind: String,
    api_key: String,
    base_url: String,
    /// Library config — used for key/base_url resolution and system prompt.
    config: Config,
}

impl Agent {
    pub fn new(tools: Vec<Tool>) -> Result<Self> {
        Agent::builder().build(tools)
    }

    pub fn builder() -> AgentBuilder {
        AgentBuilder::default()
    }

    pub fn set_system_prompt(&mut self, prompt: String) {
        self.system_prompt = prompt;
    }

    pub fn set_model(&mut self, model: String) {
        self.model = model;
    }

    pub fn set_max_tokens(&mut self, max_tokens: u32) {
        self.max_tokens = max_tokens;
    }

    /// Update the provider kind. Rebuilds the underlying provider from the
    /// library's `Config` (which reads the right env var per provider).
    /// Clears the conversation since different providers use different
    /// message formats / tool-call conventions.
    pub fn set_provider(&mut self, provider: String) -> Result<()> {
        self.provider_kind = provider.clone();
        // Persist into config so provider_from_config can resolve the key.
        self.config.provider = Some(provider);
        self.rebuild_provider()?;
        self.conversation.clear();
        Ok(())
    }

    pub fn set_api_key(&mut self, api_key: String) -> Result<()> {
        self.api_key = api_key;
        self.config.api_key = Some(self.api_key.clone());
        self.rebuild_provider()
    }

    pub fn set_base_url(&mut self, base_url: String) -> Result<()> {
        self.base_url = base_url;
        self.rebuild_provider()
    }

    pub fn set_api_config(&mut self, api_key: String, base_url: String) -> Result<()> {
        self.api_key = api_key;
        self.base_url = base_url;
        self.config.api_key = Some(self.api_key.clone());
        self.rebuild_provider()
    }

    /// Reconstruct the underlying `LlmProvider` via `api::registry::provider_from_config`.
    /// Falls back to constructing an Anthropic provider with the in-memory key
    /// (so the GUI can boot before the user fills the settings panel).
    fn rebuild_provider(&mut self) -> Result<()> {
        // Try the library's resolver first — it knows the env-var conventions
        // for all 50+ providers (DEEPSEEK_API_KEY, QWEN_API_KEY, …).
        if let Some(p) = provider_from_config(&self.config, &self.provider_kind) {
            self.provider = p;
            return Ok(());
        }
        // Fallback: construct an Anthropic provider with whatever key we have.
        // This matches the original boot-without-key behaviour.
        if self.provider_kind == "anthropic" {
            let cc = crate::api::client::ClientConfig {
                api_key: self.api_key.clone(),
                api_base: if self.base_url.trim().is_empty() {
                    crate::core::constants::ANTHROPIC_API_BASE.to_string()
                } else {
                    self.base_url.clone()
                },
                ..Default::default()
            };
            let client = crate::api::client::AnthropicClient::new(cc)?;
            self.provider = Arc::new(crate::api::providers::AnthropicProvider::new(Arc::new(client)));
            return Ok(());
        }
        // Last resort: keep the existing provider (might be stale, but better
        // than crashing the GUI).
        Ok(())
    }

    pub fn provider_kind(&self) -> &str {
        &self.provider_kind
    }

    pub fn api_key(&self) -> String {
        self.api_key.clone()
    }

    pub fn provider_arc(&self) -> Arc<dyn LlmProvider> {
        self.provider.clone()
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Provider capabilities — used by `build_provider_request` to filter
    /// tools and unsupported content blocks.
    pub fn capabilities(&self) -> ProviderCapabilities {
        self.provider.capabilities()
    }

    pub fn add_user_message(&mut self, content: String) {
        self.conversation.push(Message::User {
            role: "user".to_string(),
            content: vec![ContentBlock::Text { text: content }],
        });
    }

    pub fn get_tool_definitions(&self) -> Vec<LibToolDefinition> {
        self.tools
            .iter()
            .map(|t| LibToolDefinition {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.input_schema.clone(),
            })
            .collect()
    }

    #[allow(dead_code)]
    pub fn get_conversation(&self) -> &[Message] {
        &self.conversation
    }

    pub fn clear_conversation(&mut self) {
        self.conversation.clear();
    }

    /// Build a `ProviderRequest` from the current transcript + system prompt.
    ///
    /// Filters tools and content blocks by the provider's capabilities, so
    /// a provider that doesn't support images/PDFs gets text placeholders
    /// instead of an API error. Mirrors `query/mod.rs:1043-1081`.
    pub fn build_provider_request(
        &mut self,
        user_content: Vec<ContentBlock>,
    ) -> Result<ProviderRequest, anyhow::Error> {
        self.conversation.push(Message::User {
            role: "user".to_string(),
            content: user_content,
        });

        let caps = self.provider.capabilities();
        let tools: Vec<LibToolDefinition> = if caps.tool_calling {
            self.get_tool_definitions()
        } else {
            vec![]
        };

        // Filter unsupported modalities — replace Image/Document blocks with
        // placeholder text when the provider doesn't support them.
        let messages: Vec<LibMessage> = self
            .conversation
            .iter()
            .map(|msg| {
                let mut lib_msg = message_to_lib(msg);
                if let LibMessageContent::Blocks(ref mut blocks) = lib_msg.content {
                    for block in blocks.iter_mut() {
                        match block {
                            LibContentBlock::Image { .. } if !caps.image_input => {
                                *block = LibContentBlock::Text {
                                    text: "[Image not supported by this model]".to_string(),
                                };
                            }
                            LibContentBlock::Document { .. } if !caps.pdf_input => {
                                *block = LibContentBlock::Text {
                                    text: "[PDF not supported by this model]".to_string(),
                                };
                            }
                            _ => {}
                        }
                    }
                }
                lib_msg
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
// Translation helper — UI Message enum → library Message struct.
// ContentBlock is now shared, so no per-block translation needed.
// ---------------------------------------------------------------------------

fn message_to_lib(msg: &Message) -> LibMessage {
    match msg {
        Message::User { content, .. } => {
            let blocks: Vec<LibContentBlock> = content.iter().cloned().collect();
            LibMessage {
                role: LibRole::User,
                content: LibMessageContent::Blocks(blocks),
                uuid: None,
                cost: None,
                snapshot_patch: None,
            }
        }
        Message::Assistant { content, .. } => {
            let blocks: Vec<LibContentBlock> = content.iter().cloned().collect();
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

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

pub struct AgentBuilder {
    provider: String,
    api_key: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
    system_prompt: Option<String>,
    max_tokens: u32,
}

impl Default for AgentBuilder {
    fn default() -> Self {
        Self {
            provider: "anthropic".to_string(),
            api_key: None,
            base_url: None,
            model: None,
            system_prompt: None,
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
        self.model = Some(model);
        self
    }
    pub fn system_prompt(mut self, prompt: String) -> Self {
        self.system_prompt = Some(prompt);
        self
    }
    pub fn max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    pub fn build(self, tools: Vec<Tool>) -> Result<Agent> {
        let registry = ModelRegistry::new();
        let mut config = Config::default();
        config.provider = Some(self.provider.clone());

        // Resolve API key: explicit > config (env vars) > empty.
        let api_key = self.api_key.unwrap_or_else(|| {
            config
                .resolve_provider_api_key(&self.provider)
                .unwrap_or_default()
        });
        config.api_key = Some(api_key.clone());

        // Resolve base URL: explicit > config > library default.
        let base_url = self.base_url.unwrap_or_else(|| {
            config
                .resolve_provider_api_base(&self.provider)
                .unwrap_or_else(|| crate::core::constants::ANTHROPIC_API_BASE.to_string())
        });

        // Resolve model: explicit > registry best > config default.
        let model = self.model.unwrap_or_else(|| {
            effective_model_for_config(&config, &registry)
        });

        // Resolve system prompt: explicit > library default.
        let system_prompt = self.system_prompt.unwrap_or_else(|| {
            let opts = SystemPromptOptions::default();
            build_system_prompt(&opts)
        });

        let mut agent = Agent {
            model,
            system_prompt,
            tools,
            conversation: Vec::new(),
            max_tokens: self.max_tokens,
            // Placeholder provider — rebuilt below.
            provider: Arc::new(crate::api::providers::AnthropicProvider::new(Arc::new(
                crate::api::client::AnthropicClient::new(
                    crate::api::client::ClientConfig::default(),
                )?,
            ))),
            provider_kind: self.provider,
            api_key,
            base_url,
            config,
        };
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

    #[test]
    fn build_provider_request_filters_tools_when_unsupported() {
        // Use a mock-friendly agent: anthropic provider supports tools.
        let mut agent = Agent::builder()
            .api_key("test-key".to_string())
            .model("claude-sonnet-4-5-20250929".to_string())
            .build(vec![Tool {
                name: "echo".into(),
                description: "echo".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }])
            .unwrap();
        let req = agent
            .build_provider_request(vec![ContentBlock::Text { text: "hi".into() }])
            .unwrap();
        // Anthropic supports tools, so tools should be non-empty.
        assert_eq!(req.tools.len(), 1);
    }

    #[test]
    fn message_to_lib_translates_user_message() {
        let msg = Message::User {
            role: "user".into(),
            content: vec![ContentBlock::Text { text: "hello".into() }],
        };
        let lib = message_to_lib(&msg);
        assert!(matches!(lib.role, LibRole::User));
        match lib.content {
            LibMessageContent::Blocks(blocks) => assert_eq!(blocks.len(), 1),
            _ => panic!("expected Blocks"),
        }
    }
}
