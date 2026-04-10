//! Integration tests for Issue #343: fix_slice failure control.
//!
//! Issue #343 covers the failure mode where `agent.fix_slice` is invoked
//! (request side) but never reaches a successful worker mutation, yet the
//! parent session continues to drift: `file.edit` lands a mutation later,
//! the pre-exit repair turn is consumed, and telemetry ends with
//! `worker_observed=false`, `mutation_observed=true`, `repair_turn_observed=true`.
//!
//! Under a `requires_worker_observation` pack expectation, that is a
//! benchmark failure — the worker never produced observable evidence, but
//! parent-side salvage kept the loop running and ended in `partial`.
//!
//! The fix:
//! - classifies *why* fix_slice failed (no proposal, validation failure,
//!   rewrite failure, max iterations),
//! - exposes a failure counter + last reason on `AgentTelemetry`, and
//! - provides `should_skip_pre_exit_repair_for_worker_failure()` so the
//!   agentic loop can bail out cleanly instead of injecting a repair turn
//!   that would otherwise flip `repair_turn_observed` and degrade
//!   `completion_kind` to `partial`.

use anvil::contracts::{
    AgentTelemetry, FixSliceFailureReason, PackExpectation, PackValidationResult,
};

// ---------------------------------------------------------------------------
// FixSliceFailureReason
// ---------------------------------------------------------------------------

#[test]
fn fixslice_failure_reason_display_roundtrip() {
    for (variant, expected) in [
        (FixSliceFailureReason::NoProposal, "no_proposal"),
        (
            FixSliceFailureReason::ProposalValidationFailed,
            "proposal_validation_failed",
        ),
        (FixSliceFailureReason::RewriteFailed, "rewrite_failed"),
        (
            FixSliceFailureReason::MaxIterationsReached,
            "max_iterations_reached",
        ),
    ] {
        assert_eq!(variant.to_string(), expected);
    }
}

// ---------------------------------------------------------------------------
// record_fixslice_worker_failure
// ---------------------------------------------------------------------------

#[test]
fn record_fixslice_worker_failure_sets_count_and_reason() {
    let mut tel = AgentTelemetry::new();
    assert_eq!(tel.fixslice_worker_failure_count, 0);
    assert!(tel.fixslice_failure_reason.is_none());

    tel.record_fixslice_worker_failure(FixSliceFailureReason::MaxIterationsReached);

    assert_eq!(tel.fixslice_worker_failure_count, 1);
    assert_eq!(
        tel.fixslice_failure_reason.as_deref(),
        Some("max_iterations_reached")
    );
    assert!(
        !tel.worker_observed,
        "recording a fix_slice failure must NOT flip worker_observed"
    );
}

#[test]
fn record_fixslice_worker_failure_accumulates_count_and_tracks_latest_reason() {
    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_worker_failure(FixSliceFailureReason::NoProposal);
    tel.record_fixslice_worker_failure(FixSliceFailureReason::MaxIterationsReached);
    tel.record_fixslice_worker_failure(FixSliceFailureReason::ProposalValidationFailed);

    assert_eq!(tel.fixslice_worker_failure_count, 3);
    assert_eq!(
        tel.fixslice_failure_reason.as_deref(),
        Some("proposal_validation_failed"),
        "latest failure reason should win for observability"
    );
}

#[test]
fn has_fixslice_worker_failure_tracks_any_recorded_failure() {
    let mut tel = AgentTelemetry::new();
    assert!(!tel.has_fixslice_worker_failure());
    tel.record_fixslice_worker_failure(FixSliceFailureReason::RewriteFailed);
    assert!(tel.has_fixslice_worker_failure());
}

// ---------------------------------------------------------------------------
// should_skip_pre_exit_repair_for_worker_failure
// ---------------------------------------------------------------------------

#[test]
fn should_skip_repair_true_for_worker_required_after_fixslice_failure() {
    // The A1 shape from Issue #343:
    //  - pack expectation = requires_worker_observation
    //  - fix_slice invoked and failed (e.g. max iterations)
    //  - worker success never recorded
    //
    // Under the fix, the agentic loop must NOT inject a pre-exit repair
    // turn — the pack cannot be satisfied and the repair turn would only
    // flip repair_turn_observed and degrade completion_kind.
    let mut tel = AgentTelemetry::new();
    tel.pack_expectation = Some(PackExpectation::RequiresWorkerObservation);
    tel.record_fixslice_escalation();
    tel.record_fixslice_worker_failure(FixSliceFailureReason::MaxIterationsReached);

    assert!(tel.should_skip_pre_exit_repair_for_worker_failure());
}

#[test]
fn should_skip_repair_false_when_worker_success_was_recorded() {
    let mut tel = AgentTelemetry::new();
    tel.pack_expectation = Some(PackExpectation::RequiresWorkerObservation);
    tel.record_fixslice_escalation();
    tel.record_fixslice_worker_failure(FixSliceFailureReason::RewriteFailed);
    // A later fix_slice attempt eventually succeeds:
    tel.record_mutation_turn(6, Some(2.0), "file.rewrite");
    tel.record_worker_success();

    assert!(!tel.should_skip_pre_exit_repair_for_worker_failure());
}

#[test]
fn should_skip_repair_false_without_any_fixslice_failure() {
    let mut tel = AgentTelemetry::new();
    tel.pack_expectation = Some(PackExpectation::RequiresWorkerObservation);
    // Pack is worker-required but fix_slice was never called / never
    // failed — the existing escape-hatch behavior should remain in effect.
    assert!(!tel.should_skip_pre_exit_repair_for_worker_failure());
}

#[test]
fn should_skip_repair_false_when_pack_expectation_is_unset() {
    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_worker_failure(FixSliceFailureReason::NoProposal);
    assert!(!tel.should_skip_pre_exit_repair_for_worker_failure());
}

#[test]
fn should_skip_repair_false_for_requires_mutation_pack() {
    let mut tel = AgentTelemetry::new();
    tel.pack_expectation = Some(PackExpectation::RequiresMutation);
    tel.record_fixslice_worker_failure(FixSliceFailureReason::MaxIterationsReached);
    // requires_mutation can still be satisfied by a parent-side file.edit
    // mutation, so the worker-failure skip must not kick in.
    assert!(!tel.should_skip_pre_exit_repair_for_worker_failure());
}

#[test]
fn should_skip_repair_false_for_audit_only_pack() {
    let mut tel = AgentTelemetry::new();
    tel.pack_expectation = Some(PackExpectation::AuditOnly);
    tel.record_fixslice_worker_failure(FixSliceFailureReason::NoProposal);
    assert!(!tel.should_skip_pre_exit_repair_for_worker_failure());
}

// ---------------------------------------------------------------------------
// Interaction with pack validation
// ---------------------------------------------------------------------------

#[test]
fn worker_required_gate_still_rejects_after_fixslice_failure_only() {
    // Parity with Issue #339 semantics: a recorded fix_slice failure on
    // its own must not accidentally flip worker_observed. The gate must
    // continue to reject the session.
    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_escalation();
    tel.record_fixslice_worker_failure(FixSliceFailureReason::MaxIterationsReached);

    let result = tel.validate_against(PackExpectation::RequiresWorkerObservation);
    assert!(matches!(result, PackValidationResult::Mismatch { .. }));
    assert!(!tel.worker_observed);
}

// ---------------------------------------------------------------------------
// Telemetry artifact persistence
// ---------------------------------------------------------------------------

#[test]
fn telemetry_artifact_includes_fixslice_failure_fields() {
    let dir = tempfile::tempdir().unwrap();
    let session_id = format!("test_fixslice_failure_{}", std::process::id());
    let dir_str = dir.path().to_str().unwrap();

    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_escalation();
    tel.record_fixslice_worker_failure(FixSliceFailureReason::MaxIterationsReached);
    tel.record_fixslice_worker_failure(FixSliceFailureReason::NoProposal);
    tel.pack_expectation = Some(PackExpectation::RequiresWorkerObservation);

    tel.write_artifact_to_dir(dir_str, &session_id).unwrap();

    let file_path = dir.path().join(format!("{session_id}_telemetry.json"));
    let contents = std::fs::read_to_string(&file_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&contents).unwrap();

    assert_eq!(json["fixslice_worker_failure_count"], 2);
    assert_eq!(json["fixslice_failure_reason"], "no_proposal");
}

#[test]
fn telemetry_artifact_fixslice_failure_fields_default() {
    let dir = tempfile::tempdir().unwrap();
    let session_id = format!("test_fixslice_failure_default_{}", std::process::id());
    let dir_str = dir.path().to_str().unwrap();

    let tel = AgentTelemetry::new();
    tel.write_artifact_to_dir(dir_str, &session_id).unwrap();

    let file_path = dir.path().join(format!("{session_id}_telemetry.json"));
    let contents = std::fs::read_to_string(&file_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&contents).unwrap();

    assert_eq!(json["fixslice_worker_failure_count"], 0);
    assert!(json["fixslice_failure_reason"].is_null());
}

// ---------------------------------------------------------------------------
// Salvage classification interaction — the new failure counter is orthogonal
// to the existing repair-salvage classifier and must not suppress it.
// ---------------------------------------------------------------------------

#[test]
fn fixslice_failure_does_not_suppress_repair_salvage_classification() {
    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_escalation();
    tel.record_fixslice_worker_failure(FixSliceFailureReason::MaxIterationsReached);
    tel.record_pre_exit_repair_injected();
    tel.record_mutation_turn(11, Some(4.0), "file.edit");

    tel.classify_repair_salvage();
    assert_eq!(
        tel.fixslice_escalation_repair_salvage_count, 1,
        "repair-salvage classifier must still fire independently"
    );
    assert!(!tel.worker_observed);
}
