// ui::session::message — one UiMessage, rendered as a vertical stack of blocks.
//
// Markdown deferral: gpui-component 0.5.1 does not expose
// `gpui_component::markdown::Markdown` as a public type. The only public
// markdown API is `gpui_component::text::TextView::markdown(id, md, window, cx)`,
// which requires `&mut Window` and `&mut App` access. Since `render_message`
// returns `AnyElement` without window/app context, we cannot use it here.
// TODO: once we have a context-aware render path (or gpui-component exposes
// a stateless markdown element), switch text blocks to use the markdown
// renderer. For now `render_text_or_markdown` is a plain-text fallback.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::label::Label;
use gpui_component::{v_flex, StyledExt, Theme};

use crate::ui::model::{Role, UiBlock, UiMessage};
use super::block::render_block;

pub fn render_message(msg: &UiMessage, blocks: &[UiBlock]) -> AnyElement {
    let theme = Theme::default();
    let role = match msg.role {
        Role::User => "You",
        Role::Assistant => "Assistant",
        Role::ToolResult => "Tool",
    };
    v_flex()
        .w_full()
        .p_3()
        .gap_2()
        .border_b_1()
        .border_color(theme.border)
        .child(Label::new(role).font_bold())
        .children(blocks.iter().map(|b| render_block(b, false)))
        .into_any_element()
}

/// Plain-text fallback. Replace with a markdown renderer once the
/// `gpui_component` API allows it without window/app context.
pub fn render_text_or_markdown(text: &str) -> AnyElement {
    div()
        .text_color(Theme::default().foreground)
        .child(text.to_string())
        .into_any_element()
}
