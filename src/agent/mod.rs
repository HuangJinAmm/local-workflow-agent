//! Agent turn orchestration — shared between CLI and UI.
//!
//! The `run_turn` function drives a multi-round LLM conversation with
//! tool-use: it streams provider events, accumulates text / tool_use
//! blocks, executes tools when `stop_reason == ToolUse`, appends
//! `ToolResult` blocks, and re-calls the provider until `end_turn`,
//! cancellation, or `MAX_TOOL_ROUNDS` is reached.

mod turn;

pub use turn::{run_turn, TurnCancel, TurnEvent, TurnSink, MAX_TOOL_ROUNDS};
