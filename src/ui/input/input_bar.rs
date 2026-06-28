// ui::input::input_bar — bottom-of-pane input.
//
// `pending` holds files that have been queued for the next message
// (drag-and-drop, file picker, or pasted files in the future). The Send
// button submits text + pending to the LLM; for now the input is in
// placeholder mode and Send is just a label.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{h_flex, v_flex, Theme};

use super::attachments::ingest_paths;
use crate::ui::app::AppState;
use crate::ui::model::Attachment;

pub struct InputBar {
    pub text: String,
    pub state: Entity<AppState>,
    pub pending: Vec<Attachment>,
}

impl InputBar {
    pub fn new(state: Entity<AppState>) -> Self {
        Self { text: String::new(), state, pending: vec![] }
    }

    /// Append text (from the keyboard or a clipboard paste) to the input
    /// buffer. Notifies so the placeholder swaps to the actual text.
    pub fn append_text(&mut self, s: &str, cx: &mut Context<Self>) {
        if s.is_empty() {
            return;
        }
        self.text.push_str(s);
        cx.notify();
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let att_dir = self.state.read(cx).attachments_dir.clone();
        v_flex()
            .w_full()
            .p_2()
            .gap_2()
            .bg(theme.muted)
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
                                .bg(theme.background)
                                .border_1()
                                .border_color(theme.border)
                                .text_xs()
                                .text_color(theme.muted_foreground)
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
                            .label("📎")
                            .on_click(move |_, window, cx| {
                                let att_dir = att_dir.clone();
                                let picker = rfd::FileDialog::new().pick_files();
                                window
                                    .spawn(&*cx, async move |_async_cx| {
                                        if let Some(paths) = picker {
                                            match ingest_paths(paths, &att_dir) {
                                                Ok(atts) => {
                                                    tracing::info!(count = atts.len(), "picked attachments");
                                                }
                                                Err(e) => {
                                                    tracing::warn!(?e, "attachment ingest failed");
                                                }
                                            }
                                        }
                                    })
                                    .detach();
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(theme.background)
                            .border_1()
                            .border_color(theme.border)
                            .text_color(if self.text.is_empty() {
                                theme.muted_foreground
                            } else {
                                theme.foreground
                            })
                            .child(if self.text.is_empty() {
                                "Message the agent…  (Enter to send, Shift+Enter for newline)".to_string()
                            } else {
                                self.text.clone()
                            }),
                    )
                    .child(Button::new("send").primary().label("Send")),
            )
    }
}
