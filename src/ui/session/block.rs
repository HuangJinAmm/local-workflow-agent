// ui::session::block — render a single UiBlock. State (collapse flag) lives
// in the parent MessageView and is passed in.

use gpui::*;
use gpui_component::label::Label;
use gpui_component::{h_flex, v_flex, StyledExt, Theme};

use crate::ui::model::{BlockKind, UiBlock};

pub fn render_block(block: &UiBlock, _collapsed: bool) -> AnyElement {
    let theme = Theme::default();
    match &block.kind {
        BlockKind::Text { text } => v_flex()
            .child(
                div()
                    .text_color(theme.foreground)
                    .child(text.clone())
            )
            .into_any_element(),
        BlockKind::Thinking { thinking, .. } => v_flex()
            .rounded_md()
            .bg(theme.muted)
            .p_2()
            .gap_1()
            .child(
                Label::new("💭 Thinking").font_bold().text_color(theme.muted_foreground)
            )
            .child(
                div()
                    .text_color(theme.muted_foreground)
                    .child(thinking.clone())
            )
            .into_any_element(),
        BlockKind::ToolUse { name, input, .. } => v_flex()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .p_2()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Label::new(format!("🔧 {name}")).font_bold())
            )
            .child(
                Label::new(format!("{input}"))
                    .text_color(theme.muted_foreground)
            )
            .into_any_element(),
        BlockKind::ToolResult { content, is_error, .. } => v_flex()
            .p_2()
            .child(if *is_error {
                Label::new(format!("✗ {content}")).text_color(theme.danger).into_any_element()
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
