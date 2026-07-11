//! `run_turn` — streaming tool-use loop shared by CLI and UI.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use futures::{FutureExt, StreamExt};
use tokio_util::sync::CancellationToken;

use crate::api::provider::LlmProvider;
use crate::api::provider_types::{ProviderRequest, StopReason, StreamEvent, SystemPrompt};
use crate::core::error::ClaudeError;
use crate::core::permissions::{PermissionDecision, PermissionRequest};
use crate::core::types::{ContentBlock, Message, ToolResultContent, UsageInfo};
use crate::tools::{Tool, ToolContext};

/// Maximum number of chars of the system prompt to dump verbatim into the
/// log. Longer prompts are truncated with a length note so the log stays
/// readable.
const SYSTEM_PROMPT_LOG_PREVIEW: usize = 2000;

/// Cap on consecutive tool-use rounds to prevent infinite loops.
pub const MAX_TOOL_ROUNDS: usize = 16;

/// Events emitted by `run_turn` to its sink as the turn progresses.
#[derive(Debug)]
pub enum TurnEvent {
    /// Incremental text delta from the model.
    TextDelta { text: String },
    /// A tool call has started (the model emitted a `tool_use` block).
    ToolUseStart { id: String, name: String },
    /// Incremental JSON delta for an in-progress tool call's input.
    ToolUseDelta { id: String, partial_json: String },
    /// A tool call has finished (success or error).
    ToolEnd {
        id: String,
        result: ToolResultContent,
        is_error: bool,
    },
    /// The turn completed normally.
    Done {
        stop_reason: Option<StopReason>,
        usage: Option<UsageInfo>,
    },
    /// The turn failed due to an error.
    Failed { error: ClaudeError },
    /// The turn was cancelled by the user.
    Cancelled,
}

/// Sink type — a channel sender that receives `TurnEvent`s.
pub type TurnSink = async_channel::Sender<TurnEvent>;

/// Cancel type — a token that can be triggered to cancel the turn.
pub type TurnCancel = CancellationToken;

/// Drive a multi-round LLM conversation with tool-use.
///
/// Streams `TurnEvent`s to `sink` as the turn progresses. Returns `Ok(())`
/// on normal completion (including cancellation), `Err` on hard failure.
pub async fn run_turn(
    provider: Arc<dyn LlmProvider>,
    session_id: String,
    mut request: ProviderRequest,
    tools: Arc<Vec<Box<dyn Tool>>>,
    tool_ctx: Arc<ToolContext>,
    sink: TurnSink,
    cancel: TurnCancel,
) -> Result<(), ClaudeError> {
    log_turn_init(&session_id, &request, &tools);

    for _round in 1..=MAX_TOOL_ROUNDS {
        // Cancellation check point 1: round start.
        if cancel.is_cancelled() {
            let _ = sink.send(TurnEvent::Cancelled).await;
            return Ok(());
        }

        // --- Stream provider response ---
        let mut stream = match provider.create_message_stream(request.clone()).await {
            Ok(s) => s,
            Err(e) => {
                let err: ClaudeError = e.into();
                let ret = fail(&sink, err).await;
                return Err(ret);
            }
        };

        let mut current_blocks: Vec<ContentBlock> = Vec::new();
        let mut input_buffer: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut stop_reason: Option<StopReason> = None;
        let mut usage: Option<UsageInfo> = None;

        while let Some(event_result) = stream.next().await {
            // Cancellation check point 2: between stream events.
            if cancel.is_cancelled() {
                let _ = sink.send(TurnEvent::Cancelled).await;
                return Ok(());
            }

            let event = match event_result {
                Ok(e) => e,
                Err(e) => {
                    let err: ClaudeError = e.into();
                    let ret = fail(&sink, err).await;
                    return Err(ret);
                }
            };

            match event {
                StreamEvent::MessageStart { .. } => {}
                StreamEvent::ContentBlockStart { index, content_block } => {
                    while current_blocks.len() <= index {
                        current_blocks.push(ContentBlock::Text { text: String::new() });
                    }
                    current_blocks[index] = content_block.clone();
                    match &content_block {
                        ContentBlock::ToolUse { id, name, .. } => {
                            input_buffer.insert(id.clone(), String::new());
                            let _ = sink
                                .send(TurnEvent::ToolUseStart {
                                    id: id.clone(),
                                    name: name.clone(),
                                })
                                .await;
                        }
                        ContentBlock::Text { text } if !text.is_empty() => {
                            let _ = sink.send(TurnEvent::TextDelta { text: text.clone() }).await;
                        }
                        ContentBlock::Thinking { thinking, .. } if !thinking.is_empty() => {
                            let _ = sink
                                .send(TurnEvent::TextDelta {
                                    text: thinking.clone(),
                                })
                                .await;
                        }
                        _ => {}
                    }
                }
                StreamEvent::TextDelta { index, text } => {
                    if let Some(ContentBlock::Text { text: buf }) = current_blocks.get_mut(index) {
                        buf.push_str(&text);
                    }
                    let _ = sink.send(TurnEvent::TextDelta { text }).await;
                }
                StreamEvent::ThinkingDelta { index, thinking } => {
                    if let Some(ContentBlock::Thinking { thinking: buf, .. }) =
                        current_blocks.get_mut(index)
                    {
                        buf.push_str(&thinking);
                    }
                    let _ = sink.send(TurnEvent::TextDelta { text: thinking }).await;
                }
                StreamEvent::InputJsonDelta { index, partial_json } => {
                    if let Some(block) = current_blocks.get_mut(index) {
                        if let ContentBlock::ToolUse { id, input, .. } = block {
                            input_buffer
                                .entry(id.clone())
                                .or_default()
                                .push_str(&partial_json);
                            if let Ok(parsed) =
                                serde_json::from_str::<serde_json::Value>(&input_buffer[id])
                            {
                                *input = parsed;
                            }
                            let _ = sink
                                .send(TurnEvent::ToolUseDelta {
                                    id: id.clone(),
                                    partial_json,
                                })
                                .await;
                        }
                    }
                }
                StreamEvent::SignatureDelta { .. } => {}
                StreamEvent::ContentBlockStop { .. } => {}
                StreamEvent::MessageDelta {
                    stop_reason: sr,
                    usage: u,
                } => {
                    stop_reason = sr;
                    usage = u;
                }
                StreamEvent::MessageStop => break,
                StreamEvent::Error {
                    error_type,
                    message,
                } => {
                    let err = ClaudeError::Api(format!("{}: {}", error_type, message));
                    let ret = fail(&sink, err).await;
                    return Err(ret);
                }
                StreamEvent::ReasoningDelta { .. } => {}
            }
        }

        // Append the assistant message (with accumulated blocks) to history.
        request
            .messages
            .push(Message::assistant_blocks(current_blocks.clone()));

        // If not a tool-use stop, the turn is done.
        if !matches!(stop_reason, Some(StopReason::ToolUse)) {
            let _ = sink.send(TurnEvent::Done { stop_reason, usage }).await;
            return Ok(());
        }

        // --- Execute tools ---
        let tool_use_blocks: Vec<(String, String, serde_json::Value)> = current_blocks
            .iter()
            .filter_map(|b| {
                if let ContentBlock::ToolUse { id, name, input } = b {
                    Some((id.clone(), name.clone(), input.clone()))
                } else {
                    None
                }
            })
            .collect();

        let mut tool_results: Vec<ContentBlock> = Vec::new();

        for (tool_id, tool_name, tool_input) in tool_use_blocks {
            // Cancellation check point 3: before each tool.
            if cancel.is_cancelled() {
                let msg = "cancelled".to_string();
                let _ = sink
                    .send(TurnEvent::ToolEnd {
                        id: tool_id.clone(),
                        result: ToolResultContent::Text(msg.clone()),
                        is_error: true,
                    })
                    .await;
                tool_results.push(ContentBlock::ToolResult {
                    tool_use_id: tool_id,
                    content: ToolResultContent::Text(msg),
                    is_error: Some(true),
                });
                continue;
            }

            let (result_text, is_error) = match run_single_tool(
                &tools,
                &tool_ctx,
                &tool_name,
                tool_input,
                &sink,
                &tool_id,
            )
            .await
            {
                Some(out) => out,
                None => continue,
            };

            tool_results.push(ContentBlock::ToolResult {
                tool_use_id: tool_id,
                content: ToolResultContent::Text(result_text),
                is_error: Some(is_error),
            });
        }

        // Append tool results as a user-role message.
        request
            .messages
            .push(Message::user_blocks(tool_results));
    }

    // Exceeded MAX_TOOL_ROUNDS.
    let err = ClaudeError::Other("max tool rounds exceeded".to_string());
    let ret = fail(&sink, err).await;
    Err(ret)
}

/// Emit a `Failed` event carrying `err` (moved) and return a `ClaudeError`
/// holding the same display string. `ClaudeError` is not `Clone`, so we move
/// the original into the event and rebuild a generic error for the return.
async fn fail(sink: &TurnSink, err: ClaudeError) -> ClaudeError {
    let display = err.to_string();
    let _ = sink.send(TurnEvent::Failed { error: err }).await;
    ClaudeError::Other(display)
}

/// Flatten a `SystemPrompt` into a single `String` for logging.
fn system_prompt_text(sp: &Option<SystemPrompt>) -> String {
    match sp {
        None => String::new(),
        Some(SystemPrompt::Text(s)) => s.clone(),
        Some(SystemPrompt::Blocks(blocks)) => blocks
            .iter()
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// Log the turn's initialization info: session id, model, system prompt,
/// available tools, and request shape. Called once at the top of `run_turn`
/// so both CLI and GUI paths emit the same diagnostic.
fn log_turn_init(session_id: &str, request: &ProviderRequest, tools: &[Box<dyn Tool>]) {
    let sp_text = system_prompt_text(&request.system_prompt);
    let sp_char_count = sp_text.chars().count();
    let sp_preview = if sp_char_count > SYSTEM_PROMPT_LOG_PREVIEW {
        format!(
            "{}...(truncated, {} chars total)",
            sp_text.chars().take(SYSTEM_PROMPT_LOG_PREVIEW).collect::<String>(),
            sp_char_count
        )
    } else {
        sp_text
    };

    tracing::info!(
        "run_turn init | session={} model={} messages={} tools_available={} tools_in_request={} max_tokens={} temp={:?}",
        session_id,
        request.model,
        request.messages.len(),
        tools.len(),
        request.tools.len(),
        request.max_tokens,
        request.temperature,
    );

    // System prompt — full content (truncated for readability).
    if sp_preview.is_empty() {
        tracing::info!("run_turn system_prompt: (none)");
    } else {
        tracing::info!("run_turn system_prompt ({} chars):\n{}", sp_char_count, sp_preview);
    }

    // Available tools — name, permission level, description.
    for t in tools.iter() {
        tracing::info!(
            "run_turn tool: {:<16} [{:?}] {}",
            t.name(),
            t.permission_level(),
            t.description(),
        );
    }
}

/// Execute a single tool call. Returns `Some((text, is_error))` describing the
/// tool result to append to history, or `None` if nothing should be appended
/// (currently never).
async fn run_single_tool(
    tools: &[Box<dyn Tool>],
    tool_ctx: &ToolContext,
    tool_name: &str,
    tool_input: serde_json::Value,
    sink: &TurnSink,
    tool_id: &str,
) -> Option<(String, bool)> {
    let tool = tools.iter().find(|t| t.name() == tool_name);
    let tool = match tool {
        Some(t) => t,
        None => {
            let msg = format!("Tool '{}' not found", tool_name);
            let _ = sink
                .send(TurnEvent::ToolEnd {
                    id: tool_id.to_string(),
                    result: ToolResultContent::Text(msg.clone()),
                    is_error: true,
                })
                .await;
            return Some((msg, true));
        }
    };

    // Permission check (synchronous on the trait).
    let perm_req = PermissionRequest {
        tool_name: tool_name.to_string(),
        description: format!("Execute tool: {}", tool_name),
        details: None,
        is_read_only: false,
        path: None,
        working_dir: Some(tool_ctx.working_dir.clone()),
        allowed_roots: {
            let mut roots = tool_ctx.config.workspace_paths.clone();
            roots.extend(tool_ctx.config.additional_dirs.clone());
            roots
        },
        context_description: None,
    };
    let decision = tool_ctx.permission_handler.request_permission(&perm_req);
    match decision {
        PermissionDecision::Allow | PermissionDecision::AllowPermanently => {}
        _ => {
            let msg = "Permission denied".to_string();
            let _ = sink
                .send(TurnEvent::ToolEnd {
                    id: tool_id.to_string(),
                    result: ToolResultContent::Text(msg.clone()),
                    is_error: true,
                })
                .await;
            return Some((msg, true));
        }
    }

    // Execute with panic-safety.
    let exec_result = AssertUnwindSafe(tool.execute(tool_input.clone(), tool_ctx))
        .catch_unwind()
        .await;
    match exec_result {
        Ok(result) => {
            let is_error = result.is_error;
            let _ = sink
                .send(TurnEvent::ToolEnd {
                    id: tool_id.to_string(),
                    result: ToolResultContent::Text(result.content.clone()),
                    is_error,
                })
                .await;
            Some((result.content, is_error))
        }
        Err(_panic) => {
            let msg = "tool panicked".to_string();
            let _ = sink
                .send(TurnEvent::ToolEnd {
                    id: tool_id.to_string(),
                    result: ToolResultContent::Text(msg.clone()),
                    is_error: true,
                })
                .await;
            Some((msg, true))
        }
    }
}
