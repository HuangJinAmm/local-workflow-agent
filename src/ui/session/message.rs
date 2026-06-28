// ui::session::message — one UiMessage, rendered as a vertical stack of blocks.
//
// Markdown rendering for `BlockKind::Text` is wired through
// `gpui_component::text::TextView::markdown`. The renderers in this module
// take `&mut Window` and `&mut App` so they can construct the `TextView` with
// a unique ElementId per block. They are called from `SessionView::render`,
// which already holds both contexts.

use gpui::*;
use gpui_component::label::Label;
use gpui_component::{v_flex, ActiveTheme, StyledExt};

use crate::ui::model::{Role, UiBlock, UiMessage};
use super::block::render_block;

pub fn render_message(
    msg: &UiMessage,
    blocks: &[UiBlock],
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    // Snapshot theme colors up front so we can release the immutable borrow
    // before passing `cx` mutably into `render_block` (which needs it for
    // `TextView::markdown`).
    let theme_border = cx.theme().border;
    v_flex()
        .w_full()
        .p_3()
        .gap_2()
        .border_b_1()
        .border_color(theme_border)
        .child(Label::new(match msg.role {
            Role::User => "You",
            Role::Assistant => "Assistant",
            Role::ToolResult => "Tool",
        }).font_bold())
        .children(
            blocks
                .iter()
                .map(|b| render_block(b, false, window, cx)),
        )
        .into_any_element()
}
