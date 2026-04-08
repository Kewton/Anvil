//! Tests for Issue #301: stale plan item retire via ANVIL_PLAN_UPDATE [x] markers.
//!
//! Covers:
//! - PlanItemStatus::AlreadySatisfied integration with final gate
//! - StagnationState::retire_target_files
//! - mark_unfinished_items_by_target end-to-end
//! - try_register_plan [x] items stay Pending (parse_plan_items unchanged)

use anvil::app::stagnation_state::StagnationState;
use anvil::contracts::{ExecutionPlan, FinalGateDecision, PlanItem, PlanItemStatus};

// ---------------------------------------------------------------------------
// End-to-end: ANVIL_PLAN_UPDATE [x] → AlreadySatisfied → final gate Allow
// ---------------------------------------------------------------------------

#[test]
fn plan_update_checked_items_allow_final_gate() {
    // Setup: plan with 3 items, item 0 Done, items 1+2 pending
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new(
            "src/a.rs: implement feature".into(),
            vec!["src/a.rs".into()],
        ),
        PlanItem::new("src/b.rs: update module".into(), vec!["src/b.rs".into()]),
        PlanItem::new("src/c.rs: add tests".into(), vec!["src/c.rs".into()]),
    ]);
    plan.mark_done(0);
    plan.mark_in_progress(1);

    // Simulate [x] markers retiring items 1 and 2
    let retired = plan.mark_unfinished_items_by_target(
        &["src/b.rs".to_string(), "src/c.rs".to_string()],
        PlanItemStatus::AlreadySatisfied,
    );

    assert_eq!(retired.len(), 2);
    assert!(retired.contains(&"src/b.rs".to_string()));
    assert!(retired.contains(&"src/c.rs".to_string()));

    // Verify statuses
    assert_eq!(plan.items[0].status, PlanItemStatus::Done);
    assert_eq!(plan.items[1].status, PlanItemStatus::AlreadySatisfied);
    assert_eq!(plan.items[2].status, PlanItemStatus::AlreadySatisfied);

    // Final gate should Allow
    assert_eq!(plan.check_final_gate(), FinalGateDecision::Allow);
    assert!(plan.is_successfully_completed());
}

// ---------------------------------------------------------------------------
// Stagnation excludes retired targets
// ---------------------------------------------------------------------------

#[test]
fn stagnation_retire_target_files_removes_from_starved() {
    let mut stagnation = StagnationState::init_from_plan(&[
        "src/a.rs".to_string(),
        "src/b.rs".to_string(),
        "src/c.rs".to_string(),
    ]);
    assert_eq!(stagnation.starved_target_files.len(), 3);

    // Retire src/a.rs and src/c.rs
    stagnation.retire_target_files(&["src/a.rs".to_string(), "src/c.rs".to_string()]);

    assert_eq!(stagnation.starved_target_files.len(), 1);
    assert_eq!(stagnation.starved_target_files[0], "src/b.rs");
}

#[test]
fn stagnation_retire_target_files_empty_list_noop() {
    let mut stagnation = StagnationState::init_from_plan(&["src/a.rs".to_string()]);
    stagnation.retire_target_files(&[]);
    assert_eq!(stagnation.starved_target_files.len(), 1);
}

#[test]
fn stagnation_retire_target_files_suffix_matching() {
    let mut stagnation = StagnationState::init_from_plan(&["src/app/mod.rs".to_string()]);
    // Retire with a shorter suffix path
    stagnation.retire_target_files(&["mod.rs".to_string()]);
    assert!(stagnation.starved_target_files.is_empty());
}

// ---------------------------------------------------------------------------
// try_register_plan [x] stays Pending
// ---------------------------------------------------------------------------

#[test]
fn parse_plan_items_checked_items_stay_pending() {
    // parse_plan_items should NOT change based on [x] — all items start as Pending
    let block = "- [x] src/done.rs: already done\n- [ ] src/todo.rs: still todo";
    let items = anvil::agent::parse_plan_items(block);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].status, PlanItemStatus::Pending);
    assert_eq!(items[1].status, PlanItemStatus::Pending);
}

// ---------------------------------------------------------------------------
// AlreadySatisfied + Done mixed plan is successfully completed
// ---------------------------------------------------------------------------

#[test]
fn mixed_already_satisfied_and_done_is_successful() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("task1".into(), vec!["src/a.rs".into()]),
        PlanItem::new("task2".into(), vec!["src/b.rs".into()]),
        PlanItem::new("task3".into(), vec!["src/c.rs".into()]),
    ]);
    plan.items[0].status = PlanItemStatus::Done;
    plan.items[1].status = PlanItemStatus::AlreadySatisfied;
    plan.items[2].status = PlanItemStatus::Done;
    assert!(plan.is_successfully_completed());
    assert!(plan.all_finished());
}

// ---------------------------------------------------------------------------
// AlreadySatisfied-only plan is successfully completed
// ---------------------------------------------------------------------------

#[test]
fn already_satisfied_only_plan_is_successful() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new("task1".into(), vec!["src/a.rs".into()])]);
    plan.items[0].status = PlanItemStatus::AlreadySatisfied;
    assert!(plan.is_successfully_completed());
}

// ---------------------------------------------------------------------------
// AlreadySatisfied not counted as Blocked
// ---------------------------------------------------------------------------

#[test]
fn already_satisfied_not_blocked() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new("task1".into(), vec![])]);
    plan.items[0].status = PlanItemStatus::AlreadySatisfied;
    assert!(!plan.has_blocked_items());
}

// ---------------------------------------------------------------------------
// Format checklist shows [=] for AlreadySatisfied
// ---------------------------------------------------------------------------

#[test]
fn format_checklist_shows_already_satisfied_marker() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("done task".into(), vec![]),
        PlanItem::new("retired task".into(), vec![]),
        PlanItem::new("pending task".into(), vec![]),
    ]);
    plan.items[0].status = PlanItemStatus::Done;
    plan.items[1].status = PlanItemStatus::AlreadySatisfied;
    let checklist = plan.format_checklist();
    assert!(checklist.contains("[x] done task"));
    assert!(checklist.contains("[=] retired task"));
    assert!(checklist.contains("[ ] pending task"));
}

// ---------------------------------------------------------------------------
// DR4-001: partial target match does not retire
// ---------------------------------------------------------------------------

#[test]
fn dr4_001_partial_target_match_no_retire() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "src/a.rs, src/b.rs: update both".into(),
        vec!["src/a.rs".into(), "src/b.rs".into()],
    )]);
    // Only provide one of two targets
    let retired = plan.mark_unfinished_items_by_target(
        &["src/a.rs".to_string()],
        PlanItemStatus::AlreadySatisfied,
    );
    assert!(retired.is_empty());
    assert_eq!(plan.items[0].status, PlanItemStatus::Pending);
}

// ---------------------------------------------------------------------------
// Stagnation + retire integration: retired files excluded from stagnation scoring
// ---------------------------------------------------------------------------

#[test]
fn stagnation_after_retire_excludes_retired_from_score() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("src/a.rs: update".into(), vec!["src/a.rs".into()]),
        PlanItem::new("src/b.rs: update".into(), vec!["src/b.rs".into()]),
    ]);
    let mut stagnation =
        StagnationState::init_from_plan(&["src/a.rs".to_string(), "src/b.rs".to_string()]);

    // Retire src/a.rs via checked marker
    let retired = plan.mark_unfinished_items_by_target(
        &["src/a.rs".to_string()],
        PlanItemStatus::AlreadySatisfied,
    );
    stagnation.retire_target_files(&retired);

    // Only src/b.rs should remain starved
    assert_eq!(stagnation.starved_target_files.len(), 1);
    assert_eq!(stagnation.starved_target_files[0], "src/b.rs");
}

// ---------------------------------------------------------------------------
// Workset excludes AlreadySatisfied items
// ---------------------------------------------------------------------------

#[test]
fn current_workset_excludes_already_satisfied() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("task1".into(), vec!["src/a.rs".into()]),
        PlanItem::new("task2".into(), vec!["src/b.rs".into()]),
        PlanItem::new("task3".into(), vec!["src/c.rs".into()]),
    ]);
    plan.items[0].status = PlanItemStatus::AlreadySatisfied;
    plan.mark_in_progress(1);

    let workset = plan.current_workset();
    // Should only contain indices 1 and 2 (not 0 which is AlreadySatisfied)
    assert!(!workset.contains(&0));
    assert!(workset.contains(&1));
    assert!(workset.contains(&2));
}

// ---------------------------------------------------------------------------
// CB-001 regression: separate [x] items must NOT combine targets for retire
// ---------------------------------------------------------------------------

#[test]
fn cb001_separate_checked_items_do_not_combine_targets() {
    // Existing plan has a multi-target item: src/a.rs, src/b.rs
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "multi-target task".into(),
        vec!["src/a.rs".into(), "src/b.rs".into()],
    )]);
    plan.mark_in_progress(0);

    // Two separate [x] items each with ONE target should NOT retire the
    // multi-target item because each checked item individually has only 1
    // target while the existing item has 2 (DR4-001 target-count safety).
    let targets_a = vec!["src/a.rs".into()];
    let retired_a =
        plan.mark_unfinished_items_by_target(&targets_a, PlanItemStatus::AlreadySatisfied);
    let targets_b = vec!["src/b.rs".into()];
    let retired_b =
        plan.mark_unfinished_items_by_target(&targets_b, PlanItemStatus::AlreadySatisfied);

    // Neither call should retire the multi-target item (target count mismatch: 1 vs 2)
    assert!(
        retired_a.is_empty(),
        "should not retire with partial target match"
    );
    assert!(
        retired_b.is_empty(),
        "should not retire with partial target match"
    );
    assert_eq!(plan.items[0].status, PlanItemStatus::InProgress);
}
