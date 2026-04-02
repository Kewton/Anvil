//! Tests for Issue #255: AgentPhase integration (Stage 0 + Stage 1).
//!
//! Covers CompletionKind classification, AgentTelemetry tracking,
//! strengthened item completion conditions, and ANVIL_PLAN_UPDATE append-only.

use anvil::contracts::{
    AgentTelemetry, CompletionKind, ExecutionPlan, FinalGateDecision, PlanItem, PlanItemStatus,
};

// ---------------------------------------------------------------------------
// CompletionKind basic construction & Display
// ---------------------------------------------------------------------------

#[test]
fn completion_kind_display() {
    assert_eq!(
        CompletionKind::CompleteVerified.to_string(),
        "complete_verified"
    );
    assert_eq!(
        CompletionKind::CompleteUnverified.to_string(),
        "complete_unverified"
    );
    assert_eq!(CompletionKind::Partial.to_string(), "partial");
    assert_eq!(CompletionKind::Blocked.to_string(), "blocked");
    assert_eq!(CompletionKind::Exhausted.to_string(), "exhausted");
}

#[test]
fn completion_kind_default_is_partial() {
    let kind: CompletionKind = Default::default();
    assert_eq!(kind, CompletionKind::Partial);
}

// ---------------------------------------------------------------------------
// CompletionKind::classify from ExecutionPlan state
// ---------------------------------------------------------------------------

#[test]
fn classify_complete_unverified_when_all_items_done() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("task1".into(), vec!["src/a.rs".into()]),
        PlanItem::new("task2".into(), vec!["src/b.rs".into()]),
    ]);
    plan.mark_done(0);
    plan.mark_done(1);
    // No verify available → complete_unverified
    let kind = CompletionKind::classify(&plan, None, false);
    assert_eq!(kind, CompletionKind::CompleteUnverified);
}

#[test]
fn classify_complete_verified_when_all_done_and_verify_pass() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new("task1".into(), vec!["src/a.rs".into()])]);
    plan.mark_done(0);
    let kind = CompletionKind::classify(&plan, Some(true), false);
    assert_eq!(kind, CompletionKind::CompleteVerified);
}

#[test]
fn classify_complete_unverified_when_verify_denied() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new("task1".into(), vec!["src/a.rs".into()])]);
    plan.mark_done(0);
    // verify_pass = None means unavailable/denied
    let kind = CompletionKind::classify(&plan, None, false);
    assert_eq!(kind, CompletionKind::CompleteUnverified);
}

#[test]
fn classify_blocked_when_item_blocked_and_others_pending() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("task1".into(), vec!["src/a.rs".into()]),
        PlanItem::new("task2".into(), vec!["src/b.rs".into()]),
        PlanItem::new("task3".into(), vec!["src/c.rs".into()]),
    ]);
    plan.mark_done(0);
    // task2 blocked, task3 still pending → not all finished → Blocked
    for _ in 0..PlanItem::MAX_RETRIES {
        plan.record_failure(1);
    }
    assert_eq!(plan.items[1].status, PlanItemStatus::Blocked);
    assert_eq!(plan.items[2].status, PlanItemStatus::Pending);
    let kind = CompletionKind::classify(&plan, None, false);
    assert_eq!(kind, CompletionKind::Blocked);
}

#[test]
fn classify_exhausted_when_budget_exceeded_and_not_blocked() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("task1".into(), vec!["src/a.rs".into()]),
        PlanItem::new("task2".into(), vec!["src/b.rs".into()]),
    ]);
    plan.mark_done(0);
    plan.mark_in_progress(1);
    // budget_exhausted = true, no blocked items
    let kind = CompletionKind::classify(&plan, None, true);
    assert_eq!(kind, CompletionKind::Exhausted);
}

#[test]
fn classify_partial_when_some_done_some_pending() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("task1".into(), vec!["src/a.rs".into()]),
        PlanItem::new("task2".into(), vec!["src/b.rs".into()]),
    ]);
    plan.mark_done(0);
    // task2 still pending, not budget exhausted
    let kind = CompletionKind::classify(&plan, None, false);
    assert_eq!(kind, CompletionKind::Partial);
}

#[test]
fn classify_blocked_takes_priority_over_exhausted() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("task1".into(), vec!["src/a.rs".into()]),
        PlanItem::new("task2".into(), vec!["src/b.rs".into()]),
        PlanItem::new("task3".into(), vec!["src/c.rs".into()]),
    ]);
    plan.mark_done(0);
    for _ in 0..PlanItem::MAX_RETRIES {
        plan.record_failure(1);
    }
    // task2 blocked, task3 pending → not all finished
    // Both blocked AND exhausted → blocked wins
    let kind = CompletionKind::classify(&plan, None, true);
    assert_eq!(kind, CompletionKind::Blocked);
}

#[test]
fn classify_complete_takes_priority_over_blocked() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("task1".into(), vec!["src/a.rs".into()]),
        PlanItem::new("task2".into(), vec!["src/b.rs".into()]),
    ]);
    plan.mark_done(0);
    // task2 blocked but all finished → complete wins
    for _ in 0..PlanItem::MAX_RETRIES {
        plan.record_failure(1);
    }
    assert!(plan.all_finished());
    let kind = CompletionKind::classify(&plan, None, false);
    assert_eq!(kind, CompletionKind::CompleteUnverified);
}

// ---------------------------------------------------------------------------
// AgentTelemetry
// ---------------------------------------------------------------------------

#[test]
fn telemetry_tracks_premature_final_count() {
    let mut tel = AgentTelemetry::new();
    assert_eq!(tel.premature_final_count, 0);
    tel.record_premature_final();
    tel.record_premature_final();
    assert_eq!(tel.premature_final_count, 2);
}

#[test]
fn telemetry_tracks_plan_registration() {
    let mut tel = AgentTelemetry::new();
    tel.record_plan_registration();
    assert_eq!(tel.plan_registration_count, 1);
}

#[test]
fn telemetry_tracks_sync_from_touched_files() {
    let mut tel = AgentTelemetry::new();
    tel.record_sync_from_touched_files();
    tel.record_sync_from_touched_files();
    assert_eq!(tel.sync_from_touched_files_count, 2);
}

#[test]
fn telemetry_pfrr_calculation() {
    let mut tel = AgentTelemetry::new();
    tel.record_premature_final();
    tel.record_premature_final();
    tel.total_final_requests = 5;
    let pfrr = tel.premature_final_request_rate();
    assert!((pfrr - 0.4).abs() < f64::EPSILON);
}

#[test]
fn telemetry_pfrr_zero_when_no_finals() {
    let tel = AgentTelemetry::new();
    assert_eq!(tel.premature_final_request_rate(), 0.0);
}

// ---------------------------------------------------------------------------
// Strengthened item completion: require ALL target_files mutated
// ---------------------------------------------------------------------------

#[test]
fn item_with_multiple_targets_not_done_until_all_mutated() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "update A and B".into(),
        vec!["src/a.rs".into(), "src/b.rs".into()],
    )]);
    plan.mark_in_progress(0);

    // Mutate only src/a.rs → item should NOT be done
    plan.record_mutation_success(0, "src/a.rs");
    assert_eq!(plan.items[0].status, PlanItemStatus::InProgress);

    // Mutate src/b.rs → NOW all targets satisfied → Done
    plan.record_mutation_success(0, "src/b.rs");
    assert_eq!(plan.items[0].status, PlanItemStatus::Done);
}

#[test]
fn item_with_no_targets_done_on_first_mutation() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new("generic task".into(), vec![])]);
    plan.mark_in_progress(0);
    // No target_files → any mutation completes
    plan.record_mutation_success(0, "some/file.rs");
    assert_eq!(plan.items[0].status, PlanItemStatus::Done);
}

#[test]
fn item_with_single_target_done_on_matching_mutation() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "update A".into(),
        vec!["src/a.rs".into()],
    )]);
    plan.mark_in_progress(0);
    plan.record_mutation_success(0, "src/a.rs");
    assert_eq!(plan.items[0].status, PlanItemStatus::Done);
}

#[test]
fn mutation_success_uses_fuzzy_path_matching() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "update A".into(),
        vec!["src/a.rs".into()],
    )]);
    plan.mark_in_progress(0);
    // Full path should match target "src/a.rs" via ends_with
    plan.record_mutation_success(0, "/home/user/project/src/a.rs");
    assert_eq!(plan.items[0].status, PlanItemStatus::Done);
}

#[test]
fn mutated_files_tracked_on_plan_item() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "multi".into(),
        vec!["src/a.rs".into(), "src/b.rs".into()],
    )]);
    plan.mark_in_progress(0);
    plan.record_mutation_success(0, "src/a.rs");
    assert_eq!(plan.items[0].mutated_files.len(), 1);
    assert!(
        plan.items[0]
            .mutated_files
            .contains(&"src/a.rs".to_string())
    );
}

// ---------------------------------------------------------------------------
// ANVIL_PLAN_UPDATE append-only validation
// ---------------------------------------------------------------------------

#[test]
fn append_items_does_not_modify_existing() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new("task1".into(), vec!["src/a.rs".into()])]);
    plan.mark_done(0);

    let new_items = vec![PlanItem::new("task2".into(), vec!["src/b.rs".into()])];
    plan.append_items(new_items);

    assert_eq!(plan.items.len(), 2);
    assert_eq!(plan.items[0].status, PlanItemStatus::Done); // unchanged
    assert_eq!(plan.items[0].description, "task1"); // unchanged
    assert_eq!(plan.items[1].description, "task2");
    assert_eq!(plan.items[1].status, PlanItemStatus::Pending);
}

// ---------------------------------------------------------------------------
// sync_from_touched_files remains as fallback (rescue path)
// ---------------------------------------------------------------------------

#[test]
fn sync_from_touched_files_requires_all_targets() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "update A and B".into(),
        vec!["src/a.rs".into(), "src/b.rs".into()],
    )]);
    plan.mark_in_progress(0);

    // Only one target touched → should NOT complete
    let touched = vec!["src/a.rs".to_string()];
    plan.sync_from_touched_files(&touched);
    assert_eq!(plan.items[0].status, PlanItemStatus::InProgress);

    // Both touched → should complete
    let touched = vec!["src/a.rs".to_string(), "src/b.rs".to_string()];
    plan.sync_from_touched_files(&touched);
    assert_eq!(plan.items[0].status, PlanItemStatus::Done);
}

// ---------------------------------------------------------------------------
// Final gate with strengthened conditions
// ---------------------------------------------------------------------------

#[test]
fn final_gate_incomplete_when_items_pending() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("task1".into(), vec!["src/a.rs".into()]),
        PlanItem::new("task2".into(), vec!["src/b.rs".into()]),
    ]);
    plan.mark_in_progress(0);
    match plan.check_final_gate() {
        FinalGateDecision::Incomplete {
            remaining, total, ..
        } => {
            assert_eq!(remaining, 2);
            assert_eq!(total, 2);
        }
        other => panic!("expected Incomplete, got {:?}", other),
    }
}

#[test]
fn final_gate_allows_when_all_done() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new("task1".into(), vec!["src/a.rs".into()])]);
    plan.mark_done(0);
    assert_eq!(plan.check_final_gate(), FinalGateDecision::Allow);
}
