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
fn classify_blocked_takes_priority_over_complete() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("task1".into(), vec!["src/a.rs".into()]),
        PlanItem::new("task2".into(), vec!["src/b.rs".into()]),
    ]);
    plan.mark_done(0);
    // task2 blocked but all finished → Blocked wins (Issue #261 Task 0.5)
    for _ in 0..PlanItem::MAX_RETRIES {
        plan.record_failure(1);
    }
    assert!(plan.all_finished());
    let kind = CompletionKind::classify(&plan, None, false);
    assert_eq!(kind, CompletionKind::Blocked);
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

// ---------------------------------------------------------------------------
// Task 0.6: telemetry extension
// ---------------------------------------------------------------------------

#[test]
fn telemetry_tracks_no_op_mutation() {
    let mut tel = AgentTelemetry::new();
    assert_eq!(tel.no_op_mutation_count, 0);
    tel.record_no_op_mutation();
    tel.record_no_op_mutation();
    assert_eq!(tel.no_op_mutation_count, 2);
}

#[test]
fn telemetry_tracks_rolled_back_mutation() {
    let mut tel = AgentTelemetry::new();
    assert_eq!(tel.rolled_back_mutation_count, 0);
    tel.record_rolled_back_mutation();
    assert_eq!(tel.rolled_back_mutation_count, 1);
}

#[test]
fn telemetry_tracks_turn_metrics() {
    let mut tel = AgentTelemetry::new();
    tel.record_turn_metrics(3, 2, 100, 1);
    tel.record_turn_metrics(1, 1, 50, 2);
    assert_eq!(tel.mutations_per_turn, vec![3, 1]);
    assert_eq!(tel.items_advanced_per_turn, vec![2, 1]);
    assert_eq!(tel.guidance_chars_per_turn, vec![100, 50]);
    assert_eq!(tel.workset_size_per_turn, vec![1, 2]);
}

#[test]
fn telemetry_serde_backward_compatible() {
    // JSON without new fields should deserialize with defaults
    let json = r#"{
        "premature_final_count": 1,
        "total_final_requests": 5,
        "plan_registration_count": 1,
        "plan_update_count": 0,
        "sync_from_touched_files_count": 0,
        "completion_kind": null
    }"#;
    let tel: AgentTelemetry = serde_json::from_str(json).expect("should deserialize");
    assert_eq!(tel.premature_final_count, 1);
    assert_eq!(tel.no_op_mutation_count, 0);
    assert_eq!(tel.rolled_back_mutation_count, 0);
    assert_eq!(tel.initial_plan_miss_count, 0);
    assert!(tel.mutations_per_turn.is_empty());
    assert!(tel.items_advanced_per_turn.is_empty());
    assert!(tel.guidance_chars_per_turn.is_empty());
    assert!(tel.workset_size_per_turn.is_empty());
}

// ---------------------------------------------------------------------------
// Task 0.5: completion semantics
// ---------------------------------------------------------------------------

#[test]
fn blocked_items_do_not_classify_as_complete() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("task1".into(), vec!["src/a.rs".into()]),
        PlanItem::new("task2".into(), vec!["src/b.rs".into()]),
    ]);
    plan.mark_done(0);
    // task2 blocked → all finished but has_blocked
    for _ in 0..PlanItem::MAX_RETRIES {
        plan.record_failure(1);
    }
    assert!(plan.all_finished());
    let kind = CompletionKind::classify(&plan, None, false);
    assert_eq!(kind, CompletionKind::Blocked);
}

#[test]
fn final_gate_allows_when_all_done_or_blocked() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("task1".into(), vec!["src/a.rs".into()]),
        PlanItem::new("task2".into(), vec!["src/b.rs".into()]),
    ]);
    plan.mark_done(0);
    for _ in 0..PlanItem::MAX_RETRIES {
        plan.record_failure(1);
    }
    assert!(plan.all_finished());
    assert_eq!(plan.check_final_gate(), FinalGateDecision::Allow);
}

// ---------------------------------------------------------------------------
// Task 0.2: rolled_back / no-op mutation exclusion
// ---------------------------------------------------------------------------

fn make_mutation_result(
    tool_name: &str,
    summary: &str,
    status: anvil::tooling::ToolExecutionStatus,
    rolled_back: bool,
) -> anvil::tooling::ToolExecutionResult {
    anvil::tooling::ToolExecutionResult {
        tool_call_id: "tc-1".to_string(),
        tool_name: tool_name.to_string(),
        status,
        summary: summary.to_string(),
        payload: anvil::tooling::ToolExecutionPayload::Text("ok".to_string()),
        artifacts: vec![],
        elapsed_ms: 10,
        diff_summary: None,
        edit_detail: None,
        rolled_back,
    }
}

#[test]
fn rolled_back_mutation_does_not_advance_plan() {
    // Build a plan with one item
    let items = vec![PlanItem::new("update A".into(), vec!["src/a.rs".into()])];
    let mut plan = ExecutionPlan::new(items);
    plan.mark_in_progress(0);

    // A rolled_back Completed result
    let result = make_mutation_result(
        "file.write",
        "src/a.rs",
        anvil::tooling::ToolExecutionStatus::Completed,
        true, // rolled_back
    );

    // Simulate update_plan_from_results logic: rolled_back should be skipped
    // We test the contracts layer directly since update_plan_from_results is on App
    let mutation_tools = ["file.write", "file.edit", "file.edit_anchor"];
    for r in &[result] {
        if mutation_tools.contains(&r.tool_name.as_str())
            && r.status == anvil::tooling::ToolExecutionStatus::Completed
            && !r.rolled_back
            && !r.summary.contains("(no changes)")
            && !r.summary.is_empty()
        {
            plan.record_mutation_success(0, &r.summary);
        }
    }
    // Item should NOT be done (rolled_back was filtered)
    assert_eq!(plan.items[0].status, PlanItemStatus::InProgress);
}

#[test]
fn noop_mutation_does_not_advance_plan() {
    let items = vec![PlanItem::new("update A".into(), vec!["src/a.rs".into()])];
    let mut plan = ExecutionPlan::new(items);
    plan.mark_in_progress(0);

    let result = make_mutation_result(
        "file.edit",
        "src/a.rs (no changes)",
        anvil::tooling::ToolExecutionStatus::Completed,
        false,
    );

    let mutation_tools = ["file.write", "file.edit", "file.edit_anchor"];
    for r in &[result] {
        if mutation_tools.contains(&r.tool_name.as_str())
            && r.status == anvil::tooling::ToolExecutionStatus::Completed
            && !r.rolled_back
            && !r.summary.contains("(no changes)")
            && !r.summary.is_empty()
        {
            plan.record_mutation_success(0, &r.summary);
        }
    }
    assert_eq!(plan.items[0].status, PlanItemStatus::InProgress);
}

#[test]
fn valid_mutation_still_advances_plan() {
    let items = vec![PlanItem::new("update A".into(), vec!["src/a.rs".into()])];
    let mut plan = ExecutionPlan::new(items);
    plan.mark_in_progress(0);

    let result = make_mutation_result(
        "file.write",
        "src/a.rs",
        anvil::tooling::ToolExecutionStatus::Completed,
        false,
    );

    let mutation_tools = ["file.write", "file.edit", "file.edit_anchor"];
    for r in &[result] {
        if mutation_tools.contains(&r.tool_name.as_str())
            && r.status == anvil::tooling::ToolExecutionStatus::Completed
            && !r.rolled_back
            && !r.summary.contains("(no changes)")
            && !r.summary.is_empty()
        {
            plan.record_mutation_success(0, &r.summary);
        }
    }
    assert_eq!(plan.items[0].status, PlanItemStatus::Done);
}

// ---------------------------------------------------------------------------
// Task 0.1: raw_content preserves full response for ANVIL_PLAN extraction
// ---------------------------------------------------------------------------

#[test]
fn plan_registers_from_initial_raw_content() {
    use anvil::agent::BasicAgentLoop;

    // Response with ANVIL_PLAN outside ANVIL_FINAL (no tool blocks)
    let response = "\
```ANVIL_PLAN\n\
- [ ] src/a.rs: add function\n\
- [ ] src/b.rs: fix bug\n\
```\n\
```ANVIL_FINAL\n\
Done.\n\
```";

    let parsed = BasicAgentLoop::parse_structured_response(response).expect("should parse");

    // raw_content should contain the full response including ANVIL_PLAN
    assert!(
        parsed.raw_content.contains("ANVIL_PLAN"),
        "raw_content should contain ANVIL_PLAN block"
    );

    // final_response (from ANVIL_FINAL) should NOT contain ANVIL_PLAN
    assert!(
        !parsed.final_response.contains("ANVIL_PLAN"),
        "final_response should not contain ANVIL_PLAN"
    );
}

// ---------------------------------------------------------------------------
// Task 0.3: multi-item attribution
// ---------------------------------------------------------------------------

/// Helper: apply filtered mutation results to plan (simulates update_plan_from_results logic)
fn apply_mutations_to_plan(
    plan: &mut ExecutionPlan,
    results: &[anvil::tooling::ToolExecutionResult],
) {
    let mutation_tools = ["file.write", "file.edit", "file.edit_anchor"];

    for r in results {
        if !mutation_tools.contains(&r.tool_name.as_str())
            || r.status != anvil::tooling::ToolExecutionStatus::Completed
            || r.rolled_back
            || r.summary.contains("(no changes)")
            || r.summary.is_empty()
        {
            continue;
        }

        // Find matching unfinished items by target_files
        let mut matched = false;
        // Prefer InProgress over Pending for same file
        let mut matches: Vec<usize> = Vec::new();
        for (i, item) in plan.items.iter().enumerate() {
            if item.is_finished() {
                continue;
            }
            if item.target_files.is_empty() {
                continue;
            }
            let file_matches = item
                .target_files
                .iter()
                .any(|tf| r.summary.ends_with(tf) || tf.ends_with(&r.summary));
            if file_matches {
                matches.push(i);
            }
        }

        if !matches.is_empty() {
            // Prioritize InProgress items
            let inprogress: Vec<usize> = matches
                .iter()
                .copied()
                .filter(|&i| plan.items[i].status == PlanItemStatus::InProgress)
                .collect();
            let targets = if inprogress.is_empty() {
                matches
            } else {
                inprogress
            };
            for idx in targets {
                plan.record_mutation_success(idx, &r.summary);
            }
            matched = true;
        }

        // Fallback: if no target_files match, attribute to current item only
        if !matched && let Some(idx) = plan.next_actionable_index() {
            plan.record_mutation_success(idx, &r.summary);
        }
    }

    // Auto-advance next pending item to InProgress
    if let Some(next) = plan.next_actionable_index()
        && plan.items[next].status == PlanItemStatus::Pending
    {
        plan.mark_in_progress(next);
    }
}

#[test]
fn multi_item_mutation_advances_multiple_items() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("update A".into(), vec!["src/a.rs".into()]),
        PlanItem::new("update B".into(), vec!["src/b.rs".into()]),
    ]);
    plan.mark_in_progress(0);

    let results = vec![
        make_mutation_result(
            "file.write",
            "src/a.rs",
            anvil::tooling::ToolExecutionStatus::Completed,
            false,
        ),
        make_mutation_result(
            "file.write",
            "src/b.rs",
            anvil::tooling::ToolExecutionStatus::Completed,
            false,
        ),
    ];

    apply_mutations_to_plan(&mut plan, &results);

    assert_eq!(plan.items[0].status, PlanItemStatus::Done);
    assert_eq!(plan.items[1].status, PlanItemStatus::Done);
}

#[test]
fn empty_target_item_attributed_to_current_only() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("generic task 1".into(), vec![]),
        PlanItem::new("generic task 2".into(), vec![]),
    ]);
    plan.mark_in_progress(0);

    let results = vec![make_mutation_result(
        "file.write",
        "src/a.rs",
        anvil::tooling::ToolExecutionStatus::Completed,
        false,
    )];

    apply_mutations_to_plan(&mut plan, &results);

    // Only current item (0) should be done, not item 1
    assert_eq!(plan.items[0].status, PlanItemStatus::Done);
    assert_ne!(plan.items[1].status, PlanItemStatus::Done);
}

#[test]
fn inprogress_item_prioritized_over_pending() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("update A (first)".into(), vec!["src/a.rs".into()]),
        PlanItem::new("update A (second)".into(), vec!["src/a.rs".into()]),
    ]);
    // Mark item 1 as InProgress (not item 0)
    plan.mark_in_progress(1);

    let results = vec![make_mutation_result(
        "file.write",
        "src/a.rs",
        anvil::tooling::ToolExecutionStatus::Completed,
        false,
    )];

    apply_mutations_to_plan(&mut plan, &results);

    // InProgress item (1) should be prioritized and completed
    assert_eq!(plan.items[1].status, PlanItemStatus::Done);
    // Item 0 should NOT be Done (mutation was attributed to item 1)
    assert_ne!(plan.items[0].status, PlanItemStatus::Done);
}

// ---------------------------------------------------------------------------
// Issue #261 Task 1.2: current_workset()
// ---------------------------------------------------------------------------

#[test]
fn workset_returns_multiple_actionable_items() {
    let plan = ExecutionPlan::new(vec![
        PlanItem::new("task1".into(), vec!["src/a.rs".into()]),
        PlanItem::new("task2".into(), vec!["src/b.rs".into()]),
        PlanItem::new("task3".into(), vec!["src/c.rs".into()]),
        PlanItem::new("task4".into(), vec!["src/d.rs".into()]),
        PlanItem::new("task5".into(), vec!["src/e.rs".into()]),
        PlanItem::new("task6".into(), vec!["src/f.rs".into()]),
    ]);
    let workset = plan.current_workset();
    // Should return up to 5 actionable items
    assert_eq!(workset.len(), 5);
    assert_eq!(workset, vec![0, 1, 2, 3, 4]);
}

#[test]
fn workset_returns_empty_when_all_done() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("task1".into(), vec!["src/a.rs".into()]),
        PlanItem::new("task2".into(), vec!["src/b.rs".into()]),
    ]);
    plan.mark_done(0);
    plan.mark_done(1);
    let workset = plan.current_workset();
    assert!(workset.is_empty());
}

#[test]
fn workset_skips_done_and_blocked_items() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("task1".into(), vec!["src/a.rs".into()]),
        PlanItem::new("task2".into(), vec!["src/b.rs".into()]),
        PlanItem::new("task3".into(), vec!["src/c.rs".into()]),
        PlanItem::new("task4".into(), vec!["src/d.rs".into()]),
    ]);
    plan.mark_done(0);
    plan.items[1].status = PlanItemStatus::Blocked;
    let workset = plan.current_workset();
    assert_eq!(workset, vec![2, 3]);
}

// ---------------------------------------------------------------------------
// Issue #261 Task 1.3: batch guidance text
// ---------------------------------------------------------------------------

#[test]
fn batch_guidance_does_not_contain_sequential_text() {
    use anvil::config::GuidanceMode;
    let plan = ExecutionPlan::new(vec![
        PlanItem::new("src/a.rs: update module".into(), vec!["src/a.rs".into()]),
        PlanItem::new("src/b.rs: add field".into(), vec!["src/b.rs".into()]),
        PlanItem::new("src/c.rs: add test".into(), vec!["src/c.rs".into()]),
    ]);
    let guidance = plan.build_turn_guidance_with_mode(GuidanceMode::Batch);
    assert!(guidance.is_some());
    let text = guidance.unwrap();
    // Must NOT contain the sequential "1項目ずつ" text
    assert!(
        !text.contains("1項目ずつ"),
        "batch guidance must not contain sequential text, got: {text}"
    );
}

#[test]
fn sequential_guidance_maintains_current_text() {
    use anvil::config::GuidanceMode;
    let plan = ExecutionPlan::new(vec![
        PlanItem::new("src/a.rs: update module".into(), vec!["src/a.rs".into()]),
        PlanItem::new("src/b.rs: add field".into(), vec!["src/b.rs".into()]),
    ]);
    let guidance = plan.build_turn_guidance_with_mode(GuidanceMode::Sequential);
    assert!(guidance.is_some());
    let text = guidance.unwrap();
    // Sequential mode must contain the traditional "1項目ずつ" text
    assert!(
        text.contains("1項目ずつ"),
        "sequential guidance must contain sequential text, got: {text}"
    );
}

#[test]
fn batch_guidance_contains_workset_items() {
    use anvil::config::GuidanceMode;
    let plan = ExecutionPlan::new(vec![
        PlanItem::new("src/a.rs: update module".into(), vec!["src/a.rs".into()]),
        PlanItem::new("src/b.rs: add field".into(), vec!["src/b.rs".into()]),
        PlanItem::new("src/c.rs: add test".into(), vec!["src/c.rs".into()]),
    ]);
    let guidance = plan.build_turn_guidance_with_mode(GuidanceMode::Batch);
    assert!(guidance.is_some());
    let text = guidance.unwrap();
    // Batch guidance must list workset items
    assert!(
        text.contains("src/a.rs"),
        "batch guidance must contain workset item src/a.rs"
    );
    assert!(
        text.contains("src/b.rs"),
        "batch guidance must contain workset item src/b.rs"
    );
    assert!(
        text.contains("src/c.rs"),
        "batch guidance must contain workset item src/c.rs"
    );
}

// ---------------------------------------------------------------------------
// Issue #261 Task 1.4: missing-set guidance
// ---------------------------------------------------------------------------

#[test]
fn batch_missing_set_guidance_contains_completed_and_pending() {
    use anvil::config::GuidanceMode;
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("src/a.rs: update module".into(), vec!["src/a.rs".into()]),
        PlanItem::new("src/b.rs: add field".into(), vec!["src/b.rs".into()]),
        PlanItem::new("src/c.rs: add test".into(), vec!["src/c.rs".into()]),
        PlanItem::new("src/d.rs: add docs".into(), vec!["src/d.rs".into()]),
    ]);
    plan.mark_done(0);
    plan.mark_done(1);

    let msg = plan.build_incomplete_plan_message_with_mode(GuidanceMode::Batch);
    // Must contain completed items info
    assert!(
        msg.contains("完了済み"),
        "batch missing-set guidance must contain completed info, got: {msg}"
    );
    // Must contain pending/workset items
    assert!(
        msg.contains("src/c.rs"),
        "batch missing-set guidance must contain pending items, got: {msg}"
    );
    assert!(
        msg.contains("src/d.rs"),
        "batch missing-set guidance must contain pending items, got: {msg}"
    );
}

#[test]
fn sequential_missing_set_guidance_uses_next_item() {
    use anvil::config::GuidanceMode;
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("src/a.rs: update module".into(), vec!["src/a.rs".into()]),
        PlanItem::new("src/b.rs: add field".into(), vec!["src/b.rs".into()]),
    ]);
    plan.mark_done(0);
    plan.mark_in_progress(1);

    let msg = plan.build_incomplete_plan_message_with_mode(GuidanceMode::Sequential);
    // Sequential mode mentions the next item
    assert!(
        msg.contains("src/b.rs"),
        "sequential missing-set guidance must contain next item, got: {msg}"
    );
}
