// ui::session::session_view — middle-pane chat area with input + streaming turn loop.

use gpui::*;
use gpui_component::label::Label;
use gpui_component::{v_flex, Theme};

use crate::ui::app::AppState;
use crate::ui::input::input_bar::InputBar;
use crate::ui::model::{BlockKind, Role, UiBlock, UiMessage};
use crate::ui::stream::StreamEvent;
use crate::ui::turn::{make_user_message, new_request, run_turn, with_app_tools, TurnEvent};

pub struct SessionView {
    pub state: Entity<AppState>,
    pub session_id: Option<String>,
    pub messages: Vec<RenderedMessage>,
    pub input: String,
    pub phase: Phase,
    pub cancel_token: Option<tokio_util::sync::CancellationToken>,
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
    pub fn new(state: Entity<AppState>) -> Self {
        Self {
            state,
            session_id: None,
            messages: Vec::new(),
            input: String::new(),
            phase: Phase::Idle,
            cancel_token: None,
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

        // Mark streaming, allocate cancel token, clear input.
        let cancel = self.state.read(cx).begin_turn(session_id.clone());
        self.cancel_token = Some(cancel.clone());
        self.phase = Phase::Streaming;
        let _ = std::mem::take(&mut self.input);
        let _ = state; // release the read guard before notify
        cx.notify();

        // Spawn the producer on the tokio runtime owned by AppState.
        let runtime = self.state.read(cx).runtime.clone();
        let providers = self.state.read(cx).providers.clone();
        let (tx, rx) = tokio::sync::mpsc::channel::<TurnEvent>(64);
        let session_id_for_turn = session_id.clone();
        runtime.spawn(async move {
            run_turn(&providers, session_id_for_turn, request, tx, cancel).await;
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.clone();
        let input_bar = cx.new(move |_| InputBar::new(state));
        let theme = cx.global::<Theme>();
        v_flex()
            .size_full()
            .bg(theme.background)
            .child(div().flex_1().items_center().justify_center()
                .child(Label::new("No session selected").text_color(theme.muted_foreground)))
            .child(input_bar)
    }
}
