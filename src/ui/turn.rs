// ui::turn — the agent turn loop. Pulls events from the provider, accumulates
// into UiMessage, persists, executes tool calls, and loops until end_turn or cancel.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::api::provider::LlmProvider;
use crate::api::provider_types::{ProviderRequest, StreamEvent as PStream};
use crate::core::config::{Config, PermissionMode};
use crate::core::cost::CostTracker;
use crate::core::file_history::FileHistory;
use crate::core::permissions::AutoPermissionHandler;
use crate::core::types::{
    ContentBlock, DocumentSource, ImageSource, Message, MessageContent, Role, ToolResultContent,
};
use crate::core::ToolDefinition;
use crate::tools::{Tool, ToolContext, ToolResult};

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

/// Maximum number of provider rounds (each round = one `create_message_stream`
/// call). Guards against pathological loops where the model keeps emitting
/// tool_use forever.
pub const MAX_TOOL_ROUNDS: usize = 16;

/// Run a turn to completion: streams events to `sink`, executes tool calls
/// when the model emits `tool_use` stop reasons, and loops until the model
/// returns a non-tool stop reason, the user cancels, or an unrecoverable
/// error occurs.
pub async fn run_turn(
    provider: Arc<dyn LlmProvider>,
    session_id: SessionId,
    mut request: ProviderRequest,
    tools: Arc<Vec<Box<dyn Tool>>>,
    working_dir: PathBuf,
    sink: EventSink,
    cancel: CancellationToken,
) {
    debug!(?session_id, model = %request.model, "turn starting");

    for round in 1..=MAX_TOOL_ROUNDS {
        let outcome = run_one_round(provider.clone(), &request, &sink, &cancel).await;
        match outcome {
            RoundOutcome::Cancelled => {
                let _ = sink.send(TurnEvent::Cancelled).await;
                return;
            }
            RoundOutcome::Failed { message, retryable } => {
                let _ = sink.send(TurnEvent::Failed { message, retryable }).await;
                return;
            }
            RoundOutcome::Done {
                stop_reason,
                assistant_blocks,
            } => {
                if stop_reason != "tool_use" {
                    let _ = sink
                        .send(TurnEvent::Done { stop_reason })
                        .await;
                    return;
                }
                // Append the assistant's full content (text + tool_use) so the
                // next request keeps the conversation history consistent.
                request.messages.push(Message {
                    role: Role::Assistant,
                    content: MessageContent::Blocks(assistant_blocks.clone()),
                    uuid: None,
                    cost: None,
                    snapshot_patch: None,
                });

                if cancel.is_cancelled() {
                    let _ = sink.send(TurnEvent::Cancelled).await;
                    return;
                }

                // Execute each tool_use and collect results.
                let tool_results = execute_tool_blocks(
                    &assistant_blocks,
                    tools.as_slice(),
                    &working_dir,
                    &session_id,
                    &sink,
                    &cancel,
                )
                .await;
                if cancel.is_cancelled() {
                    let _ = sink.send(TurnEvent::Cancelled).await;
                    return;
                }
                // Emit a user tool_result message carrying the results.
                request.messages.push(Message {
                    role: Role::User,
                    content: MessageContent::Blocks(tool_results),
                    uuid: None,
                    cost: None,
                    snapshot_patch: None,
                });

                debug!(round, "tool-use round complete, continuing turn");
            }
        }
    }
    let _ = sink
        .send(TurnEvent::Failed {
            message: format!("tool-use loop exceeded {MAX_TOOL_ROUNDS} rounds"),
            retryable: false,
        })
        .await;
}

enum RoundOutcome {
    Cancelled,
    Failed { message: String, retryable: bool },
    Done {
        stop_reason: String,
        assistant_blocks: Vec<ContentBlock>,
    },
}

/// Drain one provider stream, translating events through the UI translator
/// and accumulating the assistant's content blocks for a potential follow-up
/// request. Returns the `stop_reason` and the assembled `ContentBlock` list.
async fn run_one_round(
    provider: Arc<dyn LlmProvider>,
    request: &ProviderRequest,
    sink: &EventSink,
    cancel: &CancellationToken,
) -> RoundOutcome {
    let mut stream = match provider.create_message_stream(request.clone()).await {
        Ok(s) => s,
        Err(e) => {
            return RoundOutcome::Failed {
                message: format!("create_message_stream: {e}"),
                retryable: true,
            };
        }
    };

    let mut translator = Translator::new();
    // Per-block-index accumulators. Provider content block indices are
    // unique within a single message, so this map is reset for every round.
    let mut text_blocks: HashMap<usize, String> = HashMap::new();
    let mut tool_blocks: HashMap<usize, (String, String, String)> = HashMap::new();
    // (id, name, partial_json)
    let mut stop_reason: Option<String> = None;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                drop(stream);
                return RoundOutcome::Cancelled;
            }
            next = stream.next() => {
                let Some(item) = next else {
                    // Stream ended without an explicit MessageStop — synthesize
                    // a clean end_turn so the UI sees a final MessageStop and
                    // we can return.
                    let evs = translator.push(PStream::MessageStop);
                    for ev in evs {
                        if let StreamEvent::MessageStop { stop_reason: sr, .. } = &ev {
                            stop_reason = Some(sr.clone());
                        }
                        let _ = sink.send(TurnEvent::Stream(ev)).await;
                    }
                    if stop_reason.is_none() {
                        stop_reason = Some("end_turn".into());
                    }
                    let sr = stop_reason.unwrap();
                    let blocks = materialize_blocks(&text_blocks, &tool_blocks);
                    return RoundOutcome::Done {
                        stop_reason: sr,
                        assistant_blocks: blocks,
                    };
                };
                match item {
                    Ok(pev) => {
                        // Pre-seed the tool accumulator from ContentBlockStart
                        // so the ToolUseStart UI event has an entry to grow.
                        if let PStream::ContentBlockStart {
                            index,
                            content_block:
                                ContentBlock::ToolUse {
                                    id,
                                    name,
                                    ..
                                },
                        } = &pev
                        {
                            tool_blocks
                                .insert(*index, (id.clone(), name.clone(), String::new()));
                        }
                        let evs = translator.push(pev);
                        for ev in &evs {
                            match ev {
                                StreamEvent::TextDelta { block, text } => {
                                    text_blocks
                                        .entry(*block)
                                        .or_default()
                                        .push_str(text);
                                }
                                StreamEvent::ToolUseStart { block, id, name } => {
                                    // Some events may not be preceded by
                                    // ContentBlockStart; seed if missing.
                                    tool_blocks.entry(*block).or_insert_with(|| {
                                        (id.clone(), name.clone(), String::new())
                                    });
                                }
                                StreamEvent::ToolUseDelta { block, partial_json } => {
                                    if let Some((_, _, buf)) = tool_blocks.get_mut(block) {
                                        buf.push_str(partial_json);
                                    }
                                }
                                StreamEvent::MessageStop { stop_reason: sr, .. } => {
                                    stop_reason = Some(sr.clone());
                                }
                                _ => {}
                            }
                            let _ = sink.send(TurnEvent::Stream(ev.clone())).await;
                        }
                        if let Some(sr) = stop_reason.take() {
                            let blocks = materialize_blocks(&text_blocks, &tool_blocks);
                            return RoundOutcome::Done {
                                stop_reason: sr,
                                assistant_blocks: blocks,
                            };
                        }
                    }
                    Err(e) => {
                        return RoundOutcome::Failed {
                            message: format!("stream error: {e}"),
                            retryable: true,
                        };
                    }
                }
            }
        }
    }
}

/// Merge per-block accumulators into a single `Vec<ContentBlock>` in
/// provider-index order. Text blocks land as `ContentBlock::Text`; tool
/// blocks land as `ContentBlock::ToolUse` with the input parsed as JSON
/// (or `Value::Null` if the JSON is incomplete / malformed).
fn materialize_blocks(
    text_blocks: &HashMap<usize, String>,
    tool_blocks: &HashMap<usize, (String, String, String)>,
) -> Vec<ContentBlock> {
    let mut indices: Vec<usize> = text_blocks
        .keys()
        .chain(tool_blocks.keys())
        .copied()
        .collect();
    indices.sort_unstable();
    indices.dedup();
    let mut out = Vec::new();
    for idx in indices {
        if let Some(text) = text_blocks.get(&idx) {
            out.push(ContentBlock::Text {
                text: text.clone(),
            });
        } else if let Some((id, name, partial_json)) = tool_blocks.get(&idx) {
            let input: Value =
                serde_json::from_str(partial_json).unwrap_or(Value::Null);
            out.push(ContentBlock::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input,
            });
        }
    }
    out
}

/// Execute every `ToolUse` block in `blocks`, emitting `ToolStart` /
/// `ToolEnd` events and returning a parallel list of `ToolResult`
/// `ContentBlock`s. If `cancel` fires mid-flight, the remaining tools
/// receive an error result and execution stops.
async fn execute_tool_blocks(
    blocks: &[ContentBlock],
    tools: &[Box<dyn Tool>],
    working_dir: &Path,
    session_id: &str,
    sink: &EventSink,
    cancel: &CancellationToken,
) -> Vec<ContentBlock> {
    let ctx = build_tool_context(working_dir.to_path_buf(), session_id.to_string());
    let mut results = Vec::new();
    for block in blocks {
        let ContentBlock::ToolUse { id, name, input } = block else {
            continue;
        };
        let _ = sink
            .send(TurnEvent::ToolStart {
                id: id.clone(),
                name: name.clone(),
            })
            .await;
        let result = if cancel.is_cancelled() {
            ToolResult::error("cancelled")
        } else {
            match tools.iter().find(|t| t.name() == name) {
                Some(tool) => tool.execute(input.clone(), &ctx).await,
                None => ToolResult::error(format!("unknown tool: {name}")),
            }
        };
        let _ = sink
            .send(TurnEvent::ToolEnd {
                id: id.clone(),
                content: result.content.clone(),
                is_error: result.is_error,
            })
            .await;
        results.push(ContentBlock::ToolResult {
            tool_use_id: id.clone(),
            content: ToolResultContent::Text(result.content),
            is_error: Some(result.is_error),
        });
    }
    results
}

/// Build a minimal `ToolContext` for one tool execution. The GUI doesn't yet
/// surface permission dialogs or LSP integration, so most optional fields
/// stay `None` / default.
fn build_tool_context(working_dir: PathBuf, session_id: String) -> ToolContext {
    let mode = PermissionMode::Default;
    ToolContext {
        working_dir,
        permission_mode: mode,
        permission_handler: Arc::new(AutoPermissionHandler { mode: PermissionMode::Default }),
        cost_tracker: CostTracker::new(),
        session_id,
        current_turn: Arc::new(AtomicUsize::new(0)),
        file_history: Arc::new(parking_lot::Mutex::new(FileHistory::new())),
        lsp_manager: None,
        non_interactive: true,
        mcp_manager: None,
        config: Config::default(),
        managed_agent_config: None,
        completion_notifier: None,
        pending_permissions: None,
        permission_manager: None,
        user_question_tx: None,
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
    blocks.push(ContentBlock::Text {
        text: text.to_string(),
    });
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
pub fn with_app_tools(mut req: ProviderRequest, state: &super::app::AppState) -> ProviderRequest {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::provider_types::StopReason;

    fn user_msg(text: &str) -> Message {
        Message {
            role: Role::User,
            content: MessageContent::Text(text.to_string()),
            uuid: None,
            cost: None,
            snapshot_patch: None,
        }
    }

    #[test]
    fn materialize_text_only() {
        let mut tb = HashMap::new();
        tb.insert(0, "hello".to_string());
        let out = materialize_blocks(&tb, &HashMap::new());
        assert!(matches!(&out[0], ContentBlock::Text { text } if text == "hello"));
    }

    #[test]
    fn materialize_tool_with_valid_json() {
        let mut tb = HashMap::new();
        tb.insert(0, ("id1".into(), "bash".into(), r#"{"command":"ls"}"#.into()));
        let out = materialize_blocks(&HashMap::new(), &tb);
        assert!(matches!(&out[0], ContentBlock::ToolUse { name, input, .. }
            if name == "bash" && input["command"] == "ls"));
    }

    #[test]
    fn materialize_tool_with_garbage_json_falls_back_to_null() {
        let mut tb = HashMap::new();
        tb.insert(0, ("id1".into(), "bash".into(), "{not json".into()));
        let out = materialize_blocks(&HashMap::new(), &tb);
        assert!(matches!(&out[0], ContentBlock::ToolUse { input, .. } if input.is_null()));
    }

    #[test]
    fn materialize_mixed_orders_by_index() {
        let mut text = HashMap::new();
        text.insert(1, "world".to_string());
        let mut tools = HashMap::new();
        tools.insert(0, ("id1".into(), "bash".into(), "{}".into()));
        let out = materialize_blocks(&text, &tools);
        // index 0 (tool) comes first, then index 1 (text).
        assert!(matches!(&out[0], ContentBlock::ToolUse { .. }));
        assert!(matches!(&out[1], ContentBlock::Text { text } if text == "world"));
    }

    #[test]
    fn attachment_to_content_block_image_missing_file_returns_none() {
        let att = Attachment {
            id: "a1".into(),
            kind: AttachmentKind::Image,
            display_name: "x.png".into(),
            mime: "image/png".into(),
            local_path: PathBuf::from("/nonexistent/path/x.png"),
            size_bytes: 0,
        };
        assert!(attachment_to_content_block(&att).is_none());
    }

    #[test]
    fn make_user_message_appends_text_block_after_attachments() {
        let att = Attachment {
            id: "a1".into(),
            kind: AttachmentKind::Text,
            display_name: "x.txt".into(),
            mime: "text/plain".into(),
            local_path: std::env::temp_dir().join("lwa_test_text_attachment.txt"),
            size_bytes: 0,
        };
        std::fs::write(&att.local_path, b"hello attachment").unwrap();
        let msg = make_user_message("the prompt", &[att]);
        if let MessageContent::Blocks(blocks) = &msg.content {
            // text attachment inlined as Document, then the prompt text.
            assert_eq!(blocks.len(), 2);
            assert!(matches!(&blocks[0], ContentBlock::Document { .. }));
            if let ContentBlock::Text { text } = &blocks[1] {
                assert_eq!(text, "the prompt");
            } else {
                panic!("last block should be Text");
            }
        } else {
            panic!("expected Blocks content");
        }
        let _ = std::fs::remove_file(
            std::env::temp_dir().join("lwa_test_text_attachment.txt"),
        );
    }

    #[test]
    fn make_user_message_empty_attachments_is_just_text() {
        let msg = make_user_message("hi", &[]);
        if let MessageContent::Blocks(blocks) = &msg.content {
            assert_eq!(blocks.len(), 1);
            assert!(matches!(&blocks[0], ContentBlock::Text { text } if text == "hi"));
        } else {
            panic!("expected Blocks content");
        }
    }
}
