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
use super::task_contract::TaskKind;

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

#[cfg(test)]
mod tests {
    use super::super::project_profile::{ProfileDeliverableKind, ProjectProfileConfirmation};
    use super::super::task_contract::TaskKind;
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
}
