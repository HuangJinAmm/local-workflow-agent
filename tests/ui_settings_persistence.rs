// tests/ui_settings_persistence.rs
//! Settings JSON load/save round-trip + partial-merge semantics.

use std::collections::HashSet;
use std::path::PathBuf;

use local_workflow_agent::ui::settings::persistence::{load, save, settings_path};
use local_workflow_agent::ui::settings::{Settings, ThemeMode, ToolPolicy};

fn fresh_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "lwa-settings-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn load_missing_file_returns_defaults() {
    let dir = fresh_dir("missing");
    let s = load(&dir);
    let d = Settings::default();
    assert_eq!(s.default_provider, d.default_provider);
    assert_eq!(s.default_model, d.default_model);
    assert_eq!(s.theme, d.theme);
}

#[test]
fn save_then_load_round_trip() {
    let dir = fresh_dir("rt");
    let mut s = Settings::default();
    s.default_provider = "openai".into();
    s.default_model = "gpt-4o".into();
    s.anthropic_api_key = Some("sk-ant-test".into());
    s.openai_api_key = None;
    s.theme = ThemeMode::Dark;
    s.thinking_budget_tokens = 4096;

    save(&dir, &s).expect("save ok");
    let loaded = load(&dir);
    assert_eq!(loaded.default_provider, "openai");
    assert_eq!(loaded.default_model, "gpt-4o");
    assert_eq!(loaded.anthropic_api_key.as_deref(), Some("sk-ant-test"));
    assert_eq!(loaded.openai_api_key, None);
    assert_eq!(loaded.theme, ThemeMode::Dark);
    assert_eq!(loaded.thinking_budget_tokens, 4096);
}

#[test]
fn partial_file_keeps_unset_defaults() {
    let dir = fresh_dir("partial");
    // Write a file that only overrides one field.
    let body = r#"{"default_model": "claude-haiku-4-5"}"#;
    std::fs::write(settings_path(&dir), body).unwrap();
    let loaded = load(&dir);
    assert_eq!(loaded.default_model, "claude-haiku-4-5");
    // Other fields fall back to defaults.
    let d = Settings::default();
    assert_eq!(loaded.default_provider, d.default_provider);
    assert_eq!(loaded.theme, d.theme);
}

#[test]
fn malformed_file_falls_back_to_defaults() {
    let dir = fresh_dir("malformed");
    std::fs::write(settings_path(&dir), "not valid json").unwrap();
    let loaded = load(&dir);
    let d = Settings::default();
    assert_eq!(loaded.default_provider, d.default_provider);
}

#[test]
fn save_creates_parent_dir() {
    let base = fresh_dir("nested");
    let dir = base.join("a/b/c");
    let s = Settings::default();
    save(&dir, &s).expect("save ok");
    assert!(settings_path(&dir).exists());
}

#[test]
fn tool_policy_round_trip() {
    let dir = fresh_dir("tools");
    let mut s = Settings::default();
    s.tool_policy = ToolPolicy {
        disabled: HashSet::from(["bash".into()]),
        require_confirmation: HashSet::from(["file_write".into()]),
    };
    save(&dir, &s).unwrap();
    let loaded = load(&dir);
    assert!(loaded.tool_policy.disabled.contains("bash"));
    assert!(loaded.tool_policy.require_confirmation.contains("file_write"));
}
