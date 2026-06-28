// ui::session::session_list — left-pane list of sessions.

use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::button::Button;
use gpui_component::label::Label;
use gpui_component::{h_flex, v_flex, StyledExt, Theme};

use crate::core::sqlite_storage::SqliteSessionStore;

pub struct SessionListView {
    pub storage: std::sync::Arc<SqliteSessionStore>,
    pub selected: Option<String>,
}

impl SessionListView {
    pub fn new(storage: std::sync::Arc<SqliteSessionStore>) -> Self {
        let selected = storage
            .list_sessions()
            .ok()
            .and_then(|v| v.first().map(|s| s.id.clone()));
        Self { storage, selected }
    }
}

impl Render for SessionListView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let sessions = self.storage.list_sessions().unwrap_or_default();
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
                    .child(Button::new("new-session").label("+ New").on_click(
                        |_, _, _| {
                            // Session creation is handled by AppState in a later task.
                        },
                    )),
            )
            .children(sessions.iter().map(|s| {
                let id = s.id.clone();
                let selected = self.selected.as_deref() == Some(&id);
                let title = s
                    .title
                    .clone()
                    .unwrap_or_else(|| "(untitled)".to_string());
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
            }))
    }
}
