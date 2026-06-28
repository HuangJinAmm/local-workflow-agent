// ui::session::session_view — middle-pane chat area with input + streaming turn loop.

use gpui::*;
use gpui_component::label::Label;
use gpui_component::{v_flex, ActiveTheme};

use crate::ui::app::AppState;
use crate::ui::input::input_bar::InputBar;
use crate::ui::model::{BlockKind, Role, UiBlock, UiMessage};
use crate::ui::session::message::render_message;
use crate::ui::stream::StreamEvent;
use crate::ui::turn::{make_user_message, new_request, run_turn, with_app_tools, TurnEvent};

/// A unit action fired by Ctrl/Cmd+V. The handler reads the clipboard and
/// appends its text content to the focused input bar. `pub` so the GUI
/// binary can `KeyBinding::new(..., Paste, ...)` it.
#[derive(Clone, PartialEq, Default, Debug, gpui::Action)]
#[action(namespace = session_view)]
pub struct Paste;

pub struct SessionView {
    pub state: Entity<AppState>,
    pub session_id: Option<String>,
    pub messages: Vec<RenderedMessage>,
    pub input: String,
    pub phase: Phase,
    pub cancel_token: Option<tokio_util::sync::CancellationToken>,
    pub input_bar: Entity<InputBar>,
}

/// One user/assistant message plus the blocks we've already rendered for it.
pub struct RenderedMessage {
    pub msg: UiMessage,
    pub blocks: Vec<UiBlock>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Idle,
    Streaming,
}

impl SessionView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let input_bar = cx.new(|_| InputBar::new(state.clone()));
        Self {
            state,
            session_id: None,
            messages: Vec::new(),
            input: String::new(),
            phase: Phase::Idle,
            cancel_token: None,
            input_bar,
        }
    }

    /// Append a new user message and kick off a turn.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        if self.input.trim().is_empty() || self.phase != Phase::Idle {
            return;
        }
        let session_id = self
            .session_id
            .clone()
            .unwrap_or_else(|| "demo".to_string());

        // Push user message locally (in-memory only; persistence is a follow-up).
        let user_msg = UiMessage {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.clone(),
            role: Role::User,
            created_at: chrono::Utc::now().timestamp_millis(),
            ordinal: self.messages.len() as i32,
        };
        let user_block = UiBlock {
            id: uuid::Uuid::new_v4().to_string(),
            message_id: user_msg.id.clone(),
            ordinal: 0,
            kind: BlockKind::Text { text: self.input.clone() },
        };
        self.messages.push(RenderedMessage {
            msg: user_msg,
            blocks: vec![user_block],
        });

        // Push assistant placeholder so streaming has somewhere to land.
        let assistant_msg = UiMessage {
            id: uuid::Uuid::new_v4().to_string(),
            session_id: session_id.clone(),
            role: Role::Assistant,
            created_at: chrono::Utc::now().timestamp_millis(),
            ordinal: self.messages.len() as i32,
        };
        self.messages.push(RenderedMessage {
            msg: assistant_msg,
            blocks: vec![],
        });

        // Build request. ProviderRequest is provider-agnostic; run_turn
        // dispatches to the right `LlmProvider` based on settings.
        let state = self.state.read(cx);
        let model = state.settings.read().default_model.clone();
        let user_msg = make_user_message(&self.input, &[]);
        let mut request = new_request(&model, 1024, vec![user_msg]);
        request = with_app_tools(request, &state);

        // Pick a provider up-front (clone the Arc) so we don't have to hold
        // a non-Send parking_lot read guard across the spawn boundary.
        let provider = {
            let reg = state.providers.read();
            use crate::core::provider_id::ProviderId;
            let by_model = reg.get(&ProviderId::new(model.clone()));
            match by_model.or_else(|| reg.default_provider()) {
                Some(p) => std::sync::Arc::clone(p),
                None => {
                    drop(reg);
                    let _ = state;
                    self.phase = Phase::Idle;
                    cx.notify();
                    return;
                }
            }
        };

        // Mark streaming, allocate cancel token, clear input.
        let cancel = state.begin_turn(session_id.clone());
        self.cancel_token = Some(cancel.clone());
        self.phase = Phase::Streaming;
        let _ = std::mem::take(&mut self.input);
        let _ = state; // release the read guard before notify
        cx.notify();

        // Spawn the producer on the tokio runtime owned by AppState.
        let runtime = self.state.read(cx).runtime.clone();
        let (tx, rx) = tokio::sync::mpsc::channel::<TurnEvent>(64);
        let session_id_for_turn = session_id.clone();
        runtime.spawn(async move {
            run_turn(provider, session_id_for_turn, request, tx, cancel).await;
        });

        // Spawn the consumer on the GPUI executor so it can update the entity.
        // The `Context::spawn` provides a `WeakEntity<Self>` and a
        // `&mut AsyncApp`. The future awaits events on the channel and
        // forwards each one to the entity via `WeakEntity::update`.
        let mut rx = rx;
        cx.spawn(async move |weak_entity: WeakEntity<Self>, cx: &mut AsyncApp| {
            while let Some(ev) = rx.recv().await {
                let stop = matches!(
                    ev,
                    TurnEvent::Done { .. }
                        | TurnEvent::Cancelled
                        | TurnEvent::Failed { .. }
                );
                let _ = weak_entity.update(cx, |session, ctx| {
                    session.on_turn_event(ev, ctx);
                });
                if stop {
                    break;
                }
            }
        })
        .detach();
    }

    pub fn on_turn_event(&mut self, ev: TurnEvent, cx: &mut Context<Self>) {
        match ev {
            TurnEvent::Stream(StreamEvent::TextDelta { block, text }) => {
                if let Some(last) = self.messages.last_mut() {
                    if let Some(b) = last.blocks.iter_mut().find(|b| b.ordinal as usize == block) {
                        if let BlockKind::Text { text: ref mut t } = b.kind {
                            t.push_str(&text);
                        }
                    } else {
                        last.blocks.push(UiBlock {
                            id: uuid::Uuid::new_v4().to_string(),
                            message_id: last.msg.id.clone(),
                            ordinal: block as i32,
                            kind: BlockKind::Text { text },
                        });
                    }
                }
                cx.notify();
            }
            TurnEvent::Done { .. } | TurnEvent::Cancelled | TurnEvent::Failed { .. } => {
                self.phase = Phase::Idle;
                self.cancel_token = None;
                cx.notify();
            }
            _ => {}
        }
    }
}

impl Render for SessionView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let input_bar = self.input_bar.clone();

        // Snapshot every theme color we need up front so we can release the
        // immutable borrow of `cx` (held by `cx.global::<Theme>()`) before we
        // take a mutable borrow of `cx` for `render_message`.
        let theme_bg = cx.theme().background;
        let theme_border = cx.theme().border;
        let theme_muted_fg = cx.theme().muted_foreground;

        // Take a snapshot of the messages so we can iterate without holding a
        // borrow of `self` across the recursive `render_message` calls (which
        // need `&mut App`).
        let messages: Vec<(UiMessage, Vec<UiBlock>)> = self
            .messages
            .iter()
            .map(|m| (m.msg.clone(), m.blocks.clone()))
            .collect();
        let has_session = self.session_id.is_some();
        let is_streaming = self.phase == Phase::Streaming;

        // We need `&mut App` for `render_message` -> `TextView::markdown`.
        // `Context<Self>` derefs to `App`, so reborrow.
        let app: &mut App = &mut *cx;

        let body = if has_session {
            // `overflow_y_scroll` lives on `StatefulInteractiveElement`, which
            // requires `.id(...)` first. We use a fixed id so the scrollable
            // region is stable across renders.
            div()
                .id("session-messages-scroll")
                .flex_1()
                .size_full()
                .overflow_y_scroll()
                .p_2()
                .gap_2()
                .children(
                    messages
                        .iter()
                        .map(|(msg, blocks)| render_message(msg, blocks, window, app)),
                )
                .into_any_element()
        } else {
            div()
                .flex_1()
                .size_full()
                .items_center()
                .justify_center()
                .child(
                    Label::new("No session selected").text_color(theme_muted_fg),
                )
                .into_any_element()
        };

        // Drag-and-drop listener: when files are dropped on the session
        // view, hand them to the input bar for ingestion.
        let on_drop_paths = cx.listener(|this, paths: &ExternalPaths, _window, cx| {
            this.input_bar.update(cx, |bar, ctx| {
                bar.ingest_paths(paths.paths().to_vec(), ctx);
            });
        });

        // Clipboard paste listener: Ctrl/Cmd+V -> read the clipboard text
        // and append to the focused input bar. The action is bound by the
        // GUI binary to global keys.
        let on_paste = cx.listener(|this, _action: &Paste, _window, cx| {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                this.input_bar.update(cx, |bar, ctx| {
                    bar.append_text(&text, ctx);
                });
            }
        });

        v_flex()
            .size_full()
            .bg(theme_bg)
            .on_drop::<ExternalPaths>(on_drop_paths)
            .on_action(on_paste)
            .child(body)
            .child(
                v_flex()
                    .w_full()
                    .p_2()
                    .border_t_1()
                    .border_color(theme_border)
                    .child(input_bar)
                    .children(if is_streaming {
                        Some(
                            Label::new("streaming… (Esc to cancel)")
                                .text_color(theme_muted_fg),
                        )
                    } else {
                        None
                    }),
            )
    }
}
