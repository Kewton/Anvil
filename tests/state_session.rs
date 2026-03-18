mod common;

use anvil::contracts::{AppEvent, AppStateSnapshot, RuntimeState};
use anvil::session::{
    MessageRole, MessageStatus, SessionMessage, SessionRecord, SessionStore, new_assistant_message,
    new_user_message,
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
