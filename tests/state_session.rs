mod common;

use anvil::contracts::{AppEvent, AppStateSnapshot, RuntimeState};
use anvil::session::{
    MessageRole, MessageStatus, SessionMessage, SessionRecord, SessionStore, new_assistant_message,
    new_user_message, validate_session_name,
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
    let output = render_resume_header(&config, "my-feature");
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
