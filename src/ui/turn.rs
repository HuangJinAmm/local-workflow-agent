// ui::turn — the agent turn loop. Pulls events from the provider, accumulates
// into UiMessage, persists, executes tool calls, and loops until end_turn or cancel.

use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::core::provider_id::ProviderId;

use super::app::AppState;
use super::model::*;
use super::stream::StreamEvent;

pub type EventSink = mpsc::Sender<TurnEvent>;

#[derive(Debug, Clone)]
pub enum TurnEvent {
    Stream(StreamEvent),
    ToolStart { id: String, name: String },
    ToolEnd { id: String, content: String, is_error: bool },
    Done { stop_reason: String },
    Failed { message: String, retryable: bool },
    Cancelled,
}

pub async fn run_turn(
    state: Arc<AppState>,
    session_id: SessionId,
    request: crate::api::CreateMessageRequest,
    sink: EventSink,
    cancel: CancellationToken,
) {
    debug!(?session_id, "turn starting");

    // Provider lookup. The current registry is empty (Task 12 used
    // ProviderRegistry::new()), so this fails — that's expected. A
    // later task will wire a real registry populated from env vars.
    //
    // Note: ProviderRegistry exposes `get(&ProviderId)`, not
    // `get_for_model(&str)`. We attempt a heuristic lookup by treating
    // the model name as a ProviderId. With an empty registry this
    // always returns `None`.
    let provider_id = ProviderId::new(request.model.clone());
    let provider = match state.providers.get(&provider_id) {
        Some(p) => p,
        None => {
            let _ = sink
                .send(TurnEvent::Failed {
                    message: format!("provider lookup: '{}' not registered", provider_id),
                    retryable: false,
                })
                .await;
            return;
        }
    };

    // TODO(ui-turn): The current `LlmProvider` trait exposes
    // `create_message_stream(&ProviderRequest) -> Stream<Item = Result<api::provider_types::StreamEvent, _>>`,
    // which differs from this driver in two ways:
    //   1. The request type is `ProviderRequest`, not `CreateMessageRequest`.
    //   2. The stream item is `api::provider_types::StreamEvent`, not
    //      `ui::stream::StreamEvent` (the one wrapped in `TurnEvent::Stream`).
    // Wiring the real call requires a `CreateMessageRequest -> ProviderRequest`
    // adapter and a per-provider `StreamEvent` translator (see
    // `ui::provider::anthropic::translate` for the existing Anthropic one).
    // That work is deferred to a follow-up task; the rest of the function
    // body below is intentionally guarded behind a temporary `Failed`
    // emission so the file compiles and the function shape is in place.
    let _ = provider;
    let _ = cancel;
    let _ = request;
    warn!(
        ?session_id,
        "run_turn: provider stream wiring deferred; emitting Failed"
    );
    let _ = sink
        .send(TurnEvent::Failed {
            message: "provider stream wiring deferred (see TODO in src/ui/turn.rs)".into(),
            retryable: false,
        })
        .await;
}

pub fn make_user_message(text: &str, attachments: &[Attachment]) -> crate::api::ApiMessage {
    let mut blocks: Vec<serde_json::Value> = Vec::new();
    if !attachments.is_empty() {
        let items: Vec<serde_json::Value> = attachments
            .iter()
            .map(|a| {
                serde_json::json!({
                    "id": a.id,
                    "kind": format!("{:?}", a.kind),
                    "display_name": a.display_name,
                    "mime": a.mime,
                    "local_path": a.local_path.to_string_lossy(),
                    "size_bytes": a.size_bytes,
                })
            })
            .collect();
        blocks.push(serde_json::json!({ "type": "attachments", "items": items }));
    }
    blocks.push(serde_json::json!({ "type": "text", "text": text }));
    crate::api::ApiMessage {
        role: "user".into(),
        content: serde_json::Value::Array(blocks),
    }
}

pub fn new_request(
    model: &str,
    max_tokens: u32,
    messages: Vec<crate::api::ApiMessage>,
) -> crate::api::CreateMessageRequest {
    let mut b = crate::api::CreateMessageRequest::builder(model, max_tokens);
    for m in messages {
        b = b.add_message(m);
    }
    b.build()
}
