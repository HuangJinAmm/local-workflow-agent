use std::path::PathBuf;

use gpui::*;
use gpui_component::*;
use local_workflow_agent::ui::app::AppState;
use local_workflow_agent::ui::app_view::AppView;
use local_workflow_agent::ui::session::session_view::Paste;

use gpui::actions;
actions!(agent_gui, [NewSession, ToggleTheme, OpenSettings, CancelTurn]);

fn main() -> anyhow::Result<()> {
    let app = Application::new();
    app.run(move |cx| {
        gpui_component::init(cx);
        local_workflow_agent::ui::theme::register(cx);
        cx.bind_keys([
            KeyBinding::new("cmd-n", NewSession, None),
            KeyBinding::new("ctrl-n", NewSession, None),
            KeyBinding::new("cmd-t", ToggleTheme, None),
            KeyBinding::new("ctrl-t", ToggleTheme, None),
            KeyBinding::new("cmd-,", OpenSettings, None),
            KeyBinding::new("ctrl-,", OpenSettings, None),
            KeyBinding::new("escape", CancelTurn, None),
            KeyBinding::new("cmd-v", Paste, None),
            KeyBinding::new("ctrl-v", Paste, None),
        ]);
        let working_dir = std::env::current_dir().expect("resolve cwd");
        // For the GUI binary, use a project-local data dir by default so the
        // app works in sandboxed environments. The CLI / library path keeps
        // the user-home default; the LWA_DATA_DIR env var overrides both.
        let data_dir = std::env::var("LWA_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| working_dir.join(".lwa-data"));
        let state = cx.new(move |_cx| {
            AppState::with_data_dir(working_dir.clone(), data_dir.clone())
                .expect("AppState::with_data_dir")
        });
        cx.spawn(async move |cx| {
            cx.open_window(WindowOptions::default(), |window, cx| {
                let view = cx.new(|cx| AppView::new(state, window, cx));
                // Drag-and-drop + clipboard paste are wired in
                // `SessionView::Render` (on the v_flex that wraps the messages
                // + input bar). The Paste action is dispatched to the focused
                // view; the input bar reads the clipboard via App and appends
                // text to its buffer.
                cx.new(|cx| Root::new(view, window, cx))
            })?;
            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });
    Ok(())
}
