//! Settings panel — a modal-style overlay for editing the API key, base URL,
//! and model name.
//!
//! Owned by `ChatAI` as an `Entity<SettingsPanel>` and rendered on top of the
//! chat list when `show_settings == true`. The panel holds three `InputState`
//! fields; on Save it calls the `on_save` callback (set by `ChatAI`) with the
//! new values, persists them via `crate::ui::settings::Settings`, and the
//! caller is responsible for dispatching `AgentRequest::SetApiConfig` +
//! `SetModel` to the background agent.

use gpui::{
    AppContext as _, ClickEvent, Context, Entity, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement as _, Render, SharedString, Subscription, Styled as _, Window,
    div, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, IndexPath, Sizable as _,
    button::*,
    h_flex,
    input::{Input, InputState},
    label::Label,
    select::{Select, SelectEvent, SelectItem, SelectState},
    v_flex,
};
use std::sync::Arc;

use crate::ui::settings::Settings;
use crate::api::model_registry::ModelRegistry;

/// A single option in the provider dropdown.
///
/// `title` (the human-readable label) is what the user sees in the menu;
/// `value` (the provider id) is what gets persisted and sent to the agent.
#[derive(Clone, PartialEq)]
struct ProviderItem {
    id: SharedString,
    label: SharedString,
}

impl SelectItem for ProviderItem {
    type Value = SharedString;
    fn title(&self) -> SharedString {
        self.label.clone()
    }
    fn value(&self) -> &Self::Value {
        &self.id
    }
}

/// A single option in the model dropdown.
///
/// `value` is the model id (e.g. `"claude-sonnet-4-5-20250929"`);
/// `title` is the human-readable name (e.g. `"Claude Sonnet 4.5"`).
#[derive(Clone, PartialEq)]
struct ModelItem {
    id: SharedString,
    label: SharedString,
}

impl SelectItem for ModelItem {
    type Value = SharedString;
    fn title(&self) -> SharedString {
        self.label.clone()
    }
    fn value(&self) -> &Self::Value {
        &self.id
    }
}

/// Callback invoked when the user clicks "Save".
/// Receives the freshly-collected `Settings`.
pub type OnSave = Arc<dyn Fn(&Settings, &mut Window, &mut gpui::App) + 'static>;

/// Callback invoked when the user clicks "Cancel" / closes the panel.
pub type OnCancel = Arc<dyn Fn(&mut Window, &mut gpui::App) + 'static>;

/// Modal-style settings panel.
pub struct SettingsPanel {
    /// Pre-filled with the current settings on construction.
    provider_select: Entity<SelectState<Vec<ProviderItem>>>,
    api_key_input: Entity<InputState>,
    base_url_input: Entity<InputState>,
    /// Model dropdown — refreshes when provider changes.
    model_select: Entity<SelectState<Vec<ModelItem>>>,
    /// Hidden input used as fallback when provider has no models in registry
    /// (e.g. ollama / lm-studio). Kept in sync with model_select selection.
    model_input: Entity<InputState>,
    working_dir_input: Entity<InputState>,
    focus_handle: FocusHandle,
    on_save: Option<OnSave>,
    on_cancel: Option<OnCancel>,
    /// Keeps the provider-change subscription alive.
    _provider_sub: Subscription,
    /// When set, the model dropdown needs refreshing on the next `render`
    /// (because `set_items`/`set_selected_index` require `&mut Window`).
    pending_provider_change: Option<String>,
}

impl SettingsPanel {
    /// Construct a new settings panel pre-filled with `settings`.
    pub fn new(settings: &Settings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Build provider dropdown from the library's bundled model registry.
        let registry = ModelRegistry::new();
        let providers = registry.list_providers();
        let items: Vec<ProviderItem> = providers
            .iter()
            .map(|p| ProviderItem {
                id: p.id.to_string().into(),
                label: p.name.clone().into(),
            })
            .collect();
        let selected_index = providers
            .iter()
            .position(|p| &*p.id == settings.provider.as_str())
            .map(|i| IndexPath::default().row(i));
        let provider_select =
            cx.new(|cx| SelectState::new(items, selected_index, window, cx));

        let api_key_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("sk-ant-...");
            s.set_value(settings.api_key.clone(), window, cx);
            s
        });
        let base_url_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx)
                .placeholder("https://api.anthropic.com (default)");
            s.set_value(settings.base_url.clone(), window, cx);
            s
        });
        let model_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx)
                .placeholder("claude-haiku-4-5-20251001");
            s.set_value(settings.model.clone(), window, cx);
            s
        });
        let working_dir_input = cx.new(|cx| {
            let mut s = InputState::new(window, cx)
                .placeholder("/home/user/project");
            s.set_value(settings.working_dir.display().to_string(), window, cx);
            s
        });

        // Build model dropdown from the current provider's model list.
        let (model_items, model_selected) =
            build_model_items(&registry, &settings.provider, &settings.model);
        let model_select =
            cx.new(|cx| SelectState::new(model_items, model_selected, window, cx));

        let focus_handle = cx.focus_handle();

        // Subscribe to provider selection changes → stash a pending flag so
        // the model dropdown can be refreshed in `render` (where we have
        // `&mut Window`). `set_items`/`set_selected_index` both require a
        // Window, which the subscribe callback lacks.
        let provider_sub = cx.subscribe(
            &provider_select,
            move |this, _state, event: &SelectEvent<Vec<ProviderItem>>, cx| {
                if let SelectEvent::Confirm(Some(provider_id)) = event {
                    this.pending_provider_change = Some(provider_id.to_string());
                    cx.notify();
                }
            },
        );

        Self {
            provider_select,
            api_key_input,
            base_url_input,
            model_select,
            model_input,
            working_dir_input,
            focus_handle,
            on_save: None,
            on_cancel: None,
            _provider_sub: provider_sub,
            pending_provider_change: None,
        }
    }

    /// Set the Save callback. Mutates `self` from outside via the entity
    /// (`cx.update_entity(&panel, |p, cx| p.on_save(...))`).
    pub fn set_on_save<F>(&mut self, f: F)
    where
        F: Fn(&Settings, &mut Window, &mut gpui::App) + 'static,
    {
        self.on_save = Some(Arc::new(f));
    }

    /// Set the Cancel callback.
    pub fn set_on_cancel<F>(&mut self, f: F)
    where
        F: Fn(&mut Window, &mut gpui::App) + 'static,
    {
        self.on_cancel = Some(Arc::new(f));
    }

    /// Read all input fields into a `Settings` struct (used by `ChatAI`
    /// when handling Save).
    pub fn collect(&self, cx: &gpui::App) -> Settings {
        let provider = self
            .provider_select
            .read(cx)
            .selected_value()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "anthropic".to_string());
        // Prefer model_select's selected value; fall back to hidden input
        // for providers with no models in registry.
        let model = self
            .model_select
            .read(cx)
            .selected_value()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.model_input.read(cx).text().to_string());
        Settings {
            provider,
            api_key: self.api_key_input.read(cx).text().to_string(),
            base_url: self.base_url_input.read(cx).text().to_string(),
            model,
            working_dir: std::path::PathBuf::from(
                self.working_dir_input.read(cx).text().to_string(),
            ),
        }
    }

    fn on_save(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(cb) = self.on_save.clone() {
            let s = self.collect(cx);
            cb(&s, window, cx);
        }
    }

    fn on_cancel(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(cb) = self.on_cancel.clone() {
            cb(window, cx);
        }
    }
}

/// Build the model dropdown items for a given provider, and determine which
/// item should be selected.
///
/// - If `current_model` matches one of the registry entries, that entry is
///   selected.
/// - Otherwise, the provider's best model (if any) is selected.
/// - If the registry has no models for this provider, returns an empty list
///   and `None` — the hidden `model_input` acts as fallback.
fn build_model_items(
    registry: &ModelRegistry,
    provider_id: &str,
    current_model: &str,
) -> (Vec<ModelItem>, Option<IndexPath>) {
    let models = registry.list_visible_by_provider(provider_id);
    if models.is_empty() {
        return (vec![], None);
    }
    let items: Vec<ModelItem> = models
        .iter()
        .map(|m| ModelItem {
            id: m.info.id.to_string().into(),
            label: m.info.name.clone().into(),
        })
        .collect();
    // Prefer exact match on current_model; else fall back to best model.
    let selected_idx = models
        .iter()
        .position(|m| &*m.info.id == current_model)
        .or_else(|| {
            registry
                .best_model_for_provider(provider_id)
                .and_then(|best| models.iter().position(|m| &*m.info.id == &*best))
        });
    let selected = selected_idx.map(|i| IndexPath::default().row(i));
    (items, selected)
}

impl Focusable for SettingsPanel {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Drain pending provider change: refresh model dropdown now that
        // we have `&mut Window` (subscribe callback didn't).
        if let Some(provider_id) = self.pending_provider_change.take() {
            let registry = ModelRegistry::new();
            let current_model = self
                .model_select
                .read(cx)
                .selected_value()
                .map(|s| s.to_string())
                .unwrap_or_default();
            let (items, selected) =
                build_model_items(&registry, &provider_id, &current_model);
            self.model_select.update(cx, |state, cx| {
                state.set_items(items, window, cx);
                if let Some(idx) = selected {
                    state.set_selected_index(Some(idx), window, cx);
                }
            });
        }

        let theme = cx.theme();

        // Header — plain div (NOT TitleBar) so that the window's minimize /
        // maximize / close traffic-light buttons are not duplicated here.
        // The settings panel is just an overlay, not a real window, so the
        // only button it needs is the close-X which dismisses the panel.
        let header = h_flex()
            .w_full()
            .py_2()
            .px_4()
            .justify_between()
            .child(Label::new("Settings"))
            .child(
                Button::new("settings-close")
                    .icon(Icon::empty().path("icons/x.svg"))
                    .small()
                    .ghost()
                    .on_click(cx.listener(Self::on_cancel)),
            );

        // Save / Cancel footer
        let footer = h_flex()
            .w_full()
            .p_4()
            .justify_end()
            .gap_2()
            .child(
                Button::new("settings-cancel")
                    .ghost()
                    .label("Cancel")
                    .on_click(cx.listener(Self::on_cancel)),
            )
            .child(
                Button::new("settings-save")
                    .primary()
                    .label("Save")
                    .on_click(cx.listener(Self::on_save)),
            );

        // Body — provider dropdown + three labelled inputs.
        let body = v_flex()
            .p_4()
            .gap_4()
            .child(
                v_flex()
                    .gap_1()
                    .child(Label::new("Provider"))
                    .child(
                        Select::new(&self.provider_select)
                            .appearance(false)
                            .placeholder("Select a provider"),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(Label::new("API Key"))
                    .child(Input::new(&self.api_key_input).appearance(false)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(Label::new("Base URL"))
                    .child(Input::new(&self.base_url_input).appearance(false)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(Label::new("Model"))
                    .child(
                        Select::new(&self.model_select)
                            .appearance(false)
                            .placeholder("Select a model"),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(Label::new("Working Directory"))
                    .child(Input::new(&self.working_dir_input).appearance(false)),
            );

        // Card overlay — covers the whole window.
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(theme.background.opacity(0.95))
            .child(
                v_flex()
                    .id("settings-panel-card")
                    .track_focus(&self.focus_handle)
                    .mx_auto()
                    .my_8()
                    .w(px(360.))
                    .max_h(px(440.))
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
