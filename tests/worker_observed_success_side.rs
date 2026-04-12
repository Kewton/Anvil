//! Regression tests for Issue #339: worker_observed must be a success-side
//! telemetry flag, not a request-side one.
//!
//! The exact A1 false-positive shape Issue #339 rejects:
//!
//! - runtime emitted a fix_slice escalation hint
//! - `worker_observed` was flipped to `true` at emission time
//! - the agent never produced a real mutation
//! - `mutation_observed` remained `false`
//! - pack validation returned `Satisfied` for `RequiresWorkerObservation`
//!
//! These tests pin the corrected semantics: `worker_observed` only becomes
//! `true` after `record_worker_success()`, which in runtime is called only
//! from `handle_fixslice_result` after the worker-owned `file.rewrite`
//! actually completes without rollback.

use anvil::contracts::{AgentTelemetry, PackExpectation, PackValidationResult};

// ---------------------------------------------------------------------------
// Negative: escalation-emission must NOT flip worker_observed
// ---------------------------------------------------------------------------

#[test]
fn same_path_escalation_alone_does_not_flip_worker_observed() {
    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_escalation();
    assert_eq!(tel.fixslice_escalation_count, 1);
    assert_eq!(tel.fixslice_escalation_same_path_count, 1);
    assert!(
        !tel.worker_observed,
        "same-path escalation is request-side and must not flip worker_observed"
    );
    assert!(!tel.mutation_observed);
}

#[test]
fn stagnation_escalation_alone_does_not_flip_worker_observed() {
    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_escalation_stagnation();
    assert_eq!(tel.fixslice_escalation_count, 1);
    assert_eq!(tel.fixslice_escalation_stagnation_count, 1);
    assert!(
        !tel.worker_observed,
        "stagnation escalation is request-side and must not flip worker_observed"
    );
    assert!(!tel.mutation_observed);
}

#[test]
fn repeated_escalations_never_flip_worker_observed() {
    let mut tel = AgentTelemetry::new();
    for _ in 0..5 {
        tel.record_fixslice_escalation();
        tel.record_fixslice_escalation_stagnation();
    }
    assert_eq!(tel.fixslice_escalation_count, 10);
    assert!(!tel.worker_observed);
}

// ---------------------------------------------------------------------------
// A1 false-positive shape: must be rejected by the pack validation gate
// ---------------------------------------------------------------------------

#[test]
fn a1_false_positive_shape_is_rejected_by_worker_gate() {
    // Reproduces the exact failure mode captured in Issue #339's A1 run:
    //   worker_observed=true (pre-fix) + mutation_observed=false + zero-diff
    //
    // After the fix, `worker_observed` is false because only escalation
    // hints were emitted, so the gate correctly reports Mismatch.
    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_escalation_stagnation(); // "A1 read-heavy stagnation"
    assert!(!tel.worker_observed);
    assert!(!tel.mutation_observed);

    let result = tel.validate_against(PackExpectation::RequiresWorkerObservation);
    match &result {
        PackValidationResult::Mismatch { expectation, .. } => {
            assert_eq!(*expectation, PackExpectation::RequiresWorkerObservation);
        }
        other => panic!("expected Mismatch, got {other:?}"),
    }
    assert!(tel.expectation_mismatch_reason.is_some());
}

#[test]
fn escalation_plus_unrelated_edit_mutation_still_rejected_by_worker_gate() {
    // Even if the agent later produces a mutation via ordinary `file.edit`
    // (not the worker path), a worker-required pack must still fail because
    // no real worker success was recorded.
    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_escalation();
    tel.record_mutation_turn(5, Some(2.0), "file.edit");
    assert!(tel.mutation_observed);
    assert!(!tel.worker_observed);

    let result = tel.validate_against(PackExpectation::RequiresWorkerObservation);
    assert!(matches!(result, PackValidationResult::Mismatch { .. }));
}

// ---------------------------------------------------------------------------
// Positive: escalation + real worker success satisfies the gate
// ---------------------------------------------------------------------------

#[test]
fn escalation_plus_worker_success_satisfies_worker_gate() {
    let mut tel = AgentTelemetry::new();
    // Runtime emits the escalation hint (request side).
    tel.record_fixslice_escalation();
    // Agent invokes agent.fix_slice; the resulting file.rewrite completes
    // without rollback — `handle_fixslice_result` calls both helpers.
    tel.record_mutation_turn(6, Some(3.0), "file.rewrite");
    tel.record_worker_success();

    assert!(tel.worker_observed);
    assert!(tel.mutation_observed);
    assert_eq!(tel.fixslice_escalation_count, 1);

    let result = tel.validate_against(PackExpectation::RequiresWorkerObservation);
    assert_eq!(result, PackValidationResult::Satisfied);
    assert!(tel.expectation_mismatch_reason.is_none());
}

#[test]
fn worker_success_without_prior_escalation_also_satisfies_gate() {
    // The agent may call agent.fix_slice proactively without a preceding
    // escalation hint. That path should still count as worker observation.
    let mut tel = AgentTelemetry::new();
    tel.record_mutation_turn(2, Some(0.8), "file.rewrite");
    tel.record_worker_success();

    assert!(tel.worker_observed);
    let result = tel.validate_against(PackExpectation::RequiresWorkerObservation);
    assert_eq!(result, PackValidationResult::Satisfied);
}

// ---------------------------------------------------------------------------
// Salvage classification interaction with the new semantics
// ---------------------------------------------------------------------------

#[test]
fn salvage_classification_fires_when_escalation_emitted_but_worker_did_not_succeed() {
    // Repair-only mutation after escalation: the escalation fired, but the
    // worker never actually succeeded. The session is a salvage, not a
    // worker-backed run.
    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_escalation();
    tel.record_pre_exit_repair_injected();
    tel.record_mutation_turn(7, Some(2.5), "file.edit");
    assert!(!tel.worker_observed);

    tel.classify_repair_salvage();
    assert_eq!(tel.fixslice_escalation_repair_salvage_count, 1);

    // And the worker gate still rejects salvage, even with escalation emitted.
    let result = tel.validate_against(PackExpectation::RequiresWorkerObservation);
    assert!(matches!(result, PackValidationResult::Mismatch { .. }));
}

#[test]
fn salvage_classification_suppressed_by_real_worker_success() {
    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_escalation_stagnation();
    tel.record_pre_exit_repair_injected();
    tel.record_mutation_turn(8, Some(3.0), "file.rewrite");
    tel.record_worker_success();

    tel.classify_repair_salvage();
    assert_eq!(
        tel.fixslice_escalation_repair_salvage_count, 0,
        "real worker success must suppress salvage classification"
    );
}
