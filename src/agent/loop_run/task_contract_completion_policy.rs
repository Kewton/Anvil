//! Completion authority policy for TaskContract.
//!
//! This module owns the deterministic mapping from an admitted contract's
//! required artifact roles and behavior requirements to the local evidence that
//! can satisfy completion. It deliberately does not parse raw requests.

use super::completion_evidence::{CompletionEvidence, RepoEditCategory};
use super::required_behavior::RequiredBehaviorContract;
use super::task_contract::{ArtifactRole, TaskIntent, TaskKind, role_from_repo_edit};
use crate::tools::bash::BashCommandClass;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompletionProjectIntent {
    DocsOnly,
    ArtifactOnly,
    ImplWithTest,
    ImplWithoutTest,
    AnswerOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CompletionPolicy {
    pub(super) task_kind: TaskKind,
    pub(super) project_intent: CompletionProjectIntent,
    required_artifacts: Vec<ArtifactRole>,
    verification_required: bool,
    test_execution_required: bool,
}

impl CompletionPolicy {
    pub(super) fn from_contract_parts(
        task_kind: TaskKind,
        intent: TaskIntent,
        required_artifacts: &[ArtifactRole],
        verification_required: bool,
        required_behavior: &RequiredBehaviorContract,
    ) -> Self {
        let project_intent =
            project_intent_from_required_artifacts(task_kind, intent, required_artifacts);
        let verifier_free_document_task = matches!(
            project_intent,
            CompletionProjectIntent::DocsOnly | CompletionProjectIntent::AnswerOnly
        );
        // Issue #918 (P1): the verification-requirement gate now routes through the
        // single `capability_for(TaskKind)` dispatch spine. `requires_executable_verifier`
        // is a 1:1 replacement for the old `coding_verifier_required` bare gate
        // (`task_kind == Coding && !verifier_free_document_task`); it is ANDed into each
        // of the two fields *separately*, exactly as before, so no inter-field dependency
        // is introduced and non-coding kinds remain verifier-free (§5.1 invariant).
        let coding_verifier_required = super::verifier::capability_for(task_kind)
            .requires_executable_verifier(verifier_free_document_task);
        Self {
            task_kind,
            project_intent,
            required_artifacts: required_artifacts.to_vec(),
            verification_required: verification_required && coding_verifier_required,
            test_execution_required: required_behavior.test_execution_required
                && coding_verifier_required,
        }
    }

    #[cfg(test)]
    pub(super) fn from_request(request: &str) -> Self {
        super::task_contract::TaskContract::from_request(request).completion_policy
    }

    pub(super) fn legacy_generic_code() -> Self {
        Self {
            task_kind: TaskKind::Coding,
            project_intent: CompletionProjectIntent::ImplWithoutTest,
            required_artifacts: Vec::new(),
            verification_required: false,
            test_execution_required: false,
        }
    }

    pub(super) fn required_artifacts(&self) -> &[ArtifactRole] {
        &self.required_artifacts
    }

    pub(super) fn verification_required(&self) -> bool {
        self.verification_required
    }

    pub(super) fn test_execution_required(&self) -> bool {
        self.test_execution_required
    }

    pub(super) fn accepts_evidence(&self, evidence: &CompletionEvidence) -> bool {
        if !is_deterministic_completion_authority_evidence(evidence) {
            return false;
        }
        match evidence {
            CompletionEvidence::RepoEdit { category, .. } => {
                self.accepts_repo_edit_category(*category)
            }
            CompletionEvidence::VerifierExitZero { class, .. } => {
                self.accepts_verifier_class(*class)
            }
            CompletionEvidence::RequiredSectionsPass { .. } => {
                self.project_intent == CompletionProjectIntent::DocsOnly
                    || self.required_artifacts.contains(&ArtifactRole::UsageDocs)
            }
            CompletionEvidence::StructuredDataPass { .. } => {
                self.project_intent == CompletionProjectIntent::ArtifactOnly
                    || self.required_artifacts.contains(&ArtifactRole::DataOutput)
            }
            CompletionEvidence::ReportCompletenessPass { .. } => {
                self.project_intent == CompletionProjectIntent::DocsOnly
                    || self.required_artifacts.contains(&ArtifactRole::UsageDocs)
            }
            CompletionEvidence::CommandObservation {
                exit_status,
                safety_boundary_passed,
                ..
            } => self.task_kind == TaskKind::Ops && *exit_status == 0 && *safety_boundary_passed,
            CompletionEvidence::AnswerOnly => {
                self.project_intent == CompletionProjectIntent::AnswerOnly
            }
        }
    }

    fn accepts_repo_edit_category(&self, category: RepoEditCategory) -> bool {
        let Some(role) = role_from_repo_edit(category) else {
            return self.project_intent == CompletionProjectIntent::ArtifactOnly
                && self.required_artifacts.is_empty();
        };
        match self.project_intent {
            CompletionProjectIntent::DocsOnly => role == ArtifactRole::UsageDocs,
            CompletionProjectIntent::ArtifactOnly => {
                self.required_artifacts.is_empty() || self.required_artifacts.contains(&role)
            }
            CompletionProjectIntent::ImplWithTest => {
                matches!(
                    role,
                    ArtifactRole::Implementation | ArtifactRole::Test | ArtifactRole::DataOutput
                )
            }
            CompletionProjectIntent::ImplWithoutTest => {
                matches!(
                    role,
                    ArtifactRole::Implementation | ArtifactRole::DataOutput
                )
            }
            CompletionProjectIntent::AnswerOnly => false,
        }
    }

    fn accepts_verifier_class(&self, class: BashCommandClass) -> bool {
        match class {
            BashCommandClass::BuildTest => matches!(
                self.project_intent,
                CompletionProjectIntent::ArtifactOnly
                    | CompletionProjectIntent::ImplWithTest
                    | CompletionProjectIntent::ImplWithoutTest
                    | CompletionProjectIntent::AnswerOnly
            ),
            BashCommandClass::EnvSetup => {
                self.project_intent == CompletionProjectIntent::ArtifactOnly
                    && self.required_artifacts.contains(&ArtifactRole::Setup)
            }
            _ => false,
        }
    }
}

impl Default for CompletionPolicy {
    fn default() -> Self {
        Self::legacy_generic_code()
    }
}

/// Issue #905: closed, deterministic completion-authority boundary.
///
/// `TaskContract` may only derive completion from evidence emitted by local
/// tool/verifier/deliverable checkers. Advisory context such as PAM is not
/// represented here; if a future non-deterministic variant is added to
/// `CompletionEvidence`, this predicate fails closed until that variant is
/// explicitly reviewed.
pub(super) fn is_deterministic_completion_authority_evidence(
    evidence: &CompletionEvidence,
) -> bool {
    matches!(
        evidence,
        CompletionEvidence::RepoEdit { .. }
            | CompletionEvidence::VerifierExitZero { .. }
            | CompletionEvidence::RequiredSectionsPass { .. }
            | CompletionEvidence::StructuredDataPass { .. }
            | CompletionEvidence::ReportCompletenessPass { .. }
            | CompletionEvidence::CommandObservation { .. }
            | CompletionEvidence::AnswerOnly
    )
}

fn project_intent_from_required_artifacts(
    task_kind: TaskKind,
    intent: TaskIntent,
    required_artifacts: &[ArtifactRole],
) -> CompletionProjectIntent {
    // Issue #922 (DD4 / S7-001 / DR1-001): the `Explain` → `AnswerOnly`
    // short-circuit is relaxed ONLY for a research task that carries a required
    // report obligation, so the report flows through `assess_research_report`
    // instead of completing answer-only. Every other kind (Coding/Docs/Data/Ops)
    // keeps its exact pre-#922 behavior — Explain always short-circuits — so the
    // §5.1 verifier-free invariant and existing goldens are unchanged. This is
    // the single shared signal (`research_report_obligation`) used by every
    // Explain short-circuit gate (`plan_artifact_recovery`, `evaluate_inner`).
    let research_report_obligation =
        task_kind == TaskKind::Research && !required_artifacts.is_empty();
    if matches!(intent, TaskIntent::Explain) && !research_report_obligation {
        return CompletionProjectIntent::AnswerOnly;
    }
    let has_impl = required_artifacts.contains(&ArtifactRole::Implementation);
    let has_test = required_artifacts.contains(&ArtifactRole::Test);
    let docs_only =
        !has_impl && !has_test && required_artifacts == [ArtifactRole::UsageDocs].as_slice();
    if docs_only {
        return CompletionProjectIntent::DocsOnly;
    }
    if !has_impl {
        return CompletionProjectIntent::ArtifactOnly;
    }
    if has_test {
        CompletionProjectIntent::ImplWithTest
    } else {
        CompletionProjectIntent::ImplWithoutTest
    }
}
