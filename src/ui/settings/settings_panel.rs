// ui::settings::settings_panel — right drawer for theme/provider/key/tool policy.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::label::Label;
use gpui_component::{v_flex, StyledExt, Theme};

pub struct SettingsPanel;

impl Render for SettingsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        v_flex()
            .size_full()
            .p_3()
            .gap_3()
            .bg(theme.muted)
            .child(Label::new("Settings").font_bold())
            .child(Label::new("Theme").text_color(theme.muted_foreground))
            .child(Label::new("Provider").text_color(theme.muted_foreground))
            .child(Label::new("Model").text_color(theme.muted_foreground))
            .child(Label::new("API key").text_color(theme.muted_foreground))
            .child(Label::new("Tool policy").text_color(theme.muted_foreground))
    }
}
