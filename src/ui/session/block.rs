// ui::session::block — render a single UiBlock. State (collapse flag) lives
// in the parent MessageView and is passed in.
//
// Markdown rendering for `BlockKind::Text` is provided by
// `gpui_component::text::TextView::markdown`, which requires `&mut Window` and
// `&mut App`. Because of that, the renderers in this module are context-aware
// and are called from `SessionView::render` (which already has both).

use gpui::*;
use gpui_component::label::Label;
use gpui_component::text::TextView;
use gpui_component::{h_flex, v_flex, ActiveTheme, StyledExt};

use crate::ui::model::{BlockKind, UiBlock};

pub fn render_block(
    block: &UiBlock,
    _collapsed: bool,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    // Snapshot the theme colors we need up front so the immutable borrow
    // doesn't conflict with the mutable borrow required by
    // `TextView::markdown` below.
    let theme_muted_bg = cx.theme().muted;
    let theme_muted_fg = cx.theme().muted_foreground;
    let theme_border = cx.theme().border;
    let theme_danger = cx.theme().danger;
    match &block.kind {
        BlockKind::Text { text } => {
            // Use a block-scoped ElementId so streaming updates map onto the
            // same TextView state. The state is keyed by `format!("md-{id}")`.
            // `TextView::markdown` reads the theme internally.
            let id: ElementId = SharedString::from(format!("md-{}", block.id)).into();
            TextView::markdown(id, text.clone(), window, cx).into_any_element()
        }
        BlockKind::Thinking { thinking, .. } => v_flex()
            .rounded_md()
            .bg(theme_muted_bg)
            .p_2()
            .gap_1()
            .child(
                Label::new("Thinking")
                    .font_bold()
                    .text_color(theme_muted_fg),
            )
            .child(
                div()
                    .text_color(theme_muted_fg)
                    .child(thinking.clone()),
            )
            .into_any_element(),
        BlockKind::ToolUse { name, input, .. } => v_flex()
            .rounded_md()
            .border_1()
            .border_color(theme_border)
            .p_2()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Label::new(format!("[tool] {name}")).font_bold()),
            )
            .child(
                Label::new(format!("{input}")).text_color(theme_muted_fg),
            )
            .into_any_element(),
        BlockKind::ToolResult {
            content, is_error, ..
        } => v_flex()
            .p_2()
            .child(if *is_error {
                Label::new(format!("[error] {content}"))
                    .text_color(theme_danger)
                    .into_any_element()
            } else {
                Label::new(content.clone()).into_any_element()
            })
            .into_any_element(),
        BlockKind::Attachments { items } => h_flex()
            .gap_2()
            .children(items.iter().map(|a| {
                Label::new(a.display_name.clone()).into_any_element()
            }))
            .into_any_element(),
    }
}
