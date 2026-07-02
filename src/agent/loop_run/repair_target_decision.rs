//! Typed repair-target decision boundary.
//!
//! This module deliberately does not classify failures or inspect prompts. It
//! records the admitted handoff from diagnostic/operator selection to patch
//! execution so the repair lifecycle can explain which target role/path it is
//! about to edit.

use super::repair_operator::{FailureClass, OperatorId};
use super::task_contract::{ArtifactRole, RecoveryTargetHint};

#[allow(dead_code)] // ToolFailure is projected by tool-owned recovery paths as they migrate to this delta vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RepairTargetDeltaKind {
    MissingDeliverable,
    MissingEvidence,
    EvidenceFailed,
    ToolFailure,
    StyleMismatch,
    StaleEvidence,
}

impl RepairTargetDeltaKind {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::MissingDeliverable => "missing_deliverable",
            Self::MissingEvidence => "missing_evidence",
            Self::EvidenceFailed => "evidence_failed",
            Self::ToolFailure => "tool_failure",
            Self::StyleMismatch => "style_mismatch",
            Self::StaleEvidence => "stale_evidence",
        }
    }
}

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
    pub(super) delta_kind: RepairTargetDeltaKind,
    pub(super) failure_class: Option<FailureClass>,
    pub(super) target_role: Option<ArtifactRole>,
    pub(super) target_hint: Option<RecoveryTargetHint>,
    pub(super) authority: RepairTargetAuthority,
    pub(super) operator_candidates: Vec<OperatorId>,
    pub(super) candidate_audit: Vec<RepairTargetCandidateAudit>,
    pub(super) ledger_facts: RepairTargetLedgerFacts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RepairTargetLedgerFacts {
    pub(super) selected_target_present: bool,
    pub(super) target_role: Option<ArtifactRole>,
    pub(super) target_authority: RepairTargetAuthority,
    pub(super) operator_candidate_count: usize,
    pub(super) candidate_count: usize,
    pub(super) rejected_role_mismatch_count: usize,
    pub(super) rejected_lower_authority_count: usize,
}

impl RepairTargetLedgerFacts {
    fn from_decision_parts(
        target_role: Option<ArtifactRole>,
        target_hint: Option<&RecoveryTargetHint>,
        authority: RepairTargetAuthority,
        operator_candidates: &[OperatorId],
        candidate_audit: &[RepairTargetCandidateAudit],
    ) -> Self {
        Self {
            selected_target_present: target_hint.is_some(),
            target_role,
            target_authority: authority,
            operator_candidate_count: operator_candidates.len(),
            candidate_count: candidate_audit.len(),
            rejected_role_mismatch_count: candidate_audit
                .iter()
                .filter(|entry| entry.status == RepairTargetCandidateStatus::RejectedRoleMismatch)
                .count(),
            rejected_lower_authority_count: candidate_audit
                .iter()
                .filter(|entry| entry.status == RepairTargetCandidateStatus::RejectedLowerAuthority)
                .count(),
        }
    }

    fn has_stale_selected_target_evidence(&self) -> bool {
        self.rejected_role_mismatch_count > 0 || self.rejected_lower_authority_count > 0
    }

    fn to_json_value(&self) -> serde_json::Value {
        serde_json::json!({
            "selected_target_present": self.selected_target_present,
            "target_role": self.target_role.map(ArtifactRole::label),
            "target_authority": self.target_authority.as_str(),
            "operator_candidate_count": self.operator_candidate_count,
            "candidate_count": self.candidate_count,
            "rejected_role_mismatch_count": self.rejected_role_mismatch_count,
            "rejected_lower_authority_count": self.rejected_lower_authority_count,
        })
    }
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
        let ledger_facts = RepairTargetLedgerFacts::from_decision_parts(
            target_role,
            target_hint.as_ref(),
            authority,
            &operator_candidates,
            &candidate_audit,
        );
        let delta_kind = repair_target_delta_kind(
            failure_class,
            target_hint.as_ref(),
            &operator_candidates,
            &ledger_facts,
        );
        Self {
            delta_kind,
            failure_class,
            target_role,
            target_hint,
            authority,
            operator_candidates,
            candidate_audit,
            ledger_facts,
        }
    }

    pub(super) fn target_hint(&self) -> Option<RecoveryTargetHint> {
        self.target_hint.clone()
    }

    pub(super) fn to_json_value(&self) -> serde_json::Value {
        serde_json::json!({
            "delta_kind": self.delta_kind.as_str(),
            "failure_class": self.failure_class.map(FailureClass::as_str),
            "target_role": self.target_role.map(ArtifactRole::label),
            "target_path": self.target_hint.as_ref().map(|hint| hint.path.as_str()),
            "authority": self.authority.as_str(),
            "ledger_facts": self.ledger_facts.to_json_value(),
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

fn repair_target_delta_kind(
    failure_class: Option<FailureClass>,
    target_hint: Option<&RecoveryTargetHint>,
    operator_candidates: &[OperatorId],
    ledger_facts: &RepairTargetLedgerFacts,
) -> RepairTargetDeltaKind {
    if target_hint.is_none() {
        return RepairTargetDeltaKind::MissingDeliverable;
    }
    if matches!(failure_class, Some(FailureClass::NodeTestRunnerUnbound)) {
        return RepairTargetDeltaKind::MissingEvidence;
    }
    if matches!(failure_class, Some(FailureClass::TestArtifactMismatch)) {
        return RepairTargetDeltaKind::StyleMismatch;
    }
    if !operator_candidates.is_empty() && ledger_facts.has_stale_selected_target_evidence() {
        return RepairTargetDeltaKind::StaleEvidence;
    }
    RepairTargetDeltaKind::EvidenceFailed
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

    #[test]
    fn decision_projects_style_mismatch_delta_from_typed_failure_class() {
        let selected = hint(ArtifactRole::Test, "tests/test_password_strength.py");
        let decision = RepairTargetDecision::new(
            Some(FailureClass::TestArtifactMismatch),
            Some(ArtifactRole::Test),
            Some(selected),
            RepairTargetAuthority::SemanticCluster,
            vec![OperatorId::GeneratedTestExpectation],
            Vec::new(),
        );

        assert_eq!(decision.delta_kind, RepairTargetDeltaKind::StyleMismatch);
        assert_eq!(
            decision.to_json_value()["delta_kind"],
            serde_json::json!("style_mismatch")
        );
    }

    #[test]
    fn decision_projects_missing_evidence_delta_for_unbound_runner() {
        let selected = hint(ArtifactRole::Setup, "package.json");
        let decision = RepairTargetDecision::new(
            Some(FailureClass::NodeTestRunnerUnbound),
            Some(ArtifactRole::Setup),
            Some(selected),
            RepairTargetAuthority::AssessmentHint,
            vec![OperatorId::NodeTestRunnerManifest],
            Vec::new(),
        );

        assert_eq!(decision.delta_kind, RepairTargetDeltaKind::MissingEvidence);
    }

    #[test]
    fn decision_projects_stale_evidence_when_selected_target_overrides_old_candidates() {
        let selected = hint(ArtifactRole::Setup, "Cargo.toml");
        let stale = hint(ArtifactRole::Implementation, "src/lib.rs");
        let audit = audit_repair_target_candidates(
            &[
                (Some(stale), RepairTargetAuthority::CorrectionJob),
                (
                    Some(selected.clone()),
                    RepairTargetAuthority::SemanticCluster,
                ),
            ],
            Some(&selected),
            Some(ArtifactRole::Setup),
        );
        let decision = RepairTargetDecision::new(
            Some(FailureClass::RustCrateBindingMismatch),
            Some(ArtifactRole::Setup),
            Some(selected),
            RepairTargetAuthority::SemanticCluster,
            vec![OperatorId::RustLibNameBinding],
            audit,
        );

        assert_eq!(decision.delta_kind, RepairTargetDeltaKind::StaleEvidence);
        assert_eq!(decision.ledger_facts.rejected_role_mismatch_count, 1);
    }

    #[test]
    fn tool_failure_delta_label_is_stable_for_tool_owned_recovery() {
        assert_eq!(RepairTargetDeltaKind::ToolFailure.as_str(), "tool_failure");
    }
}
