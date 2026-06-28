// tests/ui_stream_unified.rs
//! Tests for the unified `api::provider_types::StreamEvent` → `ui::stream::StreamEvent`
//! translator. This is the bridge between the project's provider layer
//! and the UI's simplified stream.

use local_workflow_agent::api::provider_types::{StopReason, StreamEvent as PStream};
use local_workflow_agent::core::types::{
    ContentBlock, Message, MessageContent, Role, UsageInfo,
};
use local_workflow_agent::ui::provider::unified::Translator;
use local_workflow_agent::ui::stream::StreamEvent;

fn msg_start() -> PStream {
    PStream::MessageStart {
        id: "msg_01".into(),
        model: "claude-sonnet-4-5".into(),
        usage: UsageInfo::default(),
    }
}

fn tool_use_start() -> PStream {
    PStream::ContentBlockStart {
        index: 1,
        content_block: ContentBlock::ToolUse {
            id: "tool_1".into(),
            name: "bash".into(),
            input: serde_json::json!({}),
        },
    }
}

#[test]
fn message_start_translates() {
    let mut t = Translator::new();
    let out = t.push(msg_start());
    assert_eq!(out.len(), 1);
    match &out[0] {
        StreamEvent::MessageStart { id, model } => {
            assert_eq!(id, "msg_01");
            assert_eq!(model, "claude-sonnet-4-5");
        }
        other => panic!("expected MessageStart, got {other:?}"),
    }
}

#[test]
fn text_delta_translates() {
    let mut t = Translator::new();
    t.push(msg_start());
    let out = t.push(PStream::TextDelta {
        index: 0,
        text: "hi".into(),
    });
    assert_eq!(out.len(), 1);
    match &out[0] {
        StreamEvent::TextDelta { block, text } => {
            assert_eq!(*block, 0);
            assert_eq!(text, "hi");
        }
        other => panic!("expected TextDelta, got {other:?}"),
    }
}

#[test]
fn thinking_delta_translates() {
    let mut t = Translator::new();
    let out = t.push(PStream::ThinkingDelta {
        index: 2,
        thinking: "reasoning…".into(),
    });
    assert_eq!(out.len(), 1);
    match &out[0] {
        StreamEvent::ThinkingDelta { block, text } => {
            assert_eq!(*block, 2);
            assert_eq!(text, "reasoning…");
        }
        other => panic!("expected ThinkingDelta, got {other:?}"),
    }
}

#[test]
fn reasoning_delta_maps_to_thinking() {
    let mut t = Translator::new();
    let out = t.push(PStream::ReasoningDelta {
        index: 0,
        reasoning: "o1 chain".into(),
    });
    assert_eq!(out.len(), 1);
    match &out[0] {
        StreamEvent::ThinkingDelta { block, text } => {
            assert_eq!(*block, 0);
            assert_eq!(text, "o1 chain");
        }
        other => panic!("expected ThinkingDelta, got {other:?}"),
    }
}

#[test]
fn tool_use_block_start_emits_tool_use_start() {
    let mut t = Translator::new();
    let out = t.push(tool_use_start());
    assert_eq!(out.len(), 1);
    match &out[0] {
        StreamEvent::ToolUseStart { block, id, name } => {
            assert_eq!(*block, 1);
            assert_eq!(id, "tool_1");
            assert_eq!(name, "bash");
        }
        other => panic!("expected ToolUseStart, got {other:?}"),
    }
}

#[test]
fn text_block_start_emits_nothing() {
    let mut t = Translator::new();
    let out = t.push(PStream::ContentBlockStart {
        index: 0,
        content_block: ContentBlock::Text {
            text: String::new(),
        },
    });
    assert!(out.is_empty(), "text/thinking block starts are implicit");
}

#[test]
fn input_json_delta_translates() {
    let mut t = Translator::new();
    t.push(tool_use_start());
    let out = t.push(PStream::InputJsonDelta {
        index: 1,
        partial_json: "{\"comma".into(),
    });
    assert_eq!(out.len(), 1);
    match &out[0] {
        StreamEvent::ToolUseDelta { block, partial_json } => {
            assert_eq!(*block, 1);
            assert_eq!(partial_json, "{\"comma");
        }
        other => panic!("expected ToolUseDelta, got {other:?}"),
    }
}

#[test]
fn message_delta_then_stop_merges() {
    let mut t = Translator::new();
    t.push(msg_start());
    let d = t.push(PStream::MessageDelta {
        stop_reason: Some(StopReason::EndTurn),
        usage: Some(UsageInfo {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        }),
    });
    assert!(
        d.is_empty(),
        "MessageDelta should be buffered until MessageStop"
    );
    let s = t.push(PStream::MessageStop);
    assert_eq!(s.len(), 1);
    match &s[0] {
        StreamEvent::MessageStop { stop_reason, usage } => {
            assert_eq!(stop_reason, "end_turn");
            assert_eq!(usage.input_tokens, 10);
            assert_eq!(usage.output_tokens, 5);
        }
        other => panic!("expected MessageStop, got {other:?}"),
    }
}

#[test]
fn stop_without_delta_uses_end_turn_default() {
    let mut t = Translator::new();
    let s = t.push(PStream::MessageStop);
    assert_eq!(s.len(), 1);
    match &s[0] {
        StreamEvent::MessageStop { stop_reason, .. } => {
            assert_eq!(stop_reason, "end_turn");
        }
        other => panic!("expected MessageStop, got {other:?}"),
    }
}

#[test]
fn tool_use_stop_reason_merges() {
    let mut t = Translator::new();
    t.push(msg_start());
    let _ = t.push(PStream::MessageDelta {
        stop_reason: Some(StopReason::ToolUse),
        usage: None,
    });
    let s = t.push(PStream::MessageStop);
    assert_eq!(s.len(), 1);
    match &s[0] {
        StreamEvent::MessageStop { stop_reason, .. } => {
            assert_eq!(stop_reason, "tool_use");
        }
        other => panic!("expected MessageStop, got {other:?}"),
    }
}

#[test]
fn error_translates() {
    let mut t = Translator::new();
    let out = t.push(PStream::Error {
        error_type: "api_error".into(),
        message: "boom".into(),
    });
    assert_eq!(out.len(), 1);
    match &out[0] {
        StreamEvent::Error { message, retryable } => {
            assert_eq!(message, "api_error: boom");
            assert!(!retryable);
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn content_block_stop_emits_nothing() {
    let mut t = Translator::new();
    let out = t.push(PStream::ContentBlockStop { index: 0 });
    assert!(out.is_empty());
}

#[test]
fn signature_delta_emits_nothing() {
    let mut t = Translator::new();
    let out = t.push(PStream::SignatureDelta {
        index: 0,
        signature: "sig".into(),
    });
    assert!(out.is_empty());
}

#[test]
fn stop_reason_other_round_trips() {
    let mut t = Translator::new();
    let _ = t.push(PStream::MessageDelta {
        stop_reason: Some(StopReason::Other("custom".into())),
        usage: None,
    });
    let s = t.push(PStream::MessageStop);
    assert_eq!(s.len(), 1);
    match &s[0] {
        StreamEvent::MessageStop { stop_reason, .. } => {
            assert_eq!(stop_reason, "custom");
        }
        other => panic!("expected MessageStop, got {other:?}"),
    }
}

#[test]
fn message_struct_is_provider_request_compatible() {
    let m = Message::user("hello");
    let v = serde_json::to_value(&m).unwrap();
    let m2: Message = serde_json::from_value(v).unwrap();
    match m2.content {
        MessageContent::Text(ref s) => assert_eq!(s, "hello"),
        _ => panic!("expected Text content"),
    }
    assert_eq!(m2.role, Role::User);
}
