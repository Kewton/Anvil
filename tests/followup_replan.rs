//! Tests for Issue #305: follow-up ANVIL_PLAN on active plan treated as replan.
//!
//! Covers:
//! - Shared pipeline (supersede, dedup, checked-first retire) via public types
//! - Stagnation state merge behavior
//! - Telemetry counter semantics
//! - Regression: initial registration and ANVIL_PLAN_UPDATE unchanged

use anvil::app::stagnation_state::StagnationState;
use anvil::contracts::{ExecutionPlan, PlanItem, PlanItemStatus};

// ---------------------------------------------------------------------------
// Helper: simulate the replan pipeline (checked-first → supersede → dedup → append)
// This mirrors apply_plan_update_pipeline() using public APIs.
// ---------------------------------------------------------------------------

/// Simulate the pipeline: supersede stale → dedup → append.
/// Returns true if any meaningful change occurred.
/// CB-001 fix: count superseded items before/after to detect new supersedes only.
fn simulate_pipeline(plan: &mut ExecutionPlan, new_items: Vec<PlanItem>) -> bool {
    let superseded_before = plan
        .items
        .iter()
        .filter(|i| i.status == PlanItemStatus::Superseded)
        .count();
    plan.supersede_stale_items(&new_items);
    let superseded_after = plan
        .items
        .iter()
        .filter(|i| i.status == PlanItemStatus::Superseded)
        .count();
    let had_supersede = superseded_after > superseded_before;
    let deduped = plan.deduplicate_new_items(new_items);
    if deduped.is_empty() {
        return had_supersede;
    }
    plan.append_items(deduped);
    true
}

// ---------------------------------------------------------------------------
// replan_basic: follow-up ANVIL_PLAN items appended to active plan
// ---------------------------------------------------------------------------

#[test]
fn replan_basic() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new(
            "src/a.rs: implement feature".into(),
            vec!["src/a.rs".into()],
        ),
        PlanItem::new("src/b.rs: add tests".into(), vec!["src/b.rs".into()]),
    ]);
    plan.mark_in_progress(0);

    // Simulate follow-up ANVIL_PLAN with a new item
    let new_items = vec![PlanItem::new(
        "src/c.rs: new task".into(),
        vec!["src/c.rs".into()],
    )];
    let changed = simulate_pipeline(&mut plan, new_items);

    assert!(changed);
    assert_eq!(plan.items.len(), 3);
    assert_eq!(plan.items[2].description, "src/c.rs: new task");
}

// ---------------------------------------------------------------------------
// replan_telemetry: plan_update_count increases, plan_registration_count unchanged
// ---------------------------------------------------------------------------

#[test]
fn replan_telemetry() {
    use anvil::contracts::AgentTelemetry;

    let mut telemetry = AgentTelemetry::new();
    // Initial registration
    telemetry.record_plan_registration();
    telemetry.record_anvil_plan_visible();
    assert_eq!(telemetry.plan_registration_count, 1);
    assert_eq!(telemetry.plan_update_count, 0);
    assert_eq!(telemetry.anvil_plan_visible_count, 1);

    // Replan: record_plan_update + record_anvil_plan_visible (NOT record_plan_registration)
    telemetry.record_plan_update();
    telemetry.record_anvil_plan_visible();
    assert_eq!(telemetry.plan_registration_count, 1); // unchanged
    assert_eq!(telemetry.plan_update_count, 1); // increased
    assert_eq!(telemetry.anvil_plan_visible_count, 2); // increased
}

// ---------------------------------------------------------------------------
// replan_supersede_stale: stale targets superseded
// ---------------------------------------------------------------------------

#[test]
fn replan_supersede_stale() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "src/a.rs: old approach".into(),
        vec!["src/a.rs".into()],
    )]);
    plan.mark_in_progress(0);

    // New item with same target → supersede old
    let new_items = vec![PlanItem::new(
        "src/a.rs: new approach".into(),
        vec!["src/a.rs".into()],
    )];
    let changed = simulate_pipeline(&mut plan, new_items);

    assert!(changed);
    assert_eq!(plan.items[0].status, PlanItemStatus::Superseded);
    // New item appended (not deduped because old was superseded and dedup still finds target match)
    // Actually, deduplicate_new_items checks ALL items including Superseded ones for target match.
    // Since the superseded item still has same target, the new item IS deduped.
    // But had_supersede=true, so changed=true.
    assert!(
        plan.items
            .iter()
            .any(|i| i.status == PlanItemStatus::Superseded)
    );
}

// ---------------------------------------------------------------------------
// replan_dedup_existing: duplicate items not appended
// ---------------------------------------------------------------------------

#[test]
fn replan_dedup_existing() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "src/a.rs: task".into(),
        vec!["src/a.rs".into()],
    )]);
    plan.mark_in_progress(0);
    // Simulate mutation to prevent supersede
    plan.items[0].mutated_files.push("src/a.rs".into());

    // Same item → deduped
    let new_items = vec![PlanItem::new(
        "src/a.rs: task".into(),
        vec!["src/a.rs".into()],
    )];
    let changed = simulate_pipeline(&mut plan, new_items);

    assert!(!changed); // No supersede, all deduped
    assert_eq!(plan.items.len(), 1);
}

// ---------------------------------------------------------------------------
// replan_done_items_not_appended: done items not re-added
// ---------------------------------------------------------------------------

#[test]
fn replan_done_items_not_appended() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("src/a.rs: task".into(), vec!["src/a.rs".into()]),
        PlanItem::new("src/b.rs: task2".into(), vec!["src/b.rs".into()]),
    ]);
    plan.mark_done(0);
    plan.mark_in_progress(1);

    // Replan includes the done item and a new one
    let new_items = vec![
        PlanItem::new("src/a.rs: task".into(), vec!["src/a.rs".into()]),
        PlanItem::new("src/c.rs: new task".into(), vec!["src/c.rs".into()]),
    ];
    let changed = simulate_pipeline(&mut plan, new_items);

    assert!(changed);
    // src/a.rs deduped (already exists), src/c.rs appended
    let c_items: Vec<_> = plan
        .items
        .iter()
        .filter(|i| i.target_files.iter().any(|f| f == "src/c.rs"))
        .collect();
    assert_eq!(c_items.len(), 1);
    assert_eq!(plan.items.len(), 3); // 2 original + 1 new
}

// ---------------------------------------------------------------------------
// replan_checked_first_retire: [x] checked items retired
// ---------------------------------------------------------------------------

#[test]
fn replan_checked_first_retire() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("src/a.rs: task".into(), vec!["src/a.rs".into()]),
        PlanItem::new("src/b.rs: task2".into(), vec!["src/b.rs".into()]),
    ]);
    plan.mark_in_progress(0);

    // Simulate [x] checked retire for src/a.rs
    let retired = plan.mark_unfinished_items_by_target(
        &["src/a.rs".to_string()],
        PlanItemStatus::AlreadySatisfied,
    );
    assert_eq!(retired.len(), 1);
    assert_eq!(plan.items[0].status, PlanItemStatus::AlreadySatisfied);
}

// ---------------------------------------------------------------------------
// replan_subset_items: partial replan works correctly
// ---------------------------------------------------------------------------

#[test]
fn replan_subset_items() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("src/a.rs: task".into(), vec!["src/a.rs".into()]),
        PlanItem::new("src/b.rs: task2".into(), vec!["src/b.rs".into()]),
        PlanItem::new("src/c.rs: task3".into(), vec!["src/c.rs".into()]),
    ]);
    plan.mark_in_progress(0);

    // Replan with only one new item
    let new_items = vec![PlanItem::new(
        "src/d.rs: new task".into(),
        vec!["src/d.rs".into()],
    )];
    let changed = simulate_pipeline(&mut plan, new_items);

    assert!(changed);
    assert_eq!(plan.items.len(), 4);
}

// ---------------------------------------------------------------------------
// replan_no_effect_returns_noblock: all deduped returns false (NoBlock equivalent)
// ---------------------------------------------------------------------------

#[test]
fn replan_no_effect_returns_noblock() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "src/a.rs: task".into(),
        vec!["src/a.rs".into()],
    )]);
    plan.mark_in_progress(0);
    // Simulate mutation to prevent supersede
    plan.items[0].mutated_files.push("src/a.rs".into());

    let new_items = vec![PlanItem::new(
        "src/a.rs: task".into(),
        vec!["src/a.rs".into()],
    )];
    let changed = simulate_pipeline(&mut plan, new_items);

    assert!(!changed); // No effect → NoBlock equivalent
}

// ---------------------------------------------------------------------------
// first_registration_unchanged: regression test for initial registration
// ---------------------------------------------------------------------------

#[test]
fn first_registration_unchanged() {
    let items = vec![PlanItem::new(
        "src/a.rs: implement feature".into(),
        vec!["src/a.rs".into()],
    )];
    let plan = ExecutionPlan::new(items);
    assert_eq!(plan.items.len(), 1);
    assert_eq!(plan.items[0].status, PlanItemStatus::Pending);
}

// ---------------------------------------------------------------------------
// plan_update_unchanged: regression test for ANVIL_PLAN_UPDATE
// ---------------------------------------------------------------------------

#[test]
fn plan_update_unchanged() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "src/a.rs: task".into(),
        vec!["src/a.rs".into()],
    )]);
    plan.mark_in_progress(0);

    // Simulate ANVIL_PLAN_UPDATE with new items
    let new_items = vec![PlanItem::new(
        "src/b.rs: new task".into(),
        vec!["src/b.rs".into()],
    )];
    plan.supersede_stale_items(&new_items);
    let deduped = plan.deduplicate_new_items(new_items);
    assert_eq!(deduped.len(), 1);
    plan.append_items(deduped);

    assert_eq!(plan.items.len(), 2);
}

// ---------------------------------------------------------------------------
// Stagnation merge: replan merges new targets, does not reinitialize
// ---------------------------------------------------------------------------

#[test]
fn replan_stagnation_merge_not_reinitialize() {
    let mut stagnation =
        StagnationState::init_from_plan(&["src/a.rs".to_string(), "src/b.rs".to_string()]);
    assert_eq!(stagnation.starved_target_files.len(), 2);

    // Simulate replan: add new target without reinitializing
    let new_target = "src/c.rs".to_string();
    let existing = &stagnation.starved_target_files;
    let is_new = !existing
        .iter()
        .any(|sf| anvil::contracts::ExecutionPlan::path_matches(sf, &new_target));
    assert!(is_new);
    stagnation.starved_target_files.push(new_target);

    assert_eq!(stagnation.starved_target_files.len(), 3);
    // Original targets still present
    assert!(
        stagnation
            .starved_target_files
            .iter()
            .any(|f| f == "src/a.rs")
    );
    assert!(
        stagnation
            .starved_target_files
            .iter()
            .any(|f| f == "src/b.rs")
    );
    assert!(
        stagnation
            .starved_target_files
            .iter()
            .any(|f| f == "src/c.rs")
    );
}
