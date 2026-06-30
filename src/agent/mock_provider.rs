//! Test-only `MockProvider` for `run_turn` unit tests.

use async_trait::async_trait;
use futures::stream;
use tokio::sync::Mutex as TokioMutex;

use crate::api::provider::{LlmProvider, ModelInfo};
use crate::api::provider_error::ProviderError;
use crate::api::provider_types::{
    ProviderCapabilities, ProviderRequest, ProviderResponse, ProviderStatus, StreamEvent,
    SystemPromptStyle,
};
use crate::core::provider_id::ProviderId;

/// A mock provider that pops a pre-scripted list of `StreamEvent`s per
/// `create_message_stream` call. When the script queue is exhausted, an
/// empty stream (`[MessageStop]`) is returned, which `run_turn` interprets
/// as a clean `end_turn`.
pub struct MockProvider {
    id: ProviderId,
    scripts: TokioMutex<Vec<Vec<StreamEvent>>>,
}

impl MockProvider {
    pub fn new(single_script: Vec<StreamEvent>) -> Self {
        Self::with_scripts(vec![single_script])
    }

    pub fn with_scripts(scripts: Vec<Vec<StreamEvent>>) -> Self {
        Self {
            id: ProviderId::new("mock"),
            scripts: TokioMutex::new(scripts),
        }
    }
}

#[async_trait]
impl LlmProvider for MockProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn name(&self) -> &str {
        "Mock"
    }

    async fn create_message(
        &self,
        _request: ProviderRequest,
    ) -> Result<ProviderResponse, ProviderError> {
        Err(ProviderError::Other {
            provider: self.id.clone(),
            message: "MockProvider only supports streaming".to_string(),
            status: None,
            body: None,
        })
    }

    async fn create_message_stream(
        &self,
        _request: ProviderRequest,
    ) -> Result<
        std::pin::Pin<
            Box<dyn futures::Stream<Item = Result<StreamEvent, ProviderError>> + Send>,
        >,
        ProviderError,
    > {
        let events = {
            let mut scripts = self.scripts.lock().await;
            if scripts.is_empty() {
                vec![StreamEvent::MessageStop]
            } else {
                scripts.remove(0)
            }
        };

        let s = stream::iter(events.into_iter().map(Ok));
        Ok(Box::pin(s))
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        Ok(vec![])
    }

    async fn health_check(&self) -> Result<ProviderStatus, ProviderError> {
        Ok(ProviderStatus::Healthy)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            tool_calling: true,
            thinking: false,
            image_input: false,
            pdf_input: false,
            audio_input: false,
            video_input: false,
            caching: false,
            structured_output: false,
            system_prompt_style: SystemPromptStyle::TopLevel,
        }
    }
}
