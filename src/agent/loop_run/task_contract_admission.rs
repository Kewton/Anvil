//! Deterministic contract-admission boundary.
//!
//! This module does not infer request semantics. It only decides which already
//! projected facts may participate in `TaskContract` construction, keeping
//! forced kind, controller packet kind, project-profile inputs, and future
//! semantic candidates behind one explicit adoption surface.

#![allow(dead_code)]

use super::project_profile::ProjectProfileConfirmation;
use super::project_profile_projection::{
    ProjectProfileContractInputs, contract_inputs_from_confirmation,
};
use super::task_contract::{TaskContract, TaskKind};
use super::task_contract_semantic_candidate::{CandidateContractDisagreement, SemanticCandidate};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ContractAdmissionRejectionReason {
    ProjectProfileLowConfidence,
    SemanticCandidateShadowOnly,
    ExplicitFactConflict,
}

impl ContractAdmissionRejectionReason {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::ProjectProfileLowConfidence => "project_profile_low_confidence",
            Self::SemanticCandidateShadowOnly => "semantic_candidate_shadow_only",
            Self::ExplicitFactConflict => "explicit_fact_conflict",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProjectProfileAdmissionDecision {
    pub(super) profile_inputs: Option<ProjectProfileContractInputs>,
    pub(super) rejection_reasons: Vec<ContractAdmissionRejectionReason>,
}

impl ProjectProfileAdmissionDecision {
    pub(super) fn rejection_labels(&self) -> Vec<&'static str> {
        self.rejection_reasons
            .iter()
            .map(|reason| reason.label())
            .collect()
    }
}

pub(super) fn admit_project_profile_contract_inputs(
    profile: Option<&ProjectProfileConfirmation>,
) -> ProjectProfileAdmissionDecision {
    let profile_inputs = contract_inputs_from_confirmation(profile);
    let rejection_reasons = if profile.is_some() && profile_inputs.is_none() {
        vec![ContractAdmissionRejectionReason::ProjectProfileLowConfidence]
    } else {
        Vec::new()
    };
    ProjectProfileAdmissionDecision {
        profile_inputs,
        rejection_reasons,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TaskKindAdmissionSource {
    ForcedKind,
    ControllerPacket,
    ProjectProfile,
    DeterministicInference,
}

impl TaskKindAdmissionSource {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::ForcedKind => "forced_kind",
            Self::ControllerPacket => "controller_packet",
            Self::ProjectProfile => "project_profile",
            Self::DeterministicInference => "deterministic_inference",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct TaskKindAdmissionDecision {
    pub(super) task_kind: TaskKind,
    pub(super) classification_confidence: f32,
    pub(super) source: TaskKindAdmissionSource,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ContractAdmissionInput<'a> {
    pub(super) forced_kind: Option<TaskKind>,
    pub(super) controller_task_kind: Option<TaskKind>,
    pub(super) profile_inputs: Option<&'a ProjectProfileContractInputs>,
    pub(super) inferred_kind: TaskKind,
    pub(super) inferred_matched: bool,
}

pub(super) fn admit_task_kind(inputs: ContractAdmissionInput<'_>) -> TaskKindAdmissionDecision {
    if let Some(kind) = inputs.forced_kind {
        return TaskKindAdmissionDecision {
            task_kind: kind,
            classification_confidence: 1.0,
            source: TaskKindAdmissionSource::ForcedKind,
        };
    }
    if let Some(kind) = inputs.controller_task_kind {
        return TaskKindAdmissionDecision {
            task_kind: kind,
            classification_confidence: 1.0,
            source: TaskKindAdmissionSource::ControllerPacket,
        };
    }
    if let Some(profile_inputs) = inputs.profile_inputs
        && let Some(kind) = profile_inputs.task_kind
    {
        return TaskKindAdmissionDecision {
            task_kind: kind,
            classification_confidence: profile_inputs.confidence,
            source: TaskKindAdmissionSource::ProjectProfile,
        };
    }
    TaskKindAdmissionDecision {
        task_kind: inputs.inferred_kind,
        classification_confidence: if inputs.inferred_matched { 1.0 } else { 0.0 },
        source: TaskKindAdmissionSource::DeterministicInference,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SemanticCandidateAdmissionStatus {
    Admitted,
    Rejected,
    Ignored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SemanticCandidateAdmissionReason {
    ShadowOnly,
    ExplicitFactConflict,
    LowerConfidenceThanContract,
    MissingArtifactIdentity,
    AmbiguousObjective,
    UnsafePath,
    UnsupportedRuntime,
}

impl SemanticCandidateAdmissionReason {
    fn label(self) -> &'static str {
        match self {
            Self::ShadowOnly => "shadow_only",
            Self::ExplicitFactConflict => "explicit_fact_conflict",
            Self::LowerConfidenceThanContract => "lower_confidence_than_contract",
            Self::MissingArtifactIdentity => "missing_artifact_identity",
            Self::AmbiguousObjective => "ambiguous_objective",
            Self::UnsafePath => "unsafe_path",
            Self::UnsupportedRuntime => "unsupported_runtime",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct SemanticCandidateAdmissionDecision {
    pub(super) status: SemanticCandidateAdmissionStatus,
    pub(super) reasons: Vec<SemanticCandidateAdmissionReason>,
    pub(super) disagreements: Vec<CandidateContractDisagreement>,
}

impl SemanticCandidateAdmissionDecision {
    pub(super) fn is_authoritative(&self) -> bool {
        matches!(self.status, SemanticCandidateAdmissionStatus::Admitted)
    }

    pub(super) fn reason_labels(&self) -> Vec<&'static str> {
        self.reasons.iter().map(|reason| reason.label()).collect()
    }

    pub(super) fn log_lines(&self) -> Vec<String> {
        let reasons = self.reason_labels().join(",");
        let mut lines = vec![format!(
            "semantic_candidate_admission status={:?} reasons={}",
            self.status, reasons
        )];
        lines.extend(self.disagreements.iter().map(|disagreement| {
            format!(
                "semantic_candidate_disagreement field={} candidate={} contract={}",
                disagreement.field, disagreement.candidate, disagreement.contract
            )
        }));
        lines
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct SemanticCandidateAdmissionInput<'a> {
    pub(super) candidate: &'a SemanticCandidate,
    pub(super) contract: &'a TaskContract,
    pub(super) allow_equivalent_current_behavior: bool,
}

pub(super) fn admit_semantic_candidate(
    inputs: SemanticCandidateAdmissionInput<'_>,
) -> SemanticCandidateAdmissionDecision {
    let disagreements = inputs
        .candidate
        .disagreements_with_contract(inputs.contract);
    if !disagreements.is_empty() {
        return SemanticCandidateAdmissionDecision {
            status: SemanticCandidateAdmissionStatus::Rejected,
            reasons: vec![SemanticCandidateAdmissionReason::ExplicitFactConflict],
            disagreements,
        };
    }

    if inputs.allow_equivalent_current_behavior {
        return SemanticCandidateAdmissionDecision {
            status: SemanticCandidateAdmissionStatus::Admitted,
            reasons: Vec::new(),
            disagreements,
        };
    }

    SemanticCandidateAdmissionDecision {
        status: SemanticCandidateAdmissionStatus::Ignored,
        reasons: vec![SemanticCandidateAdmissionReason::ShadowOnly],
        disagreements,
    }
}

#[cfg(test)]
mod tests {
    use super::super::project_profile::{ProfileDeliverableKind, ProjectProfileConfirmation};
    use super::super::task_contract::TaskKind;
    use super::super::task_contract_semantic_candidate::SemanticCandidate;
    use super::*;

    fn profile(confidence: f32) -> ProjectProfileConfirmation {
        ProjectProfileConfirmation {
            language: None,
            shape: None,
            deliverable_kind: Some(ProfileDeliverableKind::Document),
            primary_artifacts: vec!["README.md".to_string()],
            forbidden_artifacts: Vec::new(),
            evidence_kind: None,
            needs_environment_setup: None,
            preferred_runner: None,
            confidence,
            reason: None,
        }
    }

    #[test]
    fn low_confidence_project_profile_is_rejected_with_typed_reason() {
        let decision = admit_project_profile_contract_inputs(Some(&profile(0.20)));

        assert!(decision.profile_inputs.is_none());
        assert_eq!(
            decision.rejection_labels(),
            vec!["project_profile_low_confidence"]
        );
    }

    #[test]
    fn forced_kind_precedes_controller_profile_and_inference() {
        let profile_decision = admit_project_profile_contract_inputs(Some(&profile(0.95)));
        let decision = admit_task_kind(ContractAdmissionInput {
            forced_kind: Some(TaskKind::Data),
            controller_task_kind: Some(TaskKind::Ops),
            profile_inputs: profile_decision.profile_inputs.as_ref(),
            inferred_kind: TaskKind::Coding,
            inferred_matched: true,
        });

        assert_eq!(decision.task_kind, TaskKind::Data);
        assert_eq!(decision.source, TaskKindAdmissionSource::ForcedKind);
        assert_eq!(decision.classification_confidence, 1.0);
    }

    #[test]
    fn profile_task_kind_is_admitted_when_stronger_inputs_absent() {
        let profile_decision = admit_project_profile_contract_inputs(Some(&profile(0.95)));
        let decision = admit_task_kind(ContractAdmissionInput {
            forced_kind: None,
            controller_task_kind: None,
            profile_inputs: profile_decision.profile_inputs.as_ref(),
            inferred_kind: TaskKind::Coding,
            inferred_matched: true,
        });

        assert_eq!(decision.task_kind, TaskKind::Docs);
        assert_eq!(decision.source, TaskKindAdmissionSource::ProjectProfile);
        assert_eq!(decision.classification_confidence, 0.95);
    }

    #[test]
    fn semantic_candidate_shadow_mode_keeps_equivalent_candidate_non_authoritative() {
        let contract = TaskContract::from_request("Generate output.csv with columns id and total.");
        let candidate = SemanticCandidate::deterministic_shadow_from_contract(&contract);

        let decision = admit_semantic_candidate(SemanticCandidateAdmissionInput {
            candidate: &candidate,
            contract: &contract,
            allow_equivalent_current_behavior: false,
        });

        assert_eq!(decision.status, SemanticCandidateAdmissionStatus::Ignored);
        assert!(!decision.is_authoritative());
        assert_eq!(decision.reason_labels(), vec!["shadow_only"]);
        assert!(decision.disagreements.is_empty());
    }

    #[test]
    fn semantic_candidate_conflict_is_rejected_and_logged() {
        let contract = TaskContract::from_request("Generate output.csv with columns id and total.");
        let mut candidate = SemanticCandidate::deterministic_shadow_from_contract(&contract);
        candidate.objective_kind = super::super::task_contract::ObjectiveKind::Coding;

        let decision = admit_semantic_candidate(SemanticCandidateAdmissionInput {
            candidate: &candidate,
            contract: &contract,
            allow_equivalent_current_behavior: true,
        });

        assert_eq!(decision.status, SemanticCandidateAdmissionStatus::Rejected);
        assert!(!decision.is_authoritative());
        assert_eq!(decision.reason_labels(), vec!["explicit_fact_conflict"]);
        assert_eq!(decision.disagreements.len(), 1);
        assert!(
            decision
                .log_lines()
                .iter()
                .any(|line| line.contains("semantic_candidate_disagreement"))
        );
    }

    #[test]
    fn equivalent_semantic_candidate_can_be_admitted_without_mutating_contract() {
        let contract = TaskContract::from_request(
            "Create a Rust CLI. Include Cargo.toml, implementation, tests, and README.md.",
        );
        let candidate = SemanticCandidate::deterministic_shadow_from_contract(&contract);

        let decision = admit_semantic_candidate(SemanticCandidateAdmissionInput {
            candidate: &candidate,
            contract: &contract,
            allow_equivalent_current_behavior: true,
        });

        assert_eq!(decision.status, SemanticCandidateAdmissionStatus::Admitted);
        assert!(decision.is_authoritative());
        assert!(decision.reasons.is_empty());
        assert!(decision.disagreements.is_empty());
        assert_eq!(contract.task_kind, TaskKind::Coding);
    }
}
