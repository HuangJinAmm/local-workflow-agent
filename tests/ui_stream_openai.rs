// tests/ui_stream_openai.rs
use local_workflow_agent::ui::provider::openai::translate_chunk;
use local_workflow_agent::ui::stream::StreamEvent;

#[test]
fn text_delta_translates() {
    let json = serde_json::json!({
        "choices": [{
            "delta": { "content": "hi" },
            "finish_reason": null
        }]
    })
    .to_string();
    let out = translate_chunk(&json);
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
fn reasoning_maps_to_thinking() {
    let json = serde_json::json!({
        "choices": [{
            "delta": { "reasoning_content": "let me think…" },
            "finish_reason": null
        }]
    })
    .to_string();
    let out = translate_chunk(&json);
    assert_eq!(out.len(), 1);
    match &out[0] {
        StreamEvent::ThinkingDelta { block, text } => {
            assert_eq!(*block, 0);
            assert_eq!(text, "let me think…");
        }
        other => panic!("expected ThinkingDelta, got {other:?}"),
    }
}

#[test]
fn finish_reason_maps_to_message_stop() {
    let json = serde_json::json!({
        "choices": [{ "delta": {}, "finish_reason": "stop" }]
    })
    .to_string();
    let out = translate_chunk(&json);
    assert_eq!(out.len(), 1);
    match &out[0] {
        StreamEvent::MessageStop { stop_reason, .. } => assert_eq!(stop_reason, "stop"),
        other => panic!("expected MessageStop, got {other:?}"),
    }
}
