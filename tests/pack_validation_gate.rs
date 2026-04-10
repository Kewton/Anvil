//! Tests for pack validation gate (Issue #329).
//!
//! Verifies that `PackExpectation` parsing, `AgentTelemetry` observability
//! fields, and the validation gate behave correctly.

use anvil::contracts::{AgentTelemetry, PackExpectation, PackValidationResult};

// ---------------------------------------------------------------------------
// PackExpectation parsing
// ---------------------------------------------------------------------------

#[test]
fn pack_expectation_from_env_str_valid() {
    assert_eq!(
        PackExpectation::from_env_str("audit_only"),
        Some(PackExpectation::AuditOnly)
    );
    assert_eq!(
        PackExpectation::from_env_str("requires_mutation"),
        Some(PackExpectation::RequiresMutation)
    );
    assert_eq!(
        PackExpectation::from_env_str("requires_worker_observation"),
        Some(PackExpectation::RequiresWorkerObservation)
    );
}

#[test]
fn pack_expectation_from_env_str_unknown() {
    assert_eq!(PackExpectation::from_env_str("unknown_value"), None);
    assert_eq!(PackExpectation::from_env_str(""), None);
}

#[test]
fn pack_expectation_display_roundtrip() {
    for (variant, expected) in [
        (PackExpectation::AuditOnly, "audit_only"),
        (PackExpectation::RequiresMutation, "requires_mutation"),
        (
            PackExpectation::RequiresWorkerObservation,
            "requires_worker_observation",
        ),
    ] {
        assert_eq!(variant.to_string(), expected);
        assert_eq!(PackExpectation::from_env_str(expected), Some(variant));
    }
}

// ---------------------------------------------------------------------------
// Observability flags set by record_* methods
// ---------------------------------------------------------------------------

#[test]
fn mutation_observed_set_by_record_mutation_turn() {
    let mut tel = AgentTelemetry::new();
    assert!(!tel.mutation_observed);
    tel.record_mutation_turn(1, Some(0.5), "file.write");
    assert!(tel.mutation_observed);
}

#[test]
fn worker_observed_set_by_record_fixslice_escalation() {
    let mut tel = AgentTelemetry::new();
    assert!(!tel.worker_observed);
    tel.record_fixslice_escalation();
    assert!(tel.worker_observed);
}

#[test]
fn repair_turn_observed_set_by_record_pre_exit_repair_injected() {
    let mut tel = AgentTelemetry::new();
    assert!(!tel.repair_turn_observed);
    tel.record_pre_exit_repair_injected();
    assert!(tel.repair_turn_observed);
}

#[test]
fn repair_turn_observed_set_by_record_pre_exit_repair_consumed() {
    let mut tel = AgentTelemetry::new();
    assert!(!tel.repair_turn_observed);
    tel.record_pre_exit_repair_consumed();
    assert!(tel.repair_turn_observed);
}

// ---------------------------------------------------------------------------
// validate_against: audit_only
// ---------------------------------------------------------------------------

#[test]
fn validate_audit_only_satisfied_with_no_mutation() {
    let mut tel = AgentTelemetry::new();
    let result = tel.validate_against(PackExpectation::AuditOnly);
    assert_eq!(result, PackValidationResult::Satisfied);
    assert_eq!(tel.pack_expectation, Some(PackExpectation::AuditOnly));
    assert!(tel.expectation_mismatch_reason.is_none());
}

#[test]
fn validate_audit_only_satisfied_with_mutation() {
    let mut tel = AgentTelemetry::new();
    tel.record_mutation_turn(1, Some(0.5), "file.write");
    let result = tel.validate_against(PackExpectation::AuditOnly);
    assert_eq!(result, PackValidationResult::Satisfied);
}

// ---------------------------------------------------------------------------
// validate_against: requires_mutation
// ---------------------------------------------------------------------------

#[test]
fn validate_requires_mutation_mismatch_when_no_mutation() {
    let mut tel = AgentTelemetry::new();
    let result = tel.validate_against(PackExpectation::RequiresMutation);
    match &result {
        PackValidationResult::Mismatch {
            expectation,
            reason,
        } => {
            assert_eq!(*expectation, PackExpectation::RequiresMutation);
            assert!(reason.contains("zero file changes"), "reason: {reason}");
        }
        other => panic!("expected Mismatch, got {other:?}"),
    }
    assert!(tel.expectation_mismatch_reason.is_some());
}

#[test]
fn validate_requires_mutation_satisfied_when_mutation_observed() {
    let mut tel = AgentTelemetry::new();
    tel.record_mutation_turn(3, Some(1.0), "file.edit");
    let result = tel.validate_against(PackExpectation::RequiresMutation);
    assert_eq!(result, PackValidationResult::Satisfied);
    assert!(tel.expectation_mismatch_reason.is_none());
}

// ---------------------------------------------------------------------------
// validate_against: requires_worker_observation
// ---------------------------------------------------------------------------

#[test]
fn validate_requires_worker_observation_mismatch_when_nothing_observed() {
    let mut tel = AgentTelemetry::new();
    let result = tel.validate_against(PackExpectation::RequiresWorkerObservation);
    match &result {
        PackValidationResult::Mismatch {
            expectation,
            reason,
        } => {
            assert_eq!(*expectation, PackExpectation::RequiresWorkerObservation);
            assert!(reason.contains("no worker path"), "reason: {reason}");
        }
        other => panic!("expected Mismatch, got {other:?}"),
    }
}

#[test]
fn validate_requires_worker_observation_satisfied_by_fixslice() {
    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_escalation();
    let result = tel.validate_against(PackExpectation::RequiresWorkerObservation);
    assert_eq!(result, PackValidationResult::Satisfied);
}

#[test]
fn validate_requires_worker_observation_satisfied_by_repair_turn() {
    let mut tel = AgentTelemetry::new();
    tel.record_pre_exit_repair_injected();
    let result = tel.validate_against(PackExpectation::RequiresWorkerObservation);
    assert_eq!(result, PackValidationResult::Satisfied);
}

#[test]
fn validate_requires_worker_observation_satisfied_by_mutation() {
    let mut tel = AgentTelemetry::new();
    tel.record_mutation_turn(2, Some(0.5), "file.write");
    let result = tel.validate_against(PackExpectation::RequiresWorkerObservation);
    assert_eq!(result, PackValidationResult::Satisfied);
}

// ---------------------------------------------------------------------------
// Telemetry artifact includes pack validation fields (Issue #329)
// ---------------------------------------------------------------------------

#[test]
fn telemetry_artifact_includes_pack_validation_fields() {
    let dir = tempfile::tempdir().unwrap();
    let session_id = format!("test_pack_fields_{}", std::process::id());
    let dir_str = dir.path().to_str().unwrap();

    let mut tel = AgentTelemetry::new();
    tel.record_mutation_turn(1, Some(0.3), "file.write");
    tel.record_fixslice_escalation();
    tel.record_pre_exit_repair_injected();
    tel.pack_expectation = Some(PackExpectation::RequiresWorkerObservation);

    tel.write_artifact_to_dir(dir_str, &session_id).unwrap();

    let file_path = dir.path().join(format!("{session_id}_telemetry.json"));
    let contents = std::fs::read_to_string(&file_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&contents).unwrap();

    assert_eq!(json["mutation_observed"], true);
    assert_eq!(json["worker_observed"], true);
    assert_eq!(json["repair_turn_observed"], true);
    assert_eq!(json["pack_expectation"], "requires_worker_observation");
    assert!(json["expectation_mismatch_reason"].is_null());
}

#[test]
fn telemetry_artifact_pack_fields_default_false() {
    let dir = tempfile::tempdir().unwrap();
    let session_id = format!("test_pack_defaults_{}", std::process::id());
    let dir_str = dir.path().to_str().unwrap();

    let tel = AgentTelemetry::new();
    tel.write_artifact_to_dir(dir_str, &session_id).unwrap();

    let file_path = dir.path().join(format!("{session_id}_telemetry.json"));
    let contents = std::fs::read_to_string(&file_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&contents).unwrap();

    assert_eq!(json["mutation_observed"], false);
    assert_eq!(json["worker_observed"], false);
    assert_eq!(json["repair_turn_observed"], false);
    assert!(json["pack_expectation"].is_null());
    assert!(json["expectation_mismatch_reason"].is_null());
}

#[test]
fn telemetry_artifact_mismatch_reason_persisted() {
    let dir = tempfile::tempdir().unwrap();
    let session_id = format!("test_pack_mismatch_{}", std::process::id());
    let dir_str = dir.path().to_str().unwrap();

    let mut tel = AgentTelemetry::new();
    // No mutations, no worker, no repair → mismatch for requires_mutation
    tel.validate_against(PackExpectation::RequiresMutation);

    tel.write_artifact_to_dir(dir_str, &session_id).unwrap();

    let file_path = dir.path().join(format!("{session_id}_telemetry.json"));
    let contents = std::fs::read_to_string(&file_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&contents).unwrap();

    assert_eq!(json["pack_expectation"], "requires_mutation");
    assert!(
        json["expectation_mismatch_reason"]
            .as_str()
            .unwrap()
            .contains("zero file changes")
    );
}

// ---------------------------------------------------------------------------
// PackValidationResult Display
// ---------------------------------------------------------------------------

#[test]
fn pack_validation_result_display() {
    assert_eq!(
        PackValidationResult::NoExpectation.to_string(),
        "no_expectation"
    );
    assert_eq!(PackValidationResult::Satisfied.to_string(), "satisfied");

    let mismatch = PackValidationResult::Mismatch {
        expectation: PackExpectation::RequiresMutation,
        reason: "no changes".to_string(),
    };
    assert_eq!(
        mismatch.to_string(),
        "mismatch(requires_mutation): no changes"
    );
}
