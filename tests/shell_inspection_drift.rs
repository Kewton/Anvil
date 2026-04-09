//! Tests for Issue #309: shell.exec inspection drift under active unfinished plans.
//!
//! Covers:
//! - PhaseEstimator classifies shell inspection commands as Read (not Other)
//! - is_shell_inspection_command detects repo-inspection commands
//! - Late-stage closure hint when remaining==1
//! - Structured repair message for unfinished plans
//! - Shell inspection contributes to exploration streaks (not resetting them)

use anvil::app::phase_estimator::{PhaseAction, PhaseEstimator};
use anvil::contracts::{ExecutionPlan, PlanItem};
use anvil::tooling::shell_policy::is_shell_inspection_command;

// ---------------------------------------------------------------------------
// Test 1: PhaseEstimator counts shell.exec grep as Read via record_tool_call_ex
// ---------------------------------------------------------------------------

#[test]
fn phase_estimator_shell_grep_counts_as_read() {
    let mut est = PhaseEstimator::new(3, 6, 3);
    // 3 shell.exec grep calls should trigger exploring phase
    est.record_tool_call_ex("shell.exec", true, Some("grep -n pattern src/main.rs"));
    est.record_tool_call_ex("shell.exec", true, Some("grep -rn TODO src/lib.rs"));
    est.record_tool_call_ex("shell.exec", true, Some("grep -n detect src/app.rs"));
    assert_eq!(
        est.current_phase(),
        anvil::app::phase_estimator::Phase::Exploring,
        "shell.exec grep should count as read and trigger Exploring phase"
    );
}

// ---------------------------------------------------------------------------
// Test 2: PhaseEstimator counts shell.exec head/tail as Read
// ---------------------------------------------------------------------------

#[test]
fn phase_estimator_shell_head_tail_counts_as_read() {
    let mut est = PhaseEstimator::new(3, 6, 3);
    est.record_tool_call_ex("shell.exec", true, Some("head -50 src/main.rs"));
    est.record_tool_call_ex("shell.exec", true, Some("tail -20 src/lib.rs"));
    est.record_tool_call_ex("shell.exec", true, Some("cat src/app.rs"));
    assert_eq!(
        est.current_phase(),
        anvil::app::phase_estimator::Phase::Exploring,
    );
}

// ---------------------------------------------------------------------------
// Test 3: PhaseEstimator counts commandindexdev as Read
// ---------------------------------------------------------------------------

#[test]
fn phase_estimator_commandindexdev_counts_as_read() {
    let mut est = PhaseEstimator::new(3, 6, 3);
    est.record_tool_call_ex(
        "shell.exec",
        true,
        Some("commandindexdev before-change src/lib/detection/prompt-detector.ts --format llm"),
    );
    est.record_tool_call_ex(
        "shell.exec",
        true,
        Some("commandindexdev before-change src/lib/detection/cli-patterns.ts --format llm"),
    );
    est.record_tool_call_ex(
        "shell.exec",
        true,
        Some("commandindexdev before-change src/lib/polling/response-poller.ts --format llm"),
    );
    assert_eq!(
        est.current_phase(),
        anvil::app::phase_estimator::Phase::Exploring,
        "commandindexdev should count as read/exploration"
    );
}

// ---------------------------------------------------------------------------
// Test 4: Shell inspection triggers ForceTransition at threshold
// ---------------------------------------------------------------------------

#[test]
fn phase_estimator_shell_inspection_triggers_force_transition() {
    let mut est = PhaseEstimator::new(3, 6, 3);
    for i in 0..6 {
        let action = est.record_tool_call_ex("shell.exec", true, Some("grep -n pattern file.rs"));
        if i < 5 {
            assert_eq!(action, PhaseAction::Continue);
        } else {
            assert!(
                matches!(action, PhaseAction::ForceTransition(_)),
                "shell.exec grep should trigger ForceTransition at threshold"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Test 5: Non-inspection shell.exec (cargo build) still classified as Other
// ---------------------------------------------------------------------------

#[test]
fn phase_estimator_non_inspection_shell_still_other() {
    let mut est = PhaseEstimator::new(3, 6, 3);
    // cargo build is not inspection → should not count as read
    est.record_tool_call_ex("shell.exec", true, Some("cargo build"));
    est.record_tool_call_ex("shell.exec", true, Some("npm install"));
    est.record_tool_call_ex("shell.exec", true, Some("cargo test"));
    assert_eq!(
        est.current_phase(),
        anvil::app::phase_estimator::Phase::Unknown,
        "non-inspection shell.exec should not affect phase"
    );
}

// ---------------------------------------------------------------------------
// Test 6: Mixed file.read and shell inspection accumulate together
// ---------------------------------------------------------------------------

#[test]
fn phase_estimator_mixed_reads_and_shell_inspection() {
    let mut est = PhaseEstimator::new(3, 6, 3);
    est.record_tool_call("file.read", true);
    est.record_tool_call_ex("shell.exec", true, Some("grep -n pattern src/main.rs"));
    est.record_tool_call("file.read", true);
    assert_eq!(
        est.current_phase(),
        anvil::app::phase_estimator::Phase::Exploring,
        "mixed file.read + shell grep should accumulate for phase estimation"
    );
}

// ---------------------------------------------------------------------------
// Test 7: Successful write resets shell inspection counter
// ---------------------------------------------------------------------------

#[test]
fn phase_estimator_write_resets_shell_inspection_count() {
    let mut est = PhaseEstimator::new(3, 6, 3);
    est.record_tool_call_ex("shell.exec", true, Some("grep -n pattern src/main.rs"));
    est.record_tool_call_ex("shell.exec", true, Some("head -50 src/lib.rs"));
    // Write should reset
    est.record_tool_call("file.edit", true);
    // Need 3 more to reach threshold again
    est.record_tool_call_ex("shell.exec", true, Some("grep -n other src/app.rs"));
    assert_eq!(
        est.current_phase(),
        anvil::app::phase_estimator::Phase::Implementing,
        "after write, consecutive_reads should reset; one read is not enough for Exploring"
    );
}

// ---------------------------------------------------------------------------
// Test 8: is_shell_inspection_command detects all expected commands
// ---------------------------------------------------------------------------

#[test]
fn is_shell_inspection_command_covers_file_read_commands() {
    // All file read commands are also inspection commands
    assert!(is_shell_inspection_command("grep -n pattern src/main.rs"));
    assert!(is_shell_inspection_command("sed -n '10,20p' src/main.rs"));
    assert!(is_shell_inspection_command("cat src/main.rs"));
    assert!(is_shell_inspection_command("head -50 src/main.rs"));
    assert!(is_shell_inspection_command("tail -20 src/main.rs"));
    assert!(is_shell_inspection_command(
        "awk '/pattern/ {print}' src/main.rs"
    ));
}

#[test]
fn is_shell_inspection_command_covers_repo_inspection() {
    assert!(is_shell_inspection_command(
        "commandindexdev before-change src/lib.ts --format llm"
    ));
    assert!(is_shell_inspection_command("find . -name '*.rs'"));
    assert!(is_shell_inspection_command("wc -l src/main.rs"));
    assert!(is_shell_inspection_command("diff src/a.rs src/b.rs"));
    assert!(is_shell_inspection_command("rg TODO src/"));
    assert!(is_shell_inspection_command("fd --extension rs"));
    assert!(is_shell_inspection_command("tree src/"));
}

#[test]
fn is_shell_inspection_command_rejects_non_inspection() {
    assert!(!is_shell_inspection_command("cargo test"));
    assert!(!is_shell_inspection_command("npm install"));
    assert!(!is_shell_inspection_command("rm -rf target/"));
    assert!(!is_shell_inspection_command("curl https://example.com"));
}

#[test]
fn is_shell_inspection_command_rejects_injection() {
    // Pipe/chain commands are excluded (injection vectors)
    assert!(!is_shell_inspection_command("grep pattern file | head -5"));
    assert!(!is_shell_inspection_command("find . && rm -rf /"));
}

// ---------------------------------------------------------------------------
// Test 9: Late-stage closure hint fires when remaining==1
// ---------------------------------------------------------------------------

#[test]
fn late_stage_closure_hint_fires_at_remaining_one() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("src/a.rs: done".into(), vec!["src/a.rs".into()]),
        PlanItem::new("src/b.rs: done".into(), vec!["src/b.rs".into()]),
        PlanItem::new(
            "tests/test.rs: add tests".into(),
            vec!["tests/test.rs".into()],
        ),
    ]);
    plan.mark_done(0);
    plan.mark_done(1);
    // Item 2 is still Pending → remaining==1

    let hint = plan.build_late_stage_closure_hint();
    assert!(hint.is_some(), "closure hint should fire when remaining==1");
    let hint_text = hint.unwrap();
    assert!(
        hint_text.contains("CLOSURE MODE"),
        "hint should contain CLOSURE MODE marker"
    );
    assert!(
        hint_text.contains("tests/test.rs"),
        "hint should mention the remaining target file"
    );
    assert!(
        hint_text.contains("shell.exec"),
        "hint should discourage shell.exec usage"
    );
}

// ---------------------------------------------------------------------------
// Test 10: Late-stage closure hint does NOT fire when remaining > 1
// ---------------------------------------------------------------------------

#[test]
fn late_stage_closure_hint_not_fired_when_remaining_gt_one() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("src/a.rs: done".into(), vec!["src/a.rs".into()]),
        PlanItem::new("src/b.rs: todo".into(), vec!["src/b.rs".into()]),
        PlanItem::new("src/c.rs: todo".into(), vec!["src/c.rs".into()]),
    ]);
    plan.mark_done(0);
    // Items 1+2 pending → remaining==2

    let hint = plan.build_late_stage_closure_hint();
    assert!(
        hint.is_none(),
        "closure hint should NOT fire when remaining > 1"
    );
}

// ---------------------------------------------------------------------------
// Test 11: Late-stage closure hint does NOT fire when no progress
// ---------------------------------------------------------------------------

#[test]
fn late_stage_closure_hint_not_fired_when_no_progress() {
    let plan = ExecutionPlan::new(vec![PlanItem::new(
        "src/a.rs: todo".into(),
        vec!["src/a.rs".into()],
    )]);
    // 1 item, 0 finished → finished==0, should not fire

    let hint = plan.build_late_stage_closure_hint();
    assert!(
        hint.is_none(),
        "closure hint should NOT fire when no progress has been made"
    );
}

// ---------------------------------------------------------------------------
// Test 12: Late-stage closure hint does NOT fire for empty plan
// ---------------------------------------------------------------------------

#[test]
fn late_stage_closure_hint_not_fired_for_empty_plan() {
    let plan = ExecutionPlan::default();
    let hint = plan.build_late_stage_closure_hint();
    assert!(
        hint.is_none(),
        "closure hint should NOT fire for empty plan"
    );
}

// ---------------------------------------------------------------------------
// Test 13: FallbackComplete with shell inspection after write (A2 scenario)
// ---------------------------------------------------------------------------

#[test]
fn fallback_complete_with_shell_inspection_after_write() {
    // Reproduce the A2 failure shape:
    // 1. Write succeeded (has_written=true)
    // 2. Model drifts into shell-based inspection (grep, head, commandindexdev)
    // 3. With Issue #309 fix, these count as reads → FallbackComplete detection
    let mut est = PhaseEstimator::new(3, 6, 3);

    // Step 1: Successful write
    est.record_tool_call("file.edit", true);
    assert_eq!(
        est.current_phase(),
        anvil::app::phase_estimator::Phase::Implementing
    );

    // Step 2: Shell inspection drift (3+ inspection calls)
    est.record_tool_call_ex("shell.exec", true, Some("grep -n detectPrompt route.ts"));
    est.record_tool_call_ex("shell.exec", true, Some("grep -n detectPrompt manager.ts"));
    est.record_tool_call_ex("shell.exec", true, Some("grep -n detectPrompt detector.ts"));

    // Step 3: FallbackComplete should detect completion
    // (has_written=true + consecutive_reads >= completion_read_threshold=3)
    let action = est.check_empty_response();
    assert_eq!(
        action,
        PhaseAction::FallbackComplete,
        "shell inspection after write should trigger FallbackComplete"
    );
}

// ---------------------------------------------------------------------------
// Test 14: Without fix, shell inspection does NOT trigger FallbackComplete
// (verifies the old behavior was broken)
// ---------------------------------------------------------------------------

#[test]
fn old_behavior_shell_inspection_no_fallback_complete() {
    // Using record_tool_call (without _ex) simulates the old behavior
    // where shell.exec is classified as Other
    let mut est = PhaseEstimator::new(3, 6, 3);

    // Successful write
    est.record_tool_call("file.edit", true);

    // Shell.exec without command info → classified as Other (old behavior)
    est.record_tool_call("shell.exec", true);
    est.record_tool_call("shell.exec", true);
    est.record_tool_call("shell.exec", true);

    // Without shell command context, these are Other → consecutive_reads stays 0
    let action = est.check_empty_response();
    assert_eq!(
        action,
        PhaseAction::Continue,
        "without shell command context, shell.exec should not trigger FallbackComplete"
    );
}
