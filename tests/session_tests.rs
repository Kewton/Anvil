use anvil::session::compact::{
    COMPACT_SUMMARY_PREFIX, approximate_token_count, compact_messages,
    compact_messages_with_strategy,
};
use anvil::session::store::{ConversationMessage, SessionSnapshot, SessionStore, WorkingMemory};
use tempfile::tempdir;

#[test]
fn session_store_round_trip() {
    let dir = tempdir().unwrap();
    let session_id = "0199fe00-0000-7000-8000-000000000001";
    let workspace_key = "abc123";
    let session_dir = dir.path().join("sessions").join(session_id);
    std::fs::create_dir_all(&session_dir).unwrap();
    let store = SessionStore::new(dir.path(), session_id, workspace_key);
    let snapshot = SessionSnapshot {
        messages: vec![ConversationMessage::user("hello".into())],
        ..SessionSnapshot::default()
    };
    store.save(&snapshot).unwrap();
    let loaded = store.load_or_new(false).unwrap();
    assert_eq!(loaded.messages, snapshot.messages);
    assert_eq!(loaded.id, session_id);
    assert_eq!(loaded.workspace_key, workspace_key);
}

#[test]
fn session_snapshot_roundtrip_preserves_id_and_workspace_key() {
    let snapshot = SessionSnapshot {
        id: "0199fe00-0000-7000-8000-000000000002".to_string(),
        workspace_key: "deadbeef".to_string(),
        messages: vec![ConversationMessage::user("hi".into())],
        ..SessionSnapshot::default()
    };
    let json = serde_json::to_string(&snapshot).unwrap();
    let decoded: SessionSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.id, snapshot.id);
    assert_eq!(decoded.workspace_key, snapshot.workspace_key);
    assert_eq!(decoded.messages, snapshot.messages);
}

#[test]
fn session_snapshot_legacy_defaults_id_and_workspace_key() {
    let legacy =
        r#"{"mode_state":{"mode":"Act","active_plan_path":null},"messages":[],"checkpoints":[]}"#;
    let decoded: SessionSnapshot = serde_json::from_str(legacy).unwrap();
    assert_eq!(decoded.id, "");
    assert_eq!(decoded.workspace_key, "");
}

#[test]
fn session_store_fills_in_empty_id_on_load() {
    let dir = tempdir().unwrap();
    let session_id = "0199fe00-0000-7000-8000-000000000003";
    let workspace_key = "ws-key";
    let session_dir = dir.path().join("sessions").join(session_id);
    std::fs::create_dir_all(&session_dir).unwrap();
    let session_json = session_dir.join("session.json");
    std::fs::write(
        &session_json,
        r#"{"mode_state":{"mode":"Act","active_plan_path":null},"messages":[],"checkpoints":[]}"#,
    )
    .unwrap();

    let store = SessionStore::new(dir.path(), session_id, workspace_key);
    let loaded = store.load_or_new(false).unwrap();
    assert_eq!(loaded.id, session_id);
    assert_eq!(loaded.workspace_key, workspace_key);
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

#[test]
fn working_memory_formats_for_prompt() {
    let mut memory = WorkingMemory::default();
    memory.set_active_task(Some("Implement long-session memory".to_string()));
    memory.replace_constraints(vec!["Keep paths relative".to_string()]);
    memory.note_touched_file("src/main.rs".to_string());
    memory.note_error("Edit: target text not found".to_string());

    let rendered = memory.format_for_prompt().unwrap();
    assert!(rendered.contains("Active task: Implement long-session memory"));
    assert!(rendered.contains("Constraints:"));
    assert!(rendered.contains("src/main.rs"));
    assert!(rendered.contains("target text not found"));
}

#[test]
fn session_snapshot_roundtrip_preserves_working_memory() {
    let snapshot = SessionSnapshot {
        working_memory: WorkingMemory {
            active_task: Some("Do the thing".to_string()),
            constraints: vec!["Do not lose context".to_string()],
            touched_files: vec!["src/lib.rs".to_string()],
            unresolved_errors: vec!["Edit: target text not found".to_string()],
        },
        ..SessionSnapshot::default()
    };

    let json = serde_json::to_string(&snapshot).unwrap();
    let decoded: SessionSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.working_memory, snapshot.working_memory);
}
