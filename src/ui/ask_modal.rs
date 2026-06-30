//! Ask modal — an overlay that surfaces an `AskUserQuestion` tool prompt.
//!
//! Owned by `ChatAI` as an `Entity<AskModal>` and rendered on top of the chat
//! list whenever a [`UserQuestionEvent`] is shown via [`AskModal::show`].
//!
//! - If the event carries predefined `options`, they are rendered as a
//!   vertical list of buttons — clicking one submits that option immediately.
//! - Otherwise a text [`Input`] plus a Submit button is shown, and the typed
//!   text is sent back over `reply_tx`.
//!
//! The overlay rendering mirrors [`crate::ui::settings_panel::SettingsPanel`].

use gpui::{
    AppContext as _, ClickEvent, Context, Entity, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement as _, Render, Styled as _, Window, div, px,
};
use gpui_component::{
    ActiveTheme as _,
    button::*,
    h_flex,
    input::{Input, InputState},
    label::Label,
    v_flex,
};

use crate::tools::UserQuestionEvent;

/// Overlay modal for `AskUserQuestion` prompts.
pub struct AskModal {
    event: Option<UserQuestionEvent>,
    answer_input: Entity<InputState>,
    focus_handle: FocusHandle,
}

impl AskModal {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let answer_input = cx.new(|cx| InputState::new(window, cx).placeholder("Type your answer..."));
        Self {
            event: None,
            answer_input,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Display a new question, clearing any previous answer text first.
    pub fn show(
        &mut self,
        event: UserQuestionEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.event = Some(event);
        self.answer_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        cx.notify();
    }

    /// Whether a question is currently on screen.
    pub fn is_visible(&self) -> bool {
        self.event.is_some()
    }

    /// Deliver `answer` over the event's channel and hide the modal.
    fn submit_answer(&mut self, answer: String, cx: &mut Context<Self>) {
        if let Some(ev) = self.event.take() {
            let _ = ev.reply_tx.send(answer);
        }
        cx.notify();
    }

    /// Submit button handler for the free-text case — reads the input field.
    fn on_submit(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let answer = self.answer_input.read(cx).text().to_string();
        self.submit_answer(answer, cx);
    }
}

impl Focusable for AskModal {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for AskModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        let Some(event) = self.event.as_ref() else {
            // Nothing to show — render an empty div so the overlay is invisible
            // but the entity stays mounted.
            return div();
        };

        // Clone the data we need out of `self` so the element builders (and
        // the deferred click handlers) never borrow `self` — keeping within
        // GPUI's re-entrancy rules.
        let question = event.question.clone();
        let options = event.options.clone();
        let has_options = options
            .as_ref()
            .map_or(false, |opts| !opts.is_empty());

        let header = h_flex()
            .w_full()
            .py_2()
            .px_4()
            .justify_between()
            .child(Label::new(question));

        // Body depends on whether the question is multiple-choice or free text.
        let body = if has_options {
            // Render each option as a full-width button; clicking submits it.
            let opts = options.unwrap();
            let mut col = v_flex().p_4().gap_2();
            for (i, opt) in opts.into_iter().enumerate() {
                let opt_for_closure = opt.clone();
                col = col.child(
                    Button::new(("ask-option", i))
                        .ghost()
                        .w_full()
                        .label(opt)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.submit_answer(opt_for_closure.clone(), cx);
                        })),
                );
            }
            col
        } else {
            // Free-text input + Submit button.
            v_flex()
                .p_4()
                .gap_2()
                .child(Input::new(&self.answer_input).appearance(false))
                .child(
                    Button::new("ask-submit")
                        .primary()
                        .label("Submit")
                        .on_click(cx.listener(Self::on_submit)),
                )
        };

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(theme.background.opacity(0.95))
            .child(
                v_flex()
                    .id("ask-modal-card")
                    .track_focus(&self.focus_handle)
                    .mx_auto()
                    .my_8()
                    .w(px(420.))
                    .max_h(px(520.))
                    .rounded_lg()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.popover)
                    .shadow_lg()
                    .child(header)
                    .child(body),
            )
    }
}
