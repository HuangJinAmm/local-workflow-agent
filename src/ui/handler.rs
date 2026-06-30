//! Background task handlers — the bridge between the GPUI foreground and the
//! `local_workflow_agent` library.
//!
//! `handle_outgoing` consumes `AgentRequest`s and drives the library-backed
//! `Agent`; `handle_incoming` pumps `AgentResponse`s back into the foreground
//! `ChatAI` view.
//!
//! Each `Chat` request builds a `ProviderRequest` via `Agent::build_provider_request`,
//! constructs a `ToolContext` (with a `GuiPermissionHandler` whose modal
//! decisions round-trip through the response channel), and hands everything
//! to `crate::agent::run_turn`. `run_turn` streams `TurnEvent`s back through
//! an `async_channel` sink; a forwarding task wraps each event in
//! `AgentResponse::TurnEvent` before sending it to the foreground.

use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use async_channel::{Receiver, Sender};
use gpui::{AppContext, AsyncApp, WeakEntity};
use tokio_util::sync::CancellationToken;

use crate::agent::{run_turn, TurnEvent};
use crate::core::config::{Config, PermissionMode};
use crate::core::cost::CostTracker;
use crate::core::file_history::FileHistory;
use crate::core::permissions::PermissionHandler;
use crate::tools::{all_tools, ToolContext, UserQuestionEvent};
use crate::ui::{
    ChatAI,
    permission_modal::PermissionRequest as ModalPermissionRequest,
    services::agent::{
        Agent, AgentRequest, AgentResponse, ContentBlock, FileSource, GuiPermissionHandler,
        GuiPermissionRequest, UiMessage, upload_file,
    },
};

/// Entry point spawned on GPUI's `background_executor`.
///
/// GPUI's executor is NOT a Tokio runtime, but the underlying library uses
/// `reqwest` + `tokio::spawn` / `tokio::time::sleep` / `tokio::sync::mpsc`
/// internally, all of which require a Tokio 1.x reactor in the current
/// thread-local context. Calling them without one panics with
/// "there is no reactor running, must be called from the context of a
/// Tokio 1.x runtime".
///
/// To fix this we build a dedicated multi-threaded Tokio `Runtime` here and
/// drive the actual agent loop on one of its worker threads via
/// `handle.spawn(...).await`. The `JoinHandle` is awaited on the GPUI
/// executor (which only parks the task), while the real HTTP I/O happens
/// on the Tokio runtime where a reactor is available.
pub async fn handle_outgoing(
    request_rx: Receiver<AgentRequest>,
    response_tx: Sender<AgentResponse>,
) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("Failed to build Tokio runtime: {}", e);
            let _ = response_tx.try_send(AgentResponse::Error(format!(
                "Failed to initialize Tokio runtime: {}",
                e
            )));
            return;
        }
    };

    // Spawn the agent loop on the Tokio runtime so that reqwest/tokio calls
    // inside `run_turn` / `upload_file` have a reactor. `runtime`
    // stays alive in this frame until the spawned future completes, then is
    // dropped to shut the reactor down.
    let join = runtime
        .handle()
        .spawn(run_agent_loop(request_rx, response_tx));
    if let Err(e) = join.await {
        tracing::error!("Agent loop panicked: {}", e);
    }
}

async fn run_agent_loop(
    request_rx: Receiver<AgentRequest>,
    response_tx: Sender<AgentResponse>,
) {
    let mut agent = match Agent::builder()
        .system_prompt(
            "You are a helpful, succint assistant. Please respond only in markdown and no emojis."
                .to_string(),
        )
        .max_tokens(4096)
        .build(vec![])
    {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("Failed to build agent: {}", e);
            let _ = response_tx.try_send(AgentResponse::Error(format!(
                "Failed to initialize agent: {}",
                e
            )));
            return;
        }
    };

    // Working directory used for tool execution. Updated via
    // `AgentRequest::SetWorkingDir`; defaults to the process cwd.
    let mut working_dir =
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    // Cancellation token for the in-flight turn, if any. Replaced on every
    // new `Chat` request; triggered by `AgentRequest::Cancel`.
    let mut current_cancel: Option<CancellationToken> = None;

    while let Ok(request) = request_rx.recv().await {
        match request {
            AgentRequest::Chat { content, files } => {
                // 1. Upload any attached files using the current API key.
                let api_key = agent.api_key();
                let mut user_content = vec![ContentBlock::Text { text: content }];
                for path in files {
                    match upload_file(&api_key, &path).await {
                        Ok(file_id) => {
                            user_content.push(ContentBlock::Document {
                                source: FileSource::File { file_id },
                            });
                        }
                        Err(e) => {
                            tracing::error!("Failed to upload file: {}", e);
                            let _ = response_tx.try_send(AgentResponse::Error(format!(
                                "Failed to upload file: {}",
                                e
                            )));
                        }
                    }
                }

                // 2. Build the ProviderRequest (adds the user message to the
                //    in-memory transcript and translates it into library types).
                let provider_request = match agent.build_provider_request(user_content) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = response_tx
                            .try_send(AgentResponse::Error(format!("{}", e)));
                        continue;
                    }
                };

                // 3. Clone the provider — `run_turn` needs an owned `Arc<dyn LlmProvider>`.
                let provider = agent.provider_arc();

                // 4. Permission channel: the GuiPermissionHandler pushes its
                //    own `PermissionRequest` here; a forwarding task converts
                //    it to the modal's `PermissionRequest` and relays it to
                //    the foreground.
                let (perm_tx, perm_rx) =
                    async_channel::unbounded::<GuiPermissionRequest>();
                let permission_handler =
                    Arc::new(GuiPermissionHandler::new(perm_tx))
                        as Arc<dyn PermissionHandler>;

                // 5. Ask channel: the `AskUserQuestion` tool pushes its
                //    `UserQuestionEvent` here; a forwarding task relays it to
                //    the foreground.
                let (ask_tx, mut ask_rx) =
                    tokio::sync::mpsc::unbounded_channel::<UserQuestionEvent>();

                // 6. Session id (per-turn).
                let session_id = uuid::Uuid::new_v4().to_string();

                // 7. Build the ToolContext.
                let tool_ctx = Arc::new(ToolContext {
                    working_dir: working_dir.clone(),
                    permission_mode: PermissionMode::Default,
                    permission_handler,
                    cost_tracker: CostTracker::new(),
                    session_id: session_id.clone(),
                    current_turn: Arc::new(AtomicUsize::new(0)),
                    file_history: Arc::new(parking_lot::Mutex::new(
                        FileHistory::new(),
                    )),
                    lsp_manager: None,
                    non_interactive: false,
                    mcp_manager: None,
                    config: Config::default(),
                    managed_agent_config: None,
                    completion_notifier: None,
                    pending_permissions: None,
                    permission_manager: None,
                    user_question_tx: Some(ask_tx),
                });

                // 8. Cancellation token for this turn.
                let cancel = CancellationToken::new();
                current_cancel = Some(cancel.clone());

                // 9. Turn-event sink + forwarding task: relays `TurnEvent`s
                //    to the foreground wrapped in `AgentResponse::TurnEvent`.
                let (turn_sink, turn_rx) =
                    async_channel::unbounded::<TurnEvent>();
                let response_tx_clone = response_tx.clone();
                tokio::spawn(async move {
                    while let Ok(ev) = turn_rx.recv().await {
                        let _ = response_tx_clone
                            .send(AgentResponse::TurnEvent(ev))
                            .await;
                    }
                });

                // 10. Permission forwarding task: convert the handler's
                //     `PermissionRequest` to the modal's `PermissionRequest`
                //     and relay it to the foreground.
                let response_tx_clone = response_tx.clone();
                tokio::spawn(async move {
                    while let Ok(req) = perm_rx.recv().await {
                        let modal_req = ModalPermissionRequest {
                            id: req.id,
                            tool_name: req.tool_name,
                            input: serde_json::json!({
                                "description": req.description,
                                "details": req.details,
                                "path": req.path,
                            }),
                            level: if req.is_read_only {
                                "ReadOnly".to_string()
                            } else {
                                "Write".to_string()
                            },
                            reply_tx: req.reply_tx,
                        };
                        let _ = response_tx_clone
                            .send(AgentResponse::PermissionRequest(modal_req))
                            .await;
                    }
                });

                // 11. Ask forwarding task: relay `UserQuestionEvent`s to the
                //     foreground.
                let response_tx_clone = response_tx.clone();
                tokio::spawn(async move {
                    while let Some(ev) = ask_rx.recv().await {
                        let _ = response_tx_clone
                            .send(AgentResponse::UserQuestion(ev))
                            .await;
                    }
                });

                // 12. Spawn `run_turn` — it drives the multi-round tool-use
                //     loop, streaming `TurnEvent`s to `turn_sink`. The loop
                //     continues so `Cancel` can be received concurrently.
                let tools = Arc::new(all_tools());
                let response_tx_err = response_tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = run_turn(
                        provider,
                        session_id,
                        provider_request,
                        tools,
                        tool_ctx,
                        turn_sink,
                        cancel,
                    )
                    .await
                    {
                        let _ = response_tx_err
                            .send(AgentResponse::Error(e.to_string()))
                            .await;
                    }
                });
            }
            AgentRequest::Cancel => {
                if let Some(cancel) = current_cancel.take() {
                    cancel.cancel();
                }
            }
            AgentRequest::SetWorkingDir(path) => {
                working_dir = path;
            }
            AgentRequest::ClearHistory => {
                agent.clear_conversation();
            }
            AgentRequest::SetProvider(provider) => {
                tracing::debug!("Setting agent provider to: {}", provider);
                if let Err(e) = agent.set_provider(provider) {
                    let _ = response_tx.try_send(AgentResponse::Error(format!(
                        "Failed to update provider: {}",
                        e
                    )));
                }
            }
            AgentRequest::SetModel(model) => {
                tracing::debug!("Setting agent model to: {}", model);
                agent.set_model(model);
                agent.clear_conversation();
            }
            AgentRequest::SetApiKey(api_key) => {
                tracing::debug!("Updating agent API key");
                if let Err(e) = agent.set_api_key(api_key) {
                    let _ = response_tx.try_send(AgentResponse::Error(format!(
                        "Failed to update API key: {}",
                        e
                    )));
                }
            }
            AgentRequest::SetBaseUrl(base_url) => {
                tracing::debug!("Updating agent base URL to: {}", base_url);
                if let Err(e) = agent.set_base_url(base_url) {
                    let _ = response_tx.try_send(AgentResponse::Error(format!(
                        "Failed to update base URL: {}",
                        e
                    )));
                }
            }
            AgentRequest::SetApiConfig { api_key, base_url } => {
                tracing::debug!("Updating agent API config (key + base URL)");
                if let Err(e) = agent.set_api_config(api_key, base_url) {
                    let _ = response_tx.try_send(AgentResponse::Error(format!(
                        "Failed to update API config: {}",
                        e
                    )));
                }
            }
        }
    }
}

pub async fn handle_incoming(
    this: WeakEntity<ChatAI>,
    response_rx: Receiver<AgentResponse>,
    cx: &mut AsyncApp,
) {
    loop {
        let incoming_response = response_rx.recv().await;
        match incoming_response {
            Ok(response) => match response {
                AgentResponse::TurnEvent(ev) => {
                    match ev {
                        TurnEvent::TextDelta { text } => {
                            // TODO(Task 12): stream into the current assistant
                            // message via `ChatAI::handle_turn_event`. Until
                            // then the incremental deltas are not surfaced —
                            // the full text rendering is wired up in chat.rs.
                            let _ = text;
                        }
                        TurnEvent::ToolUseStart { .. }
                        | TurnEvent::ToolUseDelta { .. }
                        | TurnEvent::ToolEnd { .. } => {
                            // TODO(Task 12): render tool calls in the chat view.
                        }
                        TurnEvent::Done { .. } => {
                            if let Some(view) = this.upgrade() {
                                let _ = cx.update_entity(&view, |this, cx| {
                                    this.set_loading(false, cx);
                                });
                            }
                        }
                        TurnEvent::Failed { error } => {
                            if let Some(view) = this.upgrade() {
                                let _ = cx.update_entity(&view, |this, cx| {
                                    this.add_message(
                                        UiMessage::error(error.to_string()),
                                        cx,
                                    );
                                    this.set_loading(false, cx);
                                });
                            }
                        }
                        TurnEvent::Cancelled => {
                            if let Some(view) = this.upgrade() {
                                let _ = cx.update_entity(&view, |this, cx| {
                                    this.set_loading(false, cx);
                                });
                            }
                        }
                    }
                }
                AgentResponse::PermissionRequest(_req) => {
                    // TODO(Task 12): call `this.show_permission_modal(req, cx)`.
                    // Dropping `_req` drops its `reply_tx`, which the
                    // `GuiPermissionHandler` treats as a denial — acceptable
                    // until the modal is wired up in chat.rs.
                    tracing::warn!(
                        "permission request received; \
                         show_permission_modal not implemented yet"
                    );
                }
                AgentResponse::UserQuestion(_ev) => {
                    // TODO(Task 12): call `this.show_ask_modal(ev, cx)`.
                    tracing::warn!(
                        "user question received; \
                         show_ask_modal not implemented yet"
                    );
                }
                AgentResponse::Error(err) => {
                    if let Some(view) = this.upgrade() {
                        let _ = cx.update_entity(&view, |this, cx| {
                            this.add_message(UiMessage::error(err), cx);
                            this.set_loading(false, cx);
                        });
                    }
                }
            },
            Err(e) => {
                tracing::error!("Channel error: {}", e);
                if let Some(view) = this.upgrade() {
                    let _ = cx.update_entity(&view, |this, cx| {
                        this.set_loading(false, cx);
                    });
                }
                break;
            }
        }
    }
}
