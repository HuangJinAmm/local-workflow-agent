//! Entry point for the `agent-gui` binary.
//!
//! Ported from `chat-ai/src/main.rs` — boots a GPUI `Application`, registers
//! the bundled asset source (icons) and Catppuccin theme set, and opens a
//! single window hosting the `ChatAI` view wrapped in a `gpui-component`
//! `Root`.

use gpui::{AppContext as _, Application, KeyBinding, actions};
use gpui_component::{ActiveTheme as _, Root};

use tracing_subscriber::{
    EnvFilter, fmt,
    layer::SubscriberExt as _, util::SubscriberInitExt as _,
};

use local_workflow_agent::ui::{
    Assets, ChatAI, theme::change_color_mode,
    window::{blur_window, get_window_options},
};

actions!(window, [Quit, StandardAction]);

fn init_logging() {
    // --debug / -d forces debug-level logging; otherwise RUST_LOG is honored,
    // falling back to `warn`.
    let debug = std::env::args().any(|arg| arg == "--debug" || arg == "-d");
    let filter = if debug {
        EnvFilter::new("debug")
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"))
    };

    // On Windows, GUI binaries have no usable stdout/stderr (the subsystem
    // is `windows`, not `console`). Writing to a file under the user's
    // config directory is the most reliable way to see `tracing` output.
    // The file is also opened by `tail -f` / any text editor for live
    // monitoring. We always log to the file when `--debug` is set; otherwise
    // we still log to the file but with a more conservative filter so the
    // disk doesn't grow unbounded.
    let log_path = log_file_path();
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();

    if let Some(file) = file {
        let (file_writer, guard) = tracing_appender::non_blocking(file);
        // Keep the guard alive for the program's lifetime so the background
        // writer thread keeps flushing.
        Box::leak(Box::new(guard));
        tracing_subscriber::registry()
            .with(fmt::layer().with_target(true).with_writer(file_writer))
            .with(filter)
            .init();
        eprintln!(
            "agent-gui: debug log -> {}",
            log_path.display()
        );
    } else {
        // Fallback: stdout (useful on Linux/macOS, or when running via
        // `cargo run` from a terminal that already redirects stdout).
        tracing_subscriber::registry()
            .with(fmt::layer().with_target(true))
            .with(filter)
            .init();
    }
}

fn log_file_path() -> std::path::PathBuf {
    // Honor the same LWA_DATA_DIR override as the settings file.
    let dir = if let Ok(d) = std::env::var("LWA_DATA_DIR") {
        std::path::PathBuf::from(d)
    } else {
        dirs::config_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("local-workflow-agent")
    };
    dir.join("debug.log")
}

fn main() {
    init_logging();

    // Create app w/ assets
    let app = Application::new().with_assets(Assets);

    app.run(move |cx| {
        // Close app on macOS close icon click
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let window_opts = get_window_options(cx);
        cx.spawn(async move |cx| {
            cx.open_window(window_opts, |window, cx| {
                blur_window(window);
                // This must be called before using any GPUI Component features.
                gpui_component::init(cx);
                change_color_mode(cx.theme().mode, window, cx);
                let view = ChatAI::view(window, cx);
                // This first level on the window, should be a Root.
                cx.new(|cx| Root::new(view, window, cx))
            })?;

            Ok::<_, anyhow::Error>(())
        })
        .detach();

        // Close app w/ cmd-q
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);

        // Bring app to front
        cx.activate(true);
    });
}
