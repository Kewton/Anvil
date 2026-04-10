//! Regression tests for Issue #334: read-heavy stagnation must reach the
//! worker path even when no edit failures accumulate.
//!
//! Issue #332 solved the cross-path edit-failure case, but Phase2 drift
//! manifests as repeated reads / audits with no mutation and almost no
//! failed edit attempts. The stagnation-driven escalation therefore needs
//! a second trigger that does not depend on `edit_fail_tracker.total_failures()`.

use anvil::app::edit_fail_tracker::should_escalate_for_read_heavy_drift;
use anvil::app::stagnation_state::{StagnationState, compute_stagnation_score};
use anvil::contracts::{AgentTelemetry, PackExpectation, PackValidationResult};

// ---------------------------------------------------------------------------
// Policy pure function
// ---------------------------------------------------------------------------

#[test]
fn read_heavy_escalation_disabled_when_score_threshold_is_zero() {
    assert!(!should_escalate_for_read_heavy_drift(4, false, 10, 0, 5));
}

#[test]
fn read_heavy_escalation_requires_no_mutation_observed() {
    // Mutation already happened — not a read-heavy session anymore.
    assert!(!should_escalate_for_read_heavy_drift(4, true, 10, 2, 5));
}

#[test]
fn read_heavy_escalation_requires_score_above_threshold() {
    assert!(!should_escalate_for_read_heavy_drift(1, false, 10, 2, 5));
}

#[test]
fn read_heavy_escalation_requires_drought() {
    assert!(!should_escalate_for_read_heavy_drift(3, false, 4, 2, 5));
}

#[test]
fn read_heavy_escalation_fires_on_boundary_conditions() {
    // Exactly at both thresholds → fire.
    assert!(should_escalate_for_read_heavy_drift(2, false, 5, 2, 5));
}

#[test]
fn read_heavy_escalation_fires_when_score_and_drought_exceed_thresholds() {
    assert!(should_escalate_for_read_heavy_drift(4, false, 12, 2, 5));
}

// ---------------------------------------------------------------------------
// Integrated: stagnation state + policy reproduces the Issue #334 drift
// ---------------------------------------------------------------------------

#[test]
fn read_heavy_session_reaches_escalation_without_any_edit_failures() {
    // Reproduce the Phase2 pattern:
    //   - repeated read-only turns
    //   - no mutation observed
    //   - edit_fail_tracker.total_failures() == 0 (no failed edits)
    //   - stagnation score climbs
    //
    // The existing `should_escalate_for_stagnation` never fires because
    // it requires `total_failures >= threshold`. The new policy must.
    let mut state = StagnationState::new();
    let workset = vec![0usize];
    for _ in 0..6 {
        state.begin_turn(&workset);
        state.end_turn(false);
    }
    let score = compute_stagnation_score(&state);
    assert!(score >= 2, "expected score >= 2, got {score}");

    // mutation_observed == false, turns_since_last_mutation >= drought threshold.
    assert!(should_escalate_for_read_heavy_drift(
        score as u32,
        false,
        state.turns_since_last_mutation as u32,
        2,
        5,
    ));
}

// ---------------------------------------------------------------------------
// Runtime pack-validation gate must only accept worker_observed=true
// ---------------------------------------------------------------------------

#[test]
fn runtime_gate_rejects_repair_only_salvage_for_worker_required_pack() {
    let mut tel = AgentTelemetry::new();
    tel.record_pre_exit_repair_injected();
    tel.record_pre_exit_repair_consumed();
    tel.record_mutation_turn(8, Some(1.5), "file.edit");
    // Retrospectively classify as salvage — this is the exact A1 log pattern.
    tel.classify_repair_salvage();
    assert_eq!(tel.fixslice_escalation_repair_salvage_count, 1);
    assert!(!tel.worker_observed);

    let result = tel.validate_against(PackExpectation::RequiresWorkerObservation);
    match result {
        PackValidationResult::Mismatch { .. } => {}
        other => panic!("expected Mismatch, got {other:?}"),
    }
}

#[test]
fn runtime_gate_accepts_stagnation_triggered_worker_escalation() {
    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_escalation_stagnation();
    assert!(tel.worker_observed);

    let result = tel.validate_against(PackExpectation::RequiresWorkerObservation);
    assert_eq!(result, PackValidationResult::Satisfied);
}
