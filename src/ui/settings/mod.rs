// ui::settings — runtime configuration. Persisted via gpui_component::Preferences
// in `settings.json` under the app's standard config dir.

use std::collections::HashSet;

#[derive(Debug, Clone)]
pub struct Settings {
    pub theme: ThemeMode,
    pub default_provider: String,         // "anthropic" | "openai"
    pub default_model: String,
    pub anthropic_api_key: Option<String>,
    pub openai_api_key: Option<String>,
    pub thinking_budget_tokens: u32,
    pub tool_policy: ToolPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode { Light, Dark, System }

#[derive(Debug, Clone, Default)]
pub struct ToolPolicy {
    pub disabled: HashSet<String>,            // empty = none disabled
    pub require_confirmation: HashSet<String>,
}

impl Default for Settings {
    fn default() -> Self {
        let mut require_confirmation = HashSet::new();
        for t in ["bash", "powershell", "file_write", "file_edit", "apply_patch"] {
            require_confirmation.insert(t.to_string());
        }
        Self {
            theme: ThemeMode::System,
            default_provider: "anthropic".into(),
            default_model: "claude-sonnet-4-5".into(),
            anthropic_api_key: std::env::var("ANTHROPIC_API_KEY").ok(),
            openai_api_key: std::env::var("OPENAI_API_KEY").ok(),
            thinking_budget_tokens: 8000,
            tool_policy: ToolPolicy {
                disabled: HashSet::new(),
                require_confirmation,
            },
        }
    }
}

pub mod settings_panel;
