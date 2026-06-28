use std::path::PathBuf;

use gpui::*;
use gpui_component::*;
use local_workflow_agent::ui::app::AppState;
use local_workflow_agent::ui::app_view::AppView;

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

                // TODO(Task 19): wire drag-and-drop file ingestion + clipboard paste.
                //
                // gpui 0.2.2 does not expose a `WindowEvent` enum; the
                // platform-level `FileDropEvent` (a `MouseEvent`) is delivered
                // through the paint-phase mouse listener pipeline, and
                // clipboard paste is exposed as a `Paste` `Action` rather than
                // a window event. Implementing both here would require
                // touching `AppView::render` (paint-phase `on_mouse_event`)
                // and registering an `on_action(Paste)` handler on the root
                // view, which is out of scope for this task. The 📎 button
                // in `input_bar` (Task 18) is the primary attach path; drop
                // and paste are deferred.

                cx.new(|cx| Root::new(view, window, cx))
            })?;
            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });
    Ok(())
}
