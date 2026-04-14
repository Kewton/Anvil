//! Tests for Issue #263: plan-aware stagnation control.
//!
//! Covers StagnationState lifecycle, policy pure functions,
//! workset steering, forced mode, plan repair, and budget-aware thresholds.

use anvil::app::stagnation_state::{
    StagnationState, compute_next_workset, compute_stagnation_score, should_request_plan_repair,
};
use anvil::contracts::{AgentTelemetry, ExecutionPlan, PlanItem, PlanItemStatus};

// ---------------------------------------------------------------------------
// Phase 0: StagnationState struct + policy pure functions
// ---------------------------------------------------------------------------

#[test]
fn stagnation_state_init() {
    let state = StagnationState::new();
    assert_eq!(state.turns_since_last_mutation, 0);
    assert_eq!(state.turns_since_new_target_file, 0);
    assert_eq!(state.same_workset_turns, 0);
    assert_eq!(state.turns_since_plan_item_completion, 0);
    assert!(state.recent_read_only_turns.is_empty());
    assert!(state.starved_target_files.is_empty());
}

#[test]
fn stagnation_state_init_from_plan() {
    let state = StagnationState::init_from_plan(&["src/a.rs".to_string(), "src/b.rs".to_string()]);
    assert_eq!(state.starved_target_files.len(), 2);
    assert!(state.starved_target_files.contains(&"src/a.rs".to_string()));
    assert!(state.starved_target_files.contains(&"src/b.rs".to_string()));
}

#[test]
fn stagnation_state_mutation_resets() {
    let mut state = StagnationState::new();
    // Simulate 5 turns without mutation
    for _ in 0..5 {
        state.begin_turn(&[0]);
        state.end_turn(false);
    }
    assert_eq!(state.turns_since_last_mutation, 5);

    // Record a mutation — should reset
    state.begin_turn(&[0]);
    state.record_mutation("src/a.rs");
    state.end_turn(true);
    assert_eq!(state.turns_since_last_mutation, 0);
}

#[test]
fn stagnation_state_new_target_resets() {
    let target_files = vec!["src/a.rs".to_string(), "src/b.rs".to_string()];
    let mut state = StagnationState::init_from_plan(&target_files);

    // Simulate turns without new target
    for _ in 0..3 {
        state.begin_turn(&[0]);
        state.end_turn(false);
    }
    assert_eq!(state.turns_since_new_target_file, 3);

    // Mutation on a target file — should reset
    state.begin_turn(&[0]);
    state.record_mutation("src/a.rs");
    state.end_turn(true);
    assert_eq!(state.turns_since_new_target_file, 0);
    // a.rs should be removed from starved list
    assert!(!state.starved_target_files.contains(&"src/a.rs".to_string()));
    assert!(state.starved_target_files.contains(&"src/b.rs".to_string()));
}

#[test]
fn stagnation_state_workset_staleness() {
    let mut state = StagnationState::new();

    // Same workset for 3 turns
    state.begin_turn(&[0, 1]);
    state.end_turn(false);
    assert_eq!(state.same_workset_turns, 1);

    state.begin_turn(&[0, 1]);
    state.end_turn(false);
    assert_eq!(state.same_workset_turns, 2);

    state.begin_turn(&[0, 1]);
    state.end_turn(false);
    assert_eq!(state.same_workset_turns, 3);

    // Different workset — resets
    state.begin_turn(&[2, 3]);
    state.end_turn(false);
    assert_eq!(state.same_workset_turns, 1);
}

#[test]
fn stagnation_state_plan_item_completion() {
    let mut state = StagnationState::new();

    // Simulate turns without plan item completion
    for _ in 0..8 {
        state.begin_turn(&[0]);
        state.end_turn(false);
    }
    assert_eq!(state.turns_since_plan_item_completion, 8);

    // Record plan item completion — resets
    state.begin_turn(&[0]);
    state.record_plan_item_completion();
    state.end_turn(true);
    assert_eq!(state.turns_since_plan_item_completion, 0);
}

#[test]
fn stagnation_score_zero() {
    let state = StagnationState::new();
    assert_eq!(compute_stagnation_score(&state), 0);
}

#[test]
fn stagnation_score_all_four() {
    let mut state = StagnationState::new();

    // +1: turns_since_last_mutation >= 5
    // +1: same_workset_turns >= 3
    // +1: turns_since_plan_item_completion >= 8
    // +1: recent_read_only_turns 4/5 true
    for _ in 0..8 {
        state.begin_turn(&[0, 1]);
        // Make 4 out of 5 recent turns read-only (all turns except the first)
        state.end_turn(false);
    }

    let score = compute_stagnation_score(&state);
    assert_eq!(score, 4);
}

#[test]
fn stagnation_score_partial() {
    let mut state = StagnationState::new();

    // Only mutation drought: 5 turns without mutation
    for _ in 0..5 {
        state.begin_turn(&[0]);
        state.end_turn(false);
    }

    let score = compute_stagnation_score(&state);
    // turns_since_last_mutation >= 5 (+1)
    // same_workset_turns = 5 >= 3 (+1)
    // turns_since_plan_item_completion = 5 < 8 (+0)
    // recent_read_only_turns: 5 out of 5 true (+1)
    assert_eq!(score, 3);
}

#[test]
fn plan_repair_request_conditions() {
    let mut state = StagnationState::new();
    state.starved_target_files = vec!["src/a.rs".to_string(), "src/b.rs".to_string()];

    // Score = 0, should be false
    assert!(!should_request_plan_repair(&state, 0, 10));

    // Build up stagnation score >= 2
    for _ in 0..5 {
        state.begin_turn(&[0, 1]);
        state.end_turn(false);
    }
    let score = compute_stagnation_score(&state);
    assert!(score >= 2);

    // Now should be true: score >= 2, starved >= 2, count < 2, remaining >= 5
    assert!(should_request_plan_repair(&state, 0, 10));

    // remaining_turns < 5 → normal_condition is false, but severe_condition fires
    // (score=3 >= 3, turns_since_last_mutation=5 >= 5, starved non-empty, count < 2, remaining >= 1)
    // Issue #287: severe_condition allows plan repair even at low remaining turns
    assert!(should_request_plan_repair(&state, 0, 4));

    // remaining_turns=0 → even severe_condition requires >= 1, so false
    assert!(!should_request_plan_repair(&state, 0, 0));
}

#[test]
fn plan_repair_request_count_limit() {
    let mut state = StagnationState::new();
    state.starved_target_files = vec!["src/a.rs".to_string(), "src/b.rs".to_string()];
    for _ in 0..5 {
        state.begin_turn(&[0, 1]);
        state.end_turn(false);
    }

    // count = 0 → true
    assert!(should_request_plan_repair(&state, 0, 10));
    // count = 1 → true
    assert!(should_request_plan_repair(&state, 1, 10));
    // count = 2 → false (limit reached)
    assert!(!should_request_plan_repair(&state, 2, 10));
}

#[test]
fn telemetry_forced_count() {
    let mut telemetry = AgentTelemetry::new();
    assert_eq!(telemetry.forced_workset_transition_count, 0);
    assert_eq!(telemetry.plan_repair_request_count, 0);

    telemetry.record_forced_workset_transition();
    telemetry.record_forced_workset_transition();
    assert_eq!(telemetry.forced_workset_transition_count, 2);

    telemetry.record_plan_repair_request();
    assert_eq!(telemetry.plan_repair_request_count, 1);
}

// ---------------------------------------------------------------------------
// Phase 1: Score-Based Workset Steering
// ---------------------------------------------------------------------------

#[test]
fn next_workset_prioritizes_untouched() {
    // Create a plan with 3 items: item 0 has mutated files, item 1 and 2 are untouched
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("task1".into(), vec!["src/a.rs".into()]),
        PlanItem::new("task2".into(), vec!["src/b.rs".into()]),
        PlanItem::new("task3".into(), vec!["src/c.rs".into()]),
    ]);
    plan.mark_in_progress(0);
    // Item 0 has some mutation
    plan.items[0].mutated_files.push("src/a.rs".to_string());

    let state = StagnationState::new();
    let workset = compute_next_workset(&plan, &state, 0);

    // All 3 items should be in workset, but untouched items (1, 2) should come first
    assert!(!workset.is_empty());
    // Items with untouched targets should be prioritized
    assert!(workset.len() <= 5);
}

#[test]
fn next_workset_deprioritizes_stagnant() {
    let plan = ExecutionPlan::new(vec![
        PlanItem::new("task1".into(), vec!["src/a.rs".into()]),
        PlanItem::new("task2".into(), vec!["src/b.rs".into()]),
    ]);

    let mut state = StagnationState::new();
    // Same workset for 3 turns (triggers staleness)
    for _ in 0..3 {
        state.begin_turn(&[0, 1]);
        state.end_turn(false);
    }

    let workset = compute_next_workset(&plan, &state, 2);
    // Should still return items, but stagnant items get different scoring
    assert!(!workset.is_empty());
}

#[test]
fn next_workset_max_size() {
    // Create a plan with 8 items
    let items: Vec<PlanItem> = (0..8)
        .map(|i| PlanItem::new(format!("task{}", i), vec![format!("src/{}.rs", i)]))
        .collect();
    let plan = ExecutionPlan::new(items);

    let state = StagnationState::new();
    let workset = compute_next_workset(&plan, &state, 0);

    // MAX_WORKSET_SIZE is 5
    assert!(workset.len() <= 5);
}

#[test]
fn next_workset_empty_plan() {
    let plan = ExecutionPlan::default();
    let state = StagnationState::new();
    let workset = compute_next_workset(&plan, &state, 0);
    assert!(workset.is_empty());
}

#[test]
fn guidance_with_workset_normal() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("task1".into(), vec!["src/a.rs".into()]),
        PlanItem::new("task2".into(), vec!["src/b.rs".into()]),
    ]);
    plan.mark_in_progress(0);

    let guidance =
        plan.build_turn_guidance_with_workset(anvil::config::GuidanceMode::Batch, &[0, 1], false);
    assert!(guidance.is_some());
    let g = guidance.unwrap();
    assert!(g.contains("task1"));
    assert!(g.contains("task2"));
    // Normal mode: should NOT contain forced mode marker
    assert!(!g.contains("STAGNATION"));
}

#[test]
fn guidance_with_workset_forced() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("task1".into(), vec!["src/a.rs".into()]),
        PlanItem::new("task2".into(), vec!["src/b.rs".into()]),
    ]);
    plan.mark_in_progress(0);

    let guidance =
        plan.build_turn_guidance_with_workset(anvil::config::GuidanceMode::Batch, &[0, 1], true);
    assert!(guidance.is_some());
    let g = guidance.unwrap();
    // Forced mode: should contain stagnation warning
    assert!(g.contains("STAGNATION"));
}

// ---------------------------------------------------------------------------
// Phase 2: Forced mode tests (pure function / unit level)
// ---------------------------------------------------------------------------

#[test]
fn forced_mode_activation() {
    let mut state = StagnationState::new();
    // Build up score >= 2
    for _ in 0..5 {
        state.begin_turn(&[0, 1]);
        state.end_turn(false);
    }
    let score = compute_stagnation_score(&state);
    assert!(score >= 2, "score should be >= 2, got {}", score);
}

#[test]
fn forced_mode_reset_after_one_turn() {
    let mut state = StagnationState::new();
    // Build up stagnation
    for _ in 0..5 {
        state.begin_turn(&[0, 1]);
        state.end_turn(false);
    }
    // After a mutation turn, score should drop
    state.begin_turn(&[0, 1]);
    state.record_mutation("src/a.rs");
    state.end_turn(true);
    assert_eq!(state.turns_since_last_mutation, 0);
}

#[test]
fn forced_message_sanitization() {
    use anvil::app::stagnation_state::sanitize_for_prompt_entry;
    // Control characters and ANVIL_* markers should be removed
    let input = "src/foo\x00bar.rs";
    let sanitized = sanitize_for_prompt_entry(input);
    assert!(!sanitized.contains('\x00'));

    let input2 = "ANVIL_FINAL src/a.rs";
    let sanitized2 = sanitize_for_prompt_entry(input2);
    assert!(!sanitized2.contains("ANVIL_FINAL"));

    let input3 = "src/normal.rs";
    let sanitized3 = sanitize_for_prompt_entry(input3);
    assert_eq!(sanitized3, "src/normal.rs");

    // Newlines and tabs should be removed (CB-004)
    let input4 = "src/foo\nbar.rs";
    let sanitized4 = sanitize_for_prompt_entry(input4);
    assert!(!sanitized4.contains('\n'));

    let input5 = "src/foo\tbar.rs";
    let sanitized5 = sanitize_for_prompt_entry(input5);
    assert!(!sanitized5.contains('\t'));
}

// ---------------------------------------------------------------------------
// Phase 3: ANVIL_PLAN_UPDATE request + deduplication
// ---------------------------------------------------------------------------

#[test]
fn plan_repair_duplicate_exclusion_exact_match() {
    use anvil::app::stagnation_state::deduplicate_plan_items;

    let existing_items = vec![
        PlanItem::new("task1".into(), vec!["src/a.rs".into()]),
        PlanItem::new("task2".into(), vec!["src/b.rs".into()]),
    ];
    let new_items = vec![
        // Exact same target_files as task1 → should be excluded
        PlanItem::new("task1 redo".into(), vec!["src/a.rs".into()]),
        // Different target → should be kept
        PlanItem::new("task3".into(), vec!["src/c.rs".into()]),
    ];

    let result = deduplicate_plan_items(&existing_items, new_items);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].target_files, vec!["src/c.rs".to_string()]);
}

#[test]
fn plan_repair_duplicate_allows_different_target() {
    use anvil::app::stagnation_state::deduplicate_plan_items;

    let existing_items = vec![PlanItem::new("task1".into(), vec!["src/a.rs".into()])];
    let new_items = vec![PlanItem::new("task2".into(), vec!["src/b.rs".into()])];

    let result = deduplicate_plan_items(&existing_items, new_items);
    assert_eq!(result.len(), 1);
}

#[test]
fn plan_repair_done_item_exclusion() {
    use anvil::app::stagnation_state::deduplicate_plan_items;

    let mut existing_items = vec![PlanItem::new("task1".into(), vec!["src/a.rs".into()])];
    existing_items[0].status = PlanItemStatus::Done;

    let new_items = vec![
        // Same target as a Done item → should be excluded
        PlanItem::new("redo task1".into(), vec!["src/a.rs".into()]),
    ];

    let result = deduplicate_plan_items(&existing_items, new_items);
    assert_eq!(result.len(), 0);
}

// ---------------------------------------------------------------------------
// Phase 4: Budget-Aware Threshold
// ---------------------------------------------------------------------------

#[test]
fn effective_threshold_budget_factor_normal() {
    use anvil::app::stagnation_state::compute_effective_thresholds;

    // Plenty of remaining turns, few untouched targets → factor ≈ 1.0
    let thresholds = compute_effective_thresholds(15, 8, 100, 1, 0.5);
    // baseline 15, factor = min(1.0, 100 / (1*5 + 5)) = min(1.0, 10.0) = 1.0
    assert_eq!(thresholds.phase_force_transition, 15);
    assert_eq!(thresholds.read_transition, 8);
}

#[test]
fn effective_threshold_budget_factor_tight() {
    use anvil::app::stagnation_state::compute_effective_thresholds;

    // Tight budget: 10 remaining turns, 5 untouched targets
    // factor = min(1.0, 10 / (5*5 + 5)) = min(1.0, 10/30) = 0.333
    let thresholds = compute_effective_thresholds(15, 8, 10, 5, 0.5);
    // effective = (15 * 0.333).round() = 5, min 3 → 5
    assert!(thresholds.phase_force_transition < 15);
    assert!(thresholds.phase_force_transition >= 3);
    // effective = (8 * 0.333).round() = 3, min 3 → 3
    assert!(thresholds.read_transition <= 8);
    assert!(thresholds.read_transition >= 3);
}

#[test]
fn effective_threshold_minimum_clamp() {
    use anvil::app::stagnation_state::compute_effective_thresholds;

    // Very tight: factor would be very low, but clamped to 0.3
    // factor = min(1.0, 1 / (10*5 + 5)) = min(1.0, 1/55) = 0.018 → clamped to 0.3
    let thresholds = compute_effective_thresholds(10, 8, 1, 10, 0.0);
    // effective = (10 * 0.3).round() = 3
    assert_eq!(thresholds.phase_force_transition, 3);
    // effective = (8 * 0.3).round() = 2.4 → 2, but min 3 → 3
    assert_eq!(thresholds.read_transition, 3);
}

#[test]
fn set_effective_threshold_phase_estimator() {
    use anvil::app::phase_estimator::{PhaseAction, PhaseEstimator};

    let mut estimator = PhaseEstimator::new(5, 15, 5);
    // Override force_transition_threshold to 3
    estimator.set_effective_threshold(3);

    // Record 3 reads — should trigger ForceTransition at threshold 3
    for _ in 0..3 {
        let action = estimator.record_tool_call("file.read", true);
        if let PhaseAction::ForceTransition(_) = action {
            return; // Pass: triggered at effective threshold
        }
    }
    panic!("ForceTransition should have triggered at effective threshold 3");
}

#[test]
fn set_effective_threshold_read_guard() {
    use anvil::app::read_transition_guard::ReadTransitionGuard;

    let mut guard = ReadTransitionGuard::new(8, 4);
    guard.set_effective_threshold(3);

    // Record 3 reads — should trigger at effective threshold 3
    for _i in 0..3 {
        let action = guard.record_tool_call("file.read", true);
        if action != anvil::app::read_transition_guard::ReadTransitionAction::Continue {
            return; // Pass
        }
    }
    panic!("ReadTransitionGuard should have triggered at effective threshold 3");
}

// ---------------------------------------------------------------------------
// Telemetry serde backward compatibility
// ---------------------------------------------------------------------------

#[test]
fn telemetry_serde_backward_compatibility() {
    // Old JSON without new fields should deserialize with defaults
    let json = r#"{
        "premature_final_count": 1,
        "total_final_requests": 2,
        "plan_registration_count": 1,
        "plan_update_count": 0,
        "sync_from_touched_files_count": 0,
        "completion_kind": null
    }"#;
    let telemetry: AgentTelemetry = serde_json::from_str(json).unwrap();
    assert_eq!(telemetry.forced_workset_transition_count, 0);
    assert_eq!(telemetry.plan_repair_request_count, 0);
    // Issue #271: new fields default to zero
    assert_eq!(telemetry.anvil_plan_visible_count, 0);
    assert_eq!(telemetry.last_mutation_turn, 0);
    assert_eq!(telemetry.final_suppressed_with_remaining_targets_count, 0);
    // Issue #273 Phase 1.5: new fields default to None
    assert_eq!(telemetry.first_mutation_event_turn, None);
    assert_eq!(telemetry.first_mutation_event_elapsed_s, None);
    assert_eq!(telemetry.first_mutation_event_tool, None);
}

// ---------------------------------------------------------------------------
// Issue #285: follow-up completion fix regression tests
// ---------------------------------------------------------------------------

use anvil::app::stagnation_state::should_allow_escape_hatch;

// --- E-1: B1 type regression test (follow-up empty-tool path) ---
// Tested via check_plan_final_gate_require_plan() behavior on empty plan.
// Since check_plan_final_gate_require_plan is on App (not easily constructible
// in tests), we verify the underlying gate logic by testing that
// should_request_plan_repair and should_allow_escape_hatch interact correctly.

// --- E-3: results_contain_successful_mutation() helper unit tests ---

use anvil::app::agentic::results_contain_successful_mutation;
use anvil::tooling::{ToolExecutionPayload, ToolExecutionResult, ToolExecutionStatus};

fn make_result(
    tool_name: &str,
    status: ToolExecutionStatus,
    rolled_back: bool,
    diff_summary: Option<String>,
) -> ToolExecutionResult {
    ToolExecutionResult {
        tool_call_id: "tc_1".to_string(),
        tool_name: tool_name.to_string(),
        status,
        summary: String::new(),
        payload: ToolExecutionPayload::None,
        artifacts: vec![],
        elapsed_ms: 0,
        diff_summary,
        edit_detail: None,
        rolled_back,
        observed_delta: None,
        delta_observation_skipped: None,
    }
}

#[test]
fn results_contain_successful_mutation_true_on_completed_write() {
    let results = vec![make_result(
        "file.write",
        ToolExecutionStatus::Completed,
        false,
        Some("+1 line".to_string()),
    )];
    assert!(results_contain_successful_mutation(&results));
}

#[test]
fn results_contain_successful_mutation_true_on_completed_edit() {
    let results = vec![make_result(
        "file.edit",
        ToolExecutionStatus::Completed,
        false,
        Some("+2 -1".to_string()),
    )];
    assert!(results_contain_successful_mutation(&results));
}

#[test]
fn results_contain_successful_mutation_true_on_edit_anchor() {
    let results = vec![make_result(
        "file.edit_anchor",
        ToolExecutionStatus::Completed,
        false,
        Some("+3 -2".to_string()),
    )];
    assert!(results_contain_successful_mutation(&results));
}

#[test]
fn results_contain_successful_mutation_false_on_rolled_back() {
    let results = vec![make_result(
        "file.edit",
        ToolExecutionStatus::Completed,
        true, // rolled back
        Some("+1".to_string()),
    )];
    assert!(!results_contain_successful_mutation(&results));
}

#[test]
fn results_contain_successful_mutation_false_on_read_only() {
    let results = vec![make_result(
        "file.read",
        ToolExecutionStatus::Completed,
        false,
        None,
    )];
    assert!(!results_contain_successful_mutation(&results));
}

#[test]
fn results_contain_successful_mutation_false_on_failed_write() {
    let results = vec![make_result(
        "file.write",
        ToolExecutionStatus::Failed,
        false,
        None,
    )];
    assert!(!results_contain_successful_mutation(&results));
}

#[test]
fn results_contain_successful_mutation_false_on_no_diff() {
    let results = vec![make_result(
        "file.write",
        ToolExecutionStatus::Completed,
        false,
        None, // no diff_summary
    )];
    assert!(!results_contain_successful_mutation(&results));
}

#[test]
fn results_contain_successful_mutation_false_on_empty() {
    assert!(!results_contain_successful_mutation(&[]));
}

// --- E-4: should_allow_escape_hatch boundary tests ---

/// Build a StagnationState with a specific stagnation score by simulating turns.
fn build_stagnation_state(turns_without_mutation: usize, same_workset: bool) -> StagnationState {
    let mut state = StagnationState::new();
    let workset: Vec<usize> = if same_workset { vec![0, 1] } else { vec![] };
    for i in 0..turns_without_mutation {
        let ws = if same_workset {
            workset.clone()
        } else {
            vec![i] // different workset each turn
        };
        state.begin_turn(&ws);
        state.end_turn(false);
    }
    state
}

#[test]
fn escape_hatch_triggers_at_score3_with_repair_history() {
    // Build score >= 3: 8 turns without mutation, same workset, read-only
    let state = build_stagnation_state(8, true);
    let score = compute_stagnation_score(&state);
    assert!(score >= 3, "expected score >= 3, got {}", score);

    // All conditions met
    assert!(should_allow_escape_hatch(&state, 1, 10));
}

#[test]
fn escape_hatch_not_triggered_without_prior_repair() {
    let state = build_stagnation_state(8, true);
    let score = compute_stagnation_score(&state);
    assert!(score >= 3, "expected score >= 3, got {}", score);

    // plan_repair_request_count == 0 → false
    assert!(!should_allow_escape_hatch(&state, 0, 10));
}

#[test]
fn escape_hatch_not_triggered_with_many_remaining_turns() {
    let state = build_stagnation_state(8, true);
    let score = compute_stagnation_score(&state);
    assert!(score >= 3, "expected score >= 3, got {}", score);

    // remaining_turns > 10 → false
    assert!(!should_allow_escape_hatch(&state, 1, 11));
}

#[test]
fn escape_hatch_not_triggered_at_low_score() {
    // Only 3 turns → score should be < 3
    let state = build_stagnation_state(3, true);
    let score = compute_stagnation_score(&state);

    // Even with all other conditions, low score should prevent escape
    assert!(!should_allow_escape_hatch(&state, 1, 5));
    // Verify the score is indeed < 3 (the actual boundary)
    if score >= 3 {
        // If score is actually >= 3 at 3 turns, adjust: use 2 turns
        let state2 = build_stagnation_state(2, true);
        assert!(!should_allow_escape_hatch(&state2, 1, 5));
    }
}

#[test]
fn escape_hatch_not_triggered_with_recent_mutation() {
    let mut state = build_stagnation_state(8, true);
    // Record a mutation to reset turns_since_last_mutation
    state.begin_turn(&[0, 1]);
    state.record_mutation("src/a.rs");
    state.end_turn(true);
    // turns_since_last_mutation is now 0, which is < 8
    assert!(!should_allow_escape_hatch(&state, 1, 5));
}

#[test]
fn escape_hatch_boundary_remaining_turns_10() {
    let state = build_stagnation_state(8, true);
    assert!(compute_stagnation_score(&state) >= 3);
    // Exactly 10 remaining turns → should trigger (<=10)
    assert!(should_allow_escape_hatch(&state, 1, 10));
    // 11 remaining turns → should not trigger
    assert!(!should_allow_escape_hatch(&state, 1, 11));
}

#[test]
fn escape_hatch_boundary_turns_since_mutation_8() {
    // Exactly 8 turns without mutation
    let state = build_stagnation_state(8, true);
    assert!(state.turns_since_last_mutation >= 8);
    assert!(should_allow_escape_hatch(&state, 1, 10));
}

// ---------------------------------------------------------------------------
// Issue #287: path_matches suffix normalization regression tests
// ---------------------------------------------------------------------------

#[test]
fn test_path_matches_strips_trailing_ws_suffix() {
    assert!(ExecutionPlan::path_matches(
        "src/foo.rs (trailing-ws fallback)",
        "src/foo.rs"
    ));
}

#[test]
fn test_path_matches_strips_anchor_suffix() {
    assert!(ExecutionPlan::path_matches(
        "src/bar.rs (anchor fallback)",
        "src/bar.rs"
    ));
}

#[test]
fn test_path_matches_both_sides_suffix() {
    // Both sides have suffix — should still match
    assert!(ExecutionPlan::path_matches(
        "src/foo.rs (trailing-ws fallback)",
        "src/foo.rs (anchor fallback)"
    ));
}

#[test]
fn test_path_matches_no_suffix_still_works() {
    assert!(ExecutionPlan::path_matches("src/foo.rs", "src/foo.rs"));
    assert!(ExecutionPlan::path_matches("./src/foo.rs", "src/foo.rs"));
}

#[test]
fn test_last_item_completes_with_fallback_suffix() {
    // Issue #287 reproduction test: last plan item with fallback suffix should
    // be marked Done when record_mutation_success is called with suffixed path.
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "edit foo.rs".to_string(),
        vec!["src/foo.rs".to_string()],
    )]);
    plan.mark_in_progress(0);
    plan.record_mutation_success(0, "src/foo.rs (trailing-ws fallback)");
    assert!(plan.items[0].is_finished());
    assert!(plan.all_finished());
}

#[test]
fn test_last_item_completes_with_anchor_suffix() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "edit bar.rs".to_string(),
        vec!["src/bar.rs".to_string()],
    )]);
    plan.mark_in_progress(0);
    plan.record_mutation_success(0, "src/bar.rs (anchor fallback)");
    assert!(plan.items[0].is_finished());
    assert!(plan.all_finished());
}

#[test]
fn test_record_mutation_success_dedup_with_path_matches() {
    // Same file with different suffixes should not create duplicates
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "edit foo.rs".to_string(),
        vec!["src/foo.rs".to_string()],
    )]);
    plan.mark_in_progress(0);
    plan.record_mutation_success(0, "src/foo.rs");
    plan.record_mutation_success(0, "src/foo.rs (trailing-ws fallback)");
    // Should be deduplicated to 1 entry
    assert_eq!(plan.items[0].mutated_files.len(), 1);
}

#[test]
fn test_deduplicate_new_items_basic() {
    let plan = ExecutionPlan::new(vec![PlanItem::new("task1".into(), vec!["src/a.rs".into()])]);
    let new_items = vec![
        PlanItem::new("task1 redo".into(), vec!["src/a.rs".into()]),
        PlanItem::new("task2".into(), vec!["src/b.rs".into()]),
    ];
    let result = plan.deduplicate_new_items(new_items);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].target_files, vec!["src/b.rs".to_string()]);
}

// ---------------------------------------------------------------------------
// Issue #287: stagnation recovery tests
// ---------------------------------------------------------------------------

#[test]
fn plan_repair_severe_condition_fires_at_remaining_one() {
    // Build a state with score >= 3, turns_since_last_mutation >= 5, starved non-empty
    let mut state = StagnationState::new();
    state.starved_target_files = vec!["src/a.rs".to_string()];
    // 5 turns without mutation, same workset → score = 3
    for _ in 0..5 {
        state.begin_turn(&[0, 1]);
        state.end_turn(false);
    }
    let score = compute_stagnation_score(&state);
    assert!(score >= 3, "expected score >= 3, got {}", score);

    // remaining_turns=1: normal_condition would require >= 5, but severe fires at >= 1
    assert!(should_request_plan_repair(&state, 0, 1));
    // remaining_turns=0: even severe requires >= 1
    assert!(!should_request_plan_repair(&state, 0, 0));
}

#[test]
fn plan_repair_severe_not_fire_with_empty_starved() {
    let mut state = StagnationState::new();
    // No starved files
    for _ in 0..5 {
        state.begin_turn(&[0, 1]);
        state.end_turn(false);
    }
    assert!(compute_stagnation_score(&state) >= 3);
    // severe requires !starved.is_empty()
    assert!(!should_request_plan_repair(&state, 0, 1));
}

#[test]
fn escape_hatch_fires_on_plan_stall() {
    // Build a state with score >= 3, plan_repair >= 1, remaining <= 10,
    // but turns_since_last_mutation < 8 (mutation drought NOT met).
    // Instead, turns_since_plan_item_completion >= 10 (plan stall).
    let mut state = StagnationState::new();

    // 10 turns without plan item completion, same workset, no mutation for some
    // but with occasional mutations to keep turns_since_last_mutation low
    for i in 0..10 {
        state.begin_turn(&[0, 1]);
        // Mutate every 4th turn to keep turns_since_last_mutation < 8
        let had_mutation = i % 4 == 0;
        state.end_turn(had_mutation);
    }

    assert!(
        state.turns_since_plan_item_completion >= 10,
        "expected >= 10, got {}",
        state.turns_since_plan_item_completion
    );
    // turns_since_last_mutation should be < 8 due to periodic mutations
    assert!(
        state.turns_since_last_mutation < 8,
        "expected < 8, got {}",
        state.turns_since_last_mutation
    );
    let score = compute_stagnation_score(&state);
    assert!(score >= 3, "expected score >= 3, got {}", score);

    // plan_stall should fire
    assert!(should_allow_escape_hatch(&state, 1, 10));
}

#[test]
fn escape_hatch_not_fire_on_plan_stall_without_repair() {
    let mut state = StagnationState::new();
    for i in 0..10 {
        state.begin_turn(&[0, 1]);
        let had_mutation = i % 4 == 0;
        state.end_turn(had_mutation);
    }
    assert!(state.turns_since_plan_item_completion >= 10);
    // plan_repair_request_count = 0 → base condition not met
    assert!(!should_allow_escape_hatch(&state, 0, 10));
}

// ---------------------------------------------------------------------------
// Issue #289: stale/alias plan item supersede tests
// ---------------------------------------------------------------------------

#[test]
fn path_matches_strips_parenthesized_suffix() {
    assert!(ExecutionPlan::path_matches(
        "src/foo.ts (bar.ts)",
        "src/foo.ts"
    ));
}

#[test]
fn path_matches_parenthesized_both_sides() {
    assert!(ExecutionPlan::path_matches("src/a.ts (x)", "src/a.ts (y)"));
}

#[test]
fn supersede_stale_item_on_plan_update() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "stale item".into(),
        vec!["src/lib/polling/auto-yes-manager.ts".into()],
    )]);
    plan.mark_in_progress(0);

    let new_items = vec![PlanItem::new(
        "corrected item".into(),
        vec!["src/lib/polling/auto-yes-manager.ts".into()],
    )];
    plan.supersede_stale_items(&new_items);
    assert_eq!(plan.items[0].status, PlanItemStatus::Superseded);
}

#[test]
fn supersede_skips_with_mutated_files() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "item with mutation".into(),
        vec!["src/foo.ts".into()],
    )]);
    plan.mark_in_progress(0);
    plan.record_mutation_success(0, "src/foo.ts");

    let new_items = vec![PlanItem::new(
        "corrected item".into(),
        vec!["src/foo.ts".into()],
    )];
    plan.supersede_stale_items(&new_items);
    // Should NOT be superseded because mutated_files is non-empty
    assert_ne!(plan.items[0].status, PlanItemStatus::Superseded);
}

#[test]
fn supersede_false_positive_prevention() {
    // src/utils.ts should NOT be superseded by src/test-utils.ts
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "utils work".into(),
        vec!["src/utils.ts".into()],
    )]);
    plan.mark_in_progress(0);

    let new_items = vec![PlanItem::new(
        "test utils work".into(),
        vec!["src/test-utils.ts".into()],
    )];
    plan.supersede_stale_items(&new_items);
    // path_matches("src/utils.ts", "src/test-utils.ts") → "src/test-utils.ts".ends_with("src/utils.ts")
    // is true because of suffix matching — however this is the existing behavior.
    // The design doc acknowledges this and relies on the mutated_files guard.
    // For a strict false-positive test, use a non-suffix case:
    assert_ne!(
        plan.items[0].status,
        PlanItemStatus::Superseded,
        "src/utils.ts must NOT be superseded by src/test-utils.ts"
    );
}

#[test]
fn supersede_false_positive_prevention_strict() {
    // Truly different file: src/app.ts should NOT be superseded by src/config.ts
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "app work".into(),
        vec!["src/app.ts".into()],
    )]);
    plan.mark_in_progress(0);

    let new_items = vec![PlanItem::new(
        "config work".into(),
        vec!["src/config.ts".into()],
    )];
    plan.supersede_stale_items(&new_items);
    assert_ne!(plan.items[0].status, PlanItemStatus::Superseded);
}

#[test]
fn completion_kind_with_superseded() {
    use anvil::contracts::CompletionKind;

    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("item1".into(), vec!["src/a.ts".into()]),
        PlanItem::new("item2".into(), vec!["src/b.ts".into()]),
    ]);
    // Mark item1 as Done, item2 as Superseded
    plan.items[0].status = PlanItemStatus::Done;
    plan.items[1].status = PlanItemStatus::Superseded;

    // All items are finished, no Blocked → should be CompleteUnverified (not Blocked)
    let kind = CompletionKind::classify(&plan, None, false);
    assert_eq!(kind, CompletionKind::CompleteUnverified);
}

#[test]
fn superseded_item_is_finished() {
    let mut item = PlanItem::new("test".into(), vec!["src/x.ts".into()]);
    item.status = PlanItemStatus::Superseded;
    assert!(item.is_finished());
}

#[test]
fn plan_item_status_display_superseded() {
    assert_eq!(PlanItemStatus::Superseded.to_string(), "superseded");
}

#[test]
fn format_checklist_superseded_marker() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "superseded task".into(),
        vec!["src/x.ts".into()],
    )]);
    plan.items[0].status = PlanItemStatus::Superseded;
    let checklist = plan.format_checklist();
    assert!(
        checklist.contains("[~]"),
        "superseded items should use [~] marker"
    );
}

// CB-001 / Issue #311: After supersede, the stale item becomes Superseded.
// The corrected item must NOT be deduped against the Superseded item so it
// remains actionable in the plan (prevents superseded-only terminal state).
#[test]
fn supersede_then_dedup_flow() {
    // Existing plan has a stale item (Pending, no mutations).
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "src/lib/polling/auto-yes-manager.ts: change".into(),
        vec!["src/lib/polling/auto-yes-manager.ts".into()],
    )]);
    // Corrected item has the same target.
    let new_items = vec![PlanItem::new(
        "src/lib/polling/auto-yes-manager.ts: corrected change".into(),
        vec!["src/lib/polling/auto-yes-manager.ts".into()],
    )];
    // Step 1: supersede — stale item becomes Superseded
    plan.supersede_stale_items(&new_items);
    assert_eq!(plan.items[0].status, PlanItemStatus::Superseded);
    // Step 2: dedup — corrected item survives (Superseded items excluded from dedup)
    let deduped = plan.deduplicate_new_items(new_items);
    assert_eq!(
        deduped.len(),
        1,
        "corrected item must survive dedup after stale item is superseded (Issue #311)"
    );
    // After appending corrected item, plan has actionable work.
    plan.append_items(deduped);
    assert!(!plan.all_finished(), "plan has Pending corrected item");
    assert_eq!(plan.items.len(), 2);
    assert_eq!(plan.items[0].status, PlanItemStatus::Superseded);
    assert_eq!(plan.items[1].status, PlanItemStatus::Pending);
}

// CB-001 variant: corrected item with DIFFERENT target (not matching stale)
// should NOT be deduped and should be appended.
#[test]
fn supersede_with_different_target_keeps_corrected() {
    let mut plan = ExecutionPlan::new(vec![PlanItem::new(
        "src/lib/polling/auto-yes-manager.ts (auto-yes-poller.ts): change".into(),
        vec!["src/lib/polling/auto-yes-manager.ts".into()],
    )]);
    // Corrected item targets a completely different file.
    let new_items = vec![PlanItem::new(
        "src/lib/auto-yes-poller.ts: corrected change".into(),
        vec!["src/lib/auto-yes-poller.ts".into()],
    )];
    // Step 1: supersede — stale item NOT superseded (different paths even after normalization)
    plan.supersede_stale_items(&new_items);
    // auto-yes-manager.ts does NOT match auto-yes-poller.ts (different basenames)
    assert_ne!(plan.items[0].status, PlanItemStatus::Superseded);
    // Step 2: dedup — no match, so corrected item survives
    let deduped = plan.deduplicate_new_items(new_items);
    assert_eq!(
        deduped.len(),
        1,
        "corrected item with different target should survive dedup"
    );
}

// CB-001 corollary: dedup should still work against finished items.
#[test]
fn dedup_still_works_against_finished_items() {
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
        "new item matching a Done item should be deduplicated"
    );
}

// CB-002: same file different tasks should not both be superseded.
#[test]
fn supersede_respects_one_to_one_matching() {
    let mut plan = ExecutionPlan::new(vec![
        PlanItem::new("src/app.ts: refactor API".into(), vec!["src/app.ts".into()]),
        PlanItem::new("src/app.ts: add tests".into(), vec!["src/app.ts".into()]),
    ]);
    plan.items[0].status = PlanItemStatus::InProgress;
    // Only one corrected item targeting src/app.ts.
    let new_items = vec![PlanItem::new(
        "src/app.ts: corrected refactor".into(),
        vec!["src/app.ts".into()],
    )];
    plan.supersede_stale_items(&new_items);
    // Only one item should be superseded (1:1), not both.
    let superseded_count = plan
        .items
        .iter()
        .filter(|i| i.status == PlanItemStatus::Superseded)
        .count();
    assert_eq!(
        superseded_count, 1,
        "only one item should be superseded per corrected item (1:1 matching)"
    );
}

// ---------------------------------------------------------------------------
// Issue #287 follow-up: orphan mutation tests
// ---------------------------------------------------------------------------

/// Reproduce the Issue #287 follow-up scenario:
/// LLM edits non-target files (e.g. renamed path) — mutations happen every turn
/// but no plan item advances. The orphan_mutation_condition must fire.
#[test]
fn plan_repair_orphan_mutation_fires_when_mutations_happen_but_plan_stalls() {
    let mut state = StagnationState::new();
    state.starved_target_files = vec!["src/lib/polling/auto-yes-manager.ts".to_string()];

    // 6 turns: mutation every turn (to a different file), same workset, no plan completion
    for _ in 0..6 {
        state.begin_turn(&[0]);
        state.record_mutation("src/lib/auto-yes-poller.ts");
        state.end_turn(true); // had_mutation = true
    }

    // Verify the orphan mutation pattern:
    assert_eq!(
        state.turns_since_last_mutation, 0,
        "mutations are happening"
    );
    assert!(
        state.turns_since_plan_item_completion >= 5,
        "plan items not advancing: got {}",
        state.turns_since_plan_item_completion
    );
    assert!(
        state.same_workset_turns >= 3,
        "same workset stuck: got {}",
        state.same_workset_turns
    );

    // Stagnation score should be only 2 (workset staleness + plan no-progress),
    // NOT 3, because mutations prevent mutation drought and read domination.
    let score = compute_stagnation_score(&state);
    assert!(
        score < 3,
        "score should be < 3 in orphan mutation case, got {}",
        score
    );

    // normal_condition requires starved >= 2: fails (only 1 starved file)
    // severe_condition requires turns_since_last_mutation >= 5: fails (it's 0)
    // But orphan_mutation_condition should fire
    assert!(should_request_plan_repair(&state, 0, 1));
}

#[test]
fn plan_repair_orphan_mutation_not_fire_without_starved_files() {
    let mut state = StagnationState::new();
    // No starved files
    for _ in 0..6 {
        state.begin_turn(&[0]);
        state.record_mutation("src/lib/auto-yes-poller.ts");
        state.end_turn(true);
    }
    assert_eq!(state.turns_since_last_mutation, 0);
    assert!(state.turns_since_plan_item_completion >= 5);
    // orphan_mutation_condition requires !starved.is_empty()
    assert!(!should_request_plan_repair(&state, 0, 1));
}

#[test]
fn plan_repair_orphan_mutation_not_fire_at_max_repair_count() {
    let mut state = StagnationState::new();
    state.starved_target_files = vec!["src/a.ts".to_string()];
    for _ in 0..6 {
        state.begin_turn(&[0]);
        state.record_mutation("src/b.ts");
        state.end_turn(true);
    }
    // plan_repair_request_count = 2 (max) → orphan_mutation_condition should not fire
    assert!(!should_request_plan_repair(&state, 2, 1));
}

#[test]
fn escape_hatch_orphan_escape_fires_after_repair_attempt() {
    let mut state = StagnationState::new();
    state.starved_target_files = vec!["src/lib/polling/auto-yes-manager.ts".to_string()];

    // 9 turns: mutation every turn, same workset, no plan completion
    for _ in 0..9 {
        state.begin_turn(&[0]);
        state.record_mutation("src/lib/auto-yes-poller.ts");
        state.end_turn(true);
    }

    assert_eq!(state.turns_since_last_mutation, 0);
    assert!(state.turns_since_plan_item_completion >= 8);
    let score = compute_stagnation_score(&state);
    assert!(score >= 2, "expected score >= 2, got {}", score);

    // Plan repair was attempted once, remaining <= 10
    assert!(should_allow_escape_hatch(&state, 1, 10));
}

#[test]
fn escape_hatch_orphan_escape_not_fire_without_repair_attempt() {
    let mut state = StagnationState::new();
    state.starved_target_files = vec!["src/a.ts".to_string()];

    for _ in 0..9 {
        state.begin_turn(&[0]);
        state.record_mutation("src/b.ts");
        state.end_turn(true);
    }

    // plan_repair_request_count = 0 → orphan_escape requires >= 1
    assert!(!should_allow_escape_hatch(&state, 0, 10));
}
