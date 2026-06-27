// ui::provider::anthropic — translate AnthropicStreamEvent -> StreamEvent.

use crate::api::streaming::{AnthropicStreamEvent, ContentDelta};
use crate::core::types::ContentBlock;
use crate::ui::stream::StreamEvent;

pub fn translate(ev: &AnthropicStreamEvent) -> Vec<StreamEvent> {
    match ev {
        AnthropicStreamEvent::MessageStart { id, model, .. } => vec![
            StreamEvent::MessageStart {
                id: id.clone(),
                model: model.clone(),
            },
        ],
        AnthropicStreamEvent::ContentBlockStart {
            index,
            content_block,
        } => match content_block {
            ContentBlock::ToolUse { id, name, .. } => vec![StreamEvent::ToolUseStart {
                block: *index,
                id: id.clone(),
                name: name.clone(),
            }],
            _ => vec![],
        },
        AnthropicStreamEvent::ContentBlockDelta { index, delta } => match delta {
            ContentDelta::TextDelta { text } => vec![StreamEvent::TextDelta {
                block: *index,
                text: text.clone(),
            }],
            ContentDelta::ThinkingDelta { thinking } => vec![StreamEvent::ThinkingDelta {
                block: *index,
                text: thinking.clone(),
            }],
            ContentDelta::InputJsonDelta { partial_json } => vec![StreamEvent::ToolUseDelta {
                block: *index,
                partial_json: partial_json.clone(),
            }],
            ContentDelta::SignatureDelta { .. } => vec![],
        },
        AnthropicStreamEvent::MessageDelta { stop_reason, usage } => vec![
            StreamEvent::MessageStop {
                stop_reason: stop_reason.clone().unwrap_or_else(|| "end_turn".into()),
                usage: usage.clone().unwrap_or_default(),
            },
        ],
        AnthropicStreamEvent::ContentBlockStop { .. } => vec![],
        AnthropicStreamEvent::MessageStop => vec![],
        AnthropicStreamEvent::Ping => vec![],
        AnthropicStreamEvent::Error {
            error_type,
            message,
        } => vec![StreamEvent::Error {
            message: format!("{error_type}: {message}"),
            retryable: matches!(
                error_type.as_str(),
                "rate_limit_error" | "overloaded_error"
            ),
        }],
    }
}
