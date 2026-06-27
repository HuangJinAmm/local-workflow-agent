// tests/ui_storage.rs
use local_workflow_agent::ui::model::*;
use local_workflow_agent::ui::storage::MessageStore;
use tempfile::TempDir;

#[test]
fn create_insert_fetch_roundtrip() {
    let dir = TempDir::new().unwrap();
    let store = MessageStore::open(&dir.path().join("ui.db")).unwrap();

    let msg = UiMessage {
        id: "m1".into(),
        session_id: "s1".into(),
        role: Role::User,
        created_at: 1,
        ordinal: 0,
    };
    store.insert_message(&msg).unwrap();

    let block = UiBlock {
        id: "b1".into(),
        message_id: "m1".into(),
        ordinal: 0,
        kind: BlockKind::Text { text: "hi".into() },
    };
    store.insert_block(&block).unwrap();

    let msgs = store.list_messages("s1").unwrap();
    assert_eq!(msgs.len(), 1);
    let blocks = store.list_blocks("m1").unwrap();
    assert_eq!(blocks.len(), 1);
    match &blocks[0].kind {
        BlockKind::Text { text } => assert_eq!(text, "hi"),
        _ => panic!("expected text"),
    }
}
