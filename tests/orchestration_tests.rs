use anvil::agent::orchestration::{
    FailureClass, capture_repo_snapshot, classify_failure, should_restart_actor,
    strip_restart_heavy_messages, verify_repo_progress,
};
use anvil::session::store::ConversationMessage;
use tempfile::tempdir;

#[test]
fn classifies_failures_for_actor_restart() {
    assert_eq!(
        classify_failure("failed to contact Ollama chat API: error sending request for url"),
        FailureClass::Transport
    );
    assert_eq!(
        classify_failure("assistant did not finish within max iterations"),
        FailureClass::MaxIterations
    );
    assert_eq!(
        classify_failure("assistant kept stopping before making the requested repository edits"),
        FailureClass::Incomplete
    );
    assert!(should_restart_actor(FailureClass::Transport));
    assert!(!should_restart_actor(FailureClass::Other));
}

#[test]
fn deterministic_verifier_reports_changed_file_categories() {
    let temp = tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("src")).unwrap();
    std::fs::write(temp.path().join("package.json"), "{}").unwrap();
    let before = capture_repo_snapshot(temp.path());

    std::fs::write(temp.path().join("package.json"), "{\"test\":1}").unwrap();
    std::fs::write(
        temp.path().join("src/app.tsx"),
        "export default function App() {}",
    )
    .unwrap();
    std::fs::write(temp.path().join("src/app.test.ts"), "test('ok', () => {})").unwrap();

    let verification = verify_repo_progress(&before, temp.path());
    assert_eq!(verification.setup_files_changed, 1);
    assert_eq!(verification.implementation_files_changed, 1);
    assert_eq!(verification.test_files_changed, 1);
    assert!(verification.made_any_progress());
    assert!(verification.summary_note().contains("Verifier: changed="));
    assert!(
        verification
            .restart_note()
            .contains("Continue from the changed files")
    );
}

#[test]
fn strips_compact_summary_before_actor_restart() {
    let mut messages = vec![
        ConversationMessage::system("[compact-summary]\nold".to_string()),
        ConversationMessage::system("keep".to_string()),
        ConversationMessage::user("task".to_string()),
    ];
    strip_restart_heavy_messages(&mut messages, 3);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].content, "keep");
}
