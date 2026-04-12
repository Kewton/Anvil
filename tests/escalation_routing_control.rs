//! Integration tests for Issue #355: escalation routing + post-success early-exit.
//!
//! Two runtime bugs are fixed:
//!
//! 1. `escalated_not_invoked` — under a `requires_worker_observation` pack,
//!    once fix_slice escalation has fired the parent must actually call
//!    `agent.fix_slice`. Parent-side mutation tools (`file.edit`,
//!    `file.write`, `file.edit_anchor`) must be suspended until the worker
//!    path has been invoked.
//!
//! 2. post-success continuation timeout — once `worker_observed=true` under
//!    a worker-required pack, the session must exit cleanly on the next
//!    loop boundary instead of burning turns (or the 600s runner timeout).

use anvil::app::escalation_barrier::{
    ESCALATION_BARRIER_MESSAGE, EscalationBarrier, MAX_ESCALATION_BARRIER_BLOCKS,
};
use anvil::contracts::{AgentTelemetry, PackExpectation};
use anvil::tooling::{
    ExecutionClass, ExecutionMode, PermissionClass, PlanModePolicy, RollbackPolicy,
    ToolExecutionRequest, ToolExecutionStatus, ToolInput, ToolKind, ToolSpec,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_request(name: &str, kind: ToolKind, input: ToolInput) -> (usize, ToolExecutionRequest) {
    (
        0,
        ToolExecutionRequest {
            tool_call_id: format!("call_{name}"),
            spec: ToolSpec {
                version: 1,
                name: name.to_string(),
                kind,
                execution_class: ExecutionClass::Mutating,
                permission_class: PermissionClass::Confirm,
                execution_mode: ExecutionMode::SequentialOnly,
                plan_mode: PlanModePolicy::AllowedWithScope,
                rollback_policy: RollbackPolicy::None,
            },
            input,
            extra_field_warnings: Vec::new(),
        },
    )
}

fn edit_req() -> (usize, ToolExecutionRequest) {
    make_request(
        "file.edit",
        ToolKind::FileEdit,
        ToolInput::FileEdit {
            path: "src/foo.rs".into(),
            old_string: "old".into(),
            new_string: "new".into(),
        },
    )
}

fn write_req() -> (usize, ToolExecutionRequest) {
    make_request(
        "file.write",
        ToolKind::FileWrite,
        ToolInput::FileWrite {
            path: "src/foo.rs".into(),
            content: "content".into(),
        },
    )
}

fn read_req() -> (usize, ToolExecutionRequest) {
    make_request(
        "file.read",
        ToolKind::FileRead,
        ToolInput::FileRead {
            path: "src/foo.rs".into(),
        },
    )
}

// ---------------------------------------------------------------------------
// AgentTelemetry: should_force_fixslice_routing
// ---------------------------------------------------------------------------

#[test]
fn force_fixslice_routing_true_under_worker_required_after_escalation() {
    let mut tel = AgentTelemetry::new();
    tel.pack_expectation = Some(PackExpectation::RequiresWorkerObservation);
    tel.record_fixslice_escalation();
    assert!(tel.should_force_fixslice_routing());
}

#[test]
fn force_fixslice_routing_false_when_worker_invocation_already_recorded() {
    let mut tel = AgentTelemetry::new();
    tel.pack_expectation = Some(PackExpectation::RequiresWorkerObservation);
    tel.record_fixslice_escalation();
    tel.record_fixslice_worker_invocation();
    assert!(!tel.should_force_fixslice_routing());
}

#[test]
fn force_fixslice_routing_false_when_worker_observed() {
    let mut tel = AgentTelemetry::new();
    tel.pack_expectation = Some(PackExpectation::RequiresWorkerObservation);
    tel.record_fixslice_escalation();
    tel.record_worker_success();
    assert!(!tel.should_force_fixslice_routing());
}

#[test]
fn force_fixslice_routing_false_without_escalation() {
    let mut tel = AgentTelemetry::new();
    tel.pack_expectation = Some(PackExpectation::RequiresWorkerObservation);
    assert!(!tel.should_force_fixslice_routing());
}

#[test]
fn force_fixslice_routing_false_for_audit_only_pack() {
    let mut tel = AgentTelemetry::new();
    tel.pack_expectation = Some(PackExpectation::AuditOnly);
    tel.record_fixslice_escalation();
    assert!(!tel.should_force_fixslice_routing());
}

#[test]
fn force_fixslice_routing_false_for_requires_mutation_pack() {
    let mut tel = AgentTelemetry::new();
    tel.pack_expectation = Some(PackExpectation::RequiresMutation);
    tel.record_fixslice_escalation();
    assert!(!tel.should_force_fixslice_routing());
}

#[test]
fn force_fixslice_routing_false_when_pack_expectation_unset() {
    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_escalation();
    assert!(!tel.should_force_fixslice_routing());
}

// ---------------------------------------------------------------------------
// AgentTelemetry: should_early_exit_after_worker_success
// ---------------------------------------------------------------------------

#[test]
fn early_exit_true_under_worker_required_pack_when_worker_observed() {
    let mut tel = AgentTelemetry::new();
    tel.pack_expectation = Some(PackExpectation::RequiresWorkerObservation);
    tel.record_worker_success();
    assert!(tel.should_early_exit_after_worker_success());
}

#[test]
fn early_exit_false_when_worker_not_observed() {
    let mut tel = AgentTelemetry::new();
    tel.pack_expectation = Some(PackExpectation::RequiresWorkerObservation);
    assert!(!tel.should_early_exit_after_worker_success());
}

#[test]
fn early_exit_false_for_audit_only_pack() {
    let mut tel = AgentTelemetry::new();
    tel.pack_expectation = Some(PackExpectation::AuditOnly);
    tel.record_worker_success();
    assert!(!tel.should_early_exit_after_worker_success());
}

#[test]
fn early_exit_false_for_requires_mutation_pack() {
    let mut tel = AgentTelemetry::new();
    tel.pack_expectation = Some(PackExpectation::RequiresMutation);
    tel.record_worker_success();
    assert!(!tel.should_early_exit_after_worker_success());
}

#[test]
fn early_exit_false_when_pack_expectation_unset() {
    let mut tel = AgentTelemetry::new();
    tel.record_worker_success();
    assert!(!tel.should_early_exit_after_worker_success());
}

// ---------------------------------------------------------------------------
// Telemetry counters
// ---------------------------------------------------------------------------

#[test]
fn record_escalation_barrier_block_increments_counter() {
    let mut tel = AgentTelemetry::new();
    assert_eq!(tel.escalation_barrier_block_count, 0);
    tel.record_escalation_barrier_block();
    tel.record_escalation_barrier_block();
    assert_eq!(tel.escalation_barrier_block_count, 2);
}

#[test]
fn record_worker_required_early_exit_increments_counter() {
    let mut tel = AgentTelemetry::new();
    assert_eq!(tel.worker_required_early_exit_count, 0);
    tel.record_worker_required_early_exit();
    assert_eq!(tel.worker_required_early_exit_count, 1);
}

// ---------------------------------------------------------------------------
// finalize_fixslice_outcome interaction
// ---------------------------------------------------------------------------

#[test]
fn early_exit_path_does_not_mark_escalated_not_invoked() {
    let mut tel = AgentTelemetry::new();
    tel.pack_expectation = Some(PackExpectation::RequiresWorkerObservation);
    tel.record_fixslice_escalation();
    tel.record_fixslice_worker_invocation();
    tel.record_worker_success();

    tel.finalize_fixslice_outcome();
    assert!(
        tel.fixslice_failure_reason.is_none(),
        "worker success path must not be classified as escalated_not_invoked"
    );
}

// ---------------------------------------------------------------------------
// Telemetry artifact serialization
// ---------------------------------------------------------------------------

#[test]
fn telemetry_artifact_exposes_new_counters() {
    let dir = tempfile::tempdir().unwrap();
    let session_id = format!("test_escalation_routing_{}", std::process::id());

    let mut tel = AgentTelemetry::new();
    tel.record_escalation_barrier_block();
    tel.record_escalation_barrier_block();
    tel.record_worker_required_early_exit();
    tel.write_artifact_to_dir(dir.path().to_str().unwrap(), &session_id)
        .unwrap();

    let file_path = dir.path().join(format!("{session_id}_telemetry.json"));
    let contents = std::fs::read_to_string(&file_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&contents).unwrap();

    assert_eq!(json["escalation_barrier_block_count"], 2);
    assert_eq!(json["worker_required_early_exit_count"], 1);
}

#[test]
fn telemetry_artifact_new_counters_default_to_zero() {
    let dir = tempfile::tempdir().unwrap();
    let session_id = format!("test_escalation_routing_default_{}", std::process::id());

    let tel = AgentTelemetry::new();
    tel.write_artifact_to_dir(dir.path().to_str().unwrap(), &session_id)
        .unwrap();

    let file_path = dir.path().join(format!("{session_id}_telemetry.json"));
    let contents = std::fs::read_to_string(&file_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&contents).unwrap();

    assert_eq!(json["escalation_barrier_block_count"], 0);
    assert_eq!(json["worker_required_early_exit_count"], 0);
}

#[test]
fn telemetry_serde_default_tolerates_missing_new_counters() {
    let json = r#"{"premature_final_count":0,"total_final_requests":0,"plan_registration_count":0,"plan_update_count":0,"sync_from_touched_files_count":0}"#;
    let tel: AgentTelemetry = serde_json::from_str(json).unwrap();
    assert_eq!(tel.escalation_barrier_block_count, 0);
    assert_eq!(tel.worker_required_early_exit_count, 0);
}

// ---------------------------------------------------------------------------
// EscalationBarrier
// ---------------------------------------------------------------------------

#[test]
fn escalation_barrier_blocks_file_edit_when_armed() {
    let mut barrier = EscalationBarrier::new();
    let result = barrier.check_and_filter(vec![edit_req()], true);

    assert_eq!(result.blocked_count, 1);
    assert!(result.passed_requests.is_empty());
    let (_, blocked) = &result.blocked_results[0];
    assert_eq!(blocked.tool_name, "file.edit");
    assert_eq!(blocked.status, ToolExecutionStatus::Blocked);
    assert!(
        blocked.summary.starts_with("[fixslice_escalation_barrier]"),
        "blocked summary should start with [fixslice_escalation_barrier], got: {}",
        blocked.summary
    );
    assert!(
        blocked.summary.contains(ESCALATION_BARRIER_MESSAGE),
        "blocked summary should contain the escalation barrier message"
    );
}

#[test]
fn escalation_barrier_blocks_file_write_when_armed() {
    let mut barrier = EscalationBarrier::new();
    let result = barrier.check_and_filter(vec![write_req()], true);
    assert_eq!(result.blocked_count, 1);
    assert!(result.passed_requests.is_empty());
}

#[test]
fn escalation_barrier_allows_reads_when_armed() {
    let mut barrier = EscalationBarrier::new();
    let result = barrier.check_and_filter(vec![read_req()], true);
    assert_eq!(result.blocked_count, 0);
    assert_eq!(result.passed_requests.len(), 1);
}

#[test]
fn escalation_barrier_passes_all_when_disarmed() {
    let mut barrier = EscalationBarrier::new();
    let result = barrier.check_and_filter(vec![edit_req(), write_req()], false);
    assert_eq!(result.blocked_count, 0);
    assert_eq!(result.passed_requests.len(), 2);
}

#[test]
fn escalation_barrier_respects_max_blocks() {
    let mut barrier = EscalationBarrier::new();
    for _ in 0..MAX_ESCALATION_BARRIER_BLOCKS {
        let result = barrier.check_and_filter(vec![edit_req()], true);
        assert_eq!(result.blocked_count, 1);
    }
    // Next call should auto-release.
    let result = barrier.check_and_filter(vec![edit_req()], true);
    assert_eq!(result.blocked_count, 0);
    assert_eq!(result.passed_requests.len(), 1);
}

#[test]
fn escalation_barrier_mixed_batch_blocks_only_mutations() {
    let mut barrier = EscalationBarrier::new();
    let result = barrier.check_and_filter(vec![read_req(), edit_req(), write_req()], true);
    assert_eq!(result.passed_requests.len(), 1);
    assert_eq!(result.passed_requests[0].1.spec.name, "file.read");
    assert_eq!(result.blocked_count, 2);
}
