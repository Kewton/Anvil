use anvil::agent::recovery::{
    ActionExpectation, artifact_directed_recovery_note, broad_restart_discovery_error,
    classify_action_expectation, empty_response_recovery_note, empty_workspace_scaffold_note,
    first_scaffold_shell_edit_note, install_loop_recovery_note, is_dependency_install_command,
    is_scaffold_command, is_workspace_reset_command, no_tool_recovery_note, repeated_bash_error,
    repeated_plan_exploration_error, repo_change_after_setup_note,
    repo_change_no_tool_recovery_note, repo_change_recovery_note, should_block_bash_command,
    should_block_restart_discovery, tool_call_counts_as_repo_edit, user_prompt_requires_action,
};
use anvil::modes::plan_act::{ExecutionMode, PlanStage};

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
    assert!(user_prompt_requires_action(
        "plan the work",
        ExecutionMode::Plan
    ));
    assert_eq!(
        classify_action_expectation("plan the work", ExecutionMode::Plan),
        ActionExpectation::PlanProgress
    );
}

#[test]
fn recovery_notes_are_non_empty() {
    assert!(empty_response_recovery_note(1, true).contains("attempt=1"));
    assert!(no_tool_recovery_note(2).contains("no_tool_attempt=2"));
    assert!(repo_change_recovery_note(3).contains("repo_change_attempt=3"));
    assert!(repo_change_no_tool_recovery_note(4).contains("exactly one tool call"));
    assert!(repo_change_after_setup_note().contains("small Edit"));
    assert!(empty_workspace_scaffold_note().contains("Do not inspect it again with ls"));
    assert!(
        first_scaffold_shell_edit_note("src/app/page.tsx").contains("compact task-specific title")
    );
    assert!(install_loop_recovery_note().contains("Stop reinstalling packages"));
    assert!(repeated_bash_error("npm install jest").contains("risky Bash command blocked"));
    assert!(
        repeated_plan_exploration_error(PlanStage::Stage1, &["Goal"], "Read")
            .contains("repeated exploration blocked for Stage 1")
    );
    assert!(tool_call_counts_as_repo_edit("Write"));
    assert!(tool_call_counts_as_repo_edit("Edit"));
    assert!(!tool_call_counts_as_repo_edit("Bash"));
}

// -------------------------------------------------------------------------
// Issue #652 — `ArtifactCompletionJob` regression guards.
//
// The `ArtifactCompletionJob` private mod is intentionally NOT re-exported
// (DR3-001) — `turn.rs` is the only behavioral in-crate consumer. These
// tests therefore exercise only the externally-observable contract:
//
//   * `artifact_directed_recovery_note(role, path, attempt)` still renders
//     the `artifact_directed_attempt=N` token the actor loop emits when a
//     job is in flight (regression guard for the existing message format).
//   * Phase 2 wiring does not break the existing recovery / no-tool / repo-
//     change / install-loop messages.
//
// The role-specific budget enforcement / wrong-target / no-tool / prose-only
// classifier itself is covered by the in-crate unit suite in
// `src/agent/loop_run/artifact_completion_job.rs::tests`; the integration
// surface here pins the wiring (DR3-001 audit) and message-format compat.
// -------------------------------------------------------------------------

#[test]
fn artifact_directed_recovery_note_contains_artifact_directed_attempt_token_test_role() {
    // Issue #652 regression guard — `tests/recovery_tests.rs` baseline expects
    // the `artifact_directed_attempt=` token to be present in the recovery
    // note so the production message format remains stable across the
    // Phase 2 wiring of `ArtifactCompletionJob`.
    let note = artifact_directed_recovery_note("test", "tests/test_foo.py", 2);
    assert!(
        note.contains("artifact_directed_attempt=2"),
        "regression: artifact_directed_attempt token missing from note: {note}"
    );
    assert!(note.contains("test"), "role label must appear in note");
    assert!(
        note.contains("tests/test_foo.py"),
        "target path must appear in note"
    );
}

#[test]
fn artifact_directed_recovery_note_contains_artifact_directed_attempt_token_impl_role() {
    let note = artifact_directed_recovery_note("implementation", "src/x.py", 3);
    assert!(note.contains("artifact_directed_attempt=3"));
    assert!(note.contains("implementation"));
    assert!(note.contains("src/x.py"));
}

#[test]
fn empty_response_recovery_note_format_compatible_with_no_tool_classification() {
    // Issue #652 wiring records `NoTool` attempts on the empty-reply path
    // (turn.rs::run_actor_loop -> final_reply.is_empty branch). The message
    // emitted upstream must remain non-empty and recognizable.
    let note = empty_response_recovery_note(1, true);
    assert!(!note.is_empty());
    assert!(note.contains("attempt=1"));
}

#[test]
fn repo_change_no_tool_recovery_note_format_compatible_with_prose_only_classification() {
    // Issue #652 wiring records `ProseOnly` attempts on the prose-only path
    // (turn.rs::run_actor_loop -> final_reply non-empty + repo_change_no_tool
    // branch). Pin the existing recovery note format.
    let note = repo_change_no_tool_recovery_note(2);
    assert!(!note.is_empty());
    assert!(note.contains("exactly one tool call"));
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
    assert!(is_scaffold_command("npx create-next-app@latest . --ts"));
    assert!(is_workspace_reset_command("rm -rf .anvil"));
    assert!(should_block_bash_command(
        "rm -rf .anvil && npx create-next-app@latest . --ts",
        &[],
        0
    ));
    assert!(should_block_bash_command(
        "npx create-next-app@latest . --ts",
        &["npx create-next-app@latest app --ts".to_string()],
        0
    ));
    assert!(should_block_restart_discovery("Glob", true));
    assert!(should_block_restart_discovery("Bash", true));
    assert!(!should_block_restart_discovery("Read", true));
    assert!(broad_restart_discovery_error("Glob").contains("blocked during actor restart"));
}
