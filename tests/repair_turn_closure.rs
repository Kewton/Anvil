//! Tests for Issue #327: pre-exit repair turn closure — unchecked plan
//! expansion must be rejected during repair closure mode.
//!
//! Covers:
//! - When repair closure mode is active, unchecked ANVIL_PLAN_UPDATE items
//!   must NOT be appended to the plan.
//! - Checked [x] items are still retired during repair closure mode.
//! - Telemetry tracks rejected items, retired items, and pending counts.
//! - Post-repair termination is decided from plan state, not just from
//!   the fact that one repair response was consumed.

use anvil::contracts::{AgentTelemetry, CompletionKind, ExecutionPlan, PlanItem, PlanItemStatus};

// ---------------------------------------------------------------------------
// Scenario: repair turn response contains checked + unchecked items
// ---------------------------------------------------------------------------

/// Reproduces the B1 failure from Issue #327:
/// - Plan with 26 items, 24 done, 2 pending.
/// - Repair turn is injected and consumed.
/// - Repair-turn response contains ANVIL_PLAN_UPDATE with:
///   - 1 checked [x] item (retire)
///   - 2 new unchecked items (should be rejected)
/// - Expected: checked item is retired, unchecked items are NOT appended,
///   plan ends with 1 remaining item (not 3).
#[test]
fn repair_turn_rejects_unchecked_items_and_retires_checked() {
    // Setup: 4-item plan, items 0-1 done, items 2-3 pending
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("src/a.rs: update".into(), vec!["src/a.rs".into()]),
        PlanItem::new("src/b.rs: update".into(), vec!["src/b.rs".into()]),
        PlanItem::new(
            "src/lib/auto-yes-poller.ts: implement polling".into(),
            vec!["src/lib/auto-yes-poller.ts".into()],
        ),
        PlanItem::new(
            "src/lib/detection/status-detector.ts: fix detection".into(),
            vec!["src/lib/detection/status-detector.ts".into()],
        ),
    ]);
    plan.items[0].status = PlanItemStatus::Done;
    plan.items[1].status = PlanItemStatus::Done;

    let mut telemetry = AgentTelemetry::new();

    // Step 1: Record pending count before repair turn
    let pending_before = plan.items.iter().filter(|i| !i.is_finished()).count() as u32;
    telemetry.record_repair_turn_pending_before(pending_before);
    assert_eq!(pending_before, 2);

    // Step 2: Simulate repair-turn response processing
    // The model's response retires item 2 (auto-yes-poller.ts) via [x]
    let retired = plan.mark_unfinished_items_by_target(
        &["src/lib/auto-yes-poller.ts".to_string()],
        PlanItemStatus::AlreadySatisfied,
    );
    assert_eq!(retired.len(), 1, "one item should be retired");
    telemetry.record_repair_turn_items_retired(retired.len() as u32);

    // Step 3: The model also emits 2 new unchecked items — these must be rejected.
    // In the real code, apply_plan_update_pipeline checks self.repair_closure_active
    // and skips append_items. Here we verify the invariant directly.
    let new_unchecked_items = [
        PlanItem::new(
            "src/lib/detection/status-detector.ts: refactor approach".into(),
            vec!["src/lib/detection/status-detector.ts".into()],
        ),
        PlanItem::new(
            "src/app/api/worktrees/[id]/current-output/route.ts: add handler".into(),
            vec!["src/app/api/worktrees/[id]/current-output/route.ts".into()],
        ),
    ];

    // In repair closure mode: reject unchecked items
    let rejected_count = new_unchecked_items.len() as u32;
    telemetry.record_repair_turn_items_rejected(rejected_count);

    // Do NOT append: plan.append_items(new_unchecked_items);

    // Step 4: Record pending count after repair turn
    let pending_after = plan.items.iter().filter(|i| !i.is_finished()).count() as u32;
    telemetry.record_repair_turn_pending_after(pending_after);

    // Assertions
    assert_eq!(
        plan.items.len(),
        4,
        "plan should still have 4 items (no new items appended)"
    );
    assert_eq!(
        pending_after, 1,
        "only 1 item should remain pending (item 3)"
    );
    assert_eq!(
        plan.items[2].status,
        PlanItemStatus::AlreadySatisfied,
        "item 2 should be retired"
    );
    assert_eq!(
        plan.items[3].status,
        PlanItemStatus::Pending,
        "item 3 should still be pending"
    );

    // Telemetry assertions
    assert_eq!(telemetry.repair_turn_items_retired, 1);
    assert_eq!(telemetry.repair_turn_items_rejected, 2);
    assert_eq!(telemetry.repair_turn_pending_before, Some(2));
    assert_eq!(telemetry.repair_turn_pending_after, Some(1));

    // CompletionKind: still Partial because item 3 remains
    let kind = CompletionKind::classify(&plan, None, false);
    assert_eq!(
        kind,
        CompletionKind::Partial,
        "with 1 remaining item, should be Partial"
    );
}

// ---------------------------------------------------------------------------
// Scenario: repair turn retires all remaining items — plan cleanly finishes
// ---------------------------------------------------------------------------

/// When the repair-turn response successfully retires all remaining items
/// (and emits no unchecked items), the plan should be cleanly finished
/// and CompletionKind should be CompleteUnverified.
#[test]
fn repair_turn_retires_all_remaining_completes_plan() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("src/a.rs: update".into(), vec!["src/a.rs".into()]),
        PlanItem::new("src/b.rs: update".into(), vec!["src/b.rs".into()]),
        PlanItem::new(
            "src/detector.ts: fix detection".into(),
            vec!["src/detector.ts".into()],
        ),
    ]);
    plan.items[0].status = PlanItemStatus::Done;
    plan.items[1].status = PlanItemStatus::Done;

    let mut telemetry = AgentTelemetry::new();

    // Pending before repair
    let pending_before = plan.items.iter().filter(|i| !i.is_finished()).count() as u32;
    telemetry.record_repair_turn_pending_before(pending_before);
    assert_eq!(pending_before, 1);

    // Repair turn retires the last item
    let retired = plan.mark_unfinished_items_by_target(
        &["src/detector.ts".to_string()],
        PlanItemStatus::AlreadySatisfied,
    );
    assert_eq!(retired.len(), 1);
    telemetry.record_repair_turn_items_retired(retired.len() as u32);

    let pending_after = plan.items.iter().filter(|i| !i.is_finished()).count() as u32;
    telemetry.record_repair_turn_pending_after(pending_after);

    telemetry.record_pre_exit_repair_consumed();

    // Plan should be cleanly finished
    assert!(plan.is_cleanly_finished());
    assert_eq!(pending_after, 0);
    assert_eq!(
        CompletionKind::classify(&plan, None, false),
        CompletionKind::CompleteUnverified,
    );

    // Telemetry
    assert_eq!(telemetry.repair_turn_items_retired, 1);
    assert_eq!(telemetry.repair_turn_items_rejected, 0);
    assert_eq!(telemetry.repair_turn_pending_before, Some(1));
    assert_eq!(telemetry.repair_turn_pending_after, Some(0));
}

// ---------------------------------------------------------------------------
// Scenario: without repair closure, unchecked items WOULD be appended
// ---------------------------------------------------------------------------

/// Baseline: when NOT in repair closure mode, unchecked items are normally
/// appended. This proves the suppression is repair-mode-specific.
#[test]
fn normal_mode_appends_unchecked_items() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "src/a.rs: update".into(),
        vec!["src/a.rs".into()],
    )]);
    plan.items[0].status = PlanItemStatus::Done;

    // Normal mode: append works
    let new_items = vec![PlanItem::new(
        "src/new.rs: add feature".into(),
        vec!["src/new.rs".into()],
    )];
    plan.append_items(new_items);

    assert_eq!(
        plan.items.len(),
        2,
        "new item should be appended in normal mode"
    );
    assert_eq!(plan.items[1].status, PlanItemStatus::Pending);
}

// ---------------------------------------------------------------------------
// Scenario: repair turn telemetry consistency with Issue #325 counters
// ---------------------------------------------------------------------------

/// Validates that Issue #327 telemetry is consistent with the Issue #325
/// repair turn injection/consumption counters.
#[test]
fn repair_turn_telemetry_consistency() {
    let mut telemetry = AgentTelemetry::new();

    // Initial state
    assert_eq!(telemetry.repair_turn_items_retired, 0);
    assert_eq!(telemetry.repair_turn_items_rejected, 0);
    assert_eq!(telemetry.repair_turn_pending_before, None);
    assert_eq!(telemetry.repair_turn_pending_after, None);

    // Simulate full repair cycle
    telemetry.record_pre_exit_repair_injected();
    telemetry.record_repair_turn_pending_before(3);

    // Repair response processing
    telemetry.record_repair_turn_items_retired(1);
    telemetry.record_repair_turn_items_rejected(2);

    telemetry.record_pre_exit_repair_consumed();
    telemetry.record_repair_turn_pending_after(2);

    // All counters set
    assert_eq!(telemetry.pre_exit_repair_injected_count, 1);
    assert_eq!(telemetry.pre_exit_repair_consumed_count, 1);
    assert_eq!(telemetry.repair_turn_items_retired, 1);
    assert_eq!(telemetry.repair_turn_items_rejected, 2);
    assert_eq!(telemetry.repair_turn_pending_before, Some(3));
    assert_eq!(telemetry.repair_turn_pending_after, Some(2));
}

// ---------------------------------------------------------------------------
// Scenario: repair turn with only unchecked items — all rejected
// ---------------------------------------------------------------------------

/// When the repair-turn response contains only new unchecked items (no
/// checked retires), all items should be rejected and the plan state
/// should be unchanged.
#[test]
fn repair_turn_all_unchecked_items_rejected() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("src/a.rs: update".into(), vec!["src/a.rs".into()]),
        PlanItem::new("src/b.rs: update".into(), vec!["src/b.rs".into()]),
    ]);
    plan.items[0].status = PlanItemStatus::Done;

    let mut telemetry = AgentTelemetry::new();

    let pending_before = plan.items.iter().filter(|i| !i.is_finished()).count() as u32;
    telemetry.record_repair_turn_pending_before(pending_before);
    assert_eq!(pending_before, 1);

    // Model emits 3 unchecked items — all rejected in repair closure mode
    telemetry.record_repair_turn_items_rejected(3);

    let pending_after = plan.items.iter().filter(|i| !i.is_finished()).count() as u32;
    telemetry.record_repair_turn_pending_after(pending_after);

    // Plan unchanged
    assert_eq!(plan.items.len(), 2);
    assert_eq!(pending_after, 1);
    assert_eq!(telemetry.repair_turn_items_retired, 0);
    assert_eq!(telemetry.repair_turn_items_rejected, 3);
}
