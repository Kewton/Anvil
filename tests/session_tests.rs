use anvil::session::compact::compact_messages;
use anvil::session::store::{ConversationMessage, SessionSnapshot, SessionStore};
use tempfile::tempdir;

#[test]
fn session_store_round_trip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("session.json");
    let store = SessionStore::new(path.clone());
    let snapshot = SessionSnapshot {
        messages: vec![ConversationMessage::user("hello".into())],
        ..SessionSnapshot::default()
    };
    store.save(&snapshot).unwrap();
    let loaded = store.load_or_new(false).unwrap();
    assert_eq!(loaded.messages, snapshot.messages);
}

#[test]
fn compaction_keeps_recent_tail() {
    let mut messages = (0..40)
        .map(|index| ConversationMessage::user(format!("message {index}")))
        .collect::<Vec<_>>();
    assert!(compact_messages(&mut messages, 10));
    assert!(
        messages
            .iter()
            .any(|message| message.content.contains("[compact-summary]"))
    );
    assert!(messages.last().unwrap().content.contains("39"));
}
