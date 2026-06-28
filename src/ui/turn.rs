// ui::turn — the agent turn loop. Pulls events from the provider, accumulates
// into UiMessage, persists, executes tool calls, and loops until end_turn or cancel.

use std::path::Path;
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::api::provider::LlmProvider;
use crate::api::provider_types::{ProviderRequest, StreamEvent as PStream};
use crate::core::types::{
    ContentBlock, DocumentSource, ImageSource, Message, MessageContent, Role,
};
use crate::core::ToolDefinition;

use super::app::AppState;
use super::model::*;
use super::stream::StreamEvent;
use super::provider::unified::Translator;

pub type EventSink = mpsc::Sender<TurnEvent>;

#[derive(Debug, Clone)]
pub enum TurnEvent {
    Stream(StreamEvent),
    ToolStart { id: String, name: String },
    ToolEnd { id: String, content: String, is_error: bool },
    Done { stop_reason: String },
    Failed { message: String, retryable: bool },
    Cancelled,
}

pub async fn run_turn(
    provider: Arc<dyn LlmProvider>,
    session_id: SessionId,
    request: ProviderRequest,
    sink: EventSink,
    cancel: CancellationToken,
) {
    debug!(?session_id, model = %request.model, "turn starting");

    let mut stream = match provider.create_message_stream(request).await {
        Ok(s) => s,
        Err(e) => {
            let _ = sink
                .send(TurnEvent::Failed {
                    message: format!("create_message_stream: {e}"),
                    retryable: true,
                })
                .await;
            return;
        }
    };

    let mut translator = Translator::new();
    let mut saw_message_stop = false;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                // Drop the stream to abort the in-flight HTTP read.
                drop(stream);
                let _ = sink.send(TurnEvent::Cancelled).await;
                return;
            }
            next = stream.next() => {
                let Some(item) = next else {
                    // Stream ended without an explicit MessageStop.
                    // Synthesize a final stop event so the UI sees a
                    // clean Done with the buffered stop_reason (or the
                    // end_turn default).
                    let evs = translator.push(PStream::MessageStop);
                    for ev in evs {
                        if let StreamEvent::MessageStop { stop_reason, .. } = &ev {
                            let _ = sink
                                .send(TurnEvent::Done {
                                    stop_reason: stop_reason.clone(),
                                })
                                .await;
                            saw_message_stop = true;
                        }
                        let _ = sink.send(TurnEvent::Stream(ev)).await;
                    }
                    if !saw_message_stop {
                        let _ = sink
                            .send(TurnEvent::Done {
                                stop_reason: "end_turn".into(),
                            })
                            .await;
                    }
                    return;
                };
                match item {
                    Ok(pev) => {
                        let evs = translator.push(pev);
                        for ev in &evs {
                            let _ = sink.send(TurnEvent::Stream(ev.clone())).await;
                            if let StreamEvent::MessageStop { stop_reason, .. } = ev {
                                let _ = sink
                                    .send(TurnEvent::Done {
                                        stop_reason: stop_reason.clone(),
                                    })
                                    .await;
                                saw_message_stop = true;
                            }
                        }
                        if saw_message_stop {
                            return;
                        }
                    }
                    Err(e) => {
                        let _ = sink.send(TurnEvent::Failed {
                            message: format!("stream error: {e}"),
                            retryable: true,
                        }).await;
                        return;
                    }
                }
            }
        }
    }
}

/// Build a `User` `core::types::Message` from a text prompt and any
/// attached files. Image and PDF attachments are inlined as base64
/// `Image` / `Document` blocks; text attachments are inlined as text.
pub fn make_user_message(text: &str, attachments: &[Attachment]) -> Message {
    let mut blocks: Vec<ContentBlock> = Vec::new();
    for att in attachments {
        if let Some(cb) = attachment_to_content_block(att) {
            blocks.push(cb);
        }
    }
    blocks.push(ContentBlock::Text { text: text.to_string() });
    Message {
        role: Role::User,
        content: MessageContent::Blocks(blocks),
        uuid: None,
        cost: None,
        snapshot_patch: None,
    }
}

/// Convert a UI attachment to a `core::types::ContentBlock`. Returns `None`
/// if the file can't be read.
fn attachment_to_content_block(att: &Attachment) -> Option<ContentBlock> {
    match att.kind {
        AttachmentKind::Image => {
            let data = std::fs::read(&att.local_path).ok()?;
            Some(ContentBlock::Image {
                source: ImageSource {
                    source_type: "base64".into(),
                    media_type: Some(att.mime.clone()),
                    data: Some(B64.encode(&data)),
                    url: None,
                },
            })
        }
        AttachmentKind::Pdf => {
            let data = std::fs::read(&att.local_path).ok()?;
            Some(ContentBlock::Document {
                source: DocumentSource {
                    source_type: "base64".into(),
                    media_type: Some(att.mime.clone()),
                    data: Some(B64.encode(&data)),
                    url: None,
                },
                title: Some(att.display_name.clone()),
                context: None,
                citations: None,
            })
        }
        AttachmentKind::Text => {
            let body = std::fs::read_to_string(&att.local_path).ok()?;
            Some(ContentBlock::Document {
                source: DocumentSource {
                    source_type: "text".into(),
                    media_type: Some(att.mime.clone()),
                    data: Some(body),
                    url: None,
                },
                title: Some(att.display_name.clone()),
                context: None,
                citations: None,
            })
        }
    }
}

/// Build a `ProviderRequest` from the model name and the conversation
/// history. Tools are supplied from the `AppState` registry; system
/// prompt is left to the caller via `with_system_prompt`.
pub fn new_request(model: &str, max_tokens: u32, messages: Vec<Message>) -> ProviderRequest {
    ProviderRequest {
        model: model.to_string(),
        messages,
        system_prompt: None,
        tools: Vec::new(),
        max_tokens,
        temperature: None,
        top_p: None,
        top_k: None,
        stop_sequences: Vec::new(),
        thinking: None,
        provider_options: serde_json::Value::Object(Default::default()),
    }
}

/// Attach a system prompt to a request builder chain.
pub fn with_system_prompt(mut req: ProviderRequest, system: impl Into<String>) -> ProviderRequest {
    req.system_prompt = Some(crate::api::provider_types::SystemPrompt::Text(system.into()));
    req
}

/// Attach the tool definitions from an `AppState` to a request.
pub fn with_app_tools(mut req: ProviderRequest, state: &AppState) -> ProviderRequest {
    req.tools = state
        .tools
        .iter()
        .map(|t| ToolDefinition {
            name: t.name().to_string(),
            description: t.description().to_string(),
            input_schema: t.input_schema(),
        })
        .collect();
    req
}

/// Path to the binary used for the running session (placeholder helper
/// for code that needs to anchor a session to the project root).
#[allow(dead_code)]
pub fn working_dir_marker(p: &Path) -> &Path {
    p
}
