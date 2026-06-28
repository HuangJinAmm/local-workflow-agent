// ui::app_view — three-pane root: session list | session view | settings drawer.

use gpui::*;
use gpui_component::*;

use super::app::{apply_theme, AppState};
use super::session::session_list::SessionListView;
use super::session::session_view::SessionView;
use super::settings::settings_panel::SettingsPanel;

pub struct AppView {
    state: Entity<AppState>,
}

impl AppView {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Apply the persisted theme at view construction so the user
        // sees their saved light/dark/system choice immediately. We
        // need a `&mut Window` (for `window.appearance()` in `System`
        // mode) and we already have one in this constructor.
        let initial_mode = state.read(cx).settings.read().theme;
        if let Err(e) = state.read(cx).set_theme_persist(initial_mode) {
            tracing::warn!(?e, "AppView boot set_theme_persist failed");
        }
        apply_theme(initial_mode, Some(window), cx);
        Self { state }
    }
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let storage = self.state.read(cx).storage.clone();
        let session_list = cx.new(|_| SessionListView::new(storage));
        let session_view = cx.new({
            let state = self.state.clone();
            move |cx| SessionView::new(state, cx)
        });
        let settings = cx.new({
            let state = self.state.clone();
            move |cx| SettingsPanel::new(state, window, cx)
        });
        let theme = cx.global::<Theme>();
        div()
            .v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                h_flex()
                    .flex_1()
                    .child(session_list)
                    .child(session_view)
                    .child(div().w(px(320.)).h_full().child(settings)),
            )
    }
}
