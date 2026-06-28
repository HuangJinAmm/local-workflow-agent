// ui::session::session_view — middle-pane chat area.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::label::Label;
use gpui_component::{v_flex, Theme};

pub struct SessionView;

impl Render for SessionView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        v_flex()
            .size_full()
            .bg(theme.background)
            .items_center()
            .justify_center()
            .child(Label::new("No session selected").text_color(theme.muted_foreground))
    }
}
