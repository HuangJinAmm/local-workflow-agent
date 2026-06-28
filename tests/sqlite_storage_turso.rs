use local_workflow_agent::core::SqliteSessionStore;
use tempfile::tempdir;

#[tokio::test]
async fn save_message_is_idempotent() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("sessions.sqlite");
    let store = SqliteSessionStore::open(&db_path).await.unwrap();

    store
        .save_session("s1", Some("Test Session"), "gpt-4")
        .await
        .unwrap();

    store
        .save_message("s1", "m1", "user", "hello", Some(0.1))
        .await
        .unwrap();
    store
        .save_message("s1", "m1", "user", "hello", Some(0.1))
        .await
        .unwrap();

    let sessions = store.list_sessions().await.unwrap();
    let messages = store.list_messages("s1").await.unwrap();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "s1");
    assert_eq!(sessions[0].message_count, 1);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].id, "m1");
}

#[tokio::test]
async fn list_messages_returns_oldest_first() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("sessions.sqlite");
    let store = SqliteSessionStore::open(&db_path).await.unwrap();

    store
        .save_session("s1", Some("Ordered Session"), "gpt-4")
        .await
        .unwrap();

    store
        .save_message("s1", "m1", "user", "first", None)
        .await
        .unwrap();
    store
        .save_message("s1", "m2", "assistant", "second", None)
        .await
        .unwrap();
    store
        .save_message("s1", "m3", "user", "third", None)
        .await
        .unwrap();

    let messages = store.list_messages("s1").await.unwrap();

    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].id, "m1");
    assert_eq!(messages[0].content, "first");
    assert_eq!(messages[1].id, "m2");
    assert_eq!(messages[1].content, "second");
    assert_eq!(messages[2].id, "m3");
    assert_eq!(messages[2].content, "third");
}
