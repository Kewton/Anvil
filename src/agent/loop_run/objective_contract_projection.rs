//! ObjectiveContract projection.
//!
//! This module owns the read-only projection from the legacy `TaskContract`
//! fields into generic objective lifecycle vocabulary. It intentionally does
//! not evaluate completion, inspect prompts, or plan repairs.

use super::task_contract::{
    ArtifactRole, CompletionProjectIntent, DeliverableKind, DeliverableSpec, EvidenceSpec,
    ObjectiveDeliverableKind, ObjectiveEvidenceKind, ObjectiveKind, TaskContract, TaskKind,
};

#[allow(dead_code)] // WP-F: projected into ObjectiveContract telemetry/prompt surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ObjectiveAuthority {
    CurrentUserRequest,
}

impl ObjectiveAuthority {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::CurrentUserRequest => "current_user_request",
        }
    }
}

#[allow(dead_code)] // WP-F: context remains advisory, not contract authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ObjectiveAuxiliaryContext {
    SessionContext,
}

impl ObjectiveAuxiliaryContext {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::SessionContext => "session_context",
        }
    }
}

#[allow(dead_code)] // Issue #947: read-only ObjectiveContract projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ObjectiveContract {
    pub(super) authority: ObjectiveAuthority,
    pub(super) auxiliary_context: ObjectiveAuxiliaryContext,
    /// Classification-layer kind (kept for back-compat with existing readers).
    pub(super) task_kind: TaskKind,
    /// Issue #975: objective-layer kind — coding is `ObjectiveKind::Coding`,
    /// one kind among the non-coding objectives rather than the default.
    pub(super) objective_kind: ObjectiveKind,
    pub(super) deliverable_kind: DeliverableSpec,
    pub(super) evidence_kind: EvidenceSpec,
    /// Required deliverable roles in lifecycle order. This is the objective
    /// layer's projection of the older `TaskContract.required_artifacts` field.
    pub(super) required_deliverables: Vec<ArtifactRole>,
    /// Whether command/external evidence is mandatory after deliverables.
    pub(super) evidence_required: bool,
    pub(super) required_evidence_commands: Vec<String>,
}

impl ObjectiveContract {
    pub(super) fn from_task_contract(contract: &TaskContract) -> Self {
        if contract.completion_policy.project_intent == CompletionProjectIntent::AnswerOnly {
            return Self {
                authority: ObjectiveAuthority::CurrentUserRequest,
                auxiliary_context: ObjectiveAuxiliaryContext::SessionContext,
                task_kind: contract.task_kind,
                objective_kind: ObjectiveKind::from_task_kind(contract.task_kind),
                deliverable_kind: ObjectiveDeliverableKind::Answer,
                evidence_kind: ObjectiveEvidenceKind::ContentAcceptance,
                required_deliverables: Vec::new(),
                evidence_required: false,
                required_evidence_commands: Vec::new(),
            };
        }

        let (deliverable_kind, default_evidence_kind) = match contract.task_kind {
            TaskKind::Coding => (
                ObjectiveDeliverableKind::SourceFiles,
                ObjectiveEvidenceKind::TestRun,
            ),
            TaskKind::Docs => (
                ObjectiveDeliverableKind::DocumentSections,
                ObjectiveEvidenceKind::ContentCheck,
            ),
            TaskKind::Data => (
                ObjectiveDeliverableKind::OutputFile,
                ObjectiveEvidenceKind::SchemaCheck,
            ),
            TaskKind::Research => (
                ObjectiveDeliverableKind::ResearchNotes,
                ObjectiveEvidenceKind::SourceFetchEvidence,
            ),
            TaskKind::Ops => (
                ObjectiveDeliverableKind::CommandObservation,
                ObjectiveEvidenceKind::SafetyBoundaryEvidence,
            ),
            TaskKind::Authoring => (
                ObjectiveDeliverableKind::ProseArtifact,
                ObjectiveEvidenceKind::ContentAcceptance,
            ),
        };
        let evidence_kind = contract
            .objective_evidence_kind_override
            .unwrap_or(default_evidence_kind);

        Self {
            authority: ObjectiveAuthority::CurrentUserRequest,
            auxiliary_context: ObjectiveAuxiliaryContext::SessionContext,
            task_kind: contract.task_kind,
            objective_kind: ObjectiveKind::from_task_kind(contract.task_kind),
            deliverable_kind,
            evidence_kind,
            required_deliverables: contract.required_artifacts.clone(),
            evidence_required: contract.completion_policy.verification_required()
                || contract
                    .objective_evidence_kind_override
                    .is_some_and(objective_evidence_kind_requires_command_evidence)
                || contract
                    .required_artifact_identities
                    .iter()
                    .any(|identity| identity.kind == DeliverableKind::CommandOutput),
            required_evidence_commands: required_evidence_commands_from_contract(contract),
        }
    }

    pub(super) fn required_deliverables(&self) -> &[ArtifactRole] {
        &self.required_deliverables
    }

    pub(super) fn has_required_deliverables(&self) -> bool {
        !self.required_deliverables.is_empty()
    }

    pub(super) fn requires_evidence(&self) -> bool {
        self.evidence_required
    }
}

fn required_evidence_commands_from_contract(contract: &TaskContract) -> Vec<String> {
    let mut commands = contract
        .required_artifact_identities
        .iter()
        .filter(|identity| identity.kind == DeliverableKind::CommandOutput)
        .flat_map(|identity| {
            identity
                .acceptance_criteria
                .iter()
                .filter_map(|criterion| criterion.strip_prefix("command:"))
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    commands.sort();
    commands.dedup();
    commands
}

fn objective_evidence_kind_requires_command_evidence(evidence_kind: ObjectiveEvidenceKind) -> bool {
    matches!(
        evidence_kind,
        ObjectiveEvidenceKind::TestRun | ObjectiveEvidenceKind::SafetyBoundaryEvidence
    )
}

#[cfg(test)]
mod tests {
    use super::super::task_contract::{ArtifactRole, DeliverableKind};
    use super::*;

    #[test]
    fn projects_coding_source_files_and_test_run() {
        let contract = TaskContract::from_request(
            "Implement calculator.py and tests/test_calculator.py for addition.",
        );

        let objective = ObjectiveContract::from_task_contract(&contract);

        assert_eq!(objective.task_kind, TaskKind::Coding);
        assert_eq!(
            objective.deliverable_kind,
            ObjectiveDeliverableKind::SourceFiles
        );
        assert_eq!(objective.evidence_kind, ObjectiveEvidenceKind::TestRun);
        assert!(
            objective
                .required_deliverables
                .contains(&ArtifactRole::Implementation)
        );
    }

    #[test]
    fn projects_ops_command_evidence_from_command_output_obligation() {
        let contract = TaskContract::from_request_with_kind(
            "Run pwd and write ops-observation.md containing the exact observed directory. Do not modify code.",
            Some(TaskKind::Ops),
        );

        let objective = ObjectiveContract::from_task_contract(&contract);

        assert_eq!(objective.task_kind, TaskKind::Ops);
        assert_eq!(
            objective.deliverable_kind,
            ObjectiveDeliverableKind::CommandObservation
        );
        assert_eq!(
            objective.evidence_kind,
            ObjectiveEvidenceKind::SafetyBoundaryEvidence
        );
        assert!(objective.requires_evidence());
        assert!(
            contract
                .required_artifact_identities
                .iter()
                .any(|identity| {
                    identity.role == ArtifactRole::UsageDocs
                        && identity.kind == DeliverableKind::CommandOutput
                })
        );
    }

    #[test]
    fn answer_only_projects_without_required_deliverables() {
        let contract = TaskContract::from_request("Explain how local LLM agents work.");

        let objective = ObjectiveContract::from_task_contract(&contract);

        assert_eq!(objective.deliverable_kind, ObjectiveDeliverableKind::Answer);
        assert!(!objective.has_required_deliverables());
        assert!(!objective.requires_evidence());
    }
}
