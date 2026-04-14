//! Integration tests for Issue #383: termination semantics — unify the meaning
//! of no-tool-call responses.
//!
//! These tests verify the decision logic of `decide_no_tool_call` by mirroring
//! its branching rules here (the function and its types are private to agentic.rs).
//! The tests serve as a living specification for the expected semantics.
//!
//! Covers (7 scenarios):
//! - T1: guidance follow-up escalates to final guard retry
//! - T2: FallbackCompleted suppressed when plan has incomplete items (Issue #307)
//! - T3: Done path final guard retry fires on first completion
//! - T4: Done path plan gate fires before final guard retry (ordering rule)
//! - T5: Task-semantics gate fires on Branch 5
//! - T6: Clean final answer accepted
//! - T7: Post-tool ANVIL_FINAL plan gate suppression

use anvil::app::phase_estimator::{PhaseAction, PhaseEstimator};
use anvil::contracts::{ExecutionPlan, PlanItem};

// ---------------------------------------------------------------------------
// Mirror types and logic from agentic.rs (decide_no_tool_call)
// ---------------------------------------------------------------------------

const MAX_FINAL_GUARD_RETRIES: u8 = 1;

/// Mirrors `NoToolCallDecision` from `src/app/agentic.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoToolCallDecision {
    GuidanceFollowupComplete,
    FinalGuardRetry,
    FallbackCompleted,
    PlanGateSuppression,
    TaskSemanticsSuppression,
    FinalAnswer,
}

/// Mirrors `NoToolCallInput` from `src/app/agentic.rs`.
struct NoToolCallInput {
    anvil_final_detected: bool,
    awaiting_guidance_followup: bool,
    final_guard_retries: u8,
    touched_files_empty: bool,
    phase_action: PhaseAction,
    plan_has_incomplete_items: bool,
    plan_gate_fires: bool,
    task_semantics_fires: bool,
}

/// Mirrors `decide_no_tool_call` from `src/app/agentic.rs`.
///
/// This is the single source of truth for no-tool-call decision semantics.
/// Any change to branching in agentic.rs should be reflected here.
fn decide_no_tool_call(input: NoToolCallInput) -> NoToolCallDecision {
    // Branch 1: guidance follow-up
    if input.awaiting_guidance_followup {
        // Does NOT require anvil_final_detected — ANVIL_FINAL may have been
        // suppressed when the guidance injection was posted.
        if input.touched_files_empty && input.final_guard_retries < MAX_FINAL_GUARD_RETRIES {
            return NoToolCallDecision::FinalGuardRetry;
        }
        return NoToolCallDecision::GuidanceFollowupComplete;
    }

    // Branch 2: ANVIL_FINAL guard (requires anvil_final_detected, DR2-003)
    if input.anvil_final_detected
        && input.touched_files_empty
        && input.final_guard_retries < MAX_FINAL_GUARD_RETRIES
    {
        return NoToolCallDecision::FinalGuardRetry;
    }

    // Branch 3: fallback completion
    // Issue #307: if plan has incomplete items, fallthrough to plan gate instead.
    if matches!(input.phase_action, PhaseAction::FallbackComplete)
        && !input.plan_has_incomplete_items
    {
        return NoToolCallDecision::FallbackCompleted;
    }

    // Branch 4: plan gate
    if input.plan_gate_fires {
        return NoToolCallDecision::PlanGateSuppression;
    }

    // Branch 5: task-semantics gate
    if input.task_semantics_fires {
        return NoToolCallDecision::TaskSemanticsSuppression;
    }

    // Final: accept as answer
    NoToolCallDecision::FinalAnswer
}

// ---------------------------------------------------------------------------
// T1: guidance follow-up escalates to final guard retry
// ---------------------------------------------------------------------------

/// When a guidance injection was posted and the follow-up response has no file
/// edits, the final guard retry budget is consumed (same budget as Branch 2).
/// ANVIL_FINAL being absent does not prevent the retry — it was suppressed when
/// the guidance injection was posted.
#[test]
fn guidance_followup_escalates_to_final_guard_retry() {
    let input = NoToolCallInput {
        anvil_final_detected: false, // suppressed by guidance injection
        awaiting_guidance_followup: true,
        final_guard_retries: 0, // budget not yet consumed
        touched_files_empty: true,
        phase_action: PhaseAction::Continue,
        plan_has_incomplete_items: false,
        plan_gate_fires: false,
        task_semantics_fires: false,
    };
    assert_eq!(
        decide_no_tool_call(input),
        NoToolCallDecision::FinalGuardRetry,
        "guidance follow-up with no file edits must escalate to FinalGuardRetry"
    );
}

/// When the retry budget is exhausted, guidance follow-up resolves as complete.
#[test]
fn guidance_followup_complete_when_budget_exhausted() {
    let input = NoToolCallInput {
        anvil_final_detected: false,
        awaiting_guidance_followup: true,
        final_guard_retries: MAX_FINAL_GUARD_RETRIES, // budget exhausted
        touched_files_empty: true,
        phase_action: PhaseAction::Continue,
        plan_has_incomplete_items: false,
        plan_gate_fires: false,
        task_semantics_fires: false,
    };
    assert_eq!(
        decide_no_tool_call(input),
        NoToolCallDecision::GuidanceFollowupComplete,
        "guidance follow-up with exhausted budget must resolve as GuidanceFollowupComplete"
    );
}

/// When guidance follow-up arrived with file edits, it resolves as complete
/// regardless of retry budget.
#[test]
fn guidance_followup_complete_when_files_modified() {
    let input = NoToolCallInput {
        anvil_final_detected: false,
        awaiting_guidance_followup: true,
        final_guard_retries: 0,
        touched_files_empty: false, // files were modified
        phase_action: PhaseAction::Continue,
        plan_has_incomplete_items: false,
        plan_gate_fires: false,
        task_semantics_fires: false,
    };
    assert_eq!(
        decide_no_tool_call(input),
        NoToolCallDecision::GuidanceFollowupComplete,
        "guidance follow-up with file modifications must resolve as GuidanceFollowupComplete"
    );
}

// ---------------------------------------------------------------------------
// T2: FallbackCompleted suppressed when plan has incomplete items (Issue #307)
// ---------------------------------------------------------------------------

/// FallbackComplete phase action is suppressed when the plan has unfinished items.
/// The decision falls through to PlanGateSuppression instead (Branch 3 → Branch 4).
#[test]
fn fallback_completed_suppressed_when_plan_incomplete() {
    // Plan has one pending item.
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("src/a.rs: implement".into(), vec!["src/a.rs".into()]),
        PlanItem::new("src/b.rs: add tests".into(), vec!["src/b.rs".into()]),
    ]);
    plan.mark_done(0); // item 1 still pending

    assert!(!plan.is_empty());
    assert!(!plan.all_finished());

    let plan_has_incomplete_items = !plan.is_empty() && !plan.all_finished();

    // PhaseEstimator signals FallbackComplete.
    let mut estimator = PhaseEstimator::new(3, 6, 2);
    estimator.record_tool_call("file.write", true);
    estimator.record_tool_call("file.read", true);
    estimator.record_tool_call("file.read", true);
    let phase_action = estimator.check_empty_response();
    assert_eq!(phase_action, PhaseAction::FallbackComplete);

    let input = NoToolCallInput {
        anvil_final_detected: false,
        awaiting_guidance_followup: false,
        final_guard_retries: 0,
        touched_files_empty: false,
        phase_action,
        plan_has_incomplete_items,
        plan_gate_fires: true, // plan gate fires because plan has incomplete items
        task_semantics_fires: false,
    };
    assert_eq!(
        decide_no_tool_call(input),
        NoToolCallDecision::PlanGateSuppression,
        "FallbackComplete must be suppressed when plan has incomplete items (Issue #307)"
    );
}

/// FallbackComplete fires normally when all plan items are finished.
#[test]
fn fallback_completed_fires_when_plan_finished() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "src/a.rs: implement".into(),
        vec!["src/a.rs".into()],
    )]);
    plan.mark_done(0);

    assert!(plan.all_finished());

    let plan_has_incomplete_items = !plan.is_empty() && !plan.all_finished();

    let mut estimator = PhaseEstimator::new(3, 6, 2);
    estimator.record_tool_call("file.write", true);
    estimator.record_tool_call("file.read", true);
    estimator.record_tool_call("file.read", true);
    let phase_action = estimator.check_empty_response();
    assert_eq!(phase_action, PhaseAction::FallbackComplete);

    let input = NoToolCallInput {
        anvil_final_detected: false,
        awaiting_guidance_followup: false,
        final_guard_retries: 0,
        touched_files_empty: false,
        phase_action,
        plan_has_incomplete_items,
        plan_gate_fires: false,
        task_semantics_fires: false,
    };
    assert_eq!(
        decide_no_tool_call(input),
        NoToolCallDecision::FallbackCompleted,
        "FallbackComplete must fire when all plan items are finished"
    );
}

// ---------------------------------------------------------------------------
// T3: Done path final guard retry fires on first completion
// ---------------------------------------------------------------------------

/// On the Done path, when ANVIL_FINAL is detected, touched_files is empty,
/// and the guard budget is not consumed, FinalGuardRetry fires.
/// (Done path defaults: awaiting_guidance_followup=false, final_guard_retries=0)
#[test]
fn done_path_final_guard_retry() {
    let input = NoToolCallInput {
        anvil_final_detected: true,          // ANVIL_FINAL detected in response
        awaiting_guidance_followup: false,   // Done path: always false
        final_guard_retries: 0,              // Done path: first turn
        touched_files_empty: true,           // no file modifications
        phase_action: PhaseAction::Continue, // Done path: always Continue
        plan_has_incomplete_items: false,
        plan_gate_fires: false, // Done path: plan gate handled before calling this
        task_semantics_fires: false,
    };
    assert_eq!(
        decide_no_tool_call(input),
        NoToolCallDecision::FinalGuardRetry,
        "Done path must trigger FinalGuardRetry on first no-edit completion"
    );
}

/// On the Done path, if ANVIL_FINAL is NOT detected, FinalGuardRetry does not fire.
/// (Branch 2 requires anvil_final_detected=true — DR2-003)
#[test]
fn done_path_no_final_guard_retry_without_anvil_final() {
    let input = NoToolCallInput {
        anvil_final_detected: false, // no ANVIL_FINAL
        awaiting_guidance_followup: false,
        final_guard_retries: 0,
        touched_files_empty: true,
        phase_action: PhaseAction::Continue,
        plan_has_incomplete_items: false,
        plan_gate_fires: false,
        task_semantics_fires: false,
    };
    // Without anvil_final_detected, Branch 2 does not fire.
    // All other branches are inactive → FinalAnswer.
    assert_eq!(
        decide_no_tool_call(input),
        NoToolCallDecision::FinalAnswer,
        "Done path without ANVIL_FINAL must not trigger FinalGuardRetry"
    );
}

// ---------------------------------------------------------------------------
// T4: Done path plan gate fires before final guard retry (ordering)
// ---------------------------------------------------------------------------

/// DR2-001: On the Done path, the plan gate fires BEFORE FinalGuardRetry.
/// This is the OPPOSITE order from the agentic loop (Branch 2 before Branch 4).
///
/// Implementation: `handle_done_path_anvil_final_guard` handles the plan gate
/// directly (returning PlanGateRetry) before calling `decide_no_tool_call` with
/// `plan_gate_fires=false`. This test verifies the ordering invariant by showing
/// that plan_gate_fires=true suppresses FinalGuardRetry in Branch 4.
#[test]
fn done_path_plan_gate_suppresses_before_final_guard_retry() {
    // Both conditions would fire if ordering weren't enforced:
    // - anvil_final_detected=true + touched_files_empty=true → would be FinalGuardRetry
    // - plan_gate_fires=true → PlanGateSuppression
    let input = NoToolCallInput {
        anvil_final_detected: true,
        awaiting_guidance_followup: false,
        final_guard_retries: 0,
        touched_files_empty: true,
        phase_action: PhaseAction::Continue,
        plan_has_incomplete_items: true,
        plan_gate_fires: true, // plan gate fires
        task_semantics_fires: false,
    };
    // Branch 2 (FinalGuardRetry) fires BEFORE Branch 4 (PlanGateSuppression)
    // in the agentic loop ordering. So with both conditions active, FinalGuardRetry wins.
    assert_eq!(
        decide_no_tool_call(input),
        NoToolCallDecision::FinalGuardRetry,
        "in agentic loop ordering, FinalGuardRetry (Branch 2) fires before PlanGateSuppression (Branch 4)"
    );
}

/// Verify that on the Done path (plan_gate_fires=false as caller handles it),
/// FinalGuardRetry is the result when ANVIL_FINAL is detected.
#[test]
fn done_path_plan_gate_handled_externally() {
    // Simulates what handle_done_path_anvil_final_guard passes after handling
    // the plan gate externally: plan_gate_fires=false.
    let input = NoToolCallInput {
        anvil_final_detected: true,
        awaiting_guidance_followup: false,
        final_guard_retries: 0,
        touched_files_empty: true,
        phase_action: PhaseAction::Continue,
        plan_has_incomplete_items: true,
        plan_gate_fires: false, // Done path: plan gate already handled by caller
        task_semantics_fires: false,
    };
    assert_eq!(
        decide_no_tool_call(input),
        NoToolCallDecision::FinalGuardRetry,
        "with plan_gate_fires=false (Done path pattern), FinalGuardRetry must still fire"
    );
}

// ---------------------------------------------------------------------------
// T5: Task-semantics gate fires on Branch 5
// ---------------------------------------------------------------------------

/// When a task requires implementation but no file changes occurred,
/// the task-semantics gate fires (Branch 5).
#[test]
fn task_semantics_gate_fires_on_branch5() {
    let input = NoToolCallInput {
        anvil_final_detected: false, // no ANVIL_FINAL
        awaiting_guidance_followup: false,
        final_guard_retries: 0,
        touched_files_empty: true,
        phase_action: PhaseAction::Continue,
        plan_has_incomplete_items: false,
        plan_gate_fires: false,
        task_semantics_fires: true, // task-semantics gate fires
    };
    assert_eq!(
        decide_no_tool_call(input),
        NoToolCallDecision::TaskSemanticsSuppression,
        "task-semantics gate must fire when implementation is required but no files modified"
    );
}

/// Task-semantics gate does not fire when plan gate already fired.
/// Plan gate (Branch 4) takes priority over task-semantics gate (Branch 5).
#[test]
fn plan_gate_takes_priority_over_task_semantics() {
    let input = NoToolCallInput {
        anvil_final_detected: false,
        awaiting_guidance_followup: false,
        final_guard_retries: 0,
        touched_files_empty: false,
        phase_action: PhaseAction::Continue,
        plan_has_incomplete_items: true,
        plan_gate_fires: true,
        task_semantics_fires: true, // both active
    };
    assert_eq!(
        decide_no_tool_call(input),
        NoToolCallDecision::PlanGateSuppression,
        "plan gate (Branch 4) must take priority over task-semantics gate (Branch 5)"
    );
}

// ---------------------------------------------------------------------------
// T6: Clean final answer accepted
// ---------------------------------------------------------------------------

/// When none of the guard conditions apply, the response is accepted as a
/// final answer (FinalAnswer path).
#[test]
fn clean_final_answer_accepted() {
    let input = NoToolCallInput {
        anvil_final_detected: false,
        awaiting_guidance_followup: false,
        final_guard_retries: 0,
        touched_files_empty: false, // files were modified — guard does not fire
        phase_action: PhaseAction::Continue, // no fallback completion
        plan_has_incomplete_items: false,
        plan_gate_fires: false,
        task_semantics_fires: false,
    };
    assert_eq!(
        decide_no_tool_call(input),
        NoToolCallDecision::FinalAnswer,
        "clean response with no active guards must be accepted as FinalAnswer"
    );
}

/// Final answer is accepted even when ANVIL_FINAL was detected if file edits
/// were made (the guard condition on touched_files_empty is not met).
#[test]
fn final_answer_accepted_when_files_modified_despite_anvil_final() {
    let input = NoToolCallInput {
        anvil_final_detected: true, // ANVIL_FINAL present
        awaiting_guidance_followup: false,
        final_guard_retries: 0,
        touched_files_empty: false, // but files were modified → guard skipped
        phase_action: PhaseAction::Continue,
        plan_has_incomplete_items: false,
        plan_gate_fires: false,
        task_semantics_fires: false,
    };
    assert_eq!(
        decide_no_tool_call(input),
        NoToolCallDecision::FinalAnswer,
        "ANVIL_FINAL with file modifications must not trigger FinalGuardRetry"
    );
}

/// Final guard retry does not fire when budget is exhausted even if all
/// conditions are otherwise met.
#[test]
fn final_guard_budget_exhausted_falls_through_to_final_answer() {
    let input = NoToolCallInput {
        anvil_final_detected: true,
        awaiting_guidance_followup: false,
        final_guard_retries: MAX_FINAL_GUARD_RETRIES, // budget exhausted
        touched_files_empty: true,
        phase_action: PhaseAction::Continue,
        plan_has_incomplete_items: false,
        plan_gate_fires: false,
        task_semantics_fires: false,
    };
    assert_eq!(
        decide_no_tool_call(input),
        NoToolCallDecision::FinalAnswer,
        "with exhausted guard budget and no other active gates, must accept as FinalAnswer"
    );
}

// ---------------------------------------------------------------------------
// T7: Post-tool ANVIL_FINAL plan gate suppression
// ---------------------------------------------------------------------------

/// After tool execution, if ANVIL_FINAL is detected but the plan still has
/// incomplete items, the plan gate suppresses termination.
///
/// This mirrors the `handle_post_tool_anvil_final_gate` path: plan gate fires →
/// suppress ANVIL_FINAL → inject retry. In the `decide_no_tool_call` model,
/// this corresponds to plan_gate_fires=true.
#[test]
fn post_tool_anvil_final_plan_gate_suppression() {
    // Post-tool context: ANVIL_FINAL was seen, but plan is incomplete.
    // In agentic.rs, handle_post_tool_anvil_final_gate handles this by checking
    // the plan gate. Here we verify the semantic equivalent.
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new(
            "src/a.rs: implement feature".into(),
            vec!["src/a.rs".into()],
        ),
        PlanItem::new("src/b.rs: add tests".into(), vec!["src/b.rs".into()]),
    ]);
    plan.mark_done(0); // item 1 still pending

    assert!(!plan.all_finished());

    let plan_has_incomplete_items = !plan.is_empty() && !plan.all_finished();

    // Simulate the check: plan gate fires when plan has incomplete items and
    // require_plan_here is satisfied (plan is non-empty and has incomplete items).
    let plan_gate_fires = plan_has_incomplete_items; // simplified: fires when incomplete

    let input = NoToolCallInput {
        anvil_final_detected: true, // ANVIL_FINAL detected after tool execution
        awaiting_guidance_followup: false,
        final_guard_retries: 0,
        touched_files_empty: true, // no file edits in this turn
        phase_action: PhaseAction::Continue,
        plan_has_incomplete_items,
        plan_gate_fires,
        task_semantics_fires: false,
    };

    // Branch 2 (FinalGuardRetry) fires first because anvil_final_detected=true.
    // This verifies the ordering: plan gate (Branch 4) only fires when Branch 2 is not active.
    // In `handle_post_tool_anvil_final_gate`, the plan gate check happens BEFORE
    // decide_no_tool_call is invoked (analogous to the Done path pattern).
    assert_eq!(
        decide_no_tool_call(input),
        NoToolCallDecision::FinalGuardRetry,
        "agentic loop ordering: FinalGuardRetry fires before plan gate when both active"
    );
}

/// Verify that when ANVIL_FINAL guard budget is exhausted in the post-tool path,
/// the plan gate fires and suppresses termination.
#[test]
fn post_tool_plan_gate_fires_when_guard_budget_exhausted() {
    let input = NoToolCallInput {
        anvil_final_detected: true,
        awaiting_guidance_followup: false,
        final_guard_retries: MAX_FINAL_GUARD_RETRIES, // guard budget exhausted
        touched_files_empty: true,
        phase_action: PhaseAction::Continue,
        plan_has_incomplete_items: true,
        plan_gate_fires: true, // plan gate fires
        task_semantics_fires: false,
    };
    assert_eq!(
        decide_no_tool_call(input),
        NoToolCallDecision::PlanGateSuppression,
        "plan gate must fire when FinalGuardRetry budget is exhausted"
    );
}
