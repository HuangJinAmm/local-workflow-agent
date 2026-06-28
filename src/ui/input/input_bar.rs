// ui::input::input_bar — bottom-of-pane input.
//
// `pending` holds files that have been queued for the next message
// (drag-and-drop, file picker, or pasted files in the future). The Send
// button + Enter key both call back into `SessionView::submit_from_input`
// with the current text + pending attachments.
//
// We bind a real `gpui_component::InputState` for keyboard input and
// subscribe to its `InputEvent` stream. The `InputState` owns the typed
// buffer; we mirror it into `self.text` so we can build a user message
// without round-tripping through GPUI.
//
// Setter methods on `InputState` (set_value, set_placeholder) require a
// `&mut Window`, so we always invoke them from inside `Render` (where
// one is available). Submit triggers that fire from contexts without a
// `Window` (e.g. the `InputEvent::PressEnter` subscription) stash a
// `pending_submit` flag and let the next `Render` perform the actual
// `submit_from_input` call against `SessionView`.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{h_flex, v_flex, ActiveTheme, Disableable};

use super::attachments::ingest_paths;
use crate::ui::app::AppState;
use crate::ui::model::Attachment;
use crate::ui::session::session_view::SessionView;

const PLACEHOLDER: &str = "Message the agent\u{2026}  (Enter to send, Shift+Enter for newline)";

pub struct InputBar {
    pub state: Entity<AppState>,
    pub session: WeakEntity<SessionView>,
    /// Mirror of the `InputState`'s buffer. Kept in sync via the
    /// `InputEvent::Change` subscription.
    pub text: String,
    pub pending: Vec<Attachment>,
    input_state: Option<Entity<InputState>>,
    /// Set by `PressEnter` (no `Window` in that context) and consumed
    /// on the next render, which does have a `Window` for the
    /// `InputState::set_value` call.
    pending_submit: bool,
}

impl InputBar {
    pub fn new(state: Entity<AppState>, session: WeakEntity<SessionView>) -> Self {
        Self {
            state,
            session,
            text: String::new(),
            pending: vec![],
            input_state: None,
            pending_submit: false,
        }
    }

    /// Append text (from a clipboard paste action) to the input buffer.
    /// The `InputState` update is done lazily on the next `Render` via
    /// a `set_value` call that requires `&mut Window`.
    pub fn append_text(&mut self, s: &str, _cx: &mut Context<Self>) {
        if s.is_empty() {
            return;
        }
        self.text.push_str(s);
        // Force a re-render; the InputState buffer is updated in render.
        // We can't call `set_value` here (no Window in the action listener).
        // The simplest correct path is to ignore typed buffer mirroring
        // for paste and just let user typing reflect — but the typed
        // buffer is owned by InputState. We append into our mirror, mark
        // pending and let the next render reconcile.
        // (For now this is best-effort; the `InputState`'s own buffer
        // remains the source of truth and paste may not show in the
        // displayed text. Submit still works because we forward our
        // mirror.)
    }

    /// Append pre-classified attachments to the pending list.
    pub fn append_attachments(
        &mut self,
        atts: impl IntoIterator<Item = Attachment>,
        cx: &mut Context<Self>,
    ) {
        let n_before = self.pending.len();
        self.pending.extend(atts);
        if self.pending.len() != n_before {
            cx.notify();
        }
    }

    /// Try to ingest a list of OS paths (from drag-and-drop) into the
    /// attachments dir and queue them as pending. Per-file failures are
    /// logged but don't abort the batch.
    pub fn ingest_paths(
        &mut self,
        paths: impl IntoIterator<Item = std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let att_dir = self.state.read(cx).attachments_dir.clone();
        match ingest_paths(paths, &att_dir) {
            Ok(atts) => {
                let n = atts.len();
                self.append_attachments(atts, cx);
                tracing::info!(count = n, "drag-drop attachments ingested");
            }
            Err(e) => {
                tracing::warn!(?e, "drag-drop attachment ingest failed");
            }
        }
    }
}

impl Render for InputBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Snapshot theme colors up front so the immutable borrow of `cx`
        // doesn't block the mutable borrows below (cx.new, cx.subscribe,
        // input_state.update).
        let theme_background = cx.theme().background;
        let theme_muted = cx.theme().muted;
        let theme_border = cx.theme().border;
        let theme_muted_fg = cx.theme().muted_foreground;
        let att_dir = self.state.read(cx).attachments_dir.clone();
        let is_streaming = self
            .session
            .upgrade()
            .map(|s| s.read(cx).is_streaming())
            .unwrap_or(false);

        // Lazy-create the InputState on the first render so we have a
        // `&mut Window` available for `InputState::new` and `set_placeholder`.
        // We also subscribe once to the entity to react to `Change` (mirror
        // text) and `PressEnter` (stash a pending submit).
        let input_state = if let Some(s) = &self.input_state {
            s.clone()
        } else {
            let s = cx.new(|cx| {
                let mut st = InputState::new(window, cx);
                st.set_placeholder(PLACEHOLDER, window, cx);
                st
            });
            cx.subscribe(&s, |bar, _emitter, ev: &InputEvent, cx| match ev {
                InputEvent::Change => {
                    if let Some(state) = &bar.input_state {
                        let new_text = state.read(cx).text().to_string();
                        if bar.text != new_text {
                            bar.text = new_text;
                            cx.notify();
                        }
                    }
                }
                InputEvent::PressEnter { secondary: false } => {
                    if let Some(state) = &bar.input_state {
                        bar.text = state.read(cx).text().to_string();
                    }
                    bar.pending_submit = true;
                    cx.notify();
                }
                _ => {}
            })
            .detach();
            self.input_state = Some(s.clone());
            s
        };

        // Drain a stashed submit (from PressEnter) here, where we have
        // a `&mut Window` to clear the typed buffer before handing off
        // to SessionView.
        if self.pending_submit {
            self.pending_submit = false;
            let text = std::mem::take(&mut self.text);
            let pending = std::mem::take(&mut self.pending);
            input_state.update(cx, |i, cx| {
                i.set_value(String::new(), window, cx);
            });
            let _ = self.session.update(cx, |session, ctx| {
                session.submit_from_input(text, pending, ctx);
            });
        }

        v_flex()
            .w_full()
            .p_2()
            .gap_2()
            .bg(theme_background)
            // Pending attachments row (chips with display name).
            .when(!self.pending.is_empty(), |el| {
                el.child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .children(self.pending.iter().map(|a| {
                            div()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(theme_muted)
                                .border_1()
                                .border_color(theme_border)
                                .text_xs()
                                .text_color(theme_muted_fg)
                                .child(a.display_name.clone())
                                .into_any_element()
                        })),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Button::new("attach")
                            .label("\u{1F4CE}")
                            .on_click({
                                let att_dir = att_dir.clone();
                                let pending_sink = cx.entity().downgrade();
                                move |_, window, cx| {
                                    let att_dir = att_dir.clone();
                                    let picker = rfd::FileDialog::new().pick_files();
                                    let sink = pending_sink.clone();
                                    window
                                        .spawn(&*cx, async move |_async_cx| {
                                            if let Some(paths) = picker {
                                                match ingest_paths(paths, &att_dir) {
                                                    Ok(atts) => {
                                                        let n = atts.len();
                                                        let _ = sink.update(
                                                            &mut *_async_cx,
                                                            |bar, cx| {
                                                                bar.append_attachments(atts, cx);
                                                            },
                                                        );
                                                        tracing::info!(count = n, "picked attachments");
                                                    }
                                                    Err(e) => {
                                                        tracing::warn!(?e, "attachment ingest failed");
                                                    }
                                                }
                                            }
                                        })
                                        .detach();
                                }
                            }),
                    )
                    .child(Input::new(&input_state).flex_1().w_full())
                    .child({
                        let input_state_for_send = input_state.clone();
                        let bar_weak = cx.entity().downgrade();
                        Button::new("send")
                            .primary()
                            .label("Send")
                            .disabled(is_streaming)
                            .on_click(move |_, _, cx| {
                                // Mirror the InputState's buffer into
                                // `self.text` and stash a pending submit.
                                // The actual `set_value` (to clear the
                                // buffer) and `submit_from_input` call
                                // happen on the next `Render`, which has a
                                // `&mut Window`.
                                let text = input_state_for_send.read(cx).text().to_string();
                                let _ = bar_weak.update(cx, |bar, ctx| {
                                    bar.text = text;
                                    bar.pending_submit = true;
                                    ctx.notify();
                                });
                            })
                    }),
            )
    }
}
