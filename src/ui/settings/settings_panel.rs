// ui::settings::settings_panel — right drawer for theme/provider/key/tool policy.
// Provides read-only display of current settings plus clickable controls
// (provider switch, model cycle, API-key editor, theme switch) wired to
// `AppState::update_api_key` / `set_default_model` / theme setter.

use std::collections::HashMap;
use std::path::Path;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::label::Label;
use gpui_component::{h_flex, v_flex, ActiveTheme, StyledExt, Theme};

use super::persistence;
use super::Settings;
use super::ThemeMode;
use crate::ui::app::AppState;

const ANTHROPIC_MODELS: &[&str] = &[
    "claude-sonnet-4-5",
    "claude-opus-4-5",
    "claude-haiku-4-5",
];

const OPENAI_MODELS: &[&str] = &[
    "gpt-4o",
    "gpt-4-turbo",
    "gpt-4o-mini",
    "o1-preview",
    "o1-mini",
];

pub struct SettingsPanel {
    pub state: Entity<AppState>,
    /// One `InputState` per provider so we don't lose buffered text on switch.
    pub key_inputs: HashMap<String, Entity<InputState>>,
}

impl SettingsPanel {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let key_inputs = HashMap::new();
        let mut panel = Self { state, key_inputs };
        // Pre-create input states for both providers so a click-to-edit is instant.
        panel.ensure_key_input("anthropic", window, cx);
        panel.ensure_key_input("openai", window, cx);
        panel
    }

    fn ensure_key_input(
        &mut self,
        provider: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(existing) = self.key_inputs.get(provider) {
            return existing.clone();
        }
        let initial = self
            .state
            .read(cx)
            .settings
            .read()
            .key_for(provider)
            .unwrap_or_default();
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("paste key here…"));
        input.update(cx, |s, cx| {
            s.set_value(initial.clone(), window, cx);
        });
        self.key_inputs
            .insert(provider.to_string(), input.clone());
        input
    }

    fn set_provider(&mut self, provider: &str, cx: &mut Context<Self>) {
        let model = match provider {
            "openai" => OPENAI_MODELS[0].to_string(),
            _ => ANTHROPIC_MODELS[0].to_string(),
        };
        if let Err(e) = self.state.read(cx).set_default_model(provider, &model) {
            tracing::warn!(?e, "set_default_model failed");
        }
        cx.notify();
    }

    fn cycle_model(&mut self, dir: i32, cx: &mut Context<Self>) {
        let snapshot = self.state.read(cx).settings.read().clone();
        let presets: &[&str] = match snapshot.default_provider.as_str() {
            "openai" => OPENAI_MODELS,
            _ => ANTHROPIC_MODELS,
        };
        let current_idx = presets
            .iter()
            .position(|m| *m == snapshot.default_model)
            .unwrap_or(0);
        let next = ((current_idx as i32 + dir).rem_euclid(presets.len() as i32)) as usize;
        let new_model = presets[next].to_string();
        if let Err(e) = self
            .state
            .read(cx)
            .set_default_model(&snapshot.default_provider, &new_model)
        {
            tracing::warn!(?e, "set_default_model failed");
        }
        cx.notify();
    }

    fn save_key(&mut self, provider: &str, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.ensure_key_input(provider, window, cx);
        let value = input.read(cx).value().to_string();
        let trimmed = value.trim().to_string();
        let to_save = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        };
        if let Err(e) = self.state.read(cx).update_api_key(provider, to_save) {
            tracing::warn!(?e, "update_api_key failed");
        }
        cx.notify();
    }

    fn set_theme(&mut self, mode: ThemeMode, window: &mut Window, cx: &mut App) {
        // Split into two short-lived scopes so we can re-borrow `cx`
        // mutably for the live `Theme::change` after the settings lock
        // is released.
        if let Err(e) = self.state.read(cx).set_theme_persist(mode) {
            tracing::warn!(?e, "set_theme_persist failed");
        }
        crate::ui::app::apply_theme(mode, Some(window), cx);
    }
}

impl Settings {
    pub(crate) fn key_for(&self, provider: &str) -> Option<String> {
        match provider {
            "anthropic" => self.anthropic_api_key.clone(),
            "openai" => self.openai_api_key.clone(),
            _ => None,
        }
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.state.read(cx).settings.read().clone();
        let current_provider = snapshot.default_provider.clone();
        let current_model = snapshot.default_model.clone();
        let current_theme = snapshot.theme;
        let anthropic_key_set = snapshot
            .anthropic_api_key
            .as_deref()
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        let openai_key_set = snapshot
            .openai_api_key
            .as_deref()
            .map(|s| !s.is_empty())
            .unwrap_or(false);

        v_flex()
            .size_full()
            .p_3()
            .gap_3()
            .bg(cx.global::<Theme>().muted)
            .overflow_y_hidden()
            .child(Label::new("Settings").font_bold().text_color(cx.global::<Theme>().foreground))
            .child(provider_section(current_provider.clone(), cx))
            .child(model_section(current_model.clone(), cx))
            .child(Label::new("API keys").font_bold().text_color(cx.global::<Theme>().foreground))
            .child(api_key_section("anthropic", "Anthropic key", anthropic_key_set, cx))
            .child(api_key_section("openai", "OpenAI key", openai_key_set, cx))
            .child(Label::new("Theme").font_bold().text_color(cx.global::<Theme>().foreground))
            .child(theme_section(current_theme, cx))
            .child(Label::new("Tool policy").font_bold().text_color(cx.global::<Theme>().foreground))
            .child(Label::new(format!(
                "Confirm before: {}",
                join_sorted(&snapshot.tool_policy.require_confirmation)
            )))
            .child(Label::new(format!(
                "Disabled: {}",
                if snapshot.tool_policy.disabled.is_empty() {
                    "(none)".to_string()
                } else {
                    join_sorted(&snapshot.tool_policy.disabled)
                }
            )))
    }
}

fn join_sorted(set: &std::collections::HashSet<String>) -> String {
    let mut v: Vec<&str> = set.iter().map(String::as_str).collect();
    v.sort_unstable();
    v.join(", ")
}

fn provider_section(current: String, cx: &mut Context<SettingsPanel>) -> impl IntoElement {
    let theme = cx.global::<Theme>();
    v_flex()
        .gap_1()
        .child(Label::new("Provider").text_color(theme.muted_foreground))
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("prov-anthropic")
                        .label("Anthropic")
                        .when(current == "anthropic", |b| b.primary())
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.set_provider("anthropic", cx);
                        })),
                )
                .child(
                    Button::new("prov-openai")
                        .label("OpenAI")
                        .when(current == "openai", |b| b.primary())
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.set_provider("openai", cx);
                        })),
                ),
        )
}

fn model_section(current: String, cx: &mut Context<SettingsPanel>) -> impl IntoElement {
    let theme = cx.global::<Theme>();
    v_flex()
        .gap_1()
        .child(Label::new("Model").text_color(theme.muted_foreground))
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    Button::new("model-prev")
                        .label("◀")
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.cycle_model(-1, cx);
                        })),
                )
                .child(
                    div()
                        .flex_1()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(theme.background)
                        .border_1()
                        .border_color(theme.border)
                        .text_color(theme.foreground)
                        .child(current),
                )
                .child(
                    Button::new("model-next")
                        .label("▶")
                        .on_click(cx.listener(|this, _, _window, cx| {
                            this.cycle_model(1, cx);
                        })),
                ),
        )
}

fn api_key_section(
    provider: &'static str,
    label: &'static str,
    key_set: bool,
    cx: &mut Context<SettingsPanel>,
) -> impl IntoElement {
    let theme = cx.global::<Theme>();
    let input = {
        let entity = cx.entity();
        let panel = entity.read(cx);
        panel.key_inputs.get(provider).cloned()
    };
    let Some(input) = input else {
        return v_flex()
            .gap_1()
            .child(Label::new(label).text_color(theme.muted_foreground));
    };
    let save_label = if key_set { "Update" } else { "Save" };
    let status_color: Hsla = if key_set {
        hsla(0.34, 0.65, 0.45, 1.0) // green-ish
    } else {
        theme.muted_foreground
    };
    v_flex()
        .gap_1()
        .child(Label::new(label).text_color(theme.muted_foreground))
        .child(Input::new(&input).w_full().mask_toggle())
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new(SharedString::from(format!("save-{provider}")))
                        .label(save_label)
                        .primary()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.save_key(provider, window, cx);
                        })),
                )
                .child(Label::new(if key_set { "✓ loaded" } else { "✗ not set" }).text_color(status_color)),
        )
}

fn theme_section(
    current: ThemeMode,
    cx: &mut Context<SettingsPanel>,
) -> impl IntoElement {
    h_flex()
        .gap_2()
        .child(
            Button::new("theme-light")
                .label("Light")
                .when(current == ThemeMode::Light, |b| b.primary())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.set_theme(ThemeMode::Light, window, cx);
                    cx.notify();
                })),
        )
        .child(
            Button::new("theme-dark")
                .label("Dark")
                .when(current == ThemeMode::Dark, |b| b.primary())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.set_theme(ThemeMode::Dark, window, cx);
                    cx.notify();
                })),
        )
        .child(
            Button::new("theme-system")
                .label("System")
                .when(current == ThemeMode::System, |b| b.primary())
                .on_click(cx.listener(|this, _, window, cx| {
                    this.set_theme(ThemeMode::System, window, cx);
                    cx.notify();
                })),
        )
}

// `hsla` is not re-exported; provide a tiny wrapper to keep this file
// self-contained. (We use it for the green "loaded" indicator.)
fn hsla(h: f32, s: f32, l: f32, a: f32) -> Hsla {
    Hsla { h, s, l, a }
}
