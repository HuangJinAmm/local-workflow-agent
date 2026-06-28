// ui::theme — register light/dark themes into gpui_component and apply the
// current one based on the window's appearance.
//
// In gpui-component 0.5.1, `gpui_component::init(cx)` already installs the
// default `Theme` global and the `ThemeRegistry`, and syncs with the system
// appearance (see `gpui_component::theme::init`). Re-registering here would
// shadow the registry with an incomplete one (it only knows about "light"
// and "dark", not the schema-defined themes loaded from
// `default-theme.json`). We therefore defer to the framework's defaults and
// expose this no-op entry point so callers can switch to a custom
// registration later without changing the call site in `agent-gui.rs`.

use gpui::App;

pub fn register(_cx: &mut App) {
    // gpui_component::init already handles the default theme registry +
    // system appearance sync. Intentionally a no-op for now.
}
