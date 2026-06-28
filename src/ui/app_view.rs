// ui::app_view — three-pane root: session list | session view | settings drawer.

use gpui::*;
use gpui_component::*;

use super::app::AppState;

pub struct AppView {
    state: Entity<AppState>,
}

impl AppView {
    pub fn new(state: Entity<AppState>, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { state }
    }
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        div()
            .v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                h_flex()
                    .flex_1()
                    .child(
                        div()
                            .w(px(280.))
                            .h_full()
                            .bg(theme.muted)
                            .child("Sessions (left)"),
                    )
                    .child(div().flex_1().h_full().child("Chat (middle)"))
                    .child(
                        div()
                            .w(px(320.))
                            .h_full()
                            .bg(theme.muted)
                            .child("Settings (right)"),
                    ),
            )
    }
}
