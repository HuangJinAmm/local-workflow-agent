// ui::provider::unified — translate the project's unified
// `api::provider_types::StreamEvent` into the UI's `ui::stream::StreamEvent`.
//
// The translator is stateful because `MessageDelta` (carrying stop_reason +
// usage) is buffered until the trailing `MessageStop` event so the UI can
// emit a single merged `MessageStop { stop_reason, usage }` event.

use crate::api::provider_types::{StopReason, StreamEvent as PStream};
use crate::core::types::{ContentBlock, UsageInfo};
use crate::ui::stream::StreamEvent;

/// Stateful translator. Holds the pending `MessageDelta` payload until
/// `MessageStop` arrives.
pub struct Translator {
    pending_stop_reason: Option<StopReason>,
    pending_usage: Option<UsageInfo>,
}

impl Translator {
    pub fn new() -> Self {
        Self {
            pending_stop_reason: None,
            pending_usage: None,
        }
    }

    /// Push a provider event; return zero or more UI events.
    pub fn push(&mut self, ev: PStream) -> Vec<StreamEvent> {
        match ev {
            PStream::MessageStart { id, model, .. } => vec![StreamEvent::MessageStart {
                id,
                model,
            }],
            PStream::ContentBlockStart {
                index,
                content_block,
            } => match content_block {
                ContentBlock::ToolUse { id, name, .. } => vec![StreamEvent::ToolUseStart {
                    block: index,
                    id,
                    name,
                }],
                // Text/Thinking block starts are implicit; the first delta
                // carries the actual content.
                _ => vec![],
            },
            PStream::TextDelta { index, text } => {
                vec![StreamEvent::TextDelta { block: index, text }]
            }
            PStream::ThinkingDelta { index, thinking } => {
                vec![StreamEvent::ThinkingDelta {
                    block: index,
                    text: thinking,
                }]
            }
            PStream::ReasoningDelta { index, reasoning } => {
                // OpenAI-style reasoning_content — surface as ThinkingDelta.
                vec![StreamEvent::ThinkingDelta {
                    block: index,
                    text: reasoning,
                }]
            }
            PStream::InputJsonDelta {
                index,
                partial_json,
            } => vec![StreamEvent::ToolUseDelta {
                block: index,
                partial_json,
            }],
            PStream::SignatureDelta { .. } => vec![],
            PStream::ContentBlockStop { .. } => vec![],
            PStream::MessageDelta { stop_reason, usage } => {
                // Buffer; emit merged with MessageStop.
                self.pending_stop_reason = stop_reason;
                self.pending_usage = usage;
                vec![]
            }
            PStream::MessageStop => {
                let stop_reason = self
                    .pending_stop_reason
                    .take()
                    .map(stop_reason_to_string)
                    .unwrap_or_else(|| "end_turn".into());
                let usage = self.pending_usage.take().unwrap_or_default();
                vec![StreamEvent::MessageStop { stop_reason, usage }]
            }
            PStream::Error {
                error_type,
                message,
            } => vec![StreamEvent::Error {
                message: format!("{error_type}: {message}"),
                retryable: false,
            }],
        }
    }
}

impl Default for Translator {
    fn default() -> Self {
        Self::new()
    }
}

fn stop_reason_to_string(sr: StopReason) -> String {
    match sr {
        StopReason::EndTurn => "end_turn".into(),
        StopReason::StopSequence => "stop_sequence".into(),
        StopReason::MaxTokens => "max_tokens".into(),
        StopReason::ToolUse => "tool_use".into(),
        StopReason::ContentFiltered => "content_filtered".into(),
        StopReason::Other(s) => s,
    }
}
