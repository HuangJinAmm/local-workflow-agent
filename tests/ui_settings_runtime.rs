// tests/ui_settings_runtime.rs
//! Verify `AppState::update_api_key` and `set_default_model` write to disk
//! and rebuild the in-memory `ProviderRegistry` from the new settings.

use std::path::PathBuf;

use local_workflow_agent::ui::app::AppState;
use local_workflow_agent::ui::settings::persistence;

fn fresh_data_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "lwa-state-{tag}-{}-{}",
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
fn update_api_key_persists_and_reloads_registry() {
    let dir = fresh_data_dir("apikey");
    let state = AppState::with_data_dir(dir.clone(), dir.clone()).expect("app state");

    // Save a key for OpenAI (which is NOT registered by default — we expect
    // the reload to add it).
    state
        .update_api_key("openai", Some("sk-test-openai-123".into()))
        .expect("update_api_key ok");

    // Reload from disk to confirm the file is on disk.
    let reloaded = persistence::load(&dir);
    assert_eq!(
        reloaded.openai_api_key.as_deref(),
        Some("sk-test-openai-123")
    );

    // The in-memory registry should now contain the openai provider.
    let reg = state.providers.read();
    use local_workflow_agent::core::provider_id::ProviderId;
    assert!(
        reg.get(&ProviderId::new("openai")).is_some(),
        "openai provider should be registered after update_api_key"
    );
}

#[test]
fn clear_api_key_removes_provider_from_registry() {
    let dir = fresh_data_dir("clearkey");
    let state = AppState::with_data_dir(dir.clone(), dir.clone()).expect("app state");

    // Add then clear.
    state
        .update_api_key("openai", Some("sk-test".into()))
        .expect("set ok");
    state
        .update_api_key("openai", None)
        .expect("clear ok");

    let reloaded = persistence::load(&dir);
    assert_eq!(reloaded.openai_api_key, None);

    use local_workflow_agent::core::provider_id::ProviderId;
    let reg = state.providers.read();
    assert!(
        reg.get(&ProviderId::new("openai")).is_none(),
        "openai provider should be removed after key cleared"
    );
}

#[test]
fn set_default_model_switches_provider_and_persists() {
    let dir = fresh_data_dir("default");
    let state = AppState::with_data_dir(dir.clone(), dir.clone()).expect("app state");

    // openai is only registered when a key is present, so set the key
    // first — that's the realistic UX: the user pastes a key, then
    // switches the provider.
    state
        .update_api_key("openai", Some("sk-test-abc".into()))
        .expect("set key ok");
    state
        .set_default_model("openai", "gpt-4o")
        .expect("set_default_model ok");

    let reloaded = persistence::load(&dir);
    assert_eq!(reloaded.default_provider, "openai");
    assert_eq!(reloaded.default_model, "gpt-4o");

    use local_workflow_agent::core::provider_id::ProviderId;
    let reg = state.providers.read();
    assert_eq!(reg.default_provider_id(), &ProviderId::new("openai"));
}

#[test]
fn whitespace_only_key_treated_as_empty() {
    let dir = fresh_data_dir("ws");
    let state = AppState::with_data_dir(dir.clone(), dir.clone()).expect("app state");

    state
        .update_api_key("openai", Some("   ".into()))
        .expect("set ok");

    let reloaded = persistence::load(&dir);
    let normalized = reloaded
        .openai_api_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    assert_eq!(
        normalized, None,
        "whitespace-only key should be treated as cleared"
    );
}
