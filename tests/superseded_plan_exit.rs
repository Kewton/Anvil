//! Tests for Issue #311: superseded-only terminal plan exit semantics.
//!
//! Covers:
//! - AC6-1: contract-level supersede→dedup consistency (check_final_gate, CompletionKind, is_successfully_completed)
//! - AC6-2: is_cleanly_finished for superseded-only plans
//! - AC6-3: CompletionKind + is_cleanly_finished consistency (no complete_* + failure divergence)
//! - AC6-4: corrected replan with touched_files retains Done evidence
//! - AC6-5: genuinely unrecovered tool failure preserves failure semantics

use anvil::contracts::{
    CompletionKind, ExecutionPlan, FinalGateDecision, PlanItem, PlanItemStatus,
};

// ---------------------------------------------------------------------------
// AC6-1: contract-level supersede→dedup consistency
// After supersede→dedup, the plan must not produce a state where
// check_final_gate==Allow + CompletionKind==complete_* but
// is_successfully_completed==false.
// ---------------------------------------------------------------------------

#[test]
fn ac6_1_supersede_dedup_retains_corrected_item() {
    // Simulate the B2 scenario: replan with corrected items targeting same files.
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new(
            "prompt-detector.ts: fix parsing".into(),
            vec!["src/prompt-detector.ts".into()],
        ),
        PlanItem::new(
            "response-poller.ts: update polling".into(),
            vec!["src/response-poller.ts".into()],
        ),
    ]);

    // Corrected items with same targets
    let new_items = vec![
        PlanItem::new(
            "prompt-detector.ts: corrected fix".into(),
            vec!["src/prompt-detector.ts".into()],
        ),
        PlanItem::new(
            "response-poller.ts: corrected update".into(),
            vec!["src/response-poller.ts".into()],
        ),
    ];

    // Pipeline: supersede → dedup → append
    plan.supersede_stale_items(&new_items);
    assert_eq!(plan.items[0].status, PlanItemStatus::Superseded);
    assert_eq!(plan.items[1].status, PlanItemStatus::Superseded);

    let deduped = plan.deduplicate_new_items(new_items);
    // Issue #311: corrected items survive dedup
    assert_eq!(deduped.len(), 2, "corrected items must survive dedup");
    plan.append_items(deduped);

    // Plan has 4 items: 2 Superseded + 2 Pending
    assert_eq!(plan.items.len(), 4);
    assert!(!plan.all_finished(), "Pending corrected items remain");

    // Simulate agent completing work on corrected items
    plan.items[2].status = PlanItemStatus::Done;
    plan.items[3].status = PlanItemStatus::Done;

    // Now all predicates agree:
    assert!(plan.all_finished());
    assert!(plan.is_successfully_completed());
    assert!(plan.is_cleanly_finished());
    assert_eq!(plan.check_final_gate(), FinalGateDecision::Allow);
    assert_eq!(
        CompletionKind::classify(&plan, None, false),
        CompletionKind::CompleteUnverified
    );
}

// ---------------------------------------------------------------------------
// AC6-2: is_cleanly_finished for superseded-only plans (safety net)
// Even if a superseded-only plan somehow occurs, is_cleanly_finished
// aligns with check_final_gate / CompletionKind.
// ---------------------------------------------------------------------------

#[test]
fn ac6_2_superseded_only_cleanly_finished() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("task1".into(), vec!["src/a.ts".into()]),
        PlanItem::new("task2".into(), vec!["src/b.ts".into()]),
    ]);
    plan.items[0].status = PlanItemStatus::Superseded;
    plan.items[1].status = PlanItemStatus::Superseded;

    // is_successfully_completed requires Done/AlreadySatisfied → false
    assert!(!plan.is_successfully_completed());
    // is_cleanly_finished only requires all_finished + no blocked → true
    assert!(plan.is_cleanly_finished());
    // check_final_gate uses all_finished → Allow
    assert_eq!(plan.check_final_gate(), FinalGateDecision::Allow);
    // CompletionKind uses all_finished → CompleteUnverified
    assert_eq!(
        CompletionKind::classify(&plan, None, false),
        CompletionKind::CompleteUnverified
    );

    // Exit logic (has_tool_execution_failure) now uses is_cleanly_finished,
    // so there is no divergence: all four agree on "clean terminal state".
}

// ---------------------------------------------------------------------------
// AC6-3: CompletionKind + is_cleanly_finished consistency
// Verify that for all terminal plan states, CompletionKind::complete_* implies
// is_cleanly_finished==true (no divergence possible).
// ---------------------------------------------------------------------------

#[test]
fn ac6_3_complete_kind_implies_cleanly_finished() {
    // Case 1: All Done
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("t1".into(), vec![]),
        PlanItem::new("t2".into(), vec![]),
    ]);
    plan.items[0].status = PlanItemStatus::Done;
    plan.items[1].status = PlanItemStatus::Done;
    let kind = CompletionKind::classify(&plan, None, false);
    assert_eq!(kind, CompletionKind::CompleteUnverified);
    assert!(plan.is_cleanly_finished());

    // Case 2: Done + Superseded
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("t1".into(), vec![]),
        PlanItem::new("t2".into(), vec![]),
    ]);
    plan.items[0].status = PlanItemStatus::Done;
    plan.items[1].status = PlanItemStatus::Superseded;
    let kind = CompletionKind::classify(&plan, None, false);
    assert_eq!(kind, CompletionKind::CompleteUnverified);
    assert!(plan.is_cleanly_finished());

    // Case 3: Done + AlreadySatisfied
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("t1".into(), vec![]),
        PlanItem::new("t2".into(), vec![]),
    ]);
    plan.items[0].status = PlanItemStatus::Done;
    plan.items[1].status = PlanItemStatus::AlreadySatisfied;
    let kind = CompletionKind::classify(&plan, None, false);
    assert_eq!(kind, CompletionKind::CompleteUnverified);
    assert!(plan.is_cleanly_finished());

    // Case 4: Superseded-only (safety net)
    let mut plan = ExecutionPlan::new(vec![PlanItem::new("t1".into(), vec![])]);
    plan.items[0].status = PlanItemStatus::Superseded;
    let kind = CompletionKind::classify(&plan, None, false);
    assert_eq!(kind, CompletionKind::CompleteUnverified);
    assert!(plan.is_cleanly_finished());

    // Case 5: Blocked → CompletionKind::Blocked, not cleanly_finished
    let mut plan = ExecutionPlan::new(vec![PlanItem::new("t1".into(), vec![])]);
    plan.items[0].status = PlanItemStatus::Blocked;
    let kind = CompletionKind::classify(&plan, None, false);
    assert_eq!(kind, CompletionKind::Blocked);
    assert!(!plan.is_cleanly_finished());
}

// ---------------------------------------------------------------------------
// AC6-4: corrected replan with touched_files retains Done evidence
// When corrected items are appended and then completed, the plan
// has positive Done evidence in its terminal state.
// ---------------------------------------------------------------------------

#[test]
fn ac6_4_corrected_replan_with_mutations_reaches_done() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new(
            "src/prompt-detector.ts: fix".into(),
            vec!["src/prompt-detector.ts".into()],
        ),
        PlanItem::new("src/route.ts: update".into(), vec!["src/route.ts".into()]),
    ]);

    // Simulate replan: supersede → dedup → append
    let corrected = vec![
        PlanItem::new(
            "src/prompt-detector.ts: corrected fix".into(),
            vec!["src/prompt-detector.ts".into()],
        ),
        PlanItem::new(
            "src/route.ts: corrected update".into(),
            vec!["src/route.ts".into()],
        ),
    ];
    plan.supersede_stale_items(&corrected);
    let deduped = plan.deduplicate_new_items(corrected);
    plan.append_items(deduped);

    // Simulate agent editing files (records mutation + marks Done)
    plan.mark_in_progress(2);
    plan.record_mutation_success(2, "src/prompt-detector.ts");
    plan.mark_done(2);
    plan.mark_in_progress(3);
    plan.record_mutation_success(3, "src/route.ts");
    plan.mark_done(3);

    // Terminal state has Done evidence
    assert!(plan.is_successfully_completed());
    assert!(plan.is_cleanly_finished());
    assert_eq!(plan.check_final_gate(), FinalGateDecision::Allow);

    // Mutations are preserved in corrected items
    assert!(!plan.items[2].mutated_files.is_empty());
    assert!(!plan.items[3].mutated_files.is_empty());
}

// ---------------------------------------------------------------------------
// AC6-5: genuinely unrecovered tool failure preserves failure semantics
// When the plan has unfinished/blocked items, is_cleanly_finished is false
// and exit should still report failure.
// ---------------------------------------------------------------------------

#[test]
fn ac6_5_genuinely_unrecovered_blocked_plan() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("t1".into(), vec!["src/a.ts".into()]),
        PlanItem::new("t2".into(), vec!["src/b.ts".into()]),
    ]);
    plan.items[0].status = PlanItemStatus::Done;
    plan.items[1].status = PlanItemStatus::Blocked;

    assert!(!plan.is_successfully_completed());
    assert!(!plan.is_cleanly_finished());
    assert_eq!(
        CompletionKind::classify(&plan, None, false),
        CompletionKind::Blocked
    );
}

#[test]
fn ac6_5_genuinely_unrecovered_partial_plan() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("t1".into(), vec!["src/a.ts".into()]),
        PlanItem::new("t2".into(), vec!["src/b.ts".into()]),
    ]);
    plan.items[0].status = PlanItemStatus::Done;
    // t2 still Pending
    assert!(!plan.is_cleanly_finished());
    assert!(!plan.is_successfully_completed());
    assert_eq!(
        CompletionKind::classify(&plan, None, false),
        CompletionKind::Partial
    );
}

// ---------------------------------------------------------------------------
// Dedup still works against non-Superseded finished items
// ---------------------------------------------------------------------------

#[test]
fn dedup_works_against_done_items() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "src/foo.ts: task".into(),
        vec!["src/foo.ts".into()],
    )]);
    plan.items[0].status = PlanItemStatus::Done;
    let duplicate = vec![PlanItem::new(
        "src/foo.ts: same task".into(),
        vec!["src/foo.ts".into()],
    )];
    let deduped = plan.deduplicate_new_items(duplicate);
    assert_eq!(
        deduped.len(),
        0,
        "new item matching a Done item should still be deduplicated"
    );
}

#[test]
fn dedup_works_against_already_satisfied_items() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "src/bar.ts: task".into(),
        vec!["src/bar.ts".into()],
    )]);
    plan.items[0].status = PlanItemStatus::AlreadySatisfied;
    let duplicate = vec![PlanItem::new(
        "src/bar.ts: same task".into(),
        vec!["src/bar.ts".into()],
    )];
    let deduped = plan.deduplicate_new_items(duplicate);
    assert_eq!(
        deduped.len(),
        0,
        "new item matching an AlreadySatisfied item should still be deduplicated"
    );
}

#[test]
fn dedup_works_against_pending_items() {
    let plan = ExecutionPlan::new(vec![PlanItem::new(
        "src/baz.ts: task".into(),
        vec!["src/baz.ts".into()],
    )]);
    let duplicate = vec![PlanItem::new(
        "src/baz.ts: same task".into(),
        vec!["src/baz.ts".into()],
    )];
    let deduped = plan.deduplicate_new_items(duplicate);
    assert_eq!(
        deduped.len(),
        0,
        "new item matching a Pending item should still be deduplicated"
    );
}
