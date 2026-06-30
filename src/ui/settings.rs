//! Settings persistence — load/save the chat-ai GUI settings (API key, base
//! URL, model) to a JSON file under the user's config directory.
//!
//! Layout follows the project convention (per `project_memory`):
//! * On Windows: `%APPDATA%\local-workflow-agent\settings.json`
//! * On Linux/macOS: `~/.config/local-workflow-agent/settings.json`
//!
//! API keys are stored in plaintext — this matches the existing
//! "API keys are stored in Preferences in plaintext" hard constraint
//! from `project_memory.md`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Persisted GUI settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Provider id, e.g. `anthropic`, `openai`, `deepseek`, `ollama`.
    /// Controls which `LlmProvider` implementation the agent uses.
    #[serde(default = "default_provider")]
    pub provider: String,
    /// Anthropic API key (plaintext — see project_memory.md constraint).
    #[serde(default)]
    pub api_key: String,
    /// API base URL. Empty means "use library default"
    /// (`crate::core::constants::ANTHROPIC_API_BASE`).
    #[serde(default)]
    pub base_url: String,
    /// Model identifier, e.g. `claude-haiku-4-5-20251001`.
    #[serde(default = "default_model")]
    pub model: String,
    /// Working directory for agent file operations / shell tools.
    /// Defaults to the user's home directory.
    #[serde(default = "default_working_dir")]
    pub working_dir: std::path::PathBuf,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            api_key: String::new(),
            base_url: String::new(),
            model: default_model(),
            working_dir: default_working_dir(),
        }
    }
}

fn default_provider() -> String {
    "anthropic".to_string()
}

fn default_model() -> String {
    "claude-haiku-4-5-20251001".to_string()
}

fn default_working_dir() -> std::path::PathBuf {
    dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."))
}

impl Settings {
    /// Resolve the effective base URL — falls back to the library default
    /// when the field is blank.
    pub fn effective_base_url(&self) -> String {
        if self.base_url.trim().is_empty() {
            crate::core::constants::ANTHROPIC_API_BASE.to_string()
        } else {
            self.base_url.clone()
        }
    }

    /// Whether the user has configured an API key (env var or settings file).
    pub fn has_api_key(&self) -> bool {
        !self.api_key.trim().is_empty()
    }

    /// Load settings from the on-disk JSON file. Returns `Default` if the
    /// file does not exist (first launch). Env var `ANTHROPIC_API_KEY` is
    /// used as a fallback when the settings file has no key.
    pub fn load() -> Result<Settings> {
        let path = settings_path()?;
        if !path.exists() {
            let mut s = Settings::default();
            // Seed from env var if present, so existing users with
            // `ANTHROPIC_API_KEY` set don't see the "no API key" alert.
            if let Ok(k) = std::env::var("ANTHROPIC_API_KEY") {
                s.api_key = k;
            }
            return Ok(s);
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let mut s: Settings = serde_json::from_str(&text)
            .with_context(|| format!("parsing {}", path.display()))?;
        // Env var overrides settings file if present and file is empty.
        if s.api_key.trim().is_empty() {
            if let Ok(k) = std::env::var("ANTHROPIC_API_KEY") {
                s.api_key = k;
            }
        }
        Ok(s)
    }

    /// Save settings to the on-disk JSON file. Creates the parent dir if
    /// it doesn't exist.
    pub fn save(&self) -> Result<()> {
        let path = settings_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self)
            .context("serializing settings")?;
        std::fs::write(&path, text)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }
}

/// Resolve the settings file path under the user's config directory.
fn settings_path() -> Result<std::path::PathBuf> {
    let dir = if let Ok(d) = std::env::var("LWA_DATA_DIR") {
        std::path::PathBuf::from(d)
    } else {
        dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("cannot determine config directory"))?
            .join("local-workflow-agent")
    };
    Ok(dir.join("settings.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip() {
        let s = Settings {
            provider: "openai".into(),
            api_key: "sk-test".into(),
            base_url: "https://custom.example.com".into(),
            model: "claude-haiku-4-5-20251001".into(),
            working_dir: std::path::PathBuf::from("/tmp/work"),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.provider, "openai");
        assert_eq!(back.api_key, "sk-test");
        assert_eq!(back.base_url, "https://custom.example.com");
        assert_eq!(back.model, "claude-haiku-4-5-20251001");
        assert_eq!(back.working_dir, std::path::PathBuf::from("/tmp/work"));
    }

    #[test]
    fn empty_base_url_falls_back_to_default() {
        let s = Settings {
            provider: default_provider(),
            api_key: "".into(),
            base_url: "".into(),
            model: default_model(),
            working_dir: default_working_dir(),
        };
        assert_eq!(
            s.effective_base_url(),
            crate::core::constants::ANTHROPIC_API_BASE
        );
    }
}
