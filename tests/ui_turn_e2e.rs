// tests/ui_turn_e2e.rs
//! End-to-end test for `run_turn` driven by a `MockProvider`. Verifies that
//! the real `run_turn` consumes a scripted `api::provider_types::StreamEvent`
//! sequence, runs it through the unified translator, and emits a
//! `TurnEvent` stream that the UI's `on_turn_event` would consume.

use std::sync::Arc;
use std::time::Duration;

use local_workflow_agent::api::provider_types::{StopReason, StreamEvent as PStream};
use local_workflow_agent::api::registry::ProviderRegistry;
use local_workflow_agent::core::provider_id::ProviderId;
use local_workflow_agent::core::types::{ContentBlock, Message, UsageInfo};
use local_workflow_agent::tools::Tool;
use local_workflow_agent::ui::stream::StreamEvent as UStream;
use local_workflow_agent::ui::test_support::mock_provider::MockProvider;
use local_workflow_agent::ui::turn::{new_request, run_turn, TurnEvent};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Empty tool registry for tests that don't exercise tool execution.
fn empty_tools() -> Arc<Vec<Box<dyn Tool>>> {
    Arc::new(Vec::new())
}

fn scripted_text_response() -> Vec<PStream> {
    vec![
        PStream::MessageStart {
            id: "msg_e2e".into(),
            model: "claude-sonnet-4-5".into(),
            usage: UsageInfo::default(),
        },
        PStream::ContentBlockStart {
            index: 0,
            content_block: ContentBlock::Text { text: String::new() },
        },
        PStream::TextDelta {
            index: 0,
            text: "Hello".into(),
        },
        PStream::TextDelta {
            index: 0,
            text: ", world".into(),
        },
        PStream::ContentBlockStop { index: 0 },
        PStream::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            usage: Some(UsageInfo {
                input_tokens: 5,
                output_tokens: 2,
                ..Default::default()
            }),
        },
        PStream::MessageStop,
    ]
}

fn scripted_tool_use_response() -> Vec<PStream> {
    vec![
        PStream::MessageStart {
            id: "msg_tool".into(),
            model: "claude-sonnet-4-5".into(),
            usage: UsageInfo::default(),
        },
        PStream::ContentBlockStart {
            index: 0,
            content_block: ContentBlock::ToolUse {
                id: "tool_1".into(),
                name: "bash".into(),
                input: serde_json::json!({}),
            },
        },
        PStream::InputJsonDelta {
            index: 0,
            partial_json: r#"{"comm"#.into(),
        },
        PStream::InputJsonDelta {
            index: 0,
            partial_json: r#"and": "ls"}"#.into(),
        },
        PStream::MessageDelta {
            stop_reason: Some(StopReason::ToolUse),
            usage: None,
        },
        PStream::MessageStop,
    ]
}

fn build_registry_with_mock(events: Vec<PStream>) -> Arc<ProviderRegistry> {
    let mut reg = ProviderRegistry::new();
    let mock = MockProvider::new("anthropic", "Anthropic", events);
    reg.register(mock).set_default(ProviderId::new("anthropic"));
    Arc::new(reg)
}

#[tokio::test]
async fn run_turn_text_response_emits_stream_then_done() {
    let reg = build_registry_with_mock(scripted_text_response());
    let (tx, mut rx) = mpsc::channel::<TurnEvent>(64);
    let cancel = CancellationToken::new();
    let request = new_request("anthropic", 1024, vec![Message::user("hi")]);

    run_turn(
        Arc::clone(reg.default_provider().expect("anthropic registered")),
        "s1".into(),
        request,
        empty_tools(),
        std::path::PathBuf::from("."),
        tx,
        cancel,
    )
    .await;

    let mut collected: Vec<TurnEvent> = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        collected.push(ev);
    }

    // Expect: MessageStart, TextDelta "Hello", TextDelta ", world",
    // MessageStop (with usage merged), Done(end_turn).
    let stream_kinds: Vec<String> = collected
        .iter()
        .filter_map(|e| match e {
            TurnEvent::Stream(s) => Some(format!("{s:?}")),
            _ => None,
        })
        .collect();
    assert!(
        stream_kinds.iter().any(|s| s.contains("MessageStart")),
        "missing MessageStart in {stream_kinds:?}"
    );
    assert!(
        stream_kinds.iter().any(|s| s.contains("TextDelta") && s.contains("Hello")),
        "missing first TextDelta in {stream_kinds:?}"
    );
    assert!(
        stream_kinds.iter().any(|s| s.contains("TextDelta") && s.contains("world")),
        "missing second TextDelta in {stream_kinds:?}"
    );
    assert!(
        stream_kinds.iter().any(|s| s.contains("MessageStop") && s.contains("end_turn")),
        "missing merged MessageStop in {stream_kinds:?}"
    );

    let last = collected.last().expect("at least one event");
    assert!(
        matches!(last, TurnEvent::Done { stop_reason } if stop_reason == "end_turn"),
        "expected trailing Done, got {last:?}"
    );
}

#[tokio::test]
async fn run_turn_tool_use_response_emits_tool_use_start_and_executes() {
    // New behavior: a `tool_use` stop reason triggers tool execution and a
    // second provider round. Since the MockProvider has only one script
    // queued, the second round sees an empty stream, which `run_turn`
    // synthesizes as a clean `end_turn` Done. The tool name (`bash`)
    // doesn't exist in the empty registry, so we expect a "unknown tool"
    // ToolEnd event.
    let reg = build_registry_with_mock(scripted_tool_use_response());
    let (tx, mut rx) = mpsc::channel::<TurnEvent>(64);
    let cancel = CancellationToken::new();
    let request = new_request("anthropic", 1024, vec![Message::user("run ls")]);

    run_turn(
        Arc::clone(reg.default_provider().expect("anthropic registered")),
        "s2".into(),
        request,
        empty_tools(),
        std::path::PathBuf::from("."),
        tx,
        cancel,
    )
    .await;

    let mut collected: Vec<TurnEvent> = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        collected.push(ev);
    }

    let found_tool_start = collected.iter().any(|e| {
        matches!(
            e,
            TurnEvent::Stream(UStream::ToolUseStart { block: 0, id, name })
                if id == "tool_1" && name == "bash"
        )
    });
    assert!(found_tool_start, "expected ToolUseStart, got {collected:?}");

    let found_tool_delta = collected.iter().any(|e| {
        matches!(
            e,
            TurnEvent::Stream(UStream::ToolUseDelta { block: 0, partial_json })
                if partial_json.contains("comm")
        )
    });
    assert!(found_tool_delta, "expected ToolUseDelta, got {collected:?}");

    // The "tool started" envelope is emitted before the executor runs.
    let found_tool_start_envelope = collected.iter().any(|e| {
        matches!(
            e,
            TurnEvent::ToolStart { id, name } if id == "tool_1" && name == "bash"
        )
    });
    assert!(
        found_tool_start_envelope,
        "expected TurnEvent::ToolStart, got {collected:?}"
    );

    // The "tool ended" envelope carries the result. With no tools
    // registered, the result is the "unknown tool" error string.
    let found_tool_end_envelope = collected.iter().any(|e| {
        matches!(
            e,
            TurnEvent::ToolEnd { id, content, is_error: true }
                if id == "tool_1" && content.contains("unknown tool")
        )
    });
    assert!(
        found_tool_end_envelope,
        "expected TurnEvent::ToolEnd (unknown tool), got {collected:?}"
    );

    // The final envelope is a clean Done — the second round (exhausted
    // MockProvider queue) produces a synthesized end_turn.
    let last = collected.last().expect("at least one event");
    assert!(
        matches!(last, TurnEvent::Done { stop_reason } if stop_reason == "end_turn"),
        "expected trailing Done(end_turn), got {last:?}"
    );
}

/// A no-op test tool that records its invocations and returns a
/// deterministic success result. We use this to exercise the success
/// path through `execute_tool_blocks` without depending on any of the
/// real tools (which would require a real `ToolContext` wiring).
struct EchoTool {
    name: String,
    last_input: std::sync::Arc<parking_lot::Mutex<Option<serde_json::Value>>>,
    last_call_count: std::sync::Arc<parking_lot::Mutex<usize>>,
}

#[async_trait::async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        "echo the input back as a result"
    }
    fn permission_level(&self) -> local_workflow_agent::tools::PermissionLevel {
        local_workflow_agent::tools::PermissionLevel::None
    }
    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }
    async fn execute(
        &self,
        input: serde_json::Value,
        _ctx: &local_workflow_agent::tools::ToolContext,
    ) -> local_workflow_agent::tools::ToolResult {
        *self.last_input.lock() = Some(input.clone());
        *self.last_call_count.lock() += 1;
        let s = input.to_string();
        local_workflow_agent::tools::ToolResult::success(format!("echo: {s}"))
    }
}

fn tool_use_script() -> Vec<PStream> {
    vec![
        PStream::MessageStart {
            id: "msg_t".into(),
            model: "claude-sonnet-4-5".into(),
            usage: UsageInfo::default(),
        },
        PStream::ContentBlockStart {
            index: 0,
            content_block: ContentBlock::ToolUse {
                id: "tool_e2e_1".into(),
                name: "echo".into(),
                input: serde_json::json!({}),
            },
        },
        PStream::InputJsonDelta {
            index: 0,
            partial_json: r##"{"message":"##.into(),
        },
        PStream::InputJsonDelta {
            index: 0,
            partial_json: r##""hi"}"##.into(),
        },
        PStream::MessageDelta {
            stop_reason: Some(StopReason::ToolUse),
            usage: None,
        },
        PStream::MessageStop,
    ]
}

fn text_response_script() -> Vec<PStream> {
    vec![
        PStream::MessageStart {
            id: "msg_t2".into(),
            model: "claude-sonnet-4-5".into(),
            usage: UsageInfo::default(),
        },
        PStream::ContentBlockStart {
            index: 0,
            content_block: ContentBlock::Text { text: String::new() },
        },
        PStream::TextDelta {
            index: 0,
            text: "all done".into(),
        },
        PStream::MessageDelta {
            stop_reason: Some(StopReason::EndTurn),
            usage: Some(UsageInfo::default()),
        },
        PStream::MessageStop,
    ]
}

#[tokio::test]
async fn run_turn_executes_tool_and_continues_to_next_round() {
    // Round 1: tool_use → execute EchoTool → emit ToolResult → continue.
    // Round 2: text response with end_turn → emit Done.
    let mock = MockProvider::with_scripts(
        "anthropic",
        "Anthropic",
        vec![tool_use_script(), text_response_script()],
    );
    let last_input = std::sync::Arc::new(parking_lot::Mutex::new(None));
    let last_call_count = std::sync::Arc::new(parking_lot::Mutex::new(0));
    let tool: Box<dyn Tool> = Box::new(EchoTool {
        name: "echo".into(),
        last_input: last_input.clone(),
        last_call_count: last_call_count.clone(),
    });
    let tools: Arc<Vec<Box<dyn Tool>>> = Arc::new(vec![tool]);

    let (tx, mut rx) = mpsc::channel::<TurnEvent>(64);
    let cancel = CancellationToken::new();
    let request = new_request("anthropic", 1024, vec![Message::user("ping")]);

    run_turn(
        mock.clone(),
        "s5".into(),
        request,
        tools,
        std::path::PathBuf::from("."),
        tx,
        cancel,
    )
    .await;

    let mut collected: Vec<TurnEvent> = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        collected.push(ev);
    }

    // EchoTool was invoked exactly once.
    assert_eq!(*last_call_count.lock(), 1, "EchoTool should be called once");
    let captured = last_input.lock().clone();
    assert_eq!(
        captured,
        Some(serde_json::json!({"message": "hi"})),
        "EchoTool should see the fully-parsed JSON input"
    );

    // ToolStart envelope precedes the executor; ToolEnd follows with the result.
    let tool_start_idx = collected.iter().position(|e| matches!(e,
        TurnEvent::ToolStart { id, name } if id == "tool_e2e_1" && name == "echo"
    ));
    let tool_end_idx = collected.iter().position(|e| matches!(e,
        TurnEvent::ToolEnd { id, content, is_error: false }
            if id == "tool_e2e_1" && content.contains("echo")
    ));
    assert!(tool_start_idx.is_some(), "expected ToolStart, got {collected:?}");
    assert!(tool_end_idx.is_some(), "expected ToolEnd, got {collected:?}");
    assert!(
        tool_start_idx.unwrap() < tool_end_idx.unwrap(),
        "ToolStart should precede ToolEnd"
    );

    // Two provider rounds were issued.
    assert_eq!(
        mock.call_count(),
        2,
        "expected 2 create_message_stream calls (tool_use + text)"
    );

    // The final envelope is a clean Done(end_turn) from round 2.
    let last = collected.last().expect("at least one event");
    assert!(
        matches!(last, TurnEvent::Done { stop_reason } if stop_reason == "end_turn"),
        "expected trailing Done(end_turn), got {last:?}"
    );
}

#[tokio::test]
async fn run_turn_no_provider_emits_failed() {
    // With the new signature, `run_turn` takes a single `Arc<dyn LlmProvider>`.
    // The "no provider" path lives in the caller (session_view), so this
    // test exercises what happens when the provider's stream is empty:
    // run_turn should synthesize a MessageStop + Done and exit cleanly.
    use local_workflow_agent::ui::test_support::mock_provider::MockProvider;
    let events: Vec<PStream> = vec![];
    let mock = MockProvider::new("anthropic", "Anthropic", events);
    let provider: std::sync::Arc<dyn local_workflow_agent::api::provider::LlmProvider> = mock;
    let (tx, mut rx) = mpsc::channel::<TurnEvent>(64);
    let cancel = CancellationToken::new();
    let request = new_request("nonexistent", 1024, vec![Message::user("hi")]);

    run_turn(
        provider,
        "s3".into(),
        request,
        empty_tools(),
        std::path::PathBuf::from("."),
        tx,
        cancel,
    )
    .await;

    // Drain events and confirm a clean Done was emitted (no hang/panic).
    let mut got_done = false;
    while let Ok(ev) = rx.try_recv() {
        if matches!(ev, TurnEvent::Done { .. }) {
            got_done = true;
        }
    }
    assert!(
        got_done,
        "expected synthesized Done after empty provider stream"
    );
}

#[tokio::test]
async fn run_turn_cancel_stops_stream() {
    // A provider that yields a few events then blocks. Cancelling should
    // cut the loop and emit a TurnEvent::Cancelled.
    use async_stream::stream;

    let reg = {
        let mut r = ProviderRegistry::new();
        let events = vec![
            PStream::MessageStart {
                id: "msg_c".into(),
                model: "claude-sonnet-4-5".into(),
                usage: UsageInfo::default(),
            },
            PStream::TextDelta {
                index: 0,
                text: "partial".into(),
            },
        ];
        // Wrap in a provider that waits forever after replaying the script.
        struct SlowProvider(std::sync::Arc<MockProvider>);
        #[async_trait::async_trait]
        impl local_workflow_agent::api::provider::LlmProvider for SlowProvider {
            fn id(&self) -> &ProviderId {
                self.0.id()
            }
            fn name(&self) -> &str {
                self.0.name()
            }
            async fn create_message(
                &self,
                r: local_workflow_agent::api::provider_types::ProviderRequest,
            ) -> Result<
                local_workflow_agent::api::provider_types::ProviderResponse,
                local_workflow_agent::api::provider_error::ProviderError,
            > {
                self.0.create_message(r).await
            }
            async fn create_message_stream(
                &self,
                r: local_workflow_agent::api::provider_types::ProviderRequest,
            ) -> Result<
                std::pin::Pin<
                    Box<
                        dyn futures::Stream<
                                Item = Result<
                                    PStream,
                                    local_workflow_agent::api::provider_error::ProviderError,
                                >,
                            > + Send,
                    >,
                >,
                local_workflow_agent::api::provider_error::ProviderError,
            > {
                // Pre-replay the inner stream (consume it now) so the trait
                // object is owned by this future, not borrowed from &self.
                let mut inner = self.0.create_message_stream(r).await?;
                use futures::StreamExt;
                let mut drained: Vec<PStream> = Vec::new();
                while let Some(item) = inner.next().await {
                    drained.push(item?);
                }
                let s = stream! {
                    for ev in drained {
                        yield Ok(ev);
                    }
                    // Park forever — the cancel branch should win the race.
                    futures::future::pending::<()>().await;
                };
                Ok(Box::pin(s))
            }
            async fn list_models(
                &self,
            ) -> Result<
                Vec<local_workflow_agent::api::ModelInfo>,
                local_workflow_agent::api::provider_error::ProviderError,
            > {
                self.0.list_models().await
            }
            async fn health_check(
                &self,
            ) -> Result<
                local_workflow_agent::api::provider_types::ProviderStatus,
                local_workflow_agent::api::provider_error::ProviderError,
            > {
                self.0.health_check().await
            }
            fn capabilities(
                &self,
            ) -> local_workflow_agent::api::provider_types::ProviderCapabilities {
                self.0.capabilities()
            }
        }
        r.register(std::sync::Arc::new(SlowProvider(MockProvider::new(
            "anthropic",
            "Anthropic",
            events,
        ))))
        .set_default(ProviderId::new("anthropic"));
        Arc::new(r)
    };

    let (tx, mut rx) = mpsc::channel::<TurnEvent>(64);
    let cancel = CancellationToken::new();
    let request = new_request("anthropic", 1024, vec![Message::user("hi")]);

    let cancel_handle = cancel.clone();
    let turn_handle = tokio::spawn(async move {
        run_turn(
            Arc::clone(reg.default_provider().expect("anthropic registered")),
            "s4".into(),
            request,
            empty_tools(),
            std::path::PathBuf::from("."),
            tx,
            cancel_handle,
        )
        .await;
    });

    // Wait until at least one event has been emitted, then cancel.
    let mut got_text = false;
    for _ in 0..50 {
        if let Ok(ev) = rx.try_recv() {
            if matches!(ev, TurnEvent::Stream(UStream::TextDelta { .. })) {
                got_text = true;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(got_text, "never received first TextDelta");
    cancel.cancel();

    // Wait for run_turn to finish (it should emit Cancelled).
    let _ = tokio::time::timeout(Duration::from_secs(2), turn_handle).await;

    let mut saw_cancelled = false;
    while let Ok(ev) = rx.try_recv() {
        if matches!(ev, TurnEvent::Cancelled) {
            saw_cancelled = true;
        }
    }
    assert!(saw_cancelled, "expected Cancelled after cancel()");
}
