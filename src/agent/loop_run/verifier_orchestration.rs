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

use std::path::Path;

use super::auto_test::{AutoTestPlan, VerifierCommand};
use super::repair_attempt_outcome::RepairAttemptOutcome;
use super::repair_driver::{
    VERIFIER_REPAIR_PASS_MAX_EDITS, VERIFIER_REPAIR_PASS_MAX_OUTPUT_BYTES,
    VERIFIER_REPAIR_PASS_MAX_REASON_CHARS, VerifierRepairPassOutcome,
};
use super::repair_job::RepairJob;
use super::repair_patch_validation::VerifierRepairIntentLimits;
use super::repair_plan::AcceptedRepairPlan;
use super::required_behavior::BehaviorContractProjection;
use super::task_contract::TaskContract;
use super::turn::{
    TASK_CONTRACT_VERIFIER_ATTEMPT_LIMIT, TASK_CONTRACT_VERIFIER_REPAIR_ATTEMPT_LIMIT,
    missing_verifier_setup_hint_for_request,
};
use super::verifier_diagnostic_attempt::VerifierDiagnosticAttemptSpec;
use crate::agent::orchestration::{RepoSnapshot, RepoVerification};
use crate::session::store::ConversationMessage;

#[cfg(test)]
use super::repair_job::VerifierRepairDecision;
#[cfg(test)]
use super::repair_patch_validation::{
    RepairIntentEdit, VerifierRepairIntent, repair_intent_edits_fingerprint,
};
#[cfg(test)]
use super::task_contract::RecoveryTargetHint;
#[cfg(test)]
use super::tool_policy::workspace_relative_path_for_tool_arg;
#[cfg(test)]
use super::turn::decision_target_path;

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

pub(super) fn verifier_repair_pass_request_error_message(
    err: &str,
    attempt_timeout_secs: u64,
) -> String {
    let lower_error = err.to_ascii_lowercase();
    if lower_error.contains("timeout") || lower_error.contains("timed out") {
        format!(
            "verifier_repair_pass_timeout: patch provider request timed out after \
             {attempt_timeout_secs}s"
        )
    } else {
        format!("repair LLM request failed: {err}")
    }
}

#[cfg(test)]
pub(super) fn task_contract_verifier_target_discovery_note(
    attempt: usize,
    attempt_limit: usize,
) -> String {
    format!(
        "[Task Contract Verification] The verifier already failed, but Anvil did not identify a safe workspace repair target yet. Do not rerun verification and do not answer in prose. Emit exactly one Read, Glob, or Grep tool call to identify the local file to repair. Do not use Bash, Write, or Edit until a target file is known. task_contract_verify_discovery_attempt={attempt}/{attempt_limit}"
    )
}

pub(super) fn verifier_repair_transition_message() -> String {
    "[Verifier Repair Policy] A verifier repair transition is pending. Do not answer in prose; wait for Anvil to drive the next verifier step.".to_string()
}

pub(super) fn verifier_repair_safe_stop_message() -> String {
    "[Verifier Repair Policy] Verifier repair cannot continue safely. Do not answer in prose; Anvil will stop this repair job with an explicit verifier failure."
        .to_string()
}

pub(super) fn verifier_repair_unsafe_target_message() -> String {
    "[Verifier Repair Policy] Verifier repair target is unsafe or unavailable. Do not answer in prose; Anvil will stop this repair job with an explicit verifier failure."
        .to_string()
}

pub(super) fn verifier_repair_target_display(target: &Path, work_root: &Path) -> String {
    target
        .strip_prefix(work_root)
        .unwrap_or(target)
        .to_string_lossy()
        .replace('\\', "/")
}

pub(super) fn task_contract_verifier_failure_attempt_limit(
    previous_context: Option<&RepairJob>,
) -> usize {
    if previous_context.is_some() {
        TASK_CONTRACT_VERIFIER_REPAIR_ATTEMPT_LIMIT
    } else {
        TASK_CONTRACT_VERIFIER_ATTEMPT_LIMIT
    }
}

pub(super) fn verifier_framework_signal_for_context(context: &RepairJob) -> String {
    format!(
        "{}\n{}\n{}",
        context.failure_signature,
        context.output_excerpt,
        context.repair_error.as_deref().unwrap_or("")
    )
}

pub(super) fn verifier_setup_policy_message(active_request: &str) -> String {
    let hint = missing_verifier_setup_hint_for_request(active_request)
        .map(|hint| format!(" {hint}"))
        .unwrap_or_default();
    format!(
        "[Verifier Setup Policy] A runnable verifier is required but missing. Emit exactly one Write or Edit for project-local verifier metadata now.{hint} Do not call Bash, do not switch tasks, and do not answer in prose."
    )
}

#[cfg(test)]
pub(super) fn verifier_repair_invalid_can_continue(
    decision: &VerifierRepairDecision,
    attempted_target_hint: Option<&RecoveryTargetHint>,
    work_root: &Path,
) -> bool {
    match decision {
        VerifierRepairDecision::NeedDiagnostic | VerifierRepairDecision::NeedTargetDiscovery => {
            true
        }
        VerifierRepairDecision::NeedFreshRead(_)
        | VerifierRepairDecision::NeedWrite(_)
        | VerifierRepairDecision::NeedEdit(_) => attempted_target_hint
            .is_none_or(|hint| !verifier_repair_decision_targets_hint(decision, hint, work_root)),
        VerifierRepairDecision::ReadyToVerify
        | VerifierRepairDecision::DiagnosticUnavailable
        | VerifierRepairDecision::NoRepair => false,
    }
}

#[cfg(test)]
pub(super) fn verifier_repair_decision_targets_hint(
    decision: &VerifierRepairDecision,
    hint: &RecoveryTargetHint,
    work_root: &Path,
) -> bool {
    let Some(decision_target) = decision_target_path(decision) else {
        return false;
    };
    let decision_rel =
        workspace_relative_path_for_tool_arg(work_root, &decision_target.to_string_lossy());
    let hint_rel = workspace_relative_path_for_tool_arg(work_root, &hint.path);
    matches!((decision_rel, hint_rel), (Some(a), Some(b)) if a == b)
}

pub(super) fn verifier_repair_intent_limits() -> VerifierRepairIntentLimits {
    VerifierRepairIntentLimits {
        max_output_bytes: VERIFIER_REPAIR_PASS_MAX_OUTPUT_BYTES,
        max_edits: VERIFIER_REPAIR_PASS_MAX_EDITS,
        max_reason_chars: VERIFIER_REPAIR_PASS_MAX_REASON_CHARS,
    }
}

#[cfg(test)]
pub(super) fn repair_intent_edit_payloads(
    intents: &[VerifierRepairIntent],
) -> Vec<RepairIntentEdit<'_>> {
    intents
        .iter()
        .map(|intent| RepairIntentEdit {
            old_string: &intent.old_string,
            new_string: &intent.new_string,
            replace_all: intent.replace_all,
        })
        .collect()
}

#[cfg(test)]
pub(super) fn verifier_repair_intent_fingerprint(
    context: &RepairJob,
    relative_path: &str,
    intent: &VerifierRepairIntent,
) -> String {
    verifier_repair_intents_fingerprint(context, relative_path, std::slice::from_ref(intent))
}

#[cfg(test)]
pub(super) fn verifier_repair_intents_fingerprint(
    context: &RepairJob,
    relative_path: &str,
    intents: &[VerifierRepairIntent],
) -> String {
    repair_intent_edits_fingerprint(
        &context.failure_signature,
        relative_path,
        &repair_intent_edit_payloads(intents),
    )
}
