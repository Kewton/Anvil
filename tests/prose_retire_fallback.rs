//! Tests for Issue #307: prose-only "already implemented" confirmation from LLMs
//! doesn't get converted to structured plan retirement, causing fallback
//! completion to classify as `partial`.
//!
//! Covers:
//! - FallbackComplete suppression when plan has unfinished items
//! - FallbackComplete fires when plan is all-finished or empty
//! - ANVIL_PLAN_UPDATE [x] marker retirement completing a plan
//! - PROMPT_TOOL_RULES contains [x] retirement guidance

use anvil::app::phase_estimator::{PhaseAction, PhaseEstimator};
use anvil::contracts::{ExecutionPlan, PlanItem, PlanItemStatus};

// ---------------------------------------------------------------------------
// Helper: simulate the FallbackComplete suppression condition from agentic.rs
// ---------------------------------------------------------------------------

/// Mirrors the logic added in Issue #307 to agentic.rs:
/// Returns `true` if FallbackComplete should be suppressed (plan is non-empty
/// and has unfinished items), `false` if it should fire normally.
fn should_suppress_fallback(plan: &ExecutionPlan, action: &PhaseAction) -> bool {
    if let PhaseAction::FallbackComplete = action {
        !plan.is_empty() && !plan.all_finished()
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Test 1: FallbackComplete suppressed when plan has unfinished items
// ---------------------------------------------------------------------------

#[test]
fn fallback_complete_suppressed_when_plan_incomplete() {
    // Create a plan with 2 items: one done, one pending
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new(
            "src/a.rs: implement feature".into(),
            vec!["src/a.rs".into()],
        ),
        PlanItem::new("src/b.rs: add tests".into(), vec!["src/b.rs".into()]),
    ]);
    plan.mark_done(0);
    // Item 1 is still Pending

    assert!(!plan.is_empty());
    assert!(!plan.all_finished());

    // PhaseEstimator returns FallbackComplete
    let mut estimator = PhaseEstimator::new(3, 6, 2);
    // Simulate: write succeeded + enough reads
    estimator.record_tool_call("file.write", true);
    estimator.record_tool_call("file.read", true);
    estimator.record_tool_call("file.read", true);
    let action = estimator.check_empty_response();
    assert_eq!(action, PhaseAction::FallbackComplete);

    // With unfinished plan items, FallbackComplete should be SUPPRESSED
    assert!(
        should_suppress_fallback(&plan, &action),
        "FallbackComplete must be suppressed when plan has unfinished items"
    );
}

// ---------------------------------------------------------------------------
// Test 2: FallbackComplete fires when all plan items finished
// ---------------------------------------------------------------------------

#[test]
fn fallback_complete_fires_when_plan_finished() {
    // Create a plan with 2 items: both done
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new(
            "src/a.rs: implement feature".into(),
            vec!["src/a.rs".into()],
        ),
        PlanItem::new("src/b.rs: add tests".into(), vec!["src/b.rs".into()]),
    ]);
    plan.mark_done(0);
    plan.mark_done(1);

    assert!(!plan.is_empty());
    assert!(plan.all_finished());

    let action = PhaseAction::FallbackComplete;

    // With all items finished, FallbackComplete should NOT be suppressed
    assert!(
        !should_suppress_fallback(&plan, &action),
        "FallbackComplete must fire when all plan items are finished"
    );
}

// ---------------------------------------------------------------------------
// Test 3: FallbackComplete fires when no plan exists
// ---------------------------------------------------------------------------

#[test]
fn fallback_complete_fires_when_no_plan() {
    let plan = ExecutionPlan::default();
    assert!(plan.is_empty());

    let action = PhaseAction::FallbackComplete;

    // With empty plan, FallbackComplete should NOT be suppressed
    assert!(
        !should_suppress_fallback(&plan, &action),
        "FallbackComplete must fire when plan is empty"
    );
}

// ---------------------------------------------------------------------------
// Test 4: [x] marker retirement via ANVIL_PLAN_UPDATE completes plan
// ---------------------------------------------------------------------------

#[test]
fn checked_marker_retirement_completes_plan() {
    // Create a plan with 3 items: item 0 done, items 1+2 pending
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new(
            "src/a.rs: implement feature".into(),
            vec!["src/a.rs".into()],
        ),
        PlanItem::new("src/b.rs: update module".into(), vec!["src/b.rs".into()]),
        PlanItem::new("src/c.rs: add tests".into(), vec!["src/c.rs".into()]),
    ]);
    plan.mark_done(0);

    // Before retirement: plan is not finished
    assert!(!plan.all_finished());

    // Simulate [x] markers retiring items 1 and 2 (AlreadySatisfied)
    let retired = plan.mark_unfinished_items_by_target(
        &["src/b.rs".to_string(), "src/c.rs".to_string()],
        PlanItemStatus::AlreadySatisfied,
    );

    assert_eq!(retired.len(), 2);
    assert_eq!(plan.items[1].status, PlanItemStatus::AlreadySatisfied);
    assert_eq!(plan.items[2].status, PlanItemStatus::AlreadySatisfied);

    // After retirement: plan IS finished
    assert!(
        plan.all_finished(),
        "plan must be all_finished after [x] retirement"
    );

    // FallbackComplete should now fire (not suppressed)
    let action = PhaseAction::FallbackComplete;
    assert!(
        !should_suppress_fallback(&plan, &action),
        "FallbackComplete must fire after [x] retirement completes plan"
    );
}

// ---------------------------------------------------------------------------
// Test 5: PROMPT_TOOL_RULES contains [x] retirement guidance
// ---------------------------------------------------------------------------

#[test]
fn prompt_tool_rules_contains_checked_retirement_guidance() {
    let prompt = anvil::agent::tool_protocol_system_prompt_all_tools(&[], None);
    assert!(
        prompt.contains("[x] in ANVIL_PLAN_UPDATE"),
        "system prompt should contain [x] retirement guidance for ANVIL_PLAN_UPDATE. \
         Got prompt excerpt around ANVIL_PLAN_UPDATE: {:?}",
        prompt
            .find("ANVIL_PLAN_UPDATE")
            .map(|pos| &prompt[pos.saturating_sub(50)..prompt.len().min(pos + 100)])
    );
}
