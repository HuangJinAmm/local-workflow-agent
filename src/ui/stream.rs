// ui::stream — unified streaming events consumed by the UI.
// Provider adapters in src/ui/provider/* translate their wire format into these.

use crate::core::types::UsageInfo;

#[derive(Debug, Clone)]
pub enum StreamEvent {
    MessageStart { id: String, model: String },
    TextDelta { block: usize, text: String },
    ThinkingDelta { block: usize, text: String },
    ToolUseStart { block: usize, id: String, name: String },
    ToolUseDelta { block: usize, partial_json: String },
    MessageStop { stop_reason: String, usage: UsageInfo },
    Error { message: String, retryable: bool },
}
