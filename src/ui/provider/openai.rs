// ui::provider::openai — translate OpenAI chat.completions streaming chunks
// into our unified StreamEvent.
//
// `delta.reasoning_content` (o1/o3 style) is mapped to ThinkingDelta.

use crate::core::types::UsageInfo;
use crate::ui::stream::StreamEvent;

pub fn translate_chunk(line: &str) -> Vec<StreamEvent> {
    let v: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    let Some(choice) = v["choices"].get(0) else { return vec![] };
    let delta = &choice["delta"];
    let mut out = Vec::new();

    if let Some(text) = delta["content"].as_str() {
        if !text.is_empty() {
            out.push(StreamEvent::TextDelta { block: 0, text: text.to_string() });
        }
    }
    if let Some(think) = delta["reasoning_content"].as_str() {
        if !think.is_empty() {
            out.push(StreamEvent::ThinkingDelta { block: 0, text: think.to_string() });
        }
    }
    if let Some(reason) = choice["finish_reason"].as_str() {
        out.push(StreamEvent::MessageStop {
            stop_reason: reason.to_string(),
            usage: UsageInfo::default(),
        });
    }
    if let Some(usage) = v.get("usage") {
        if let Ok(u) = serde_json::from_value::<UsageInfo>(usage.clone()) {
            if let Some(StreamEvent::MessageStop { usage: cur, .. }) = out.last_mut() {
                *cur = u;
            } else {
                out.push(StreamEvent::MessageStop {
                    stop_reason: "stop".into(),
                    usage: u,
                });
            }
        }
    }
    out
}
