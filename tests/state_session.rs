mod common;

use anvil::contracts::{AppEvent, AppStateSnapshot, RuntimeState};
use anvil::session::{
    MessageRole, MessageStatus, NoteCategory, SessionMessage, SessionNote, SessionRecord,
    SessionStore, WorkingMemory, build_conversation_text_for_summary, extract_file_targets,
    extract_session_notes, new_assistant_message, new_user_message, validate_session_name,
};
use anvil::state::{StateMachine, StateTransition};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn initial_app_snapshot_is_ready() {
    let mut app = common::build_app();
    let snapshot = app
        .initial_snapshot()
        .expect("initial snapshot should build");

    assert_eq!(snapshot.state, RuntimeState::Ready);
    assert_eq!(snapshot.last_event, Some(AppEvent::StartupCompleted));
    assert_eq!(app.state_machine().snapshot().state, RuntimeState::Ready);
    assert_eq!(
        app.state_machine().snapshot().last_event,
        Some(AppEvent::StateChanged)
    );
    assert!(app.session_store().file_path().exists());
    assert_eq!(
        app.session().last_snapshot,
        Some(app.state_machine().snapshot().clone())
    );
    assert_eq!(app.session().session_event, Some(AppEvent::SessionSaved));
    assert!(app.session().event_log.contains(&AppEvent::SessionLoaded));
    assert!(app.session().event_log.contains(&AppEvent::SessionSaved));
}

#[test]
fn app_records_user_input_into_session_history() {
    let mut app = common::build_app();
    let before = app.session().message_count();

    app.record_user_input("msg_001", "review src/session")
        .expect("user input should persist");

    assert_eq!(app.session().message_count(), before + 1);
    let last = app.session().messages.last().expect("message should exist");
    assert_eq!(last.author, "you");
    assert_eq!(last.content, "review src/session");
    assert_eq!(app.session().session_event, Some(AppEvent::SessionSaved));
    assert!(app.session().event_log.contains(&AppEvent::SessionSaved));
}

#[test]
fn state_machine_allows_interrupt_from_awaiting_approval() {
    let mut machine = StateMachine::new();
    let thinking = AppStateSnapshot::new(RuntimeState::Thinking);
    machine
        .transition_to(thinking, StateTransition::StartThinking)
        .expect("ready -> thinking should be valid");

    let approval = AppStateSnapshot::new(RuntimeState::AwaitingApproval);
    machine
        .transition_to(approval, StateTransition::RequestApproval)
        .expect("thinking -> approval should be valid");

    let interrupted = AppStateSnapshot::new(RuntimeState::Interrupted);
    machine
        .transition_to(interrupted, StateTransition::Interrupt)
        .expect("approval -> interrupted should be valid");
}

#[test]
fn state_machine_allows_working_to_resume_thinking() {
    let mut machine = StateMachine::new();
    machine
        .transition_to(
            AppStateSnapshot::new(RuntimeState::Thinking),
            StateTransition::StartThinking,
        )
        .expect("ready -> thinking should be valid");
    machine
        .transition_to(
            AppStateSnapshot::new(RuntimeState::Working),
            StateTransition::StartWorking,
        )
        .expect("thinking -> working should be valid");
    machine
        .transition_to(
            AppStateSnapshot::new(RuntimeState::Thinking),
            StateTransition::ResumeThinking,
        )
        .expect("working -> thinking should be valid");
}

#[test]
fn state_machine_allows_reset_from_thinking() {
    let mut machine = StateMachine::new();
    machine
        .transition_to(
            AppStateSnapshot::new(RuntimeState::Thinking),
            StateTransition::StartThinking,
        )
        .expect("ready -> thinking should be valid");
    machine
        .transition_to(
            AppStateSnapshot::new(RuntimeState::Ready),
            StateTransition::ResetToReady,
        )
        .expect("thinking -> ready reset should be valid");
}

#[test]
fn session_store_persists_and_restores_messages() {
    let root = unique_test_dir("session_roundtrip");
    let mut config =
        anvil::config::EffectiveConfig::default_for_test().expect("config should load");
    config.paths.cwd = root.clone();
    config.paths.workspace_dir = root.join("workspace");
    config.paths.config_file = root.join(".anvil").join("config");
    config.paths.state_dir = root.join(".anvil").join("state");
    config.paths.session_dir = root.join(".anvil").join("sessions");
    config.paths.session_file = config.paths.session_dir.join("session_roundtrip.json");
    config.paths.logs_dir = root.join(".anvil").join("logs");

    let store = SessionStore::from_config(&config);
    let mut session = store
        .load_or_create(&config.paths.cwd)
        .expect("session should load");
    session.push_message(new_user_message("msg_001", "inspect src/session"));
    session.push_message(new_assistant_message(
        "msg_002",
        "working on session persistence",
        MessageStatus::Committed,
    ));
    session.set_last_snapshot(AppStateSnapshot::new(RuntimeState::Done));
    store.save(&session).expect("session should save");

    let reloaded = store.load().expect("session should reload");
    assert_eq!(reloaded.message_count(), 2);
    assert!(reloaded.estimated_token_count() > 0);
    assert!(!reloaded.event_log.is_empty());
    assert_eq!(
        reloaded.last_snapshot.expect("snapshot should exist").state,
        RuntimeState::Done
    );
}

fn unique_test_dir(label: &str) -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be monotonic")
        .as_millis();
    std::env::temp_dir().join(format!("anvil_{label}_{millis}"))
}

#[test]
fn session_normalization_marks_partial_messages_as_interrupted() {
    let mut session = SessionRecord::new(PathBuf::from("/tmp/anvil-session-test"));
    session.push_message(new_user_message("msg_001", "run diagnostics"));
    session.push_message(new_assistant_message(
        "msg_002",
        "",
        MessageStatus::InProgress,
    ));
    session.set_last_snapshot(AppStateSnapshot::new(RuntimeState::Interrupted));

    session.normalize_interrupted_turn("provider turn");

    let last = session.messages.last().expect("message should exist");
    assert_eq!(last.status, MessageStatus::Interrupted);
    assert!(last.content.contains("interrupted"));
    assert_eq!(
        session.session_event,
        Some(AppEvent::SessionNormalizedAfterInterrupt)
    );
}

#[test]
fn session_normalization_adds_synthetic_entry_when_no_partial_message_exists() {
    let mut session = SessionRecord::new(PathBuf::from("/tmp/anvil-session-test-no-partial"));
    session.push_message(new_user_message("msg_001", "stop current turn"));

    session.normalize_interrupted_turn("provider turn");

    let last = session.messages.last().expect("message should exist");
    assert_eq!(last.author, "anvil");
    assert_eq!(last.status, MessageStatus::Interrupted);
    assert!(last.content.contains("interrupted"));
}

#[test]
fn compact_history_keeps_recent_messages_and_mentions_file_targets() {
    let mut session = SessionRecord::new(PathBuf::from("/tmp/anvil-session-compact"));
    session.push_message(new_user_message(
        "msg_001",
        "inspect src/provider/openai.rs",
    ));
    session.push_message(
        anvil::session::SessionMessage::new(
            anvil::session::MessageRole::Tool,
            "tool",
            "file.write wrote ./sandbox/demo/Invader.html",
        )
        .with_id("tool_001"),
    );
    session.push_message(new_assistant_message(
        "msg_002",
        "I updated ./sandbox/demo/Invader.html and reviewed the code.",
        MessageStatus::Committed,
    ));
    for index in 3..14 {
        session.push_message(new_user_message(
            format!("msg_{index:03}"),
            format!("follow up {index}"),
        ));
    }

    let changed = session.compact_history(8);

    assert!(changed);
    assert!(
        session.messages[0]
            .content
            .contains("[compacted session summary]")
    );
    assert!(
        session.messages[0]
            .content
            .contains("./sandbox/demo/Invader.html")
    );
    assert!(session.event_log.contains(&AppEvent::SessionCompacted));
}

#[test]
fn state_machine_rejects_invalid_transition_from_ready_to_working() {
    let mut machine = StateMachine::new();
    let result = machine.transition_to(
        AppStateSnapshot::new(RuntimeState::Working),
        StateTransition::StartWorking,
    );
    assert!(
        result.is_err(),
        "Ready -> Working should be invalid (must go through Thinking)"
    );
    let err = result.unwrap_err();
    assert_eq!(err.from, RuntimeState::Ready);
    assert_eq!(err.to, RuntimeState::Working);
}

#[test]
fn state_machine_allows_full_happy_path_cycle() {
    let mut machine = StateMachine::new();

    // Ready -> Thinking
    machine
        .transition_to(
            AppStateSnapshot::new(RuntimeState::Thinking),
            StateTransition::StartThinking,
        )
        .expect("Ready -> Thinking");

    // Thinking -> Working
    machine
        .transition_to(
            AppStateSnapshot::new(RuntimeState::Working),
            StateTransition::StartWorking,
        )
        .expect("Thinking -> Working");

    // Working -> Thinking (resume)
    machine
        .transition_to(
            AppStateSnapshot::new(RuntimeState::Thinking),
            StateTransition::ResumeThinking,
        )
        .expect("Working -> Thinking (resume)");

    // Thinking -> Done
    machine
        .transition_to(
            AppStateSnapshot::new(RuntimeState::Done),
            StateTransition::Finish,
        )
        .expect("Thinking -> Done");

    // Done -> Ready (reset)
    machine
        .transition_to(
            AppStateSnapshot::new(RuntimeState::Ready),
            StateTransition::ResetToReady,
        )
        .expect("Done -> Ready (reset)");
}

#[test]
fn state_machine_allows_error_recovery() {
    let mut machine = StateMachine::new();

    machine
        .transition_to(
            AppStateSnapshot::new(RuntimeState::Thinking),
            StateTransition::StartThinking,
        )
        .expect("Ready -> Thinking");

    machine
        .transition_to(
            AppStateSnapshot::new(RuntimeState::Error),
            StateTransition::Fail,
        )
        .expect("Thinking -> Error");

    // Error -> Thinking (retry)
    machine
        .transition_to(
            AppStateSnapshot::new(RuntimeState::Thinking),
            StateTransition::StartThinking,
        )
        .expect("Error -> Thinking (retry)");
}

#[test]
fn state_machine_allows_reset_from_all_terminal_states() {
    for from_state in [
        RuntimeState::Done,
        RuntimeState::Error,
        RuntimeState::Interrupted,
    ] {
        let mut machine = StateMachine::from_snapshot(AppStateSnapshot::new(from_state));
        machine
            .transition_to(
                AppStateSnapshot::new(RuntimeState::Ready),
                StateTransition::ResetToReady,
            )
            .unwrap_or_else(|_| panic!("Reset from {from_state:?} should be valid"));
    }
}

#[test]
fn estimated_token_count_caches_across_calls() {
    let mut session = SessionRecord::new(PathBuf::from("/tmp/anvil-token-cache"));
    session.push_message(new_user_message("msg_001", "hello world"));

    let first = session.estimated_token_count();
    let second = session.estimated_token_count();
    assert_eq!(first, second);
    assert!(first > 0);

    // Adding a message should update cache incrementally
    session.push_message(new_user_message("msg_002", "more tokens here"));
    let after_push = session.estimated_token_count();
    assert!(after_push > first);
}

#[test]
fn session_interrupt_persists_and_resumes_correctly() {
    use anvil::agent::PendingTurnState;

    let root = unique_test_dir("interrupt_resume");
    let mut config =
        anvil::config::EffectiveConfig::default_for_test().expect("config should load");
    config.paths.cwd = root.clone();
    config.paths.session_dir = root.join(".anvil").join("sessions");
    config.paths.session_file = config.paths.session_dir.join("session_interrupt.json");
    config.paths.logs_dir = root.join(".anvil").join("logs");

    let store = anvil::session::SessionStore::from_config(&config);
    let mut session = store
        .load_or_create(&config.paths.cwd)
        .expect("session should load");

    // Simulate a turn that was interrupted during approval
    session.push_message(new_user_message("msg_001", "do something"));
    session.push_message(new_assistant_message(
        "msg_002",
        "",
        MessageStatus::InProgress,
    ));
    session.set_pending_turn(PendingTurnState {
        waiting_tool_call_id: "call_001".to_string(),
        remaining_events: vec![],
        pending_tool_calls: vec![],
    });
    session.normalize_interrupted_turn("provider turn");
    store.save(&session).expect("session should save");

    // Reload and verify state
    let reloaded = store.load().expect("session should reload");
    assert!(
        reloaded.has_pending_turn(),
        "pending turn should survive reload"
    );
    assert_eq!(
        reloaded.pending_turn.as_ref().unwrap().waiting_tool_call_id,
        "call_001"
    );

    // Verify interrupted messages are properly marked
    let last_msg = reloaded
        .messages
        .iter()
        .rev()
        .find(|m| m.status == MessageStatus::Interrupted);
    assert!(
        last_msg.is_some(),
        "should have an interrupted message after normalize"
    );
    assert!(
        last_msg.unwrap().content.contains("interrupted"),
        "interrupted message should contain reason"
    );

    // Verify the session can be cleared and new turn started
    let mut resumed = reloaded;
    resumed.clear_pending_turn();
    assert!(!resumed.has_pending_turn());
    resumed.push_message(new_user_message("msg_003", "continue"));
    assert_eq!(resumed.message_count(), 3); // original 2 (msg_002 marked interrupted in-place) + new
}

#[test]
fn web_search_pending_turn_state_serde_roundtrip() {
    use anvil::agent::PendingTurnState;
    use anvil::tooling::{ToolCallRequest, ToolInput};

    let pending = PendingTurnState {
        waiting_tool_call_id: "call_ws_001".to_string(),
        remaining_events: vec![],
        pending_tool_calls: vec![ToolCallRequest::new(
            "call_ws_001",
            "web.search",
            ToolInput::WebSearch {
                query: "rust serde tutorial".to_string(),
            },
        )],
    };

    let json = serde_json::to_string(&pending).expect("serialize should succeed");
    let deserialized: PendingTurnState =
        serde_json::from_str(&json).expect("deserialize should succeed");

    assert_eq!(pending, deserialized);
    assert_eq!(deserialized.pending_tool_calls[0].tool_name, "web.search");
    match &deserialized.pending_tool_calls[0].input {
        ToolInput::WebSearch { query } => {
            assert_eq!(query, "rust serde tutorial");
        }
        other => panic!("unexpected tool input: {other:?}"),
    }
}

#[test]
fn file_edit_pending_turn_state_serde_roundtrip() {
    use anvil::agent::PendingTurnState;
    use anvil::tooling::{ToolCallRequest, ToolInput};

    let pending = PendingTurnState {
        waiting_tool_call_id: "call_edit_001".to_string(),
        remaining_events: vec![],
        pending_tool_calls: vec![ToolCallRequest::new(
            "call_edit_001",
            "file.edit",
            ToolInput::FileEdit {
                path: "./src/main.rs".to_string(),
                old_string: "fn main()".to_string(),
                new_string: "fn main() -> Result<()>".to_string(),
            },
        )],
    };

    let json = serde_json::to_string(&pending).expect("serialize should succeed");
    let deserialized: PendingTurnState =
        serde_json::from_str(&json).expect("deserialize should succeed");

    assert_eq!(pending, deserialized);
    assert_eq!(deserialized.pending_tool_calls[0].tool_name, "file.edit");
    match &deserialized.pending_tool_calls[0].input {
        ToolInput::FileEdit {
            path,
            old_string,
            new_string,
        } => {
            assert_eq!(path, "./src/main.rs");
            assert_eq!(old_string, "fn main()");
            assert_eq!(new_string, "fn main() -> Result<()>");
        }
        other => panic!("unexpected tool input: {other:?}"),
    }
}

#[test]
fn atomic_write_leaves_no_tmp_file_on_success() {
    let root = unique_test_dir("atomic_write");
    let mut config =
        anvil::config::EffectiveConfig::default_for_test().expect("config should load");
    config.paths.cwd = root.clone();
    config.paths.workspace_dir = root.join("workspace");
    config.paths.config_file = root.join(".anvil").join("config");
    config.paths.state_dir = root.join(".anvil").join("state");
    config.paths.session_dir = root.join(".anvil").join("sessions");
    config.paths.session_file = config.paths.session_dir.join("session_atomic.json");
    config.paths.logs_dir = root.join(".anvil").join("logs");

    let store = SessionStore::from_config(&config);
    let mut session = store
        .load_or_create(&config.paths.cwd)
        .expect("session should load");
    session.push_message(new_user_message("msg_001", "atomic write test"));
    store.save(&session).expect("session should save");

    // The session file should exist
    assert!(
        config.paths.session_file.exists(),
        "session file should exist"
    );

    // The temporary file (.json.tmp) should NOT exist after a successful save
    let tmp_path = config.paths.session_file.with_extension("json.tmp");
    assert!(
        !tmp_path.exists(),
        "temporary file should not exist after successful atomic write"
    );

    // Verify the saved data is readable
    let reloaded = store
        .load()
        .expect("session should reload after atomic write");
    assert_eq!(reloaded.message_count(), session.message_count());
}

// ── SessionMessage image_paths tests ──────────────────────────────────

#[test]
fn session_message_image_paths_default_is_none() {
    let msg = SessionMessage::new(MessageRole::Tool, "tool", "hello");
    assert_eq!(msg.image_paths, None);
}

#[test]
fn session_message_with_image_paths_builder() {
    let msg = SessionMessage::new(MessageRole::Tool, "tool", "[画像: test.png]")
        .with_image_paths(vec!["test.png".to_string()]);
    assert_eq!(msg.image_paths, Some(vec!["test.png".to_string()]));
}

#[test]
fn session_message_backward_compat_deserialize_without_image_paths() {
    // JSON without image_paths field should deserialize successfully
    let json = r#"{
        "id": "msg_1",
        "role": "User",
        "author": "you",
        "content": "hello",
        "status": "Committed",
        "tool_call_id": null
    }"#;
    let msg: SessionMessage = serde_json::from_str(json).expect("should deserialize");
    assert_eq!(msg.image_paths, None);
    assert_eq!(msg.content, "hello");
}

#[test]
fn session_message_deserialize_with_image_paths() {
    let json = r#"{
        "id": "msg_2",
        "role": "Tool",
        "author": "tool",
        "content": "[画像: test.png]",
        "status": "Committed",
        "tool_call_id": null,
        "image_paths": ["test.png", "photo.jpg"]
    }"#;
    let msg: SessionMessage = serde_json::from_str(json).expect("should deserialize");
    assert_eq!(
        msg.image_paths,
        Some(vec!["test.png".to_string(), "photo.jpg".to_string()])
    );
}

#[test]
fn push_message_with_images_adds_image_tokens() {
    let mut session = SessionRecord::new(PathBuf::from("/tmp/test"));
    // First get baseline with a text message
    let text_msg =
        SessionMessage::new(MessageRole::User, "you", "hello").with_id("msg_1".to_string());
    session.push_message(text_msg);
    let baseline = session.estimated_token_count();

    // Now add a message with 2 image paths
    let img_msg = SessionMessage::new(MessageRole::Tool, "tool", "[画像]")
        .with_id("msg_2".to_string())
        .with_image_paths(vec!["a.png".to_string(), "b.png".to_string()]);
    session.push_message(img_msg);

    let new_count = session.estimated_token_count();
    // Should have added at least 600 tokens (300 per image)
    assert!(
        new_count >= baseline + 600,
        "expected at least {} but got {}",
        baseline + 600,
        new_count
    );
}

#[test]
fn estimated_token_count_accounts_for_images_on_recalc() {
    let mut session = SessionRecord::new(PathBuf::from("/tmp/test"));
    let img_msg = SessionMessage::new(MessageRole::Tool, "tool", "x")
        .with_id("msg_1".to_string())
        .with_image_paths(vec!["a.png".to_string()]);
    session.push_message(img_msg);

    // Force cache invalidation by compacting (which sets cache to None)
    // Then re-check estimated_token_count recalculates including images
    let count_before = session.estimated_token_count();

    // Compact to invalidate cache
    session.compact_history(0);
    let _count_after = session.estimated_token_count();

    // Both should include image tokens (before compact had the image message)
    assert!(
        count_before >= 300,
        "expected at least 300 but got {}",
        count_before
    );
    // After compact, image message was replaced with summary (no images), so count_after may differ
    // The important thing is the count_before properly included image tokens
}

// ── Task 1.1: validate_session_name tests ──────────────────────────────

#[test]
fn validate_session_name_accepts_valid_names() {
    for name in &["default", "my-session", "bugfix_123", "A", "a1-b2_c3"] {
        assert!(
            validate_session_name(name).is_ok(),
            "expected '{name}' to be valid"
        );
    }
}

#[test]
fn validate_session_name_accepts_max_length_name() {
    let name: String = "a".repeat(64);
    assert!(validate_session_name(&name).is_ok());
}

#[test]
fn validate_session_name_rejects_empty_name() {
    assert!(validate_session_name("").is_err());
}

#[test]
fn validate_session_name_rejects_too_long_name() {
    let name: String = "a".repeat(65);
    assert!(validate_session_name(&name).is_err());
}

#[test]
fn validate_session_name_rejects_special_characters() {
    for name in &["foo/bar", "foo\\bar", "foo bar", "hello!", "a@b", "a.b"] {
        assert!(
            validate_session_name(name).is_err(),
            "expected '{name}' to be rejected"
        );
    }
}

#[test]
fn validate_session_name_rejects_dot_and_dotdot() {
    assert!(validate_session_name(".").is_err());
    assert!(validate_session_name("..").is_err());
}

#[test]
fn validate_session_name_rejects_nul_byte() {
    assert!(validate_session_name("foo\0bar").is_err());
}

// ── Task 1.2: SessionRecord::new_named tests ──────────────────────────

#[test]
fn session_record_new_named_sets_session_id_to_name() {
    let record = SessionRecord::new_named("my-session", PathBuf::from("/tmp/test"))
        .expect("should create named session");
    assert_eq!(record.metadata.session_id, "my-session");
    assert_eq!(record.metadata.cwd, PathBuf::from("/tmp/test"));
    assert!(record.messages.is_empty());
}

#[test]
fn session_record_new_named_rejects_invalid_name() {
    let result = SessionRecord::new_named("invalid name!", PathBuf::from("/tmp/test"));
    assert!(result.is_err());
}

// ── Task 1.3: SessionStore list/delete tests ───────────────────────────

#[test]
fn session_store_list_sessions_empty_dir() {
    let root = unique_test_dir("list_empty");
    let session_dir = root.join(".anvil").join("sessions");
    std::fs::create_dir_all(&session_dir).expect("should create dir");
    let store = SessionStore::new(session_dir.join("default.json"), session_dir.clone());
    let sessions = store.list_sessions().expect("should list");
    assert!(sessions.is_empty());
}

#[test]
fn session_store_list_sessions_returns_session_info() {
    let root = unique_test_dir("list_sessions");
    let session_dir = root.join(".anvil").join("sessions");
    std::fs::create_dir_all(&session_dir).expect("should create dir");

    // Create two sessions
    let store1 = SessionStore::new(session_dir.join("alpha.json"), session_dir.clone());
    let mut s1 = SessionRecord::new_named("alpha", root.clone()).expect("named session");
    s1.push_message(new_user_message("m1", "hello"));
    store1.save(&s1).expect("save alpha");

    let store2 = SessionStore::new(session_dir.join("beta.json"), session_dir.clone());
    let s2 = SessionRecord::new_named("beta", root.clone()).expect("named session");
    store2.save(&s2).expect("save beta");

    let store = SessionStore::new(session_dir.join("default.json"), session_dir.clone());
    let mut sessions = store.list_sessions().expect("should list");
    sessions.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].name, "alpha");
    assert_eq!(sessions[0].message_count, 1);
    assert_eq!(sessions[1].name, "beta");
    assert_eq!(sessions[1].message_count, 0);
}

#[test]
fn session_store_list_sessions_ignores_non_json_files() {
    let root = unique_test_dir("list_nonjson");
    let session_dir = root.join(".anvil").join("sessions");
    std::fs::create_dir_all(&session_dir).expect("should create dir");

    // Create a valid session
    let store = SessionStore::new(session_dir.join("valid.json"), session_dir.clone());
    let s = SessionRecord::new_named("valid", root.clone()).expect("named session");
    store.save(&s).expect("save");

    // Create a non-json file
    std::fs::write(session_dir.join("notes.txt"), "not a session").expect("write txt");

    let sessions = store.list_sessions().expect("should list");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].name, "valid");
}

#[test]
fn session_store_delete_session_removes_file() {
    let root = unique_test_dir("delete_session");
    let session_dir = root.join(".anvil").join("sessions");
    std::fs::create_dir_all(&session_dir).expect("should create dir");

    let store = SessionStore::new(session_dir.join("default.json"), session_dir.clone());
    let target = session_dir.join("target.json");
    let s = SessionRecord::new_named("target", root.clone()).expect("named session");
    let target_store = SessionStore::new(target.clone(), session_dir.clone());
    target_store.save(&s).expect("save target");
    assert!(target.exists());

    store.delete_session("target").expect("should delete");
    assert!(!target.exists());
}

#[test]
fn session_store_delete_session_not_found() {
    let root = unique_test_dir("delete_notfound");
    let session_dir = root.join(".anvil").join("sessions");
    std::fs::create_dir_all(&session_dir).expect("should create dir");

    let store = SessionStore::new(session_dir.join("default.json"), session_dir.clone());
    let result = store.delete_session("nonexistent");
    assert!(result.is_err());
}

#[test]
fn session_store_delete_session_rejects_invalid_name() {
    let root = unique_test_dir("delete_invalid");
    let session_dir = root.join(".anvil").join("sessions");
    std::fs::create_dir_all(&session_dir).expect("should create dir");

    let store = SessionStore::new(session_dir.join("default.json"), session_dir.clone());
    let result = store.delete_session("../escape");
    assert!(result.is_err());
}

// ── Task 1.4: Migration tests ──────────────────────────────────────────

#[test]
fn session_store_migrates_old_hash_file_to_named() {
    let root = unique_test_dir("migration");
    let session_dir = root.join(".anvil").join("sessions");
    std::fs::create_dir_all(&session_dir).expect("should create dir");

    // Create an old-style session file using hash-based name
    let old_session = SessionRecord::new(root.clone());
    let old_key = &old_session.metadata.session_id; // session_<hash>
    let old_path = session_dir.join(format!("{old_key}.json"));
    let old_store = SessionStore::new(old_path.clone(), session_dir.clone());
    old_store.save(&old_session).expect("save old session");
    assert!(old_path.exists());

    // Now load with new-style default.json path
    let new_path = session_dir.join("default.json");
    let new_store = SessionStore::new(new_path.clone(), session_dir.clone());
    let migrated = new_store.load_or_create(&root).expect("should migrate");

    // Old file should be gone, new file should exist
    assert!(!old_path.exists());
    assert!(new_path.exists());
    // session_id should be updated to name-based
    assert_eq!(migrated.metadata.session_id, "default");
}

#[test]
fn session_store_skips_migration_when_new_file_exists() {
    let root = unique_test_dir("migration_skip");
    let session_dir = root.join(".anvil").join("sessions");
    std::fs::create_dir_all(&session_dir).expect("should create dir");

    // Create the new-style file
    let new_path = session_dir.join("default.json");
    let new_store = SessionStore::new(new_path.clone(), session_dir.clone());
    let session = SessionRecord::new_named("default", root.clone()).expect("named");
    new_store.save(&session).expect("save new");

    // load_or_create should load it directly, not migrate
    let loaded = new_store.load_or_create(&root).expect("should load");
    assert_eq!(loaded.metadata.session_id, "default");
}

#[test]
fn session_store_creates_new_named_session_when_no_old_file() {
    let root = unique_test_dir("migration_new");
    let session_dir = root.join(".anvil").join("sessions");
    std::fs::create_dir_all(&session_dir).expect("should create dir");

    let new_path = session_dir.join("my-project.json");
    let new_store = SessionStore::new(new_path.clone(), session_dir.clone());
    let created = new_store.load_or_create(&root).expect("should create new");

    assert!(new_path.exists());
    assert_eq!(created.metadata.session_id, "my-project");
}

// ── Task 6.2: Named session integration tests ───────────────────────────

#[test]
fn named_session_create_save_and_reload() {
    let root = unique_test_dir("named_roundtrip");
    let session_dir = root.join(".anvil").join("sessions");
    std::fs::create_dir_all(&session_dir).expect("should create dir");

    let store = SessionStore::new(session_dir.join("bugfix-42.json"), session_dir.clone());
    let mut session =
        SessionRecord::new_named("bugfix-42", root.clone()).expect("should create named session");
    session.push_message(new_user_message("m1", "fix the bug"));
    session.push_message(new_assistant_message(
        "m2",
        "investigating",
        MessageStatus::Committed,
    ));
    store.save(&session).expect("save should succeed");

    let reloaded = store.load().expect("reload should succeed");
    assert_eq!(reloaded.metadata.session_id, "bugfix-42");
    assert_eq!(reloaded.message_count(), 2);
    assert_eq!(reloaded.messages[0].content, "fix the bug");
}

#[test]
fn session_flag_config_sets_session_file() {
    use anvil::config::{CliArgs, EffectiveConfig};

    let mut config = EffectiveConfig::default_for_test().expect("config should load");
    let cli = CliArgs {
        session: Some("my-feature".to_string()),
        ..Default::default()
    };
    config.apply_cli_args(&cli).expect("apply should succeed");

    assert!(
        config.paths.session_file.ends_with("my-feature.json"),
        "session_file should end with my-feature.json, got: {:?}",
        config.paths.session_file
    );
}

#[test]
fn session_flag_config_rejects_invalid_name() {
    use anvil::config::{CliArgs, EffectiveConfig};

    let mut config = EffectiveConfig::default_for_test().expect("config should load");
    let cli = CliArgs {
        session: Some("../escape".to_string()),
        ..Default::default()
    };
    let result = config.apply_cli_args(&cli);
    assert!(result.is_err(), "invalid session name should be rejected");
}

// ── Task 3.1: parse_session_command tests ───────────────────────────────

#[test]
fn parse_session_command_list() {
    use anvil::extensions::{ExtensionRegistry, SlashCommandAction};

    let registry = ExtensionRegistry::new();
    let spec = registry
        .find_slash_command("/session")
        .expect("should find /session");
    assert_eq!(spec.action, SlashCommandAction::SessionList);

    let spec2 = registry
        .find_slash_command("/session list")
        .expect("should find /session list");
    assert_eq!(spec2.action, SlashCommandAction::SessionList);
}

#[test]
fn parse_session_command_switch() {
    use anvil::extensions::{ExtensionRegistry, SlashCommandAction};

    let registry = ExtensionRegistry::new();
    let spec = registry
        .find_slash_command("/session switch my-feature")
        .expect("should find /session switch");
    assert_eq!(
        spec.action,
        SlashCommandAction::SessionSwitch("my-feature".to_string())
    );
}

#[test]
fn parse_session_command_delete() {
    use anvil::extensions::{ExtensionRegistry, SlashCommandAction};

    let registry = ExtensionRegistry::new();
    let spec = registry
        .find_slash_command("/session delete old-work")
        .expect("should find /session delete");
    assert_eq!(
        spec.action,
        SlashCommandAction::SessionDelete("old-work".to_string())
    );
}

#[test]
fn parse_session_command_unknown_subcommand_returns_none() {
    use anvil::extensions::ExtensionRegistry;

    let registry = ExtensionRegistry::new();
    // Unknown subcommand should not match parse_session_command
    let result = registry.find_slash_command("/session foobar");
    assert!(result.is_none(), "unknown subcommand should not be matched");
}

// ── Task 5.1: render_resume_header session name test ────────────────────

#[test]
fn render_resume_header_includes_session_name() {
    use anvil::app::render::render_resume_header;
    use anvil::config::EffectiveConfig;

    let config = EffectiveConfig::default_for_test().expect("config should load");
    let output = render_resume_header(
        &config.runtime.model,
        config.runtime.context_window,
        &config,
        "my-feature",
    );
    assert!(
        output.contains("my-feature"),
        "resume header should include session name"
    );
    assert!(output.contains("Session"));
}

// ── Task 4.1: App current_session_name test ─────────────────────────────

#[test]
fn app_current_session_name_from_default() {
    let app = common::build_app();
    assert_eq!(app.current_session_name(), "default");
}

#[test]
fn app_current_session_name_from_custom_config() {
    let root = common::unique_test_dir("custom_session_name");
    let mut config = common::build_config_in(root);
    config.paths.session_file = config.paths.session_dir.join("my-work.json");

    let provider =
        anvil::provider::ProviderRuntimeContext::bootstrap(&config).expect("provider bootstrap");
    let shutdown_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let app = anvil::app::App::new(config, provider, shutdown_flag).expect("app should init");
    assert_eq!(app.current_session_name(), "my-work");
}

// ── Task 6.3: /session switch creates new session if non-existent ───────

#[test]
fn session_switch_to_nonexistent_creates_new_session() {
    let root = unique_test_dir("switch_new");
    let session_dir = root.join(".anvil").join("sessions");
    std::fs::create_dir_all(&session_dir).expect("should create dir");

    // Create an existing "default" session
    let store = SessionStore::new(session_dir.join("default.json"), session_dir.clone());
    let mut s = SessionRecord::new_named("default", root.clone()).expect("named");
    s.push_message(new_user_message("m1", "hello"));
    store.save(&s).expect("save default");

    // Verify "brand-new" does not yet exist
    let new_path = session_dir.join("brand-new.json");
    assert!(!new_path.exists());

    // Simulate what switch_session does for a non-existent name:
    // load_or_create or new_named when the file doesn't exist
    let new_store = SessionStore::new(new_path.clone(), session_dir.clone());
    let new_session =
        SessionRecord::new_named("brand-new", root.clone()).expect("should create new session");
    new_store.save(&new_session).expect("save new");

    assert!(new_path.exists());
    assert_eq!(new_session.metadata.session_id, "brand-new");
    assert_eq!(new_session.message_count(), 0);
}

// ── Task 6.4: /session switch rejected during PendingTurn ────────────────
// Verified at the SessionRecord level: has_pending_turn() returns true after
// set_pending_turn(). The App::switch_session() method checks this and returns
// Err(AppError::PendingApprovalRequired). switch_session() is pub(crate),
// so we test the invariant at the session level.

#[test]
fn session_record_has_pending_turn_blocks_conceptual_switch() {
    use anvil::agent::PendingTurnState;

    let mut session = SessionRecord::new(PathBuf::from("/tmp/anvil-pending-test"));
    assert!(!session.has_pending_turn());

    session.set_pending_turn(PendingTurnState {
        waiting_tool_call_id: "call_test".to_string(),
        remaining_events: vec![],
        pending_tool_calls: vec![],
    });
    assert!(
        session.has_pending_turn(),
        "has_pending_turn should be true after set_pending_turn"
    );

    // After clearing, switch would be allowed
    session.clear_pending_turn();
    assert!(
        !session.has_pending_turn(),
        "has_pending_turn should be false after clear_pending_turn"
    );
}

// ── Task 6.5: --session + --fresh-session creates new session ────────────

#[test]
fn session_and_fresh_session_flags_create_new_session() {
    use anvil::config::{CliArgs, EffectiveConfig};

    let mut config = EffectiveConfig::default_for_test().expect("config should load");
    let cli = CliArgs {
        session: Some("fresh-feature".to_string()),
        fresh_session: true,
        ..Default::default()
    };
    config.apply_cli_args(&cli).expect("apply should succeed");

    // session_file should point to fresh-feature.json
    assert!(
        config.paths.session_file.ends_with("fresh-feature.json"),
        "session_file should end with fresh-feature.json, got: {:?}",
        config.paths.session_file
    );
    // fresh_session flag should be set
    assert!(config.mode.fresh_session);
}

// ── Task 6.6: default session name when --session omitted ────────────────

#[test]
fn default_session_file_is_default_json() {
    use anvil::config::EffectiveConfig;

    let config = EffectiveConfig::default_for_test().expect("config should load");
    assert!(
        config.paths.session_file.ends_with("default.json"),
        "default session_file should end with default.json, got: {:?}",
        config.paths.session_file
    );
}

// ── Task 6.7: session list shows name, message count ─────────────────────

#[test]
fn session_list_shows_updated_at_ms() {
    let root = unique_test_dir("list_updated_at");
    let session_dir = root.join(".anvil").join("sessions");
    std::fs::create_dir_all(&session_dir).expect("should create dir");

    let store = SessionStore::new(session_dir.join("test-session.json"), session_dir.clone());
    let mut s = SessionRecord::new_named("test-session", root.clone()).expect("named");
    s.push_message(new_user_message("m1", "hello"));
    s.push_message(new_user_message("m2", "world"));
    store.save(&s).expect("save");

    let listing_store = SessionStore::new(session_dir.join("default.json"), session_dir.clone());
    let sessions = listing_store.list_sessions().expect("should list");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].name, "test-session");
    assert_eq!(sessions[0].message_count, 2);
    assert!(
        sessions[0].updated_at_ms > 0,
        "updated_at_ms should be non-zero"
    );
}

// ── Issue #80: Smart Compact tests ──────────────────────────────────────

#[test]
fn should_smart_compact_below_threshold() {
    let mut session = SessionRecord::new(PathBuf::from("/tmp/test"));
    session.smart_compact_threshold_ratio = 0.75;
    session.push_message(new_user_message("m1", "hello"));
    // 1 message, far below any threshold
    assert!(!session.should_smart_compact(200_000, None));
}

#[test]
fn should_smart_compact_above_threshold() {
    let mut session = SessionRecord::new(PathBuf::from("/tmp/test"));
    session.smart_compact_threshold_ratio = 0.75;
    // Fill with enough messages to exceed threshold for a small context window
    for i in 0..100 {
        session.push_message(new_user_message(
            format!("m{i}"),
            "a".repeat(200), // ~50 tokens each
        ));
    }
    // With context_window=1000, threshold = 750 tokens
    // 100 messages * ~50 tokens = ~5000 tokens > 750
    assert!(session.should_smart_compact(1000, None));
}

#[test]
fn should_smart_compact_ratio_zero_disabled() {
    let mut session = SessionRecord::new(PathBuf::from("/tmp/test"));
    session.smart_compact_threshold_ratio = 0.0;
    for i in 0..100 {
        session.push_message(new_user_message(format!("m{i}"), "a".repeat(200)));
    }
    // ratio=0.0 means smart compact is disabled
    assert!(!session.should_smart_compact(1000, None));
}

#[test]
fn should_smart_compact_small_context_window() {
    let mut session = SessionRecord::new(PathBuf::from("/tmp/test"));
    session.smart_compact_threshold_ratio = 0.75;
    // MIN_CONTEXT_WINDOW = 1000, threshold = 750
    session.push_message(new_user_message("m1", "a".repeat(400)));
    // ~100 tokens, below 750
    assert!(!session.should_smart_compact(1000, None));
}

/// Issue #200: should_smart_compact respects context_budget
#[test]
fn should_smart_compact_respects_context_budget() {
    let mut session = SessionRecord::new(PathBuf::from("/tmp/test"));
    session.smart_compact_threshold_ratio = 0.75;
    // Fill with messages to get ~5000 tokens
    for i in 0..100 {
        session.push_message(new_user_message(
            format!("m{i}"),
            "a".repeat(200), // ~50 tokens each
        ));
    }
    // context_window=262144 alone → threshold=196608, won't trigger
    assert!(!session.should_smart_compact(262_144, None));
    // With context_budget=4000 → effective=4000, threshold=3000 → 5000>3000 triggers
    assert!(session.should_smart_compact(262_144, Some(4000)));
    // context_budget larger than context_window → effective=context_window, no change
    assert!(!session.should_smart_compact(262_144, Some(300_000)));
}

#[test]
fn compute_importance_scores_system_role() {
    use anvil::session::compute_importance_scores;
    let messages = vec![SessionMessage::new(
        MessageRole::System,
        "anvil",
        "system prompt",
    )];
    let scores = compute_importance_scores(&messages, 1);
    assert_eq!(scores.len(), 1);
    // System gets SCORE_SYSTEM_PROMPT (10), no recency bonus when compact_end=1
    assert_eq!(scores[0], 10);
}

#[test]
fn compute_importance_scores_user_role() {
    use anvil::session::compute_importance_scores;
    let messages = vec![SessionMessage::new(MessageRole::User, "you", "hello")];
    let scores = compute_importance_scores(&messages, 1);
    assert_eq!(scores[0], 3); // SCORE_USER_INPUT
}

#[test]
fn compute_importance_scores_tool_role() {
    use anvil::session::compute_importance_scores;
    let messages = vec![SessionMessage::new(MessageRole::Tool, "tool", "result")];
    let scores = compute_importance_scores(&messages, 1);
    assert_eq!(scores[0], -2); // SCORE_TOOL_RESULT_PENALTY
}

#[test]
fn compute_importance_scores_error_bonus() {
    use anvil::session::compute_importance_scores;
    let mut msg = SessionMessage::new(MessageRole::Tool, "tool", "error result");
    msg.is_error = true;
    let messages = vec![msg];
    let scores = compute_importance_scores(&messages, 1);
    // SCORE_TOOL_RESULT_PENALTY(-2) + SCORE_ERROR_BONUS(5) = 3
    assert_eq!(scores[0], 3);
}

#[test]
fn compute_importance_scores_recency_bonus() {
    use anvil::session::compute_importance_scores;
    let messages: Vec<_> = (0..5)
        .map(|i| SessionMessage::new(MessageRole::Assistant, "anvil", format!("msg {i}")))
        .collect();
    let scores = compute_importance_scores(&messages, 5);
    // First message: recency = 10*0/4 = 0
    // Last message: recency = 10*4/4 = 10
    assert!(
        scores[4] > scores[0],
        "later messages should have higher recency bonus"
    );
}

#[test]
fn compute_importance_scores_compact_end_zero() {
    use anvil::session::compute_importance_scores;
    let messages = vec![SessionMessage::new(MessageRole::User, "you", "hello")];
    let scores = compute_importance_scores(&messages, 0);
    assert!(scores.is_empty());
}

#[test]
fn compute_importance_scores_compact_end_one() {
    use anvil::session::compute_importance_scores;
    let messages = vec![SessionMessage::new(MessageRole::User, "you", "hello")];
    let scores = compute_importance_scores(&messages, 1);
    assert_eq!(scores.len(), 1);
    // Only SCORE_USER_INPUT(3), no recency bonus when compact_end=1
    assert_eq!(scores[0], 3);
}

#[test]
fn compute_token_based_keep_recent_all_messages() {
    use anvil::session::compute_token_based_keep_recent;
    let messages: Vec<_> = (0..3)
        .map(|i| SessionMessage::new(MessageRole::User, "you", format!("msg {i}")))
        .collect();
    // Very high target: should keep all messages
    let keep = compute_token_based_keep_recent(&messages, 1_000_000);
    assert_eq!(keep, 3);
}

#[test]
fn compute_token_based_keep_recent_partial() {
    use anvil::session::compute_token_based_keep_recent;
    let mut messages = Vec::new();
    for i in 0..10 {
        messages.push(SessionMessage::new(
            MessageRole::User,
            "you",
            format!("message content number {i} with some text"),
        ));
    }
    // Small target: should keep fewer than all messages
    let keep = compute_token_based_keep_recent(&messages, 10);
    assert!(keep < 10, "should keep fewer than all messages");
    assert!(keep >= 1, "should keep at least 1 message");
}

#[test]
fn summarize_tool_result_above_threshold() {
    use anvil::session::summarize_tool_result;
    let msg = SessionMessage::new(MessageRole::Tool, "file.read", "x".repeat(3000));
    let result = summarize_tool_result(&msg);
    assert!(result.is_some());
    let summary = result.unwrap();
    assert!(summary.contains("[要約]"));
    assert!(summary.contains("file.read"));
}

#[test]
fn summarize_tool_result_below_threshold() {
    use anvil::session::summarize_tool_result;
    let msg = SessionMessage::new(MessageRole::Tool, "file.read", "short result");
    let result = summarize_tool_result(&msg);
    assert!(result.is_none());
}

#[test]
fn summarize_tool_result_non_tool_role() {
    use anvil::session::summarize_tool_result;
    let msg = SessionMessage::new(MessageRole::User, "you", "x".repeat(3000));
    let result = summarize_tool_result(&msg);
    assert!(result.is_none());
}

#[test]
fn summarize_tool_result_invalid_tool_name() {
    use anvil::session::summarize_tool_result;
    let msg = SessionMessage::new(
        MessageRole::Tool,
        "<script>alert</script>",
        "x".repeat(3000),
    );
    let result = summarize_tool_result(&msg);
    assert!(result.is_some());
    assert!(result.unwrap().contains("unknown_tool"));
}

#[test]
fn summarize_tool_result_empty_tool_name() {
    use anvil::session::summarize_tool_result;
    let msg = SessionMessage::new(MessageRole::Tool, "", "x".repeat(3000));
    let result = summarize_tool_result(&msg);
    assert!(result.is_some());
    assert!(result.unwrap().contains("unknown_tool"));
}

#[test]
fn replace_tool_results_with_summaries_mixed() {
    use anvil::session::replace_tool_results_with_summaries;
    let mut messages = vec![
        SessionMessage::new(MessageRole::User, "you", "hello"),
        SessionMessage::new(MessageRole::Tool, "file.read", "x".repeat(3000)),
        SessionMessage::new(MessageRole::Tool, "file.read", "short"),
        SessionMessage::new(MessageRole::Assistant, "anvil", "response"),
    ];
    replace_tool_results_with_summaries(&mut messages);
    // First tool message (large): should be summarized
    assert!(messages[1].content.contains("[要約]"));
    // Second tool message (short): should remain unchanged
    assert_eq!(messages[2].content, "short");
    // Non-tool messages: unchanged
    assert_eq!(messages[0].content, "hello");
    assert_eq!(messages[3].content, "response");
}

#[test]
fn mask_sensitive_in_command_api_key() {
    use anvil::session::mask_sensitive_in_command;
    let cmd = "curl -H 'Authorization: Bearer sk-abc123def' https://api.example.com";
    let masked = mask_sensitive_in_command(cmd);
    assert!(!masked.contains("sk-abc123def"));
    assert!(masked.contains("***"));
}

#[test]
fn mask_sensitive_in_command_no_sensitive() {
    use anvil::session::mask_sensitive_in_command;
    let cmd = "ls -la /tmp";
    let masked = mask_sensitive_in_command(cmd);
    assert_eq!(masked, "ls -la /tmp");
}

#[test]
fn mask_sensitive_in_command_ghp_prefix() {
    use anvil::session::mask_sensitive_in_command;
    let cmd = "git clone https://ghp_abcdef123456@github.com/user/repo";
    let masked = mask_sensitive_in_command(cmd);
    assert!(!masked.contains("ghp_abcdef123456"));
    assert!(masked.contains("***"));
}

#[test]
fn to_relative_path_matching_prefix() {
    use anvil::session::to_relative_path;
    let result = to_relative_path("/home/user/project/src/main.rs", "/home/user/project");
    assert_eq!(result, "src/main.rs");
}

#[test]
fn to_relative_path_no_matching_prefix() {
    use anvil::session::to_relative_path;
    let result = to_relative_path("/other/path/file.rs", "/home/user/project");
    assert_eq!(result, "/other/path/file.rs");
}

#[test]
fn clamp_smart_compact_ratio_nan() {
    let mut config = anvil::config::EffectiveConfig::default_for_test().expect("config");
    config.runtime.smart_compact_threshold_ratio = f64::NAN;
    config.clamp_smart_compact_ratio();
    assert!((config.runtime.smart_compact_threshold_ratio - 0.75).abs() < f64::EPSILON);
}

#[test]
fn clamp_smart_compact_ratio_infinity() {
    let mut config = anvil::config::EffectiveConfig::default_for_test().expect("config");
    config.runtime.smart_compact_threshold_ratio = f64::INFINITY;
    config.clamp_smart_compact_ratio();
    assert!((config.runtime.smart_compact_threshold_ratio - 0.75).abs() < f64::EPSILON);
}

#[test]
fn clamp_smart_compact_ratio_neg_infinity() {
    let mut config = anvil::config::EffectiveConfig::default_for_test().expect("config");
    config.runtime.smart_compact_threshold_ratio = f64::NEG_INFINITY;
    config.clamp_smart_compact_ratio();
    assert!((config.runtime.smart_compact_threshold_ratio - 0.75).abs() < f64::EPSILON);
}

#[test]
fn clamp_smart_compact_ratio_normal_value() {
    let mut config = anvil::config::EffectiveConfig::default_for_test().expect("config");
    config.runtime.smart_compact_threshold_ratio = 0.5;
    config.clamp_smart_compact_ratio();
    assert!((config.runtime.smart_compact_threshold_ratio - 0.5).abs() < f64::EPSILON);
}

#[test]
fn clamp_smart_compact_ratio_below_min() {
    let mut config = anvil::config::EffectiveConfig::default_for_test().expect("config");
    config.runtime.smart_compact_threshold_ratio = 0.01;
    config.clamp_smart_compact_ratio();
    assert!((config.runtime.smart_compact_threshold_ratio - 0.1).abs() < f64::EPSILON);
}

#[test]
fn clamp_smart_compact_ratio_above_max() {
    let mut config = anvil::config::EffectiveConfig::default_for_test().expect("config");
    config.runtime.smart_compact_threshold_ratio = 0.99;
    config.clamp_smart_compact_ratio();
    assert!((config.runtime.smart_compact_threshold_ratio - 0.95).abs() < f64::EPSILON);
}

#[test]
fn compact_history_with_smart_compact_generates_scored_summary() {
    let mut session = SessionRecord::new(PathBuf::from("/tmp/test"));
    // Add a system message (high score)
    session.push_message(
        SessionMessage::new(MessageRole::System, "anvil", "You are a helpful assistant")
            .with_id("sys_1"),
    );
    // Add user messages
    for i in 0..10 {
        session.push_message(new_user_message(format!("m{i}"), format!("question {i}")));
    }
    // Add a large tool result that should be summarized
    session.push_message(
        SessionMessage::new(MessageRole::Tool, "file.read", "x".repeat(3000)).with_id("tool_1"),
    );

    let changed = session.compact_history(4);
    assert!(changed);
    assert!(
        session.messages[0]
            .content
            .contains("[compacted session summary]")
    );
}

// ── Working Memory integration tests (Issue #130) ─────────────────────

#[test]
fn working_memory_serialize_roundtrip() {
    let mut session = SessionRecord::new(PathBuf::from("/tmp/wm_test"));
    session
        .working_memory
        .set_active_task(Some("implement #130".to_string()));
    session.working_memory.update_touched_files("src/main.rs");
    session.working_memory.add_error("file.edit: not found");
    session
        .working_memory
        .add_constraint("no unsafe".to_string());
    session
        .working_memory
        .set_recent_diffs(Some("diff content".to_string()));

    let json = serde_json::to_string(&session).expect("serialize");
    let restored: SessionRecord = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(session.working_memory, restored.working_memory);
}

#[test]
fn working_memory_backward_compat_old_json() {
    // Simulate a JSON from before Issue #130 — no working_memory field
    let old_json = r#"{
        "metadata": {
            "session_id": "test",
            "cwd": "/tmp",
            "created_at_ms": 0,
            "updated_at_ms": 0
        },
        "messages": [],
        "used_tools": []
    }"#;

    let record: SessionRecord = serde_json::from_str(old_json).expect("should deserialize");
    assert_eq!(record.working_memory, WorkingMemory::default());
    assert!(record.working_memory.is_empty());
}

#[test]
fn compact_history_preserves_working_memory_except_context_notice() {
    let mut session = SessionRecord::new(PathBuf::from("/tmp/wm_compact"));
    session
        .working_memory
        .set_active_task(Some("task A".to_string()));
    session.working_memory.update_touched_files("a.rs");
    session.working_memory.add_error("some error");
    session
        .working_memory
        .set_context_notice(Some("5 messages pruned".to_string()));

    // Add enough messages to trigger compaction
    for i in 0..20 {
        session.push_message(
            SessionMessage::new(MessageRole::User, "you", format!("msg {i}"))
                .with_id(format!("u_{i}")),
        );
        session.push_message(
            SessionMessage::new(MessageRole::Assistant, "anvil", format!("reply {i}"))
                .with_id(format!("a_{i}")),
        );
    }

    let compacted = session.compact_history(10);
    assert!(compacted);

    // Core working memory fields should be preserved after compaction
    assert_eq!(
        session.working_memory.active_task,
        Some("task A".to_string())
    );
    assert!(
        session
            .working_memory
            .touched_files
            .contains(&"a.rs".to_string())
    );
    assert!(
        session
            .working_memory
            .unresolved_errors
            .contains(&"some error".to_string())
    );

    // context_notice should be cleared after compaction (Issue #157)
    assert!(
        session.working_memory.context_notice.is_none(),
        "context_notice should be cleared after compact_history"
    );
}

#[test]
fn working_memory_context_notice_serialize_roundtrip() {
    let mut session = SessionRecord::new(PathBuf::from("/tmp/wm_notice_test"));
    session
        .working_memory
        .set_context_notice(Some("10 earlier messages pruned".to_string()));

    let json = serde_json::to_string(&session).expect("serialize");
    let restored: SessionRecord = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(
        session.working_memory.context_notice,
        restored.working_memory.context_notice
    );
}

// ── WorkingMemory record_tool_result behavior tests (Issue #157) ────────────

#[test]
fn working_memory_recent_diffs_accumulates_entries() {
    let mut wm = WorkingMemory::default();
    // Simulate record_tool_result accumulation logic
    let diff1 = "wrote 100 bytes to src/main.rs".to_string();
    wm.set_recent_diffs(Some(diff1.clone()));
    assert_eq!(
        wm.recent_diffs.as_deref(),
        Some("wrote 100 bytes to src/main.rs")
    );

    // Second diff appended
    let diff2 = "src/lib.rs: replaced 10 chars with 20 chars".to_string();
    let current = wm.recent_diffs.clone().unwrap_or_default();
    let updated = format!("{}\n{}", current, diff2);
    wm.set_recent_diffs(Some(updated));
    let diffs = wm.recent_diffs.as_ref().expect("should have diffs");
    assert!(diffs.contains(&diff1), "should still contain first diff");
    assert!(diffs.contains(&diff2), "should contain second diff");
}

#[test]
fn working_memory_recent_diffs_truncation_keeps_latest() {
    let mut wm = WorkingMemory::default();
    // Create old + new diffs where total exceeds limit
    let old_diff = "a".repeat(3000);
    let new_diff = "b".repeat(3000);
    let combined = format!("{}\n{}", old_diff, new_diff);
    wm.set_recent_diffs(Some(combined));
    let diffs = wm.recent_diffs.as_ref().expect("should have diffs");
    // After CB-003 fix, truncation keeps the tail (latest content)
    assert!(
        diffs.starts_with("[truncated]..."),
        "should have truncation prefix"
    );
    // The latest content (b's) should be preserved more than old content (a's)
    let b_count = diffs.chars().filter(|c| *c == 'b').count();
    let a_count = diffs.chars().filter(|c| *c == 'a').count();
    assert!(
        b_count > a_count,
        "latest diffs (b's) should be preserved over old diffs (a's): b={b_count}, a={a_count}"
    );
}

#[test]
fn working_memory_touched_files_updated_for_file_edit_anchor_pattern() {
    let mut wm = WorkingMemory::default();
    // Simulate what record_tool_result does for file.edit_anchor
    // The artifacts contain the resolved path; update_touched_files takes relative path
    wm.update_touched_files("src/tooling/mod.rs");
    assert!(
        wm.touched_files.contains(&"src/tooling/mod.rs".to_string()),
        "touched_files should contain the edited file"
    );
    // Verify deduplication
    wm.update_touched_files("src/tooling/mod.rs");
    let count = wm
        .touched_files
        .iter()
        .filter(|f| *f == "src/tooling/mod.rs")
        .count();
    assert_eq!(count, 1, "touched_files should deduplicate entries");
}

#[test]
fn working_memory_format_includes_recent_diffs_section() {
    let mut wm = WorkingMemory::default();
    wm.set_recent_diffs(Some("wrote 50 bytes to test.txt".to_string()));
    let prompt = wm.format_for_prompt().expect("should produce prompt");
    assert!(
        prompt.contains("**Recent diffs:**"),
        "prompt should include recent diffs section"
    );
    assert!(
        prompt.contains("wrote 50 bytes to test.txt"),
        "prompt should include the diff content"
    );
}

// ── Task 2.1: build_conversation_text_for_summary tests ──────────────────

#[test]
fn build_conversation_text_for_summary_basic() {
    let messages = vec![
        SessionMessage::new(MessageRole::User, "you", "Hello, please help me"),
        SessionMessage::new(MessageRole::Assistant, "anvil", "Sure, I can help"),
    ];
    let text = build_conversation_text_for_summary(&messages, 50, 500, 8000);
    assert!(text.contains("user: Hello, please help me"));
    assert!(text.contains("assistant: Sure, I can help"));
}

#[test]
fn build_conversation_text_for_summary_cjk_safe() {
    // CJK characters: each is a single char but multi-byte in UTF-8
    let cjk_content = "日本語のテスト文字列です。これは長い文章で切り詰めをテストします。";
    let messages = vec![SessionMessage::new(MessageRole::User, "you", cjk_content)];
    // max_chars_per_msg = 10: should safely truncate CJK at char boundary
    let text = build_conversation_text_for_summary(&messages, 50, 10, 8000);
    assert!(!text.is_empty());
    // Should not panic on UTF-8 boundary
    let char_count: usize = text.lines().map(|l| l.chars().count()).sum();
    assert!(char_count > 0);
}

#[test]
fn build_conversation_text_for_summary_max_limits() {
    let mut messages = Vec::new();
    for i in 0..100 {
        messages.push(SessionMessage::new(
            MessageRole::User,
            "you",
            format!("Message number {i} with some content"),
        ));
    }
    // max_messages = 5: should only include last 5
    let text = build_conversation_text_for_summary(&messages, 5, 500, 8000);
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines.len() <= 5, "should respect max_messages limit");

    // max_total_chars: should respect total character limit
    let text2 = build_conversation_text_for_summary(&messages, 50, 500, 100);
    assert!(
        text2.chars().count() <= 200,
        "should approximately respect max_total_chars"
    );
}

#[test]
fn conversation_text_for_summary_delegates() {
    let mut session = SessionRecord::new(PathBuf::from("/tmp/conv-text-delegate"));
    session.push_message(SessionMessage::new(
        MessageRole::User,
        "you",
        "test message",
    ));
    let text = session.conversation_text_for_summary(50, 500, 8000);
    assert!(text.contains("user: test message"));
}

#[test]
fn build_conversation_text_for_summary_with_increased_limits() {
    // Simulate a file.read result containing code with function signatures
    let code_content = r#"use std::collections::HashMap;

pub struct PromptDetectionResult {
    pub detected: bool,
    pub confidence: f64,
}

pub fn detect_prompt(output: &str, options: Option<&DetectPromptOptions>) -> PromptDetectionResult {
    // implementation details...
    PromptDetectionResult { detected: false, confidence: 0.0 }
}

pub fn process_tokens(input: &[u8], max_length: usize) -> Result<Vec<Token>, TokenError> {
    // implementation details...
    Ok(vec![])
}

impl TokenProcessor {
    pub fn new(config: &Config) -> Self {
        Self { config: config.clone() }
    }

    pub fn validate(&self, tokens: &[Token]) -> bool {
        tokens.iter().all(|t| t.is_valid())
    }
}"#;

    let messages = vec![
        SessionMessage::new(MessageRole::User, "you", "Read src/detection.rs"),
        SessionMessage::new(MessageRole::Tool, "tool", code_content),
        SessionMessage::new(
            MessageRole::Assistant,
            "anvil",
            "I'll modify the detect_prompt function to add logging.",
        ),
    ];

    // With increased limits: max_chars_per_msg=1000, max_total_chars=12000
    let text = build_conversation_text_for_summary(&messages, 50, 1000, 12000);

    // Function signatures should be preserved within 1000 chars
    assert!(
        text.contains("detect_prompt"),
        "function signature detect_prompt should be preserved with 1000 char limit"
    );
    assert!(
        text.contains("PromptDetectionResult"),
        "type name PromptDetectionResult should be preserved with 1000 char limit"
    );
    assert!(
        text.contains("process_tokens"),
        "function signature process_tokens should be preserved with 1000 char limit"
    );

    // Verify the old limit (500) loses later signatures while new limit preserves them.
    // The code_content is ~680 chars, so at 500 chars the `validate` method near
    // the end should be truncated, while at 1000 chars it should be preserved.
    let text_old = build_conversation_text_for_summary(&messages, 50, 500, 8000);
    assert!(
        !text_old.contains("validate"),
        "with 500 char limit, the validate method near end of code should be truncated"
    );
    assert!(
        text.contains("validate"),
        "with 1000 char limit, the validate method should be preserved"
    );
}

// ── Task 2.2: extract_file_targets tests ─────────────────────────────────

#[test]
fn extract_file_targets_finds_paths() {
    let messages = vec![
        SessionMessage::new(
            MessageRole::User,
            "you",
            "Please review src/provider/openai.rs and fix the bug",
        ),
        SessionMessage::new(
            MessageRole::Tool,
            "tool",
            "file.write wrote ./sandbox/demo/test.html",
        ),
    ];
    let targets = extract_file_targets(&messages);
    assert!(
        targets.iter().any(|t| t.contains("src/provider/openai.rs")),
        "should find src/provider/openai.rs in targets: {:?}",
        targets
    );
}

#[test]
fn extract_file_targets_empty_messages() {
    let messages: Vec<SessionMessage> = Vec::new();
    let targets = extract_file_targets(&messages);
    assert!(targets.is_empty());
}

// ── Task 2.3: compact_history_with_llm_summary tests ─────────────────────

#[test]
fn compact_history_with_llm_summary_uses_llm_text() {
    let mut session = SessionRecord::new(PathBuf::from("/tmp/llm-summary-test"));
    // Add enough messages to allow compaction
    for i in 0..15 {
        session.push_message(new_user_message(
            format!("msg_{i:03}"),
            format!("discuss topic {i}"),
        ));
    }

    let llm_summary = Some("LLM generated bullet points summary here".to_string());
    let changed = session.compact_history_with_llm_summary(5, llm_summary);

    assert!(changed);
    assert!(
        session.messages[0]
            .content
            .contains("LLM generated bullet points summary here"),
        "summary should contain LLM text, got: {}",
        session.messages[0].content
    );
}

#[test]
fn compact_history_with_llm_summary_preserves_file_targets() {
    let mut session = SessionRecord::new(PathBuf::from("/tmp/llm-file-targets"));
    session.push_message(new_user_message("msg_001", "edit src/main.rs"));
    session.push_message(
        SessionMessage::new(
            MessageRole::Tool,
            "tool",
            "file.write wrote ./src/config/mod.rs",
        )
        .with_id("tool_001"),
    );
    for i in 2..15 {
        session.push_message(new_user_message(
            format!("msg_{i:03}"),
            format!("follow up {i}"),
        ));
    }

    let llm_summary = Some("Bullet point summary".to_string());
    let changed = session.compact_history_with_llm_summary(5, llm_summary);

    assert!(changed);
    // Should contain file references even with LLM summary
    let content = &session.messages[0].content;
    assert!(
        content.contains("src/main.rs") || content.contains("src/config/mod.rs"),
        "should preserve file targets in summary, got: {}",
        content
    );
}

#[test]
fn compact_history_with_llm_summary_none_falls_back() {
    let mut session = SessionRecord::new(PathBuf::from("/tmp/llm-none-fallback"));
    session.push_message(new_user_message(
        "msg_001",
        "inspect src/provider/openai.rs",
    ));
    for i in 1..15 {
        session.push_message(new_user_message(
            format!("msg_{i:03}"),
            format!("follow up {i}"),
        ));
    }

    let changed = session.compact_history_with_llm_summary(5, None);

    assert!(changed);
    // Should use rule-based summary (contains [compacted session summary])
    assert!(
        session.messages[0]
            .content
            .contains("[compacted session summary]"),
        "None should fall back to rule-based summary"
    );
}

#[test]
fn compact_history_public_signature_unchanged() {
    let mut session = SessionRecord::new(PathBuf::from("/tmp/sig-check"));
    for i in 0..15 {
        session.push_message(new_user_message(
            format!("msg_{i:03}"),
            format!("message {i}"),
        ));
    }

    // Verify compact_history() still works with original signature (keep_recent: usize) -> bool
    let changed: bool = session.compact_history(5);
    assert!(changed);
}

// ── Issue #219: Session Note tests ──────────────────────────────────

#[test]
fn test_session_note_serde_roundtrip() {
    let note = SessionNote {
        source_index: 5,
        category: NoteCategory::TaskProgress,
        content: "Implement feature #219".to_string(),
    };
    let json = serde_json::to_string(&note).expect("serialize");
    let restored: SessionNote = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(note, restored);

    // Also test Constraint category
    let constraint_note = SessionNote {
        source_index: 10,
        category: NoteCategory::Constraint,
        content: "Build failed: missing dependency".to_string(),
    };
    let json2 = serde_json::to_string(&constraint_note).expect("serialize");
    let restored2: SessionNote = serde_json::from_str(&json2).expect("deserialize");
    assert_eq!(constraint_note, restored2);
}

#[test]
fn test_working_memory_backward_compat() {
    // JSON without session_notes or last_note_message_index should deserialize fine
    let json = r#"{"active_task":null,"constraints":[],"touched_files":[],"unresolved_errors":[],"recent_diffs":null,"context_notice":null}"#;
    let wm: WorkingMemory = serde_json::from_str(json).expect("deserialize");
    assert!(wm.session_notes.is_empty());
    assert_eq!(wm.last_note_message_index, 0);
}

#[test]
fn test_last_note_message_index_backward_compat() {
    // JSON without last_note_message_index field should default to 0
    let json = r#"{"active_task":null,"constraints":[],"touched_files":[],"unresolved_errors":[],"recent_diffs":null,"context_notice":null,"session_notes":[]}"#;
    let wm: WorkingMemory = serde_json::from_str(json).expect("deserialize");
    assert_eq!(wm.last_note_message_index, 0);
}

#[test]
fn test_is_empty_with_session_notes() {
    let mut wm = WorkingMemory::default();
    assert!(wm.is_empty());

    wm.session_notes.push(SessionNote {
        source_index: 0,
        category: NoteCategory::TaskProgress,
        content: "test".to_string(),
    });
    assert!(!wm.is_empty());
}

#[test]
fn test_add_session_note_normalizes_and_redacts() {
    let mut wm = WorkingMemory::default();

    // Normal note should be added
    wm.add_session_note(SessionNote {
        source_index: 0,
        category: NoteCategory::TaskProgress,
        content: "Implement feature".to_string(),
    });
    assert_eq!(wm.session_notes.len(), 1);
    assert_eq!(wm.session_notes[0].content, "Implement feature");

    // Note with sensitive content should be redacted entirely
    wm.add_session_note(SessionNote {
        source_index: 1,
        category: NoteCategory::Constraint,
        content: "Bearer sk-1234567890abcdef".to_string(),
    });
    assert_eq!(wm.session_notes.len(), 2);
    assert_eq!(wm.session_notes[1].content, "[redacted sensitive value]");

    // Empty note should be rejected
    wm.add_session_note(SessionNote {
        source_index: 2,
        category: NoteCategory::TaskProgress,
        content: "   ".to_string(),
    });
    assert_eq!(wm.session_notes.len(), 2); // no new note added

    // Note with ANVIL_TOOL markers should be sanitized
    wm.add_session_note(SessionNote {
        source_index: 3,
        category: NoteCategory::TaskProgress,
        content: "ANVIL_TOOL some task".to_string(),
    });
    assert_eq!(wm.session_notes.len(), 3);
    assert!(!wm.session_notes[2].content.contains("ANVIL_TOOL"));

    // Long note should be truncated to 200 chars
    let long_content = "x".repeat(300);
    wm.add_session_note(SessionNote {
        source_index: 4,
        category: NoteCategory::TaskProgress,
        content: long_content,
    });
    assert_eq!(wm.session_notes.len(), 4);
    assert!(wm.session_notes[3].content.chars().count() <= 200);
}

#[test]
fn test_add_session_note_fifo_eviction() {
    let mut wm = WorkingMemory::default();
    for i in 0..25 {
        wm.add_session_note(SessionNote {
            source_index: i,
            category: NoteCategory::TaskProgress,
            content: format!("note {i}"),
        });
    }
    assert_eq!(wm.session_notes.len(), WorkingMemory::MAX_SESSION_NOTES);
    // Oldest notes (0..5) should have been evicted
    assert_eq!(wm.session_notes[0].source_index, 5);
    assert_eq!(wm.session_notes.last().unwrap().source_index, 24);
}

#[test]
fn test_add_session_note_dedup() {
    let mut wm = WorkingMemory::default();
    wm.add_session_note(SessionNote {
        source_index: 5,
        category: NoteCategory::TaskProgress,
        content: "first version".to_string(),
    });
    wm.add_session_note(SessionNote {
        source_index: 5,
        category: NoteCategory::TaskProgress,
        content: "updated version".to_string(),
    });
    // Should have replaced, not added
    assert_eq!(wm.session_notes.len(), 1);
    assert_eq!(wm.session_notes[0].content, "updated version");

    // Different category at same index should coexist
    wm.add_session_note(SessionNote {
        source_index: 5,
        category: NoteCategory::Constraint,
        content: "some constraint".to_string(),
    });
    assert_eq!(wm.session_notes.len(), 2);
}

#[test]
fn test_format_for_prompt_includes_notes() {
    let mut wm = WorkingMemory::default();
    wm.add_session_note(SessionNote {
        source_index: 0,
        category: NoteCategory::TaskProgress,
        content: "Implement issue #219".to_string(),
    });
    wm.add_session_note(SessionNote {
        source_index: 3,
        category: NoteCategory::Constraint,
        content: "cargo build requires nightly".to_string(),
    });

    let prompt = wm.format_for_prompt().expect("should produce prompt");
    assert!(prompt.contains("Session notes:"));
    assert!(prompt.contains("[progress] Implement issue #219"));
    assert!(prompt.contains("[constraint] cargo build requires nightly"));
}

#[test]
fn test_note_category_label() {
    assert_eq!(NoteCategory::TaskProgress.label(), "progress");
    assert_eq!(NoteCategory::Constraint.label(), "constraint");
}

#[test]
fn test_extract_task_progress() {
    let messages = vec![
        new_user_message("u1", "Please implement the session memory feature"),
        SessionMessage::new(MessageRole::Assistant, "anvil", "I will implement it now"),
    ];
    let notes = extract_session_notes(&messages, 0, &[]);
    let progress_notes: Vec<_> = notes
        .iter()
        .filter(|n| n.category == NoteCategory::TaskProgress)
        .collect();
    assert_eq!(progress_notes.len(), 1);
    assert!(
        progress_notes[0]
            .content
            .contains("implement the session memory")
    );
}

#[test]
fn test_extract_constraints_no_dup() {
    let mut error_msg = SessionMessage::new(
        MessageRole::Tool,
        "bash",
        "error: missing dependency libssl",
    );
    error_msg.is_error = true;

    let messages = vec![new_user_message("u1", "run the build"), error_msg];

    // Without matching unresolved_errors: should extract
    let notes = extract_session_notes(&messages, 0, &[]);
    let constraint_notes: Vec<_> = notes
        .iter()
        .filter(|n| n.category == NoteCategory::Constraint)
        .collect();
    assert_eq!(constraint_notes.len(), 1);

    // With matching unresolved_errors: should NOT extract (dedup)
    let unresolved = vec!["error: missing dependency libssl".to_string()];
    let notes2 = extract_session_notes(&messages, 0, &unresolved);
    let constraint_notes2: Vec<_> = notes2
        .iter()
        .filter(|n| n.category == NoteCategory::Constraint)
        .collect();
    assert_eq!(constraint_notes2.len(), 0);
}

#[test]
fn test_extract_from_index_window() {
    let messages = vec![
        new_user_message("u1", "first task"),
        SessionMessage::new(MessageRole::Assistant, "anvil", "done with first"),
        new_user_message("u2", "second task"),
        SessionMessage::new(MessageRole::Assistant, "anvil", "done with second"),
    ];

    // Extract from index 2: should only see "second task"
    let notes = extract_session_notes(&messages, 2, &[]);
    let progress: Vec<_> = notes
        .iter()
        .filter(|n| n.category == NoteCategory::TaskProgress)
        .collect();
    assert_eq!(progress.len(), 1);
    assert!(progress[0].content.contains("second task"));
    assert_eq!(progress[0].source_index, 2);
}

#[test]
fn test_extract_from_beyond_messages_returns_empty() {
    let messages = vec![new_user_message("u1", "hello")];
    let notes = extract_session_notes(&messages, 100, &[]);
    assert!(notes.is_empty());
}

#[test]
fn test_compact_adjusts_last_note_index() {
    let mut session = SessionRecord::new(PathBuf::from("/tmp/compact-note-idx"));
    for i in 0..20 {
        session.push_message(new_user_message(
            format!("msg_{i:03}"),
            format!("message {i}"),
        ));
    }

    // Case A: last_note_message_index within compacted range
    session.working_memory.last_note_message_index = 5;
    session.compact_history(10); // split_at = 10
    // Should be set to 1 (skip summary at index 0)
    assert_eq!(session.working_memory.last_note_message_index, 1);

    // Reset for Case B
    let mut session2 = SessionRecord::new(PathBuf::from("/tmp/compact-note-idx2"));
    for i in 0..20 {
        session2.push_message(new_user_message(
            format!("msg_{i:03}"),
            format!("message {i}"),
        ));
    }
    // Case B: last_note_message_index beyond compacted range
    session2.working_memory.last_note_message_index = 15;
    session2.compact_history(10); // split_at = 10
    // Should be 15 - 10 + 1 = 6
    assert_eq!(session2.working_memory.last_note_message_index, 6);
}

#[test]
fn test_session_store_load_clamps_session_notes() {
    let dir = common::unique_test_dir("clamp_notes");
    std::fs::create_dir_all(&dir).unwrap();
    let session_dir = dir.join("sessions");
    std::fs::create_dir_all(&session_dir).unwrap();

    // Create a session with too many notes
    let mut session = SessionRecord::new(dir.clone());
    for i in 0..30 {
        session.working_memory.session_notes.push(SessionNote {
            source_index: i,
            category: NoteCategory::TaskProgress,
            content: format!("note {i}"),
        });
    }

    let store = SessionStore::new(session_dir.join("default.json"), session_dir.clone());
    store.save(&session).unwrap();

    let loaded = store.load().unwrap();
    assert_eq!(
        loaded.working_memory.session_notes.len(),
        WorkingMemory::MAX_SESSION_NOTES
    );
    // Should have kept the last 20 (FIFO: first 10 drained)
    assert_eq!(loaded.working_memory.session_notes[0].source_index, 10);
}

#[test]
fn test_session_store_load_clamps_last_note_index() {
    let dir = common::unique_test_dir("clamp_idx");
    std::fs::create_dir_all(&dir).unwrap();
    let session_dir = dir.join("sessions");
    std::fs::create_dir_all(&session_dir).unwrap();

    let mut session = SessionRecord::new(dir.clone());
    session.push_message(new_user_message("u1", "hello"));
    // Set index beyond messages.len()
    session.working_memory.last_note_message_index = 999;

    let store = SessionStore::new(session_dir.join("default.json"), session_dir.clone());
    store.save(&session).unwrap();

    let loaded = store.load().unwrap();
    assert_eq!(
        loaded.working_memory.last_note_message_index,
        loaded.messages.len()
    );
}

#[test]
fn test_notes_survive_compaction() {
    let mut session = SessionRecord::new(PathBuf::from("/tmp/notes-survive"));
    for i in 0..20 {
        session.push_message(new_user_message(
            format!("msg_{i:03}"),
            format!("message {i}"),
        ));
    }
    session.working_memory.add_session_note(SessionNote {
        source_index: 3,
        category: NoteCategory::TaskProgress,
        content: "important note".to_string(),
    });

    session.compact_history(10);

    // Session notes should survive compaction
    assert_eq!(session.working_memory.session_notes.len(), 1);
    assert_eq!(
        session.working_memory.session_notes[0].content,
        "important note"
    );
}

#[test]
fn test_notes_in_system_prompt() {
    let mut wm = WorkingMemory::default();
    wm.add_session_note(SessionNote {
        source_index: 0,
        category: NoteCategory::TaskProgress,
        content: "Working on issue #219".to_string(),
    });
    let prompt = wm.format_for_prompt().unwrap();
    assert!(prompt.contains("Session notes:"));
    assert!(prompt.contains("[progress] Working on issue #219"));
}

#[test]
fn test_resume_preserves_notes() {
    let dir = common::unique_test_dir("resume_notes");
    std::fs::create_dir_all(&dir).unwrap();
    let session_dir = dir.join("sessions");
    std::fs::create_dir_all(&session_dir).unwrap();

    let mut session = SessionRecord::new(dir.clone());
    session.working_memory.add_session_note(SessionNote {
        source_index: 5,
        category: NoteCategory::TaskProgress,
        content: "ongoing work".to_string(),
    });
    session.working_memory.last_note_message_index = 10;

    let store = SessionStore::new(session_dir.join("default.json"), session_dir.clone());
    store.save(&session).unwrap();

    let loaded = store.load().unwrap();
    assert_eq!(loaded.working_memory.session_notes.len(), 1);
    assert_eq!(
        loaded.working_memory.session_notes[0].content,
        "ongoing work"
    );
    // last_note_message_index was 10, but messages.len() is 0, so clamped
    assert_eq!(loaded.working_memory.last_note_message_index, 0);
}

#[test]
fn test_sensitive_tool_error_is_redacted_or_dropped() {
    let mut wm = WorkingMemory::default();

    // Bearer token should be redacted
    wm.add_session_note(SessionNote {
        source_index: 0,
        category: NoteCategory::Constraint,
        content: "Authorization: Bearer abc123def456".to_string(),
    });
    assert_eq!(wm.session_notes[0].content, "[redacted sensitive value]");

    // API key pattern should be redacted
    wm.add_session_note(SessionNote {
        source_index: 1,
        category: NoteCategory::Constraint,
        content: "api_key=sk_live_12345678".to_string(),
    });
    assert_eq!(wm.session_notes[1].content, "[redacted sensitive value]");
}
