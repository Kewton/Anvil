use anvil::agent::recovery::{
    ActionExpectation, classify_action_expectation, empty_response_recovery_note,
    install_loop_recovery_note, is_dependency_install_command, no_tool_recovery_note,
    repeated_bash_error, repo_change_recovery_note, should_block_bash_command,
    tool_call_counts_as_repo_edit, user_prompt_requires_action,
};
use anvil::modes::plan_act::ExecutionMode;

#[test]
fn detects_action_prompts_in_english_and_japanese() {
    assert!(user_prompt_requires_action(
        "Please edit src/main.rs and fix the bug",
        ExecutionMode::Act
    ));
    assert!(user_prompt_requires_action(
        "README を修正して",
        ExecutionMode::Act
    ));
    assert_eq!(
        classify_action_expectation(
            "最高に面白いゲームを next.js で開発してください",
            ExecutionMode::Act
        ),
        ActionExpectation::RepoChange
    );
    assert_eq!(
        classify_action_expectation("テストを実行して", ExecutionMode::Act),
        ActionExpectation::ToolAction
    );
    assert!(!user_prompt_requires_action(
        "Explain the architecture",
        ExecutionMode::Act
    ));
    assert!(!user_prompt_requires_action(
        "plan the work",
        ExecutionMode::Plan
    ));
}

#[test]
fn recovery_notes_are_non_empty() {
    assert!(empty_response_recovery_note(1, true).contains("attempt=1"));
    assert!(no_tool_recovery_note(2).contains("no_tool_attempt=2"));
    assert!(repo_change_recovery_note(3).contains("repo_change_attempt=3"));
    assert!(install_loop_recovery_note().contains("Stop reinstalling packages"));
    assert!(repeated_bash_error("npm install jest").contains("repeated Bash command"));
    assert!(tool_call_counts_as_repo_edit("Write"));
    assert!(tool_call_counts_as_repo_edit("Edit"));
    assert!(!tool_call_counts_as_repo_edit("Bash"));
}

#[test]
fn detects_dependency_install_loops() {
    assert!(is_dependency_install_command(
        "npm install --save-dev jest ts-jest"
    ));
    assert!(should_block_bash_command(
        "npm install --save-dev jest",
        &[],
        2
    ));
    assert!(should_block_bash_command(
        "npm install --save-dev jest",
        &["npm install --save-dev jest".to_string()],
        0
    ));
    assert!(!should_block_bash_command("npm test", &[], 0));
}
