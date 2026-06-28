// ui::session::session_view — middle-pane chat area.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::label::Label;
use gpui_component::{v_flex, Theme};

use crate::ui::app::AppState;
use crate::ui::input::input_bar::InputBar;

pub struct SessionView {
    pub state: Entity<AppState>,
}

impl SessionView {
    pub fn new(state: Entity<AppState>) -> Self { Self { state } }
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
