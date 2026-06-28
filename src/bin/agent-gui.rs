use std::path::PathBuf;

use gpui::*;
use gpui_component::*;
use local_workflow_agent::ui::app::AppState;
use local_workflow_agent::ui::app_view::AppView;

fn main() -> anyhow::Result<()> {
    let app = Application::new();
    app.run(move |cx| {
        gpui_component::init(cx);
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
                cx.new(|cx| Root::new(view, window, cx))
            })?;
            Ok::<_, anyhow::Error>(())
        })
        .detach();
    });
    Ok(())
}
