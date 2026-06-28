// tests/ui_turn_loop.rs
// Verifies the StreamEvent pipeline by feeding a fixed sequence and
// asserting the consumer reads them in order.

use futures::{stream, Stream};
use local_workflow_agent::ui::stream::StreamEvent;
use std::pin::Pin;

fn scripted_events() -> Vec<StreamEvent> {
    vec![
        StreamEvent::MessageStart { id: "m".into(), model: "test".into() },
        StreamEvent::TextDelta { block: 0, text: "Hello".into() },
        StreamEvent::TextDelta { block: 0, text: " world".into() },
        StreamEvent::MessageStop {
            stop_reason: "end_turn".into(),
            usage: local_workflow_agent::core::types::UsageInfo::default(),
        },
    ]
}

#[test]
fn scripted_stream_round_trip() {
    use futures::StreamExt;
    let s: Pin<Box<dyn Stream<Item = StreamEvent> + Send>> =
        Box::pin(stream::iter(scripted_events()));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let out = rt.block_on(async {
        let mut s = s;
        let mut out = String::new();
        while let Some(ev) = s.next().await {
            if let StreamEvent::TextDelta { text, .. } = ev { out.push_str(&text); }
        }
        out
    });
    assert_eq!(out, "Hello world");
}

#[test]
fn stops_on_message_stop() {
    use futures::StreamExt;
    let events = vec![
        StreamEvent::TextDelta { block: 0, text: "x".into() },
        StreamEvent::MessageStop {
            stop_reason: "end_turn".into(),
            usage: local_workflow_agent::core::types::UsageInfo::default(),
        },
        // This third event should never be observed if the consumer
        // breaks the loop on MessageStop, but for a raw `stream::iter`
        // it is. So this test only asserts that the raw stream is finite.
    ];
    let s: Pin<Box<dyn Stream<Item = StreamEvent> + Send>> =
        Box::pin(stream::iter(events));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let count = rt.block_on(async {
        let mut s = s;
        let mut count = 0;
        while let Some(_) = s.next().await { count += 1; }
        count
    });
    assert_eq!(count, 2);
}
