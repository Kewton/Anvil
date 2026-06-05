//! Issue #993 (parent #988, Issue E): generic `EvidenceBindingFailedJob`.
//!
//! v0.6.4 showed a recurring shape that is *not* a runtime-specific bug: a
//! task already produced its evidence deliverable (a Node test file, a docs
//! document, a data output file, research source notes) but the evidence
//! runner could not be *bound* to it — `package.json`/`scripts.test` was
//! missing, the content check had no target document, the schema check had no
//! output file, the citation check had no source notes. The old controller
//! rounded all of these into `missing_verification` (`missing_evidence`),
//! which schedules a *missing-evidence* recovery for evidence that, in fact,
//! already exists.
//!
//! This module models that failure once, generically: a deliverable exists but
//! its evidence runner cannot bind. The runtime difference is confined to the
//! [`BindingCheck`] enum and the [`BindingRecovery`] it selects — both *data*,
//! not control flow — so the recovery loop never grows a per-task-kind branch.
//! It mirrors `evidence_runner.rs`: a thin, pure seam that focused tests pin
//! before broad callers wire it. No provider layer, no new process path.
//!
//! `pub(super)` limited / no facade re-export (DR3-001).

#![allow(dead_code)] // Extension seam; focused tests pin the shape before wiring broad callers.

use super::active_job_arbiter::RecoveryJobKind;
use super::evidence_runner::EvidenceRunnerKind;
use super::summary::GenericTerminalState;
use super::task_contract::TaskKind;

/// Which deliverable → evidence-runner binding is being checked. One variant
/// per runtime family; the only runtime-specific decision is which
/// [`BindingRecovery`] re-establishes it (see [`BindingCheck::recovery`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BindingCheck {
    /// Coding / Node: a test deliverable exists but no test runner manifest
    /// (`package.json` + `scripts.test`, a `Cargo.toml` test target, ...)
    /// binds it.
    RunnerManifest,
    /// Docs: a document exists but the required content check cannot bind to a
    /// target document / section.
    DocumentSection,
    /// Data: an output exists but the schema check cannot bind to an output
    /// file.
    SchemaOutput,
    /// Research: notes exist but the citation check cannot bind to source
    /// notes.
    SourceCitation,
}

impl BindingCheck {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            BindingCheck::RunnerManifest => "runner_manifest",
            BindingCheck::DocumentSection => "document_section",
            BindingCheck::SchemaOutput => "schema_output",
            BindingCheck::SourceCitation => "source_citation",
        }
    }

    /// The recovery family that re-establishes this binding so the
    /// EvidenceRunner can rerun. Each binding check maps to exactly one
    /// recovery — this is the single runtime-specific data point in the
    /// generic lifecycle.
    pub(super) fn recovery(self) -> BindingRecovery {
        match self {
            BindingCheck::RunnerManifest => BindingRecovery::MaterializeRunnerManifest,
            BindingCheck::DocumentSection => BindingRecovery::RecoverDocumentSection,
            BindingCheck::SchemaOutput => BindingRecovery::RecoverSchemaOutput,
            BindingCheck::SourceCitation => BindingRecovery::RecoverSourceCitation,
        }
    }

    /// The binding check for a task kind, mirroring
    /// `evidence_runner::evidence_runner_for_task_kind`.
    ///
    /// Ops and Authoring observe command / content evidence directly; there is
    /// no separate deliverable → runner binding step that can fail, so they
    /// return `None`.
    pub(super) fn for_task_kind(task_kind: TaskKind) -> Option<Self> {
        match task_kind {
            TaskKind::Coding => Some(BindingCheck::RunnerManifest),
            TaskKind::Docs => Some(BindingCheck::DocumentSection),
            TaskKind::Data => Some(BindingCheck::SchemaOutput),
            TaskKind::Research => Some(BindingCheck::SourceCitation),
            TaskKind::Ops | TaskKind::Authoring => None,
        }
    }

    /// The binding check for an evidence-runner kind. Keeps the binding
    /// vocabulary aligned with `evidence_runner::EvidenceRunnerKind`.
    pub(super) fn for_evidence_runner_kind(kind: EvidenceRunnerKind) -> Option<Self> {
        match kind {
            EvidenceRunnerKind::CodingBuildTest => Some(BindingCheck::RunnerManifest),
            EvidenceRunnerKind::DocsContentCheck => Some(BindingCheck::DocumentSection),
            EvidenceRunnerKind::DataSchemaCheck => Some(BindingCheck::SchemaOutput),
            EvidenceRunnerKind::ResearchSourceFetch => Some(BindingCheck::SourceCitation),
            EvidenceRunnerKind::OpsCommandObservation
            | EvidenceRunnerKind::AuthoringContentCheck => None,
        }
    }
}

/// The recovery that re-binds a deliverable to its evidence runner. Every
/// variant implies the same next step once it completes: rerun the
/// EvidenceRunner against the now-bound evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BindingRecovery {
    /// Materialize / complete the runner manifest (e.g. the Node
    /// `node_runner_manifest` operator writes `package.json`).
    MaterializeRunnerManifest,
    /// Recover the target document / section the content check binds to.
    RecoverDocumentSection,
    /// Recover the schema / output file the schema check binds to.
    RecoverSchemaOutput,
    /// Recover the source notes / citation the citation check binds to.
    RecoverSourceCitation,
}

impl BindingRecovery {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            BindingRecovery::MaterializeRunnerManifest => "materialize_runner_manifest",
            BindingRecovery::RecoverDocumentSection => "recover_document_section",
            BindingRecovery::RecoverSchemaOutput => "recover_schema_output",
            BindingRecovery::RecoverSourceCitation => "recover_source_citation",
        }
    }
}

/// A generic binding failure: a deliverable exists but its evidence runner
/// cannot be bound. This is a binding-order failure, NOT a missing-evidence
/// terminal — the deliverable is present, only the runner binding is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct EvidenceBindingFailedJob {
    pub(super) check: BindingCheck,
    pub(super) recovery: BindingRecovery,
}

impl EvidenceBindingFailedJob {
    fn new(check: BindingCheck) -> Self {
        Self {
            check,
            recovery: check.recovery(),
        }
    }

    /// Project to the generic terminal vocabulary. Binding failures are a
    /// distinct terminal from `MissingEvidence` (the deliverable exists) and
    /// from `EvidenceFailed` (the runner never bound, so it never ran).
    pub(super) fn generic_terminal_state(self) -> GenericTerminalState {
        GenericTerminalState::EvidenceBindingFailed
    }

    /// Project to the generic recovery-job vocabulary.
    pub(super) fn recovery_job_kind(self) -> RecoveryJobKind {
        RecoveryJobKind::EvidenceBindingFailedJob
    }
}

/// Result of a binding check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BindingState {
    /// The evidence runner is bound (or there is no deliverable to bind yet);
    /// proceed with the normal EvidenceRunner lifecycle.
    Bound,
    /// A deliverable exists but its evidence runner cannot be bound. Recover
    /// the binding, then rerun the EvidenceRunner.
    Failed(EvidenceBindingFailedJob),
}

impl BindingState {
    pub(super) fn failed_job(self) -> Option<EvidenceBindingFailedJob> {
        match self {
            BindingState::Bound => None,
            BindingState::Failed(job) => Some(job),
        }
    }
}

/// The single, runtime-agnostic binding evaluation.
///
/// - `deliverable_present` — the evidence deliverable exists (a test file, a
///   target document, an output file, source notes).
/// - `runner_bindable` — the evidence runner can already bind to it.
///
/// When a deliverable exists but the runner cannot bind, this is a binding
/// failure routed to the [`BindingRecovery`] for `check`. Otherwise the
/// binding is satisfied for the purpose of this check (no deliverable yet, or
/// the runner already binds).
pub(super) fn evaluate_binding(
    check: BindingCheck,
    deliverable_present: bool,
    runner_bindable: bool,
) -> BindingState {
    if deliverable_present && !runner_bindable {
        BindingState::Failed(EvidenceBindingFailedJob::new(check))
    } else {
        BindingState::Bound
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_CHECKS: [BindingCheck; 4] = [
        BindingCheck::RunnerManifest,
        BindingCheck::DocumentSection,
        BindingCheck::SchemaOutput,
        BindingCheck::SourceCitation,
    ];

    #[test]
    fn deliverable_present_but_unbound_is_a_binding_failure() {
        for check in ALL_CHECKS {
            let state = evaluate_binding(check, true, false);
            let job = state
                .failed_job()
                .unwrap_or_else(|| panic!("{} should be a binding failure", check.as_str()));
            assert_eq!(job.check, check);
            assert_eq!(job.recovery, check.recovery());
            assert_eq!(
                job.generic_terminal_state(),
                GenericTerminalState::EvidenceBindingFailed
            );
            assert_eq!(
                job.recovery_job_kind(),
                RecoveryJobKind::EvidenceBindingFailedJob
            );
        }
    }

    #[test]
    fn bound_or_absent_deliverable_is_not_a_binding_failure() {
        for check in ALL_CHECKS {
            // Runner already binds.
            assert_eq!(evaluate_binding(check, true, true), BindingState::Bound);
            // No deliverable yet -> missing-evidence territory, not binding.
            assert_eq!(evaluate_binding(check, false, false), BindingState::Bound);
            assert_eq!(evaluate_binding(check, false, true), BindingState::Bound);
        }
    }

    #[test]
    fn binding_check_maps_each_runtime_to_a_single_recovery() {
        let cases = [
            (
                BindingCheck::RunnerManifest,
                BindingRecovery::MaterializeRunnerManifest,
            ),
            (
                BindingCheck::DocumentSection,
                BindingRecovery::RecoverDocumentSection,
            ),
            (
                BindingCheck::SchemaOutput,
                BindingRecovery::RecoverSchemaOutput,
            ),
            (
                BindingCheck::SourceCitation,
                BindingRecovery::RecoverSourceCitation,
            ),
        ];
        for (check, recovery) in cases {
            assert_eq!(check.recovery(), recovery);
        }
    }

    #[test]
    fn binding_check_for_task_kind_mirrors_evidence_runner_selection() {
        assert_eq!(
            BindingCheck::for_task_kind(TaskKind::Coding),
            Some(BindingCheck::RunnerManifest)
        );
        assert_eq!(
            BindingCheck::for_task_kind(TaskKind::Docs),
            Some(BindingCheck::DocumentSection)
        );
        assert_eq!(
            BindingCheck::for_task_kind(TaskKind::Data),
            Some(BindingCheck::SchemaOutput)
        );
        assert_eq!(
            BindingCheck::for_task_kind(TaskKind::Research),
            Some(BindingCheck::SourceCitation)
        );
        // Ops / Authoring observe evidence directly: no binding step to fail.
        assert_eq!(BindingCheck::for_task_kind(TaskKind::Ops), None);
        assert_eq!(BindingCheck::for_task_kind(TaskKind::Authoring), None);
    }

    #[test]
    fn binding_check_for_evidence_runner_kind_matches_task_kind_mapping() {
        let cases = [
            (
                EvidenceRunnerKind::CodingBuildTest,
                Some(BindingCheck::RunnerManifest),
            ),
            (
                EvidenceRunnerKind::DocsContentCheck,
                Some(BindingCheck::DocumentSection),
            ),
            (
                EvidenceRunnerKind::DataSchemaCheck,
                Some(BindingCheck::SchemaOutput),
            ),
            (
                EvidenceRunnerKind::ResearchSourceFetch,
                Some(BindingCheck::SourceCitation),
            ),
            (EvidenceRunnerKind::OpsCommandObservation, None),
            (EvidenceRunnerKind::AuthoringContentCheck, None),
        ];
        for (runner_kind, expected) in cases {
            assert_eq!(
                BindingCheck::for_evidence_runner_kind(runner_kind),
                expected
            );
        }
    }

    #[test]
    fn label_round_trip_is_stable() {
        // Wire labels feed the offline transition metrics; keep them pinned.
        assert_eq!(BindingCheck::RunnerManifest.as_str(), "runner_manifest");
        assert_eq!(BindingCheck::DocumentSection.as_str(), "document_section");
        assert_eq!(BindingCheck::SchemaOutput.as_str(), "schema_output");
        assert_eq!(BindingCheck::SourceCitation.as_str(), "source_citation");
        assert_eq!(
            BindingRecovery::MaterializeRunnerManifest.as_str(),
            "materialize_runner_manifest"
        );
        assert_eq!(
            BindingRecovery::RecoverDocumentSection.as_str(),
            "recover_document_section"
        );
        assert_eq!(
            BindingRecovery::RecoverSchemaOutput.as_str(),
            "recover_schema_output"
        );
        assert_eq!(
            BindingRecovery::RecoverSourceCitation.as_str(),
            "recover_source_citation"
        );
    }
}
