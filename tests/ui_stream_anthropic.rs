// tests/ui_stream_anthropic.rs
use local_workflow_agent::api::streaming::AnthropicStreamEvent;
use local_workflow_agent::api::streaming::ContentDelta;
use local_workflow_agent::ui::provider::anthropic::translate;
use local_workflow_agent::ui::stream::StreamEvent;

#[test]
fn text_delta_translates() {
    let ev = AnthropicStreamEvent::ContentBlockDelta {
        index: 0,
        delta: ContentDelta::TextDelta { text: "hi".into() },
    };
    let out = translate(&ev);
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
    let ev = AnthropicStreamEvent::ContentBlockDelta {
        index: 1,
        delta: ContentDelta::ThinkingDelta {
            thinking: "…".into(),
        },
    };
    let out = translate(&ev);
    assert_eq!(out.len(), 1);
    match &out[0] {
        StreamEvent::ThinkingDelta { block, text } => {
            assert_eq!(*block, 1);
            assert_eq!(text, "…");
        }
        other => panic!("expected ThinkingDelta, got {other:?}"),
    }
}

#[test]
fn error_marks_rate_limit_retryable() {
    let ev = AnthropicStreamEvent::Error {
        error_type: "rate_limit_error".into(),
        message: "slow down".into(),
    };
    let out = translate(&ev);
    assert!(matches!(
        out.as_slice(),
        [StreamEvent::Error { retryable: true, .. }]
    ));
}
