// ui::app_view — three-pane root: session list | session view | settings drawer.

use gpui::*;
use gpui_component::*;

use super::app::AppState;
use super::session::session_list::SessionListView;
use super::session::session_view::SessionView;
use super::settings::settings_panel::SettingsPanel;

pub struct AppView {
    state: Entity<AppState>,
}

impl AppView {
    pub fn new(state: Entity<AppState>, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self { state }
    }
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let storage = self.state.read(cx).storage.clone();
        let session_list = cx.new(|_| SessionListView::new(storage));
        let session_view = cx.new({
            let state = self.state.clone();
            move |_| SessionView::new(state)
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
