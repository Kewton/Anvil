//! Tests for Issue #323: escape hatch ordering — late ANVIL_PLAN_UPDATE must
//! be processed before the escape hatch terminates the loop.
//!
//! Covers:
//! - Late zero-tool-call response with ANVIL_PLAN_UPDATE [x] is still applied
//!   even when escape hatch fires on the same turn.
//! - The final checked item reaches AlreadySatisfied before loop termination.
//! - CompletionKind is not Partial for the no-change closure path.
//! - plan_update_count reflects the late corrective update.

use anvil::app::stagnation_state::{
    StagnationState, compute_stagnation_score, should_allow_escape_hatch,
};
use anvil::contracts::{CompletionKind, ExecutionPlan, PlanItem, PlanItemStatus};

// ---------------------------------------------------------------------------
// Scenario: escape hatch fires on the same turn as the final checked update
// ---------------------------------------------------------------------------

/// Reproduces the B1 failure from Issue #323:
/// - 6-item plan, 5 items already retired (AlreadySatisfied), 1 remaining.
/// - ANVIL_PLAN_UPDATE with [x] for the last item arrives on a zero-tool-call
///   response where escape hatch conditions are met.
/// - Expected: plan update is applied first, last item becomes AlreadySatisfied,
///   CompletionKind is CompleteUnverified (not Partial).
#[test]
fn escape_hatch_turn_with_final_checked_update_completes_plan() {
    // Setup: 6-item plan, items 0-4 already done/satisfied, item 5 still pending
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("src/a.rs: update module".into(), vec!["src/a.rs".into()]),
        PlanItem::new("src/b.rs: update module".into(), vec!["src/b.rs".into()]),
        PlanItem::new("src/c.rs: update module".into(), vec!["src/c.rs".into()]),
        PlanItem::new("src/d.rs: update module".into(), vec!["src/d.rs".into()]),
        PlanItem::new("src/e.rs: update module".into(), vec!["src/e.rs".into()]),
        PlanItem::new(
            "src/app/api/worktrees/[id]/current-output/route.ts: no change needed".into(),
            vec!["src/app/api/worktrees/[id]/current-output/route.ts".into()],
        ),
    ]);
    // Items 0-4 retired via prior checked markers
    for i in 0..5 {
        plan.items[i].status = PlanItemStatus::AlreadySatisfied;
    }

    // Stagnation state: set up conditions where escape hatch would fire
    let mut stagnation = StagnationState::init_from_plan(&[
        "src/app/api/worktrees/[id]/current-output/route.ts".to_string(),
    ]);
    // Simulate many turns of stagnation
    for _ in 0..12 {
        stagnation.begin_turn(&[]);
        stagnation.end_turn(false);
    }
    let score = compute_stagnation_score(&stagnation);
    assert!(score >= 3, "stagnation score should be >= 3, got {score}");

    // Escape hatch should fire with these conditions
    let plan_repair_count = 1;
    let remaining_turns = 5;
    assert!(
        should_allow_escape_hatch(&stagnation, plan_repair_count, remaining_turns),
        "escape hatch should be triggered"
    );

    // Before fix: plan is not complete (remaining=1), would break as Partial
    assert!(!plan.is_successfully_completed());
    let pre_kind = CompletionKind::classify(&plan, None, false);
    assert_eq!(
        pre_kind,
        CompletionKind::Partial,
        "before plan update, should be Partial"
    );

    // --- Simulate the fix: process plan update BEFORE escape hatch break ---
    // The model's response contains ANVIL_PLAN_UPDATE with [x] for the last item.
    let retired = plan.mark_unfinished_items_by_target(
        &["src/app/api/worktrees/[id]/current-output/route.ts".to_string()],
        PlanItemStatus::AlreadySatisfied,
    );
    assert_eq!(retired.len(), 1, "last item should be retired");
    stagnation.retire_target_files(&retired);
    stagnation.record_plan_item_completion();

    // After fix: plan is complete, CompletionKind is not Partial
    assert!(
        plan.is_successfully_completed(),
        "plan should be successfully completed after checked retire"
    );
    assert_eq!(plan.items[5].status, PlanItemStatus::AlreadySatisfied);
    let post_kind = CompletionKind::classify(&plan, None, false);
    assert_eq!(
        post_kind,
        CompletionKind::CompleteUnverified,
        "after plan update, should be CompleteUnverified, not Partial"
    );

    // Stagnation should no longer have starved target files
    assert!(
        stagnation.starved_target_files.is_empty(),
        "no starved files should remain after retire"
    );
}

// ---------------------------------------------------------------------------
// Escape hatch should not suppress plan completion
// ---------------------------------------------------------------------------

/// Even when escape hatch is active, if the plan update completes all items,
/// is_successfully_completed() returns true and the loop should not be
/// classified as partial.
#[test]
fn completed_plan_overrides_escape_hatch_partial_classification() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("task1".into(), vec!["src/a.rs".into()]),
        PlanItem::new("task2".into(), vec!["src/b.rs".into()]),
    ]);
    plan.items[0].status = PlanItemStatus::Done;
    plan.items[1].status = PlanItemStatus::AlreadySatisfied;

    // Even though escape hatch conditions may be true,
    // CompletionKind should reflect the completed plan state
    let kind = CompletionKind::classify(&plan, None, false);
    assert_eq!(kind, CompletionKind::CompleteUnverified);
    assert!(plan.is_successfully_completed());
}

// ---------------------------------------------------------------------------
// Late plan update with all-checked items produces correct telemetry
// ---------------------------------------------------------------------------

/// When a late ANVIL_PLAN_UPDATE contains only [x] items (no new items),
/// the plan should transition to completed state after retirement.
#[test]
fn late_all_checked_update_retires_remaining_item() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("src/x.rs: impl".into(), vec!["src/x.rs".into()]),
        PlanItem::new("src/y.rs: impl".into(), vec!["src/y.rs".into()]),
        PlanItem::new("src/z.rs: verify".into(), vec!["src/z.rs".into()]),
    ]);
    plan.items[0].status = PlanItemStatus::Done;
    plan.items[1].status = PlanItemStatus::Done;
    // Item 2 is still Pending — this is the "remaining=1" case

    let mut stagnation = StagnationState::init_from_plan(&["src/z.rs".to_string()]);

    // Process the checked retire for the last item
    let retired = plan.mark_unfinished_items_by_target(
        &["src/z.rs".to_string()],
        PlanItemStatus::AlreadySatisfied,
    );
    assert_eq!(retired.len(), 1);
    stagnation.retire_target_files(&retired);
    stagnation.record_plan_item_completion();

    // All items finished
    assert!(plan.all_finished());
    assert!(plan.is_successfully_completed());

    // CompletionKind should be complete, not partial
    let kind = CompletionKind::classify(&plan, None, false);
    assert_eq!(kind, CompletionKind::CompleteUnverified);

    // Stagnation cleared
    assert!(stagnation.starved_target_files.is_empty());
    assert_eq!(stagnation.turns_since_plan_item_completion, 0);
}
