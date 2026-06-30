//! Permission modal — an overlay that asks the user to approve a tool call.
//!
//! Owned by `ChatAI` as an `Entity<PermissionModal>` and rendered on top of
//! the chat list whenever a `PermissionRequest` is shown via [`PermissionModal::show`].
//! When the user clicks a decision button, the request's `reply_tx` is
//! consumed and a [`PermissionDecision`] is sent back to the background
//! agent, after which the modal hides itself.
//!
//! The overlay rendering mirrors [`crate::ui::settings_panel::SettingsPanel`]:
//! a full-screen `div().absolute().top_0().left_0().size_full()` tinted with
//! `theme.background.opacity(0.95)` and a centred card.

use gpui::{
    ClickEvent, Context, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement as _, Render, Styled as _, Window, div, px,
};
use gpui_component::{
    ActiveTheme as _,
    button::*,
    h_flex,
    label::Label,
    v_flex,
};

use crate::core::permissions::PermissionDecision;

/// A permission request sent from the background agent to the GUI.
///
/// Defined here (rather than in `services/agent/permission_handler.rs`) so the
/// modal owns the canonical type — the handler imports it from here.
pub struct PermissionRequest {
    /// Unique id for this request (for logging / dedup).
    pub id: String,
    /// The tool that wants to run, e.g. `"Bash"`.
    pub tool_name: String,
    /// The parsed input the tool was called with.
    pub input: serde_json::Value,
    /// The risk level: `"None"`, `"ReadOnly"`, `"Write"`, `"Execute"`, or
    /// `"Dangerous"`.
    pub level: String,
    /// Channel used to deliver the user's decision back to the agent.
    pub reply_tx: tokio::sync::oneshot::Sender<PermissionDecision>,
}

/// Overlay modal for tool-permission prompts.
pub struct PermissionModal {
    request: Option<PermissionRequest>,
    focus_handle: FocusHandle,
}

impl PermissionModal {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            request: None,
            focus_handle: cx.focus_handle(),
        }
    }

    /// Display a new permission request, replacing any pending one. Dropping
    /// the previous sender signals "denied" to the agent (the receive side
    /// treats a dropped channel as a denial).
    pub fn show(&mut self, request: PermissionRequest, cx: &mut Context<Self>) {
        self.request = Some(request);
        cx.notify();
    }

    /// Whether a request is currently on screen.
    pub fn is_visible(&self) -> bool {
        self.request.is_some()
    }

    /// Deliver `decision` over the request's channel and hide the modal.
    fn decide(&mut self, decision: PermissionDecision, cx: &mut Context<Self>) {
        if let Some(req) = self.request.take() {
            let _ = req.reply_tx.send(decision);
        }
        cx.notify();
    }

    fn on_allow(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.decide(PermissionDecision::Allow, cx);
    }

    fn on_deny(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.decide(PermissionDecision::Deny, cx);
    }

    fn on_always_allow(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.decide(PermissionDecision::AllowPermanently, cx);
    }
}

impl Focusable for PermissionModal {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PermissionModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        let Some(request) = self.request.as_ref() else {
            // Nothing to show — render an empty div so the overlay is invisible
            // but the entity stays mounted.
            return div();
        };

        // Clone the data we need out of `self` so the element builders (and
        // the deferred click handlers) never borrow `self` — keeping within
        // GPUI's re-entrancy rules.
        let tool_name = request.tool_name.clone();
        let level = request.level.clone();
        let input_json = serde_json::to_string_pretty(&request.input).unwrap_or_default();
        let is_dangerous = level == "Dangerous";

        let title_text = format!("Tool Permission: {}", tool_name);
        let title = if is_dangerous {
            Label::new(title_text).text_color(theme.danger)
        } else {
            Label::new(title_text)
        };

        let header = h_flex()
            .w_full()
            .py_2()
            .px_4()
            .justify_between()
            .child(title);

        let body = v_flex()
            .p_4()
            .gap_2()
            .child(Label::new(format!("Level: {}", level)))
            .child(
                div()
                    .w_full()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background.opacity(0.6))
                    .child(Label::new(input_json)),
            );

        let mut footer = h_flex()
            .w_full()
            .p_4()
            .justify_end()
            .gap_2()
            .child(
                Button::new("perm-deny")
                    .ghost()
                    .label("Deny")
                    .on_click(cx.listener(Self::on_deny)),
            )
            .child(
                Button::new("perm-allow")
                    .primary()
                    .label("Allow")
                    .on_click(cx.listener(Self::on_allow)),
            );

        // "Always Allow" is hidden for the Dangerous level.
        if !is_dangerous {
            footer = footer.child(
                Button::new("perm-always-allow")
                    .ghost()
                    .label("Always Allow")
                    .on_click(cx.listener(Self::on_always_allow)),
            );
        }

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(theme.background.opacity(0.95))
            .child(
                v_flex()
                    .id("permission-modal-card")
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
                    .child(body)
                    .child(footer),
            )
    }
}
