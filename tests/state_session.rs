mod common;

use anvil::contracts::{AppEvent, AppStateSnapshot, RuntimeState};
use anvil::session::{
    MessageStatus, SessionRecord, SessionStore, new_assistant_message, new_user_message,
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
