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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RepairTargetCandidateStatus {
    Selected,
    RejectedLowerAuthority,
    RejectedRoleMismatch,
    Unavailable,
}

impl RepairTargetCandidateStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Selected => "selected",
            Self::RejectedLowerAuthority => "rejected_lower_authority",
            Self::RejectedRoleMismatch => "rejected_role_mismatch",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RepairTargetCandidateAudit {
    pub(super) authority: RepairTargetAuthority,
    pub(super) role: Option<ArtifactRole>,
    pub(super) path: Option<String>,
    pub(super) status: RepairTargetCandidateStatus,
}

impl RepairTargetCandidateAudit {
    pub(super) fn new(
        authority: RepairTargetAuthority,
        hint: Option<&RecoveryTargetHint>,
        status: RepairTargetCandidateStatus,
    ) -> Self {
        Self {
            authority,
            role: hint.map(|hint| hint.role),
            path: hint.map(|hint| hint.path.clone()),
            status,
        }
    }

    fn to_json_value(&self) -> serde_json::Value {
        serde_json::json!({
            "authority": self.authority.as_str(),
            "role": self.role.map(ArtifactRole::label),
            "path": self.path.as_deref(),
            "status": self.status.as_str(),
        })
    }
}

pub(super) fn audit_repair_target_candidates(
    candidates: &[(Option<RecoveryTargetHint>, RepairTargetAuthority)],
    selected: Option<&RecoveryTargetHint>,
    preferred_role: Option<ArtifactRole>,
) -> Vec<RepairTargetCandidateAudit> {
    candidates
        .iter()
        .map(|(hint, authority)| {
            let status = repair_target_candidate_status(hint.as_ref(), selected, preferred_role);
            RepairTargetCandidateAudit::new(*authority, hint.as_ref(), status)
        })
        .collect()
}

fn repair_target_candidate_status(
    candidate: Option<&RecoveryTargetHint>,
    selected: Option<&RecoveryTargetHint>,
    preferred_role: Option<ArtifactRole>,
) -> RepairTargetCandidateStatus {
    let Some(candidate) = candidate else {
        return RepairTargetCandidateStatus::Unavailable;
    };
    if selected
        .is_some_and(|selected| selected.role == candidate.role && selected.path == candidate.path)
    {
        return RepairTargetCandidateStatus::Selected;
    }
    if preferred_role.is_some_and(|role| candidate.role != role) {
        return RepairTargetCandidateStatus::RejectedRoleMismatch;
    }
    RepairTargetCandidateStatus::RejectedLowerAuthority
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RepairTargetDecision {
    pub(super) failure_class: Option<FailureClass>,
    pub(super) target_role: Option<ArtifactRole>,
    pub(super) target_hint: Option<RecoveryTargetHint>,
    pub(super) authority: RepairTargetAuthority,
    pub(super) operator_candidates: Vec<OperatorId>,
    pub(super) candidate_audit: Vec<RepairTargetCandidateAudit>,
}

impl RepairTargetDecision {
    pub(super) fn new(
        failure_class: Option<FailureClass>,
        target_role: Option<ArtifactRole>,
        target_hint: Option<RecoveryTargetHint>,
        authority: RepairTargetAuthority,
        operator_candidates: Vec<OperatorId>,
        candidate_audit: Vec<RepairTargetCandidateAudit>,
    ) -> Self {
        Self {
            failure_class,
            target_role,
            target_hint,
            authority,
            operator_candidates,
            candidate_audit,
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
            "candidate_audit": self.candidate_audit
                .iter()
                .map(RepairTargetCandidateAudit::to_json_value)
                .collect::<Vec<_>>(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint(role: ArtifactRole, path: &str) -> RecoveryTargetHint {
        RecoveryTargetHint {
            role,
            path: path.to_string(),
            reason: "test".to_string(),
        }
    }

    #[test]
    fn audit_marks_selected_role_mismatch_lower_authority_and_unavailable() {
        let selected = hint(ArtifactRole::Test, "tests/test_app.py");
        let candidates = vec![
            (
                Some(hint(ArtifactRole::Implementation, "app.py")),
                RepairTargetAuthority::CorrectionJob,
            ),
            (
                Some(selected.clone()),
                RepairTargetAuthority::SemanticCluster,
            ),
            (
                Some(hint(ArtifactRole::Test, "tests/old_test.py")),
                RepairTargetAuthority::AssessmentHint,
            ),
            (None, RepairTargetAuthority::RepairJobHint),
        ];

        let audit =
            audit_repair_target_candidates(&candidates, Some(&selected), Some(ArtifactRole::Test));

        assert_eq!(
            audit.iter().map(|entry| entry.status).collect::<Vec<_>>(),
            vec![
                RepairTargetCandidateStatus::RejectedRoleMismatch,
                RepairTargetCandidateStatus::Selected,
                RepairTargetCandidateStatus::RejectedLowerAuthority,
                RepairTargetCandidateStatus::Unavailable,
            ]
        );
    }
}
