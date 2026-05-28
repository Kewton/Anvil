//! Issue #682 (parent #680, Phase 2): verifier orchestration data types
//! extracted from `turn.rs`.
//!
//! Hosts the 7 `pub(super)` types that flow through the task-contract
//! verifier driver:
//!
//! * `JobInstallOutcome` — result of installing an artifact-completion
//!   job from a task-contract hint (PR-001 SSOT invariant for
//!   `current_artifact_recovery_target`).
//! * `VerifierDiagnosticPassOutcome` — accepted / retry / unavailable /
//!   skipped state of a single verifier diagnostic pass.
//! * `PreparedVerifierDiagnosticPass` — context + attempt spec + active
//!   request + optional behavior projection consumed by the diagnostic
//!   pass dispatcher.
//! * `PreparedVerifierRepairPass` — context + accepted repair plan +
//!   conversation history + selected model consumed by the repair pass
//!   dispatcher.
//! * `VerifierRepairAttemptProgress` — Return / Continue / Break trichotomy
//!   driving the repair-pass attempt loop.
//! * `StructuredTaskContractVerifierRun` — auto-test plan + verifier
//!   command + display command + bound test artifact metadata for the
//!   structured verifier run path.
//! * `TaskContractVerifierFlowArgs<'a, 'b>` — `&mut` state bundle for
//!   `drive_task_contract_verifier`.
//!
//! Phase 2 scope (Issue #682): this PR migrates the **type definitions
//! only**. The dispatch / orchestration methods on `impl Agent`
//! (`run_verifier_diagnostic_pass`, `verifier_repair_pass`,
//! `drive_task_contract_verifier`, `record_controller_verifier_repair_invalid`,
//! `emit_safe_stop_report_for_repair_exhausted`) stay in `turn.rs` for
//! now and will be migrated in follow-up PRs, mirroring the Phase 1
//! actor_loop_flow pattern.
//!
//! `pub(super)` limited / no facade re-export (DR3-001).
//! `turn.rs` is the only in-crate consumer; `actor_loop_flow.rs`
//! re-imports `TaskContractVerifierFlowArgs` via `super::turn::` until
//! a follow-up Phase 2 PR moves the consumer-side reference.

use super::auto_test::{AutoTestPlan, VerifierCommand};
use super::repair_attempt_outcome::RepairAttemptOutcome;
use super::repair_driver::VerifierRepairPassOutcome;
use super::repair_job::RepairJob;
use super::repair_plan::AcceptedRepairPlan;
use super::required_behavior::BehaviorContractProjection;
use super::task_contract::TaskContract;
use super::verifier_diagnostic_attempt::VerifierDiagnosticAttemptSpec;
use crate::agent::orchestration::{RepoSnapshot, RepoVerification};
use crate::session::store::ConversationMessage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum JobInstallOutcome {
    /// Either a new job was installed, an existing job's identity-refresh
    /// was kept, or the hint role does not require a job. Caller may
    /// commit the projection.
    InstalledOrSkipped,
    /// A Test-role hint failed `ArtifactCompletionJob::new` validation.
    /// Caller MUST clear `current_artifact_recovery_target` as well so
    /// the projection cannot survive without a backing job (PR-001 SSOT
    /// invariant).
    ValidationFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum VerifierDiagnosticPassOutcome {
    Accepted,
    RetryPending { error: String },
    Unavailable { error: String },
    Skipped,
}

#[derive(Debug, Clone)]
pub(super) struct PreparedVerifierDiagnosticPass {
    pub(super) context: RepairJob,
    pub(super) attempt_spec: VerifierDiagnosticAttemptSpec,
    pub(super) active_request: String,
    pub(super) behavior_projection: Option<BehaviorContractProjection>,
}

#[derive(Debug, Clone)]
pub(super) struct PreparedVerifierRepairPass {
    pub(super) context: RepairJob,
    pub(super) accepted_plan: AcceptedRepairPlan,
    pub(super) messages: Vec<ConversationMessage>,
    pub(super) model: String,
}

pub(super) enum VerifierRepairAttemptProgress {
    Return(VerifierRepairPassOutcome),
    Continue {
        last_error: String,
        last_invalid_outcome: Option<RepairAttemptOutcome>,
    },
    Break {
        last_error: String,
    },
}

#[derive(Debug, Clone)]
pub(super) struct StructuredTaskContractVerifierRun {
    pub(super) plan: AutoTestPlan,
    pub(super) command: VerifierCommand,
    pub(super) display_command: String,
    pub(super) bound_test_artifacts_count: usize,
    pub(super) bound_test_artifacts_paths: Vec<String>,
}

pub(super) struct TaskContractVerifierFlowArgs<'a, 'b> {
    pub(super) before_snapshot: &'a RepoSnapshot,
    pub(super) accumulated: &'a [RepoVerification],
    pub(super) repo_edit_calls_made_this_turn: usize,
    pub(super) task_contract: Option<&'a TaskContract>,
    pub(super) contract_verification_retries: &'b mut usize,
    pub(super) contract_verifier_repair_edit_count: &'b mut Option<usize>,
    pub(super) repo_change_retries: &'b mut usize,
    pub(super) verifier_repair_retries: &'b mut usize,
    pub(super) task_contract_verify_commands_collected: &'b mut Vec<String>,
    pub(super) task_contract_verifier_passed_in_loop: &'b mut bool,
    pub(super) last_iter: usize,
}
