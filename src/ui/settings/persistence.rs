// ui::settings::persistence — load/save `Settings` to a JSON file
// under the app data dir. The file is `settings.json`. Keys
// present in the file override defaults; keys absent fall back to
// defaults (so a partial file is safe).

use std::path::{Path, PathBuf};

use crate::ui::settings::{Settings, ToolPolicy};

pub fn settings_path(data_dir: &Path) -> PathBuf {
    data_dir.join("settings.json")
}

pub fn load(data_dir: &Path) -> Settings {
    let path = settings_path(data_dir);
    match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<PartialSettings>(&text) {
            Ok(partial) => partial.merge_with(Settings::default()),
            Err(e) => {
                tracing::warn!("settings parse failed ({e}); using defaults");
                Settings::default()
            }
        },
        Err(_) => Settings::default(),
    }
}

pub fn save(data_dir: &Path, settings: &Settings) -> std::io::Result<()> {
    let path = settings_path(data_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(settings)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(path, json)
}

#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
struct PartialSettings {
    #[serde(default)]
    default_provider: Option<String>,
    #[serde(default)]
    default_model: Option<String>,
    #[serde(default)]
    anthropic_api_key: Option<String>,
    #[serde(default)]
    openai_api_key: Option<String>,
    #[serde(default)]
    theme: Option<String>,
    #[serde(default)]
    thinking_budget_tokens: Option<u32>,
    #[serde(default)]
    tool_policy: Option<ToolPolicy>,
}

impl PartialSettings {
    fn merge_with(self, mut base: Settings) -> Settings {
        if let Some(v) = self.default_provider {
            base.default_provider = v;
        }
        if let Some(v) = self.default_model {
            base.default_model = v;
        }
        if let Some(v) = self.anthropic_api_key {
            base.anthropic_api_key = Some(v);
        }
        if let Some(v) = self.openai_api_key {
            base.openai_api_key = Some(v);
        }
        if let Some(v) = self.theme {
            base.theme = match v.to_ascii_lowercase().as_str() {
                "light" => crate::ui::settings::ThemeMode::Light,
                "dark" => crate::ui::settings::ThemeMode::Dark,
                _ => crate::ui::settings::ThemeMode::System,
            };
        }
        if let Some(v) = self.thinking_budget_tokens {
            base.thinking_budget_tokens = v;
        }
        if let Some(v) = self.tool_policy {
            base.tool_policy = v;
        }
        base
    }
}
