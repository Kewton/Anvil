use anvil::session::compact::{
    COMPACT_SUMMARY_PREFIX, approximate_token_count, compact_messages,
    compact_messages_with_strategy,
};
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
            .any(|message| message.content.contains(COMPACT_SUMMARY_PREFIX))
    );
    assert!(messages.last().unwrap().content.contains("39"));
}

#[test]
fn compaction_can_use_custom_summary_strategy() {
    let mut messages = (0..20)
        .map(|index| ConversationMessage::user(format!("message {index}")))
        .collect::<Vec<_>>();
    let changed = compact_messages_with_strategy(&mut messages, 5, |_| {
        Ok("custom sidecar summary".to_string())
    })
    .unwrap();
    assert!(changed);
    assert!(
        messages
            .iter()
            .any(|message| message.content.contains("custom sidecar summary"))
    );
}

#[test]
fn approximate_token_count_is_positive() {
    let messages = vec![
        ConversationMessage::user("hello".into()),
        ConversationMessage::assistant("world".into(), Vec::new()),
    ];
    assert!(approximate_token_count(&messages) > 0);
}
