// ui::app_view — three-pane root: session list | session view | settings drawer.
//
// The child views are owned as fields and created once in `new`. We
// cannot recreate them in `Render` because that would discard their
// state on every redraw (selected session, in-memory messages, settings
// draft, etc.).

use gpui::*;
use gpui_component::*;

use super::app::{apply_theme, AppState};
use super::session::session_list::SessionListView;
use super::session::session_view::SessionView;
use super::settings::settings_panel::SettingsPanel;

pub struct AppView {
    state: Entity<AppState>,
    session_list: Entity<SessionListView>,
    session_view: Entity<SessionView>,
    settings: Entity<SettingsPanel>,
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

        // SessionView is created first; SessionListView uses a
        // WeakEntity to it to drive session loading on click.
        let session_view = cx.new({
            let state = state.clone();
            move |cx| SessionView::new(state, cx)
        });
        let sv_weak = session_view.downgrade();
        let storage = state.read(cx).storage.clone();
        let session_list = cx.new(move |_| SessionListView::new(storage, sv_weak));

        // Wire "+ New": call SessionView::new_session (which persists
        // the row) and then mark the new id as selected in the list.
        let sv_for_new = session_view.downgrade();
        let sl_for_new = session_list.downgrade();
        session_list.update(cx, move |list, _ctx| {
            list.set_on_new(move |app| {
                let new_id: Option<String> = sv_for_new
                    .update(app, |view, ctx| Some(view.new_session(ctx)))
                    .ok()
                    .flatten();
                if let Some(new_id) = new_id {
                    let _ = sl_for_new.update(app, |l, ctx| {
                        l.selected = Some(new_id);
                        ctx.notify();
                    });
                }
            });
        });

        let settings = cx.new({
            let state = state.clone();
            move |cx| SettingsPanel::new(state, window, cx)
        });

        Self {
            state,
            session_list,
            session_view,
            settings,
        }
    }
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        div()
            .v_flex()
            .size_full()
            .bg(theme.background)
            .child(
                h_flex()
                    .flex_1()
                    .child(self.session_list.clone())
                    .child(self.session_view.clone())
                    .child(div().w(px(320.)).h_full().child(self.settings.clone())),
            )
    }
}
