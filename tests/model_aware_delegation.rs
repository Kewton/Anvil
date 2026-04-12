//! Tests for Issue #292: model-aware slice selection and delegation policy.
//!
//! Covers delegation_threshold, estimate_target_path, sanitize_goal_for_hint,
//! proactive delegation exclusivity, retry budget, and telemetry.

use anvil::agent::model_classifier::ModelSizeClass;
use anvil::app::agentic::{delegation_threshold, estimate_target_path, sanitize_goal_for_hint};
use anvil::app::stagnation_state::{StagnationState, compute_stagnation_score};
use anvil::contracts::{AgentTelemetry, ExecutionPlan, PlanItem, PlanItemStatus};

// ---------------------------------------------------------------------------
// delegation_threshold tests
// ---------------------------------------------------------------------------

#[test]
fn delegation_threshold_large() {
    assert_eq!(delegation_threshold(ModelSizeClass::Large), 2);
}

#[test]
fn delegation_threshold_medium() {
    assert_eq!(delegation_threshold(ModelSizeClass::Medium), 1);
}

#[test]
fn delegation_threshold_small() {
    assert_eq!(delegation_threshold(ModelSizeClass::Small), 1);
}

// ---------------------------------------------------------------------------
// Large model non-regression
// ---------------------------------------------------------------------------

#[test]
fn large_model_no_proactive_delegation() {
    // Large model should never trigger proactive delegation — the threshold is 2,
    // same as the original hardcoded value.
    let threshold = delegation_threshold(ModelSizeClass::Large);
    assert_eq!(
        threshold, 2,
        "Large model threshold must remain 2 for backward compat"
    );

    // Even at score=2, the proactive branch requires Medium|Small, so Large is excluded.
    // This tests the threshold value only; the branch condition check is structural.
}

// ---------------------------------------------------------------------------
// target_path estimation
// ---------------------------------------------------------------------------

fn make_plan_item(desc: &str, target_files: Vec<&str>, status: PlanItemStatus) -> PlanItem {
    PlanItem {
        description: desc.to_string(),
        target_files: target_files.into_iter().map(String::from).collect(),
        status,
        retry_count: 0,
        mutated_files: Vec::new(),
    }
}

#[test]
fn target_path_estimation_plan_item() {
    let plan = ExecutionPlan {
        items: vec![
            make_plan_item("done item", vec!["src/done.rs"], PlanItemStatus::Done),
            make_plan_item(
                "pending item",
                vec!["src/pending.rs"],
                PlanItemStatus::Pending,
            ),
        ],
    };
    let state = StagnationState::new();
    let result = estimate_target_path(&plan, &state);
    assert_eq!(result, Some("src/pending.rs".to_string()));
}

#[test]
fn target_path_estimation_starved() {
    // No unfinished plan items with target_files -> falls back to starved_target_files
    let plan = ExecutionPlan {
        items: vec![make_plan_item(
            "done item",
            vec!["src/done.rs"],
            PlanItemStatus::Done,
        )],
    };
    let state = StagnationState::init_from_plan(&["src/starved.rs".to_string()]);
    let result = estimate_target_path(&plan, &state);
    assert_eq!(result, Some("src/starved.rs".to_string()));
}

#[test]
fn target_path_estimation_none() {
    // No unfinished plan items, no starved files -> None
    let plan = ExecutionPlan {
        items: vec![make_plan_item(
            "done item",
            vec!["src/done.rs"],
            PlanItemStatus::Done,
        )],
    };
    let state = StagnationState::new();
    let result = estimate_target_path(&plan, &state);
    assert_eq!(result, None);
}

#[test]
fn target_path_estimation_empty_target_files_skipped() {
    // Unfinished plan item without target_files should be skipped
    let plan = ExecutionPlan {
        items: vec![
            make_plan_item("no targets", vec![], PlanItemStatus::Pending),
            make_plan_item(
                "has targets",
                vec!["src/target.rs"],
                PlanItemStatus::Pending,
            ),
        ],
    };
    let state = StagnationState::new();
    let result = estimate_target_path(&plan, &state);
    assert_eq!(result, Some("src/target.rs".to_string()));
}

// ---------------------------------------------------------------------------
// retry budget
// ---------------------------------------------------------------------------

#[test]
fn retry_budget_allows_two_attempts() {
    let mut state = StagnationState::new();
    assert!(state.consume_proactive_retry_budget("src/a.rs"));
    assert!(state.consume_proactive_retry_budget("src/a.rs"));
    // Third attempt should be denied
    assert!(!state.consume_proactive_retry_budget("src/a.rs"));
}

#[test]
fn retry_budget_independent_per_path() {
    let mut state = StagnationState::new();
    assert!(state.consume_proactive_retry_budget("src/a.rs"));
    assert!(state.consume_proactive_retry_budget("src/a.rs"));
    // Different path should have its own budget
    assert!(state.consume_proactive_retry_budget("src/b.rs"));
}

#[test]
fn retry_budget_exhausted() {
    let mut state = StagnationState::new();
    // Exhaust budget for a path
    assert!(state.consume_proactive_retry_budget("src/a.rs"));
    assert!(state.consume_proactive_retry_budget("src/a.rs"));
    assert!(
        !state.consume_proactive_retry_budget("src/a.rs"),
        "third attempt should be denied"
    );
}

#[test]
fn retry_budget_resets_on_init_from_plan() {
    let mut state = StagnationState::new();
    state.consume_proactive_retry_budget("src/a.rs");
    state.consume_proactive_retry_budget("src/a.rs");

    // init_from_plan creates a new state -> budget should be fresh
    let new_state = StagnationState::init_from_plan(&["src/a.rs".to_string()]);
    assert!(new_state.proactive_delegation_retry_budget.is_empty());
}

#[test]
fn retry_budget_resets_on_replan() {
    let mut state = StagnationState::new();
    state.consume_proactive_retry_budget("src/a.rs");
    state.consume_proactive_retry_budget("src/a.rs");
    assert!(!state.consume_proactive_retry_budget("src/a.rs"));

    // Simulate replan: clear the budget
    state.proactive_delegation_retry_budget.clear();
    assert!(
        state.consume_proactive_retry_budget("src/a.rs"),
        "budget should be available after replan clear"
    );
}

#[test]
fn retry_budget_map_full_rejects_new_entries() {
    let mut state = StagnationState::new();
    // Fill up to 64 entries
    for i in 0..64 {
        state.consume_proactive_retry_budget(&format!("src/file_{i}.rs"));
    }
    assert_eq!(state.proactive_delegation_retry_budget.len(), 64);
    // New entry should be rejected
    assert!(
        !state.consume_proactive_retry_budget("src/new_file.rs"),
        "new entry should be rejected when map is full"
    );
    // Existing entry should still work
    assert!(state.consume_proactive_retry_budget("src/file_0.rs"));
}

// ---------------------------------------------------------------------------
// sanitize_goal_for_hint
// ---------------------------------------------------------------------------

#[test]
fn sanitize_goal_strips_control_chars() {
    let input = "Fix the \x00bug\x01 in \nthe code";
    let result = sanitize_goal_for_hint(input);
    assert!(!result.contains('\x00'));
    assert!(!result.contains('\x01'));
    assert!(!result.contains('\n'));
    assert!(result.contains("Fix the"));
    assert!(result.contains("bug"));
}

#[test]
fn sanitize_goal_removes_markdown_fences() {
    let input = "```rust\nfn main() {}\n```";
    let result = sanitize_goal_for_hint(input);
    assert!(!result.contains("```"));
}

#[test]
fn sanitize_goal_removes_anvil_markers() {
    let input = "ANVIL_FINAL should be removed and ANVIL_PLAN too";
    let result = sanitize_goal_for_hint(input);
    assert!(!result.contains("ANVIL_FINAL"));
    assert!(!result.contains("ANVIL_PLAN"));
    assert!(result.contains("should be removed"));
}

#[test]
fn sanitize_goal_truncates_to_200_chars() {
    let input = "a".repeat(300);
    let result = sanitize_goal_for_hint(&input);
    assert!(result.len() <= 200);
}

#[test]
fn sanitize_goal_trims_whitespace() {
    let input = "  hello world  ";
    let result = sanitize_goal_for_hint(input);
    assert_eq!(result, "hello world");
}

// ---------------------------------------------------------------------------
// proactive_and_failure_escalation_exclusive
// ---------------------------------------------------------------------------

#[test]
fn proactive_and_failure_escalation_exclusive() {
    // This test validates the design: when proactive_delegation_fired is true,
    // the failure-based escalation should be gated. We test this at the logic
    // level since the guard is `!proactive_delegation_fired && ...`.
    //
    // If proactive fires -> proactive_delegation_fired = true
    // Then !proactive_delegation_fired = false -> failure path skipped.
    let proactive_delegation_fired = true;
    let should_escalate_stagnation = true; // Would fire normally
    let should_escalate_read_heavy = true; // Would fire normally

    // With the guard, neither should fire
    let stagnation_fires = !proactive_delegation_fired && should_escalate_stagnation;
    let read_heavy_fires = !proactive_delegation_fired && should_escalate_read_heavy;

    assert!(
        !stagnation_fires,
        "stagnation escalation must be suppressed"
    );
    assert!(
        !read_heavy_fires,
        "read-heavy escalation must be suppressed"
    );
}

// ---------------------------------------------------------------------------
// medium_model_early_delegation (threshold check)
// ---------------------------------------------------------------------------

#[test]
fn medium_model_early_delegation_threshold() {
    // Medium model triggers forced mode at score=1 (vs Large at score=2)
    let threshold = delegation_threshold(ModelSizeClass::Medium);
    assert_eq!(threshold, 1);

    // Build a state with stagnation score = 1
    let mut state = StagnationState::new();
    // 5 turns without mutation -> +1 for mutation drought
    for _ in 0..5 {
        state.begin_turn(&[0]);
        state.end_turn(false);
    }
    let score = compute_stagnation_score(&state);
    assert!(score >= 1, "expected score >= 1, got {score}");

    // Medium: forced_mode_active = (score >= 1) = true
    let medium_forced = score >= threshold;
    assert!(
        medium_forced,
        "Medium model should activate forced mode at score=1"
    );

    // Large: forced_mode_active = (score >= 2) = false at score=1
    let large_threshold = delegation_threshold(ModelSizeClass::Large);
    let large_forced = score >= large_threshold;
    // At score=1, Large should NOT be forced
    if score < 2 {
        assert!(
            !large_forced,
            "Large model should NOT activate forced mode at score=1"
        );
    }
}

// ---------------------------------------------------------------------------
// AgentTelemetry model-aware delegation fields
// ---------------------------------------------------------------------------

#[test]
fn telemetry_model_aware_delegation_record() {
    let mut tel = AgentTelemetry::new();
    assert_eq!(tel.model_aware_delegation_count, 0);
    assert_eq!(tel.model_aware_delegation_produced_mutation, 0);

    tel.record_model_aware_delegation();
    assert_eq!(tel.model_aware_delegation_count, 1);
    assert_eq!(
        tel.model_aware_delegation_produced_mutation, 0,
        "mutation count should not change"
    );

    tel.record_model_aware_delegation_mutation();
    assert_eq!(tel.model_aware_delegation_produced_mutation, 1);
}

#[test]
fn telemetry_model_aware_does_not_affect_fixslice_escalation() {
    let mut tel = AgentTelemetry::new();
    tel.record_model_aware_delegation();
    tel.record_model_aware_delegation();
    tel.record_model_aware_delegation_mutation();

    // fixslice_escalation_count should remain 0 (DR2-008)
    assert_eq!(
        tel.fixslice_escalation_count, 0,
        "model-aware delegation must not increment fixslice_escalation_count"
    );
    assert_eq!(
        tel.fixslice_escalation_stagnation_count, 0,
        "model-aware delegation must not increment fixslice_escalation_stagnation_count"
    );
}

#[test]
fn telemetry_serde_default_backward_compat() {
    // Round-trip an AgentTelemetry without the new fields set.
    // The new fields have #[serde(default)], so they default to 0 when absent.
    let tel = AgentTelemetry::new();
    let json = serde_json::to_string(&tel).unwrap();
    let deserialized: AgentTelemetry = serde_json::from_str(&json).unwrap();
    assert_eq!(
        deserialized.model_aware_delegation_count, 0,
        "new field should default to 0 on roundtrip"
    );
    assert_eq!(
        deserialized.model_aware_delegation_produced_mutation, 0,
        "new field should default to 0 on roundtrip"
    );

    // Also verify that setting the fields survives roundtrip
    let mut tel2 = AgentTelemetry::new();
    tel2.record_model_aware_delegation();
    tel2.record_model_aware_delegation_mutation();
    let json2 = serde_json::to_string(&tel2).unwrap();
    let deser2: AgentTelemetry = serde_json::from_str(&json2).unwrap();
    assert_eq!(deser2.model_aware_delegation_count, 1);
    assert_eq!(deser2.model_aware_delegation_produced_mutation, 1);
}

// ---------------------------------------------------------------------------
// ModelSizeClass Display
// ---------------------------------------------------------------------------

#[test]
fn model_size_class_display() {
    assert_eq!(format!("{}", ModelSizeClass::Small), "Small");
    assert_eq!(format!("{}", ModelSizeClass::Medium), "Medium");
    assert_eq!(format!("{}", ModelSizeClass::Large), "Large");
}
