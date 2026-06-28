// ui::input::input_bar — bottom-of-pane input.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{h_flex, Theme};

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
}

impl Render for InputBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let att_dir = self.state.read(cx).attachments_dir.clone();
        h_flex()
            .w_full()
            .p_2()
            .gap_2()
            .bg(theme.muted)
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
                    .text_color(theme.muted_foreground)
                    .child("Message the agent…  (Enter to send, Shift+Enter for newline)"),
            )
            .child(Button::new("send").primary().label("Send"))
    }
}
