// ui::input::input_bar — bottom-of-pane input.

use gpui::*;
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{h_flex, Theme};

// NOTE: gpui-component 0.5.1 has no `TextInput`. The text-input element is
// `Input`, and it requires an `Entity<InputState>` constructed with
// `&mut Window` — wiring it up cleanly is left for a later task. For now we
// render a plain div that mimics the shape of the bar (input area + Send
// button). TODO(Task 16 follow-up): replace this div with `Input::new(&state)`.

pub struct InputBar {
    pub text: String,
}

impl InputBar {
    pub fn new() -> Self { Self { text: String::new() } }
}

impl Render for InputBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        h_flex()
            .w_full()
            .p_2()
            .gap_2()
            .bg(theme.muted)
            .items_center()
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
