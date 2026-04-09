//! Tests for Issue #315: no-path / summary-only ANVIL_PLAN items must not block final gate.
//!
//! Covers:
//! - auto_retire_no_path_items: no-path items are AlreadySatisfied after registration
//! - retire_no_path_items_by_description: checked [x] no-path items retire existing no-path items
//! - check_final_gate: abstract summary items do not block ANVIL_FINAL
//! - mixed plans: no-path items alongside file-backed items

use anvil::contracts::{ExecutionPlan, FinalGateDecision, PlanItem, PlanItemStatus};

// ---------------------------------------------------------------------------
// auto_retire_no_path_items: basic auto-retirement
// ---------------------------------------------------------------------------

#[test]
fn auto_retire_marks_no_path_items_already_satisfied() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("src/foo.rs: implement".into(), vec!["src/foo.rs".into()]),
        PlanItem::new("残っている gap があれば修正".into(), vec![]),
        PlanItem::new("src/bar.rs: update".into(), vec!["src/bar.rs".into()]),
    ]);
    let retired = plan.auto_retire_no_path_items();
    assert_eq!(retired, 1);
    assert_eq!(plan.items[0].status, PlanItemStatus::Pending);
    assert_eq!(plan.items[1].status, PlanItemStatus::AlreadySatisfied);
    assert_eq!(plan.items[2].status, PlanItemStatus::Pending);
}

#[test]
fn auto_retire_multiple_no_path_items() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("Add integration tests".into(), vec![]),
        PlanItem::new("Update documentation".into(), vec![]),
        PlanItem::new("src/a.rs: fix bug".into(), vec!["src/a.rs".into()]),
    ]);
    let retired = plan.auto_retire_no_path_items();
    assert_eq!(retired, 2);
    assert_eq!(plan.items[0].status, PlanItemStatus::AlreadySatisfied);
    assert_eq!(plan.items[1].status, PlanItemStatus::AlreadySatisfied);
    assert_eq!(plan.items[2].status, PlanItemStatus::Pending);
}

#[test]
fn auto_retire_skips_already_finished_items() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new("no-path item".into(), vec![])]);
    plan.items[0].status = PlanItemStatus::Done;
    let retired = plan.auto_retire_no_path_items();
    assert_eq!(retired, 0);
    assert_eq!(plan.items[0].status, PlanItemStatus::Done);
}

#[test]
fn auto_retire_noop_when_all_have_paths() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("src/a.rs: fix".into(), vec!["src/a.rs".into()]),
        PlanItem::new("src/b.rs: fix".into(), vec!["src/b.rs".into()]),
    ]);
    let retired = plan.auto_retire_no_path_items();
    assert_eq!(retired, 0);
}

// ---------------------------------------------------------------------------
// final gate: no-path items do not block after auto-retire
// ---------------------------------------------------------------------------

#[test]
fn final_gate_allows_after_auto_retire_no_path_items() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("src/a.rs: implement".into(), vec!["src/a.rs".into()]),
        PlanItem::new("監査結果: 全て正常".into(), vec![]),
    ]);
    plan.auto_retire_no_path_items();
    plan.mark_done(0);

    assert_eq!(plan.check_final_gate(), FinalGateDecision::Allow);
}

#[test]
fn final_gate_incomplete_without_auto_retire_for_no_path() {
    // Without auto-retire, no-path item blocks final gate
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("src/a.rs: implement".into(), vec!["src/a.rs".into()]),
        PlanItem::new("残っている gap があれば修正".into(), vec![]),
    ]);
    // Don't call auto_retire
    plan.mark_done(0);

    // No-path item blocks final gate
    match plan.check_final_gate() {
        FinalGateDecision::Incomplete { remaining, .. } => {
            assert_eq!(remaining, 1);
        }
        other => panic!("expected Incomplete, got {:?}", other),
    }
}

#[test]
fn final_gate_allows_all_no_path_plan_after_auto_retire() {
    // Plan with only no-path items (read-only audit scenario)
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("監査結果: 全ての live path が正しく使用".into(), vec![]),
        PlanItem::new("残っている gap があれば修正、なければ完了".into(), vec![]),
    ]);
    plan.auto_retire_no_path_items();

    assert_eq!(plan.check_final_gate(), FinalGateDecision::Allow);
    assert!(plan.is_cleanly_finished());
}

// ---------------------------------------------------------------------------
// retire_no_path_items_by_description: description-based retirement
// ---------------------------------------------------------------------------

#[test]
fn description_retire_matches_exact() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("監査結果: 全て正常".into(), vec![]),
        PlanItem::new("src/a.rs: fix".into(), vec!["src/a.rs".into()]),
    ]);
    let count = plan.retire_no_path_items_by_description(
        &["監査結果: 全て正常".to_string()],
        PlanItemStatus::AlreadySatisfied,
    );
    assert_eq!(count, 1);
    assert_eq!(plan.items[0].status, PlanItemStatus::AlreadySatisfied);
    // File-backed item is untouched
    assert_eq!(plan.items[1].status, PlanItemStatus::Pending);
}

#[test]
fn description_retire_case_insensitive() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new("Add Integration Tests".into(), vec![])]);
    let count = plan.retire_no_path_items_by_description(
        &["add integration tests".to_string()],
        PlanItemStatus::AlreadySatisfied,
    );
    assert_eq!(count, 1);
    assert_eq!(plan.items[0].status, PlanItemStatus::AlreadySatisfied);
}

#[test]
fn description_retire_whitespace_normalized() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new("監査結果:  全て  正常".into(), vec![])]);
    let count = plan.retire_no_path_items_by_description(
        &["監査結果: 全て 正常".to_string()],
        PlanItemStatus::AlreadySatisfied,
    );
    assert_eq!(count, 1);
}

#[test]
fn description_retire_skips_file_backed_items() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "src/a.rs: fix".into(),
        vec!["src/a.rs".into()],
    )]);
    let count = plan.retire_no_path_items_by_description(
        &["src/a.rs: fix".to_string()],
        PlanItemStatus::AlreadySatisfied,
    );
    assert_eq!(count, 0);
    assert_eq!(plan.items[0].status, PlanItemStatus::Pending);
}

#[test]
fn description_retire_skips_already_finished() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new("no-path item".into(), vec![])]);
    plan.items[0].status = PlanItemStatus::Done;
    let count = plan.retire_no_path_items_by_description(
        &["no-path item".to_string()],
        PlanItemStatus::AlreadySatisfied,
    );
    assert_eq!(count, 0);
}

#[test]
fn description_retire_no_match_returns_zero() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new("item A".into(), vec![])]);
    let count = plan.retire_no_path_items_by_description(
        &["item B".to_string()],
        PlanItemStatus::AlreadySatisfied,
    );
    assert_eq!(count, 0);
    assert_eq!(plan.items[0].status, PlanItemStatus::Pending);
}

// ---------------------------------------------------------------------------
// B1 scenario: read-only audit with generic summary item
// ---------------------------------------------------------------------------

#[test]
fn b1_scenario_generic_summary_item_does_not_block() {
    // Simulates B1 from the issue: plan with 8 items, 7 file-backed (done),
    // 1 no-path generic summary item
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("src/a.rs: check".into(), vec!["src/a.rs".into()]),
        PlanItem::new("src/b.rs: check".into(), vec!["src/b.rs".into()]),
        PlanItem::new("src/c.rs: check".into(), vec!["src/c.rs".into()]),
        PlanItem::new("src/d.rs: check".into(), vec!["src/d.rs".into()]),
        PlanItem::new("src/e.rs: check".into(), vec!["src/e.rs".into()]),
        PlanItem::new("src/f.rs: check".into(), vec!["src/f.rs".into()]),
        PlanItem::new("src/g.rs: check".into(), vec!["src/g.rs".into()]),
        PlanItem::new(
            "残っている gap があれば修正、なければ already done として完了".into(),
            vec![],
        ),
    ]);

    // Auto-retire the no-path item
    plan.auto_retire_no_path_items();

    // Mark file-backed items as Done
    for i in 0..7 {
        plan.mark_done(i);
    }

    // Final gate should Allow
    assert_eq!(plan.check_final_gate(), FinalGateDecision::Allow);
    assert!(plan.is_successfully_completed());
}

// ---------------------------------------------------------------------------
// A2 scenario: checked no-path item in ANVIL_PLAN_UPDATE
// ---------------------------------------------------------------------------

#[test]
fn a2_scenario_checked_no_path_retires_existing() {
    // Simulates A2: model outputs [x] for a no-path summary item
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("src/a.rs: implement".into(), vec!["src/a.rs".into()]),
        PlanItem::new(
            "監査結果: 全ての live path が buildDetectPromptOptions() を正しく使用しており、numbered list 対応の実装は既に存在".into(),
            vec![],
        ),
    ]);

    // Even without auto-retire, description-based retire should work
    let count = plan.retire_no_path_items_by_description(
        &["監査結果: 全ての live path が buildDetectPromptOptions() を正しく使用しており、numbered list 対応の実装は既に存在".to_string()],
        PlanItemStatus::AlreadySatisfied,
    );
    assert_eq!(count, 1);

    plan.mark_done(0);
    assert_eq!(plan.check_final_gate(), FinalGateDecision::Allow);
}

// ---------------------------------------------------------------------------
// next_actionable_index skips auto-retired no-path items
// ---------------------------------------------------------------------------

#[test]
fn next_actionable_skips_auto_retired_no_path() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("summary item".into(), vec![]),
        PlanItem::new("src/a.rs: real work".into(), vec!["src/a.rs".into()]),
    ]);
    plan.auto_retire_no_path_items();

    // next_actionable should skip the auto-retired item 0 and return item 1
    assert_eq!(plan.next_actionable_index(), Some(1));
}

// ---------------------------------------------------------------------------
// closure mode: no-path remaining==1 does not cause empty spin
// ---------------------------------------------------------------------------

#[test]
fn closure_mode_no_path_remaining_does_not_block() {
    // If all file-backed items are done but a no-path item remains,
    // auto-retire should have already handled it
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("src/a.rs: fix".into(), vec!["src/a.rs".into()]),
        PlanItem::new("conclusion summary".into(), vec![]),
    ]);
    plan.auto_retire_no_path_items();
    plan.mark_done(0);

    // Plan should be fully finished, not stuck in closure mode
    assert!(plan.all_finished());
    assert_eq!(plan.check_final_gate(), FinalGateDecision::Allow);
}
