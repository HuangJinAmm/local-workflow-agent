// tests/core_sqlite_storage.rs — roundtrip tests for SqliteSessionStore.
use local_workflow_agent::core::sqlite_storage::SqliteSessionStore;
use tempfile::TempDir;

#[test]
fn list_messages_roundtrip() {
    let dir = TempDir::new().unwrap();
    let store = SqliteSessionStore::open(&dir.path().join("s.db")).unwrap();

    // No messages yet for an unknown session.
    assert!(store.list_messages("s1").unwrap().is_empty());

    // Create a session and persist two messages, then read them back
    // in insertion order.
    store.save_session("s1", Some("My chat"), "claude-opus").unwrap();
    store
        .save_message("s1", "m1", "user", "hi there", None)
        .unwrap();
    store
        .save_message("s1", "m2", "assistant", "hello", Some(0.001))
        .unwrap();

    let stored = store.list_messages("s1").unwrap();
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].id, "m1");
    assert_eq!(stored[0].role, "user");
    assert_eq!(stored[0].content, "hi there");
    assert_eq!(stored[1].id, "m2");
    assert_eq!(stored[1].role, "assistant");
    assert_eq!(stored[1].content, "hello");
    // created_at round-trips as a non-empty RFC3339 string.
    assert!(!stored[0].created_at.is_empty());
    assert!(
        chrono::DateTime::parse_from_rfc3339(&stored[0].created_at).is_ok(),
        "created_at must be RFC3339"
    );
}

#[test]
fn list_messages_isolates_sessions() {
    let dir = TempDir::new().unwrap();
    let store = SqliteSessionStore::open(&dir.path().join("s.db")).unwrap();
    store.save_session("s1", None, "m").unwrap();
    store.save_session("s2", None, "m").unwrap();
    store.save_message("s1", "a", "user", "in s1", None).unwrap();
    store.save_message("s2", "b", "user", "in s2", None).unwrap();

    assert_eq!(store.list_messages("s1").unwrap().len(), 1);
    assert_eq!(store.list_messages("s2").unwrap().len(), 1);
    assert_eq!(store.list_messages("s1").unwrap()[0].content, "in s1");
    assert_eq!(store.list_messages("s2").unwrap()[0].content, "in s2");
}
