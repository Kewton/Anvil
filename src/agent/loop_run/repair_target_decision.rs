//! Typed repair-target decision boundary.
//!
//! This module deliberately does not classify failures or inspect prompts. It
//! records the admitted handoff from diagnostic/operator selection to patch
//! execution so the repair lifecycle can explain which target role/path it is
//! about to edit.

use super::repair_operator::{FailureClass, OperatorId};
use super::task_contract::{ArtifactRole, RecoveryTargetHint};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RepairTargetAuthority {
    CorrectionJob,
    SemanticCluster,
    SemanticChangedFile,
    SemanticObservedTarget,
    AssessmentPlan,
    AssessmentHint,
    RepairJobHint,
    InitialFailureHint,
    None,
}

impl RepairTargetAuthority {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::CorrectionJob => "correction_job",
            Self::SemanticCluster => "semantic_cluster",
            Self::SemanticChangedFile => "semantic_changed_file",
            Self::SemanticObservedTarget => "semantic_observed_target",
            Self::AssessmentPlan => "assessment_plan",
            Self::AssessmentHint => "assessment_hint",
            Self::RepairJobHint => "repair_job_hint",
            Self::InitialFailureHint => "initial_failure_hint",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RepairTargetDecision {
    pub(super) failure_class: Option<FailureClass>,
    pub(super) target_role: Option<ArtifactRole>,
    pub(super) target_hint: Option<RecoveryTargetHint>,
    pub(super) authority: RepairTargetAuthority,
    pub(super) operator_candidates: Vec<OperatorId>,
}

impl RepairTargetDecision {
    pub(super) fn new(
        failure_class: Option<FailureClass>,
        target_role: Option<ArtifactRole>,
        target_hint: Option<RecoveryTargetHint>,
        authority: RepairTargetAuthority,
        operator_candidates: Vec<OperatorId>,
    ) -> Self {
        Self {
            failure_class,
            target_role,
            target_hint,
            authority,
            operator_candidates,
        }
    }

    pub(super) fn target_hint(&self) -> Option<RecoveryTargetHint> {
        self.target_hint.clone()
    }

    pub(super) fn to_json_value(&self) -> serde_json::Value {
        serde_json::json!({
            "failure_class": self.failure_class.map(FailureClass::as_str),
            "target_role": self.target_role.map(ArtifactRole::label),
            "target_path": self.target_hint.as_ref().map(|hint| hint.path.as_str()),
            "authority": self.authority.as_str(),
            "operator_candidates": self.operator_candidates
                .iter()
                .map(|id| id.as_str())
                .collect::<Vec<_>>(),
        })
    }
}
