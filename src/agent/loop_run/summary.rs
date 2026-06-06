use super::active_job_arbiter::RecoveryJobKind;
use super::repair_job::{RepairNextAction, VerifierBootstrapNextAction};
use super::task_contract::{ArtifactRecoveryAction, ArtifactRole};

#[allow(dead_code)] // Issue #888: additive generic state vocabulary for future serialized projections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RunState {
    DeliverableMissing,
    EvidenceMissing,
    EvidenceInvalid,
    CorrectionPlanning,
    CorrectionApplying,
    CompletionReady,
    Completed,
    SafeStopped,
}

impl RunState {
    #[allow(dead_code)] // Issue #888 migration surface; existing wire labels stay on ExitReason.
    pub(super) fn label(self) -> &'static str {
        match self {
            RunState::DeliverableMissing => "deliverable_missing",
            RunState::EvidenceMissing => "evidence_missing",
            RunState::EvidenceInvalid => "evidence_invalid",
            RunState::CorrectionPlanning => "correction_planning",
            RunState::CorrectionApplying => "correction_applying",
            RunState::CompletionReady => "completion_ready",
            RunState::Completed => "completed",
            RunState::SafeStopped => "safe_stopped",
        }
    }

    #[allow(dead_code)] // Issue #888: mapping documentation for ArtifactRecoveryAction.
    pub(super) fn from_artifact_recovery_action(action: &ArtifactRecoveryAction) -> Self {
        match action {
            ArtifactRecoveryAction::Continue { .. } => RunState::DeliverableMissing,
            ArtifactRecoveryAction::RunVerifier => RunState::CompletionReady,
            ArtifactRecoveryAction::RepairArtifact { .. } => RunState::CorrectionApplying,
            ArtifactRecoveryAction::Done => RunState::Completed,
            ArtifactRecoveryAction::SafeStop { .. } => RunState::SafeStopped,
        }
    }

    #[allow(dead_code)] // Issue #888: mapping documentation for RepairNextAction.
    pub(super) fn from_repair_next_action(action: &RepairNextAction) -> Self {
        match action {
            RepairNextAction::RequestDiagnostic | RepairNextAction::Replan => {
                RunState::CorrectionPlanning
            }
            RepairNextAction::RequestPatch { .. } => RunState::CorrectionApplying,
            RepairNextAction::RerunVerifier => RunState::CompletionReady,
            RepairNextAction::SafeStop { .. } => RunState::SafeStopped,
            RepairNextAction::VerifiedDone => RunState::Completed,
        }
    }

    #[allow(dead_code)] // Issue #888: mapping documentation for MissingVerifierJob.
    pub(super) fn from_verifier_bootstrap_next_action(
        action: &VerifierBootstrapNextAction,
    ) -> Self {
        match action {
            VerifierBootstrapNextAction::RequestSetupEdit => RunState::DeliverableMissing,
            VerifierBootstrapNextAction::RerunVerifier => RunState::CompletionReady,
            VerifierBootstrapNextAction::SafeStop { .. } => RunState::SafeStopped,
        }
    }
}

#[allow(dead_code)] // Issue #947: generic terminal vocabulary before every caller migrates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GenericTerminalState {
    Completed,
    MissingDeliverable,
    MissingEvidence,
    EvidenceFailed,
    EvidenceBindingFailed,
    EvidenceRunnerMissing,
    EvidenceRepairExhausted,
    EvidenceRepairSafeStop,
    ControlLoopExhausted,
    ModelOutputFailure,
    TransportFailure,
    Interrupted,
}

impl GenericTerminalState {
    pub(super) fn label(self) -> &'static str {
        match self {
            GenericTerminalState::Completed => "completed",
            GenericTerminalState::MissingDeliverable => "missing_deliverable",
            GenericTerminalState::MissingEvidence => "missing_evidence",
            GenericTerminalState::EvidenceFailed => "evidence_failed",
            GenericTerminalState::EvidenceBindingFailed => "evidence_binding_failed",
            GenericTerminalState::EvidenceRunnerMissing => "evidence_runner_missing",
            GenericTerminalState::EvidenceRepairExhausted => "evidence_repair_exhausted",
            GenericTerminalState::EvidenceRepairSafeStop => "evidence_repair_safe_stop",
            GenericTerminalState::ControlLoopExhausted => "control_loop_exhausted",
            GenericTerminalState::ModelOutputFailure => "model_output_failure",
            GenericTerminalState::TransportFailure => "transport_failure",
            GenericTerminalState::Interrupted => "interrupted",
        }
    }
}

#[allow(dead_code)] // Issue #888: additive context; legacy labels remain the serialized default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MissingDeliverable {
    RepositoryChange,
    ArtifactRole(ArtifactRole),
}

impl MissingDeliverable {
    #[allow(dead_code)] // Useful for future telemetry serialization.
    pub(super) fn label(self) -> &'static str {
        match self {
            MissingDeliverable::RepositoryChange => "repository_change",
            MissingDeliverable::ArtifactRole(role) => role.label(),
        }
    }
}

#[allow(dead_code)] // Issue #888: additive context; legacy labels remain the serialized default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MissingEvidence {
    VerificationResult,
    VerificationEnvironment,
    ValidVerificationResult,
}

impl MissingEvidence {
    #[allow(dead_code)] // Useful for future telemetry serialization.
    pub(super) fn label(self) -> &'static str {
        match self {
            MissingEvidence::VerificationResult => "verification_result",
            MissingEvidence::VerificationEnvironment => "verification_environment",
            MissingEvidence::ValidVerificationResult => "valid_verification_result",
        }
    }
}

const NO_MISSING_DELIVERABLES: &[MissingDeliverable] = &[];
const NO_MISSING_EVIDENCE: &[MissingEvidence] = &[];
const REPOSITORY_CHANGE_MISSING: &[MissingDeliverable] = &[MissingDeliverable::RepositoryChange];
const VERIFICATION_RESULT_MISSING: &[MissingEvidence] = &[MissingEvidence::VerificationResult];
const VERIFICATION_ENVIRONMENT_MISSING: &[MissingEvidence] =
    &[MissingEvidence::VerificationEnvironment];
const VALID_VERIFICATION_RESULT_MISSING: &[MissingEvidence] =
    &[MissingEvidence::ValidVerificationResult];

#[allow(dead_code)] // Issue #888: terminal metadata projection for future eval-log migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RunTerminalOutcome {
    pub(super) state: RunState,
    pub(super) generic_state: GenericTerminalState,
    pub(super) legacy_label: &'static str,
    pub(super) missing_deliverables: &'static [MissingDeliverable],
    pub(super) missing_evidence: &'static [MissingEvidence],
}

impl RunTerminalOutcome {
    pub(super) fn from_exit_reason(reason: ExitReason) -> Self {
        match reason {
            ExitReason::Done => Self {
                state: RunState::Completed,
                generic_state: GenericTerminalState::Completed,
                legacy_label: "done",
                missing_deliverables: NO_MISSING_DELIVERABLES,
                missing_evidence: NO_MISSING_EVIDENCE,
            },
            ExitReason::MissingRepoEdits => Self {
                state: RunState::DeliverableMissing,
                generic_state: GenericTerminalState::MissingDeliverable,
                legacy_label: "missing_repo_edits",
                missing_deliverables: REPOSITORY_CHANGE_MISSING,
                missing_evidence: NO_MISSING_EVIDENCE,
            },
            ExitReason::MissingVerification => Self {
                state: RunState::EvidenceMissing,
                generic_state: GenericTerminalState::MissingEvidence,
                legacy_label: "missing_verification",
                missing_deliverables: NO_MISSING_DELIVERABLES,
                missing_evidence: VERIFICATION_RESULT_MISSING,
            },
            ExitReason::VerifierFailed => Self {
                state: RunState::EvidenceInvalid,
                generic_state: GenericTerminalState::EvidenceFailed,
                legacy_label: reason.legacy_label(),
                missing_deliverables: NO_MISSING_DELIVERABLES,
                missing_evidence: VALID_VERIFICATION_RESULT_MISSING,
            },
            ExitReason::SafeStopVerifierWeak => Self {
                state: RunState::EvidenceInvalid,
                generic_state: GenericTerminalState::EvidenceBindingFailed,
                legacy_label: "safe_stop_verifier_weak",
                missing_deliverables: NO_MISSING_DELIVERABLES,
                missing_evidence: VALID_VERIFICATION_RESULT_MISSING,
            },
            ExitReason::SafeStopVerifierMissing => Self {
                state: RunState::EvidenceMissing,
                generic_state: GenericTerminalState::EvidenceRunnerMissing,
                legacy_label: "safe_stop_verifier_missing",
                missing_deliverables: NO_MISSING_DELIVERABLES,
                missing_evidence: VERIFICATION_ENVIRONMENT_MISSING,
            },
            ExitReason::RepairExhausted => Self {
                state: RunState::SafeStopped,
                generic_state: GenericTerminalState::EvidenceRepairExhausted,
                legacy_label: "repair_exhausted",
                missing_deliverables: NO_MISSING_DELIVERABLES,
                missing_evidence: NO_MISSING_EVIDENCE,
            },
            ExitReason::RepairSafeStop => Self {
                state: RunState::SafeStopped,
                generic_state: GenericTerminalState::EvidenceRepairSafeStop,
                legacy_label: "repair_safe_stop",
                missing_deliverables: NO_MISSING_DELIVERABLES,
                missing_evidence: NO_MISSING_EVIDENCE,
            },
            ExitReason::MaxIterations | ExitReason::PlanIncomplete => Self {
                state: RunState::SafeStopped,
                generic_state: GenericTerminalState::ControlLoopExhausted,
                legacy_label: reason.legacy_label(),
                missing_deliverables: NO_MISSING_DELIVERABLES,
                missing_evidence: NO_MISSING_EVIDENCE,
            },
            ExitReason::EmptyResponses
            | ExitReason::NoToolCalls
            | ExitReason::ToolCallFormatError => Self {
                state: RunState::SafeStopped,
                generic_state: GenericTerminalState::ModelOutputFailure,
                legacy_label: reason.legacy_label(),
                missing_deliverables: NO_MISSING_DELIVERABLES,
                missing_evidence: NO_MISSING_EVIDENCE,
            },
            ExitReason::TransportError => Self {
                state: RunState::SafeStopped,
                generic_state: GenericTerminalState::TransportFailure,
                legacy_label: "transport_error",
                missing_deliverables: NO_MISSING_DELIVERABLES,
                missing_evidence: NO_MISSING_EVIDENCE,
            },
            ExitReason::Interrupted => Self {
                state: RunState::SafeStopped,
                generic_state: GenericTerminalState::Interrupted,
                legacy_label: "interrupted",
                missing_deliverables: NO_MISSING_DELIVERABLES,
                missing_evidence: NO_MISSING_EVIDENCE,
            },
        }
    }

    #[allow(dead_code)] // Issue #947: compatibility projection for eval/report migrations.
    pub(super) fn generic_label(self) -> &'static str {
        self.generic_state.label()
    }

    #[allow(dead_code)] // Issue #947: keeps legacy evaluation labels explicit.
    pub(super) fn legacy_label_for_eval(self) -> &'static str {
        self.legacy_label
    }

    #[allow(dead_code)] // Issue #948: generic recovery jobs before every caller migrates.
    pub(super) fn recovery_job_kind(self) -> Option<RecoveryJobKind> {
        match self.generic_state {
            GenericTerminalState::Completed
            | GenericTerminalState::ControlLoopExhausted
            | GenericTerminalState::Interrupted => None,
            GenericTerminalState::MissingDeliverable => {
                Some(RecoveryJobKind::MissingDeliverableJob)
            }
            GenericTerminalState::MissingEvidence => Some(RecoveryJobKind::MissingEvidenceJob),
            // Issue #993 (parent #988, Issue E): a deliverable exists but its
            // evidence runner cannot be bound. Distinct recovery job from a
            // runner that bound and failed (`EvidenceFailed*`).
            GenericTerminalState::EvidenceBindingFailed => {
                Some(RecoveryJobKind::EvidenceBindingFailedJob)
            }
            GenericTerminalState::EvidenceFailed
            | GenericTerminalState::EvidenceRepairExhausted
            | GenericTerminalState::EvidenceRepairSafeStop => {
                Some(RecoveryJobKind::EvidenceFailedJob)
            }
            GenericTerminalState::EvidenceRunnerMissing
            | GenericTerminalState::ModelOutputFailure
            | GenericTerminalState::TransportFailure => Some(RecoveryJobKind::ToolFailureJob),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExitReason {
    Done,
    MaxIterations,
    EmptyResponses,
    NoToolCalls,
    MissingRepoEdits,
    MissingVerification,
    VerifierFailed,
    PlanIncomplete,
    ToolCallFormatError,
    TransportError,
    Interrupted,
    /// Issue #651 Task 4.2: `CompletionDecision::SafeStop` with the
    /// `VerifierWeak` reason — a structurally runnable verifier was
    /// detected, but the current task's owned test artifact could not
    /// be bound to its argv. The agent stops without claiming Done so
    /// we never report false-positive completion. Distinct from
    /// `VerifierFailed` (verifier ran and the suite failed) and from
    /// `MissingVerification` (no verifier evidence was recorded yet).
    SafeStopVerifierWeak,
    /// Issue #651 Task 4.2: `CompletionDecision::SafeStop` with the
    /// `VerifierMissing` reason — either no allowlisted runner was
    /// detected at all, or the request literally asked for tests but
    /// no owned test artifact is staged on the verifier command line.
    SafeStopVerifierMissing,
    /// Verifier repair reached a controlled budget/exhaustion terminal.
    /// This is distinct from a raw verifier failure: the repair controller
    /// has already emitted a bounded safe-stop report with the last failure
    /// packet and exhausted repair evidence.
    RepairExhausted,
    /// Verifier repair reached a controlled safe-stop terminal that is not
    /// specifically a budget exhaustion. This is still distinct from raw
    /// verifier failure because the repair controller selected the terminal
    /// state and emitted actionable diagnostics.
    RepairSafeStop,
}

impl ExitReason {
    pub(super) fn is_success(self) -> bool {
        matches!(self, ExitReason::Done)
    }

    pub(super) fn keeps_repl_alive(self) -> bool {
        matches!(
            self,
            ExitReason::MaxIterations
                | ExitReason::EmptyResponses
                | ExitReason::NoToolCalls
                | ExitReason::MissingRepoEdits
                | ExitReason::MissingVerification
                | ExitReason::VerifierFailed
                | ExitReason::PlanIncomplete
                | ExitReason::ToolCallFormatError
                | ExitReason::Interrupted
                | ExitReason::SafeStopVerifierWeak
                | ExitReason::SafeStopVerifierMissing
                | ExitReason::RepairExhausted
                | ExitReason::RepairSafeStop
        )
    }

    pub(super) fn label(self) -> &'static str {
        RunTerminalOutcome::from_exit_reason(self).legacy_label
    }

    #[allow(dead_code)] // Issue #947: generic internal terminal vocabulary.
    pub(super) fn generic_terminal_state(self) -> GenericTerminalState {
        RunTerminalOutcome::from_exit_reason(self).generic_state
    }

    #[allow(dead_code)] // Issue #947: generic internal terminal vocabulary.
    pub(super) fn generic_label(self) -> &'static str {
        self.generic_terminal_state().label()
    }

    #[allow(dead_code)] // Issue #948: generic recovery jobs before every caller migrates.
    pub(super) fn recovery_job_kind(self) -> Option<RecoveryJobKind> {
        RunTerminalOutcome::from_exit_reason(self).recovery_job_kind()
    }

    fn legacy_label(self) -> &'static str {
        match self {
            ExitReason::Done => "done",
            ExitReason::MaxIterations => "max_iterations",
            ExitReason::EmptyResponses => "empty_responses",
            ExitReason::NoToolCalls => "no_tool_calls",
            ExitReason::MissingRepoEdits => "missing_repo_edits",
            ExitReason::MissingVerification => "missing_verification",
            ExitReason::VerifierFailed => "verifier_failed",
            ExitReason::PlanIncomplete => "plan_incomplete",
            ExitReason::ToolCallFormatError => "tool_call_format_error",
            ExitReason::TransportError => "transport_error",
            ExitReason::Interrupted => "interrupted",
            ExitReason::SafeStopVerifierWeak => "safe_stop_verifier_weak",
            ExitReason::SafeStopVerifierMissing => "safe_stop_verifier_missing",
            ExitReason::RepairExhausted => "repair_exhausted",
            ExitReason::RepairSafeStop => "repair_safe_stop",
        }
    }

    pub(super) fn default_error_text(self) -> &'static str {
        match self {
            ExitReason::Done => "",
            ExitReason::MaxIterations => "assistant did not finish within max iterations",
            ExitReason::EmptyResponses => "assistant returned empty responses repeatedly",
            ExitReason::NoToolCalls => {
                "assistant kept describing actions without using tools to perform them"
            }
            ExitReason::MissingRepoEdits => {
                "assistant kept stopping before making the requested repository edits"
            }
            ExitReason::MissingVerification => {
                "assistant completed repository artifacts but did not obtain required verification"
            }
            ExitReason::VerifierFailed => "required verifier failed after repository edits",
            ExitReason::PlanIncomplete => {
                "assistant did not finish the plan after repeated planning retries"
            }
            ExitReason::ToolCallFormatError => {
                "assistant emitted malformed or truncated tool calls repeatedly"
            }
            ExitReason::TransportError => "transport error: request failed after retries",
            ExitReason::Interrupted => "",
            ExitReason::SafeStopVerifierWeak => {
                "assistant stopped: structured verifier could not bind the task's owned test artifact"
            }
            ExitReason::SafeStopVerifierMissing => {
                "assistant stopped: request asks for test execution but no owned test artifact reached the verifier"
            }
            ExitReason::RepairExhausted => {
                "assistant stopped: verifier repair budget exhausted with actionable diagnostics"
            }
            ExitReason::RepairSafeStop => {
                "assistant stopped: verifier repair reached a controlled safe stop with actionable diagnostics"
            }
        }
    }
}

pub(super) struct LoopStats {
    pub iter_used: usize,
    pub iter_max: usize,
    pub duration_secs: u64,
    /// Optional generic lifecycle label for display surfaces. `ExitReason`
    /// remains the legacy compatibility label.
    pub terminal_outcome_label: Option<&'static str>,
    /// Display-capped changed files for summaries.
    pub changed_files: Box<[String]>,
    /// Complete changed file list for protocol-level evidence.
    pub all_changed_files: Box<[String]>,
    pub total_changed: usize,
    /// Issue #471: classification counts from `build_stats`. NOT derived from
    /// `changed_files` (which is truncated to 16) — these come from the full
    /// `RepoVerification` accumulator (DR3-001).
    pub changed_impl_count: usize,
    pub changed_test_count: usize,
    pub changed_setup_count: usize,
}

/// Ok((prose, stats)) on success; Err((reason, error_text, stats)) on failure.
/// Stats are included in both arms so the caller can always emit a summary line.
pub(super) type LoopResult = Result<(String, LoopStats), (ExitReason, String, LoopStats)>;

fn sanitize_filename(name: &str) -> String {
    const MAX_LEN: usize = 120;
    let sanitized: String = name
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect();
    if sanitized.chars().count() > MAX_LEN {
        let truncated: String = sanitized.chars().take(MAX_LEN).collect();
        format!("{truncated}...")
    } else {
        sanitized
    }
}

pub(super) fn format_run_summary(reason: ExitReason, stats: &LoopStats) -> String {
    let mark = if reason.is_success() { "✔" } else { "✘" };
    let label = stats
        .terminal_outcome_label
        .unwrap_or_else(|| reason.label());
    let iter = format!("iter {}/{}", stats.iter_used, stats.iter_max);
    let duration = format!("duration {}s", stats.duration_secs);

    let total = stats.total_changed;
    let file_part = if total == 0 {
        "edited 0 files".to_string()
    } else {
        let shown: Vec<String> = stats
            .changed_files
            .iter()
            .take(3)
            .map(|f| sanitize_filename(f))
            .collect();
        let names = shown.join(", ");
        if total > shown.len() {
            let extra = total - shown.len();
            format!("edited {total} files ({names}, +{extra} more)")
        } else {
            format!("edited {total} files ({names})")
        }
    };

    format!("{mark} {label}  {iter}  {duration}  {file_part}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(
        iter_used: usize,
        iter_max: usize,
        duration_secs: u64,
        files: Vec<&str>,
        total_changed: usize,
    ) -> LoopStats {
        LoopStats {
            iter_used,
            iter_max,
            duration_secs,
            terminal_outcome_label: None,
            changed_files: files
                .iter()
                .map(|file| (*file).to_string())
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            all_changed_files: files
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            total_changed,
            changed_impl_count: 0,
            changed_test_count: 0,
            changed_setup_count: 0,
        }
    }

    #[test]
    fn done_shows_checkmark_and_fields() {
        let s = stats(
            18,
            40,
            324,
            vec!["page.tsx", "lib/game.ts", "lib/entities.ts"],
            3,
        );
        let out = format_run_summary(ExitReason::Done, &s);
        assert!(out.starts_with("✔ done"), "got: {out}");
        assert!(out.contains("iter 18/40"), "got: {out}");
        assert!(out.contains("duration 324s"), "got: {out}");
        assert!(out.contains("edited 3 files"), "got: {out}");
    }

    #[test]
    fn max_iterations_shows_cross() {
        let s = stats(40, 40, 680, vec!["lib/game.ts", "tests/game.test.ts"], 2);
        let out = format_run_summary(ExitReason::MaxIterations, &s);
        assert!(out.starts_with("✘ max_iterations"), "got: {out}");
        assert!(out.contains("iter 40/40"), "got: {out}");
    }

    #[test]
    fn generic_terminal_label_overrides_legacy_summary_label() {
        let mut s = stats(5, 5, 12, vec!["output.csv"], 1);
        s.terminal_outcome_label = Some("evidence_repair_exhausted");
        let out = format_run_summary(ExitReason::MissingRepoEdits, &s);
        assert!(out.starts_with("✘ evidence_repair_exhausted"), "got: {out}");
        assert!(!out.starts_with("✘ missing_repo_edits"), "got: {out}");
    }

    #[test]
    fn zero_files_shows_no_parentheses() {
        let s = stats(5, 40, 10, vec![], 0);
        let out = format_run_summary(ExitReason::Done, &s);
        assert!(out.contains("edited 0 files"), "got: {out}");
        assert!(!out.contains('('), "got: {out}");
    }

    #[test]
    fn more_than_three_files_shows_plus_extra() {
        // changed_files has 3 entries but total_changed is 5
        let s = stats(10, 40, 50, vec!["a.rs", "b.rs", "c.rs"], 5);
        let out = format_run_summary(ExitReason::Done, &s);
        assert!(out.contains("+2 more"), "got: {out}");
        assert!(out.contains("edited 5 files"), "got: {out}");
    }

    #[test]
    fn control_chars_in_filename_are_sanitized() {
        let s = stats(1, 10, 5, vec!["bad\nfile.rs", "ok.rs"], 2);
        let out = format_run_summary(ExitReason::Done, &s);
        assert!(!out.contains('\n'), "got: {out}");
        assert!(out.contains('?'), "got: {out}");
    }

    #[test]
    fn all_exit_reason_labels_are_distinct() {
        let reasons = [
            ExitReason::Done,
            ExitReason::MaxIterations,
            ExitReason::EmptyResponses,
            ExitReason::NoToolCalls,
            ExitReason::MissingRepoEdits,
            ExitReason::MissingVerification,
            ExitReason::VerifierFailed,
            ExitReason::PlanIncomplete,
            ExitReason::ToolCallFormatError,
            ExitReason::TransportError,
            ExitReason::Interrupted,
            ExitReason::SafeStopVerifierWeak,
            ExitReason::SafeStopVerifierMissing,
            ExitReason::RepairExhausted,
            ExitReason::RepairSafeStop,
        ];
        let labels: Vec<_> = reasons.iter().map(|r| r.label()).collect();
        let unique: std::collections::HashSet<_> = labels.iter().collect();
        assert_eq!(labels.len(), unique.len());
    }

    #[test]
    fn run_terminal_outcome_describes_legacy_missing_states_generically() {
        let deliverable = RunTerminalOutcome::from_exit_reason(ExitReason::MissingRepoEdits);
        assert_eq!(deliverable.legacy_label, "missing_repo_edits");
        assert_eq!(deliverable.generic_label(), "missing_deliverable");
        assert_eq!(deliverable.state, RunState::DeliverableMissing);
        assert_eq!(
            deliverable
                .missing_deliverables
                .iter()
                .map(|item| item.label())
                .collect::<Vec<_>>(),
            vec!["repository_change"]
        );
        assert!(deliverable.missing_evidence.is_empty());

        let evidence = RunTerminalOutcome::from_exit_reason(ExitReason::MissingVerification);
        assert_eq!(evidence.legacy_label, "missing_verification");
        assert_eq!(evidence.generic_label(), "missing_evidence");
        assert_eq!(evidence.state, RunState::EvidenceMissing);
        assert!(evidence.missing_deliverables.is_empty());
        assert_eq!(
            evidence
                .missing_evidence
                .iter()
                .map(|item| item.label())
                .collect::<Vec<_>>(),
            vec!["verification_result"]
        );
    }

    #[test]
    fn terminal_outcome_projects_generic_states_and_legacy_eval_labels() {
        let cases = [
            (
                ExitReason::MissingRepoEdits,
                GenericTerminalState::MissingDeliverable,
                "missing_deliverable",
                "missing_repo_edits",
            ),
            (
                ExitReason::MissingVerification,
                GenericTerminalState::MissingEvidence,
                "missing_evidence",
                "missing_verification",
            ),
            (
                ExitReason::SafeStopVerifierMissing,
                GenericTerminalState::EvidenceRunnerMissing,
                "evidence_runner_missing",
                "safe_stop_verifier_missing",
            ),
            (
                ExitReason::SafeStopVerifierWeak,
                GenericTerminalState::EvidenceBindingFailed,
                "evidence_binding_failed",
                "safe_stop_verifier_weak",
            ),
            (
                ExitReason::RepairExhausted,
                GenericTerminalState::EvidenceRepairExhausted,
                "evidence_repair_exhausted",
                "repair_exhausted",
            ),
            (
                ExitReason::RepairSafeStop,
                GenericTerminalState::EvidenceRepairSafeStop,
                "evidence_repair_safe_stop",
                "repair_safe_stop",
            ),
        ];

        for (reason, generic_state, generic_label, legacy_label) in cases {
            let outcome = RunTerminalOutcome::from_exit_reason(reason);
            assert_eq!(outcome.generic_state, generic_state, "reason={reason:?}");
            assert_eq!(outcome.generic_label(), generic_label, "reason={reason:?}");
            assert_eq!(
                outcome.legacy_label_for_eval(),
                legacy_label,
                "reason={reason:?}"
            );
            assert_eq!(reason.generic_terminal_state(), generic_state);
            assert_eq!(reason.generic_label(), generic_label);
            assert_eq!(reason.label(), legacy_label);
        }
    }

    #[test]
    fn terminal_outcome_projects_current_failures_to_recovery_jobs() {
        let cases = [
            (
                ExitReason::MissingRepoEdits,
                Some(RecoveryJobKind::MissingDeliverableJob),
                "missing_repo_edits",
            ),
            (
                ExitReason::MissingVerification,
                Some(RecoveryJobKind::MissingEvidenceJob),
                "missing_verification",
            ),
            (
                ExitReason::VerifierFailed,
                Some(RecoveryJobKind::EvidenceFailedJob),
                "verifier_failed",
            ),
            (
                ExitReason::SafeStopVerifierMissing,
                Some(RecoveryJobKind::ToolFailureJob),
                "safe_stop_verifier_missing",
            ),
            (
                ExitReason::RepairExhausted,
                Some(RecoveryJobKind::EvidenceFailedJob),
                "repair_exhausted",
            ),
            (
                ExitReason::RepairSafeStop,
                Some(RecoveryJobKind::EvidenceFailedJob),
                "repair_safe_stop",
            ),
            (
                ExitReason::ToolCallFormatError,
                Some(RecoveryJobKind::ToolFailureJob),
                "tool_call_format_error",
            ),
        ];

        for (reason, recovery_job_kind, legacy_label) in cases {
            let outcome = RunTerminalOutcome::from_exit_reason(reason);
            assert_eq!(outcome.recovery_job_kind(), recovery_job_kind);
            assert_eq!(reason.recovery_job_kind(), recovery_job_kind);
            assert_eq!(outcome.legacy_label_for_eval(), legacy_label);
            assert_eq!(reason.label(), legacy_label);
        }
    }

    #[test]
    fn binding_failure_routes_to_its_own_recovery_job() {
        // Issue #993 (parent #988, Issue E): the `EvidenceBindingFailed`
        // terminal (a deliverable exists but its runner cannot bind) projects
        // to the dedicated `EvidenceBindingFailedJob`, NOT the generic
        // `EvidenceFailedJob` used when a bound runner actually failed.
        let outcome = RunTerminalOutcome::from_exit_reason(ExitReason::SafeStopVerifierWeak);
        assert_eq!(
            outcome.generic_state,
            GenericTerminalState::EvidenceBindingFailed
        );
        assert_eq!(
            outcome.recovery_job_kind(),
            Some(RecoveryJobKind::EvidenceBindingFailedJob)
        );
        assert_ne!(
            outcome.recovery_job_kind(),
            Some(RecoveryJobKind::EvidenceFailedJob)
        );
        // A bound runner that failed still routes to `EvidenceFailedJob`.
        let failed = RunTerminalOutcome::from_exit_reason(ExitReason::VerifierFailed);
        assert_eq!(
            failed.recovery_job_kind(),
            Some(RecoveryJobKind::EvidenceFailedJob)
        );
    }

    #[test]
    fn tool_protocol_failure_is_not_confused_with_deliverable_or_evidence_failure() {
        // Issue #979 acceptance: a tool *protocol* failure must classify as a
        // ToolFailure / model_output_failure, never as a missing-deliverable or
        // evidence failure. The legacy `tool_call_format_error` eval label is
        // also preserved as the compatibility projection.
        let outcome = RunTerminalOutcome::from_exit_reason(ExitReason::ToolCallFormatError);
        assert_eq!(
            outcome.generic_state,
            GenericTerminalState::ModelOutputFailure
        );
        assert_eq!(
            outcome.recovery_job_kind(),
            Some(RecoveryJobKind::ToolFailureJob)
        );
        assert_ne!(
            outcome.recovery_job_kind(),
            Some(RecoveryJobKind::MissingDeliverableJob)
        );
        assert_ne!(
            outcome.recovery_job_kind(),
            Some(RecoveryJobKind::EvidenceFailedJob)
        );
        assert_eq!(outcome.legacy_label_for_eval(), "tool_call_format_error");
    }

    #[test]
    fn run_state_projects_existing_controller_actions() {
        let continue_action = ArtifactRecoveryAction::Continue {
            missing: vec![ArtifactRole::UsageDocs],
            target_hint: None,
        };
        assert_eq!(
            RunState::from_artifact_recovery_action(&continue_action),
            RunState::DeliverableMissing
        );
        assert_eq!(
            RunState::from_artifact_recovery_action(&ArtifactRecoveryAction::RunVerifier),
            RunState::CompletionReady
        );

        let patch_action = RepairNextAction::RequestPatch {
            target_hint: super::super::task_contract::RecoveryTargetHint {
                role: ArtifactRole::Implementation,
                path: "src/lib.rs".to_string(),
                reason: "repair implementation".to_string(),
            },
        };
        assert_eq!(
            RunState::from_repair_next_action(&RepairNextAction::RequestDiagnostic),
            RunState::CorrectionPlanning
        );
        assert_eq!(
            RunState::from_repair_next_action(&patch_action),
            RunState::CorrectionApplying
        );
        assert_eq!(
            RunState::from_verifier_bootstrap_next_action(
                &VerifierBootstrapNextAction::RequestSetupEdit
            ),
            RunState::DeliverableMissing
        );
    }

    #[test]
    fn exit_reason_labels_remain_legacy_compatible() {
        assert_eq!(ExitReason::MissingRepoEdits.label(), "missing_repo_edits");
        assert_eq!(
            ExitReason::MissingVerification.label(),
            "missing_verification"
        );
        assert_eq!(ExitReason::Done.label(), "done");
    }

    #[test]
    fn legacy_terminal_labels_remain_projected_for_issue_975() {
        // Issue #975 requirement 5 / AC1: the coding-era terminal labels named in
        // the issue must survive as legacy projections even though the internal
        // terminal vocabulary is now generic. Each legacy label must reproduce
        // from its ExitReason AND carry a generic projection + recovery routing.
        let cases = [
            (ExitReason::MissingRepoEdits, "missing_repo_edits"),
            (ExitReason::MissingVerification, "missing_verification"),
            (
                ExitReason::SafeStopVerifierMissing,
                "safe_stop_verifier_missing",
            ),
            (ExitReason::RepairExhausted, "repair_exhausted"),
            (ExitReason::ToolCallFormatError, "tool_call_format_error"),
        ];
        for (reason, legacy_label) in cases {
            let outcome = RunTerminalOutcome::from_exit_reason(reason);
            assert_eq!(reason.label(), legacy_label, "reason={reason:?}");
            assert_eq!(
                outcome.legacy_label_for_eval(),
                legacy_label,
                "reason={reason:?}"
            );
            // The generic projection is distinct from the legacy label (the
            // internal vocabulary is generic, the wire label stays legacy).
            assert_ne!(
                outcome.generic_label(),
                legacy_label,
                "generic label should not equal legacy for reason={reason:?}"
            );
            // Every named legacy failure routes to a generic recovery job.
            assert!(
                reason.recovery_job_kind().is_some(),
                "reason={reason:?} must project to a recovery job"
            );
        }
    }

    #[test]
    fn success_only_for_done() {
        assert!(ExitReason::Done.is_success());
        assert!(!ExitReason::MaxIterations.is_success());
        assert!(!ExitReason::EmptyResponses.is_success());
        assert!(!ExitReason::NoToolCalls.is_success());
        assert!(!ExitReason::MissingRepoEdits.is_success());
        assert!(!ExitReason::MissingVerification.is_success());
        assert!(!ExitReason::VerifierFailed.is_success());
        assert!(!ExitReason::PlanIncomplete.is_success());
        assert!(!ExitReason::ToolCallFormatError.is_success());
        assert!(!ExitReason::TransportError.is_success());
        assert!(!ExitReason::Interrupted.is_success());
        assert!(!ExitReason::SafeStopVerifierWeak.is_success());
        assert!(!ExitReason::SafeStopVerifierMissing.is_success());
        assert!(!ExitReason::RepairExhausted.is_success());
        assert!(!ExitReason::RepairSafeStop.is_success());
    }

    #[test]
    fn safe_stop_variants_keep_repl_alive() {
        assert!(ExitReason::SafeStopVerifierWeak.keeps_repl_alive());
        assert!(ExitReason::SafeStopVerifierMissing.keeps_repl_alive());
        assert!(ExitReason::RepairExhausted.keeps_repl_alive());
        assert!(ExitReason::RepairSafeStop.keeps_repl_alive());
    }

    #[test]
    fn safe_stop_variants_have_distinct_default_error_text() {
        // Issue #651 Task 4.2: each reason must communicate the
        // structured-verifier failure mode to the user so the run
        // summary explains *why* the agent stopped without claiming
        // completion.
        let weak = ExitReason::SafeStopVerifierWeak.default_error_text();
        let missing = ExitReason::SafeStopVerifierMissing.default_error_text();
        let repair = ExitReason::RepairExhausted.default_error_text();
        let repair_safe_stop = ExitReason::RepairSafeStop.default_error_text();
        assert!(!weak.is_empty());
        assert!(!missing.is_empty());
        assert!(!repair.is_empty());
        assert!(!repair_safe_stop.is_empty());
        assert_ne!(weak, missing);
        assert_ne!(weak, repair);
        assert_ne!(weak, repair_safe_stop);
        assert_ne!(missing, repair);
        assert_ne!(missing, repair_safe_stop);
        assert_ne!(repair, repair_safe_stop);
    }

    #[test]
    fn interrupted_renders_cross_with_label() {
        let s = stats(3, 10, 15, vec!["a.rs"], 1);
        let out = format_run_summary(ExitReason::Interrupted, &s);
        assert!(out.starts_with("✘ interrupted"), "got: {out}");
        assert!(out.contains("iter 3/10"), "got: {out}");
    }

    #[test]
    fn interrupted_has_empty_default_error_text() {
        assert_eq!(ExitReason::Interrupted.default_error_text(), "");
    }

    #[test]
    fn soft_failures_keep_repl_alive() {
        assert!(ExitReason::MaxIterations.keeps_repl_alive());
        assert!(ExitReason::EmptyResponses.keeps_repl_alive());
        assert!(ExitReason::NoToolCalls.keeps_repl_alive());
        assert!(ExitReason::MissingRepoEdits.keeps_repl_alive());
        assert!(ExitReason::MissingVerification.keeps_repl_alive());
        assert!(ExitReason::VerifierFailed.keeps_repl_alive());
        assert!(ExitReason::PlanIncomplete.keeps_repl_alive());
        assert!(ExitReason::ToolCallFormatError.keeps_repl_alive());
        assert!(ExitReason::Interrupted.keeps_repl_alive());
        assert!(ExitReason::RepairExhausted.keeps_repl_alive());
        assert!(ExitReason::RepairSafeStop.keeps_repl_alive());
        assert!(!ExitReason::TransportError.keeps_repl_alive());
    }
}
