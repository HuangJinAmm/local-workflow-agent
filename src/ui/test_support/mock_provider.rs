// ui::test_support::mock_provider — an `LlmProvider` implementation that
// replays a scripted event sequence. Used by `tests/ui_turn_e2e.rs` to
// drive `run_turn` end-to-end without a real network.

use std::pin::Pin;
use std::sync::Arc;

use async_stream::stream;
use async_trait::async_trait;
use futures::Stream;
use parking_lot::Mutex;

use crate::api::ModelInfo;
use crate::api::provider::LlmProvider;
use crate::api::provider_error::ProviderError;
use crate::api::provider_types::{
    ProviderCapabilities, ProviderRequest, ProviderResponse, ProviderStatus,
    StreamEvent as PStream, SystemPromptStyle,
};
use crate::core::provider_id::{ModelId, ProviderId};

/// A scripted provider. `events` is replayed in order on each
/// `create_message_stream` call; once exhausted, the stream ends.
pub struct MockProvider {
    id: ProviderId,
    name: String,
    events: Arc<Mutex<Vec<PStream>>>,
}

impl MockProvider {
    pub fn new(id: impl Into<String>, name: impl Into<String>, events: Vec<PStream>) -> Arc<Self> {
        Arc::new(Self {
            id: ProviderId::new(id.into()),
            name: name.into(),
            events: Arc::new(Mutex::new(events)),
        })
    }
}

#[async_trait]
impl LlmProvider for MockProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }

    async fn create_message(
        &self,
        _request: ProviderRequest,
    ) -> Result<ProviderResponse, ProviderError> {
        Err(ProviderError::InvalidRequest {
            provider: self.id.clone(),
            message: "MockProvider does not support non-streaming".into(),
        })
    }

    async fn create_message_stream(
        &self,
        _request: ProviderRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<PStream, ProviderError>> + Send>>, ProviderError>
    {
        let events = Arc::clone(&self.events);
        let s = stream! {
            loop {
                let next: Option<PStream> = {
                    let mut guard = events.lock();
                    if guard.is_empty() {
                        None
                    } else {
                        Some(guard.remove(0))
                    }
                }; // guard dropped here
                match next {
                    Some(ev) => yield Ok(ev),
                    None => break,
                }
            }
        };
        Ok(Box::pin(s))
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        Ok(vec![ModelInfo {
            id: ModelId::new("mock-model"),
            provider_id: self.id.clone(),
            name: "Mock Model".into(),
            context_window: 200_000,
            max_output_tokens: 8_192,
        }])
    }

    async fn health_check(&self) -> Result<ProviderStatus, ProviderError> {
        Ok(ProviderStatus::Healthy)
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            tool_calling: true,
            thinking: true,
            image_input: true,
            pdf_input: true,
            audio_input: false,
            video_input: false,
            caching: false,
            structured_output: true,
            system_prompt_style: SystemPromptStyle::TopLevel,
        }
    }
}
