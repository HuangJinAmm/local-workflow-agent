// ui::session::session_list — left-pane list of sessions.
//
// Renders a "+ New" button and one row per `SessionSummary`. Clicking
// a row asks the parent `SessionView` (via a `WeakEntity`) to load that
// session. Clicking "+ New" asks it to create a new one.
//
// The "on_new" handler is stored as `Arc<dyn Fn>` so we can clone it
// into per-click `on_click` closures (which must be `Fn` + `'static`).

use std::sync::Arc;

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::button::Button;
use gpui_component::label::Label;
use gpui_component::{h_flex, v_flex, StyledExt, Theme};

use crate::core::sqlite_storage::SqliteSessionStore;
use crate::ui::session::session_view::SessionView;

type OnNew = Arc<dyn Fn(&mut App) + Send + Sync>;

pub struct SessionListView {
    pub storage: Arc<SqliteSessionStore>,
    pub selected: Option<String>,
    /// Drives `SessionView::load_session` when a row is clicked.
    session_view: WeakEntity<SessionView>,
    on_new: OnNew,
}

impl SessionListView {
    pub fn new(
        storage: Arc<SqliteSessionStore>,
        session_view: WeakEntity<SessionView>,
    ) -> Self {
        let initial = storage
            .list_sessions()
            .ok()
            .and_then(|v| v.first().map(|s| s.id.clone()));
        Self {
            storage,
            selected: initial,
            session_view,
            on_new: Arc::new(|_app| {}),
        }
    }

    /// Replace the "+ New" handler. Used by AppView to wire the button
    /// to a real session creation flow.
    pub fn set_on_new(
        &mut self,
        handler: impl Fn(&mut App) + Send + Sync + 'static,
    ) {
        self.on_new = Arc::new(handler);
    }
}

impl Render for SessionListView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let sessions = self.storage.list_sessions().unwrap_or_default();
        let on_new = self.on_new.clone();
        let session_view = self.session_view.clone();

        v_flex()
            .size_full()
            .bg(theme.muted)
            .p_2()
            .gap_1()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(Label::new("Sessions").font_bold())
                    .child(
                        Button::new("new-session")
                            .label("+ New")
                            .on_click(move |_, window, cx| {
                                let on_new = on_new.clone();
                                window.defer(cx, move |_w, app: &mut App| {
                                    (on_new)(app);
                                });
                            }),
                    ),
            )
            .children(sessions.iter().map(|s| {
                let id = s.id.clone();
                let selected = self.selected.as_deref() == Some(&id);
                let title = s
                    .title
                    .clone()
                    .unwrap_or_else(|| "(untitled)".to_string());
                let session_view = session_view.clone();
                let id_for_click = id.clone();
                div()
                    .id(ElementId::Name(format!("session-{id}").into()))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .when(selected, |d| d.bg(theme.accent))
                    .hover(|d| d.bg(theme.background))
                    .cursor_pointer()
                    .child(
                        Label::new(title)
                            .when(selected, |l| l.text_color(theme.foreground))
                            .when(!selected, |l| l.text_color(theme.muted_foreground)),
                    )
                    .on_click(move |_, window, cx| {
                        let session_view = session_view.clone();
                        let id = id_for_click.clone();
                        window.defer(cx, move |_w, app: &mut App| {
                            let _ = session_view.update(app, |view, ctx| {
                                view.load_session(id.clone(), ctx);
                            });
                        });
                    })
            }))
    }
}
