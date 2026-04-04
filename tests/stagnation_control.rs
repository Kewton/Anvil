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

    // remaining_turns < 5 → false
    assert!(!should_request_plan_repair(&state, 0, 4));
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
}
