// ui::session::tools — tool execution + confirmation gate.

use crate::ui::app::AppState;

pub fn requires_confirmation(state: &AppState, name: &str) -> bool {
    state.settings.read().tool_policy.require_confirmation.contains(name)
}

pub fn is_enabled(state: &AppState, name: &str) -> bool {
    !state.settings.read().tool_policy.disabled.contains(name)
}

pub fn find_tool(state: &AppState, name: &str) -> Option<usize> {
    state.tools.iter().position(|t| t.name() == name)
}

pub fn tool_name(state: &AppState, idx: usize) -> Option<String> {
    state.tools.get(idx).map(|t| t.name().to_string())
}

pub fn tool_count(state: &AppState) -> usize {
    state.tools.len()
}
