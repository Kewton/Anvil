//! v0.4.16: accepted repair-plan boundary.
//!
//! Diagnostic output is a proposal. It becomes a plan only after controller
//! validation against the bounded failure packet and authority evidence.

#![allow(dead_code)]

use super::failure_packet::FailurePacket;
use super::repair_action::RepairAction;
use super::repair_authority::{
    AuthorityEvidence, RepairPlanRejection, build_repair_action_with_authority,
};
use super::repair_brief::{RepairBrief, RepairBriefSource};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct RepairPlanProposal {
    brief: RepairBrief,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct AcceptedRepairPlan {
    pub(super) action: RepairAction,
    pub(super) proposal_source: RepairBriefSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RepairPlanValidationError {
    Rejected(RepairPlanRejection),
}

impl RepairPlanProposal {
    pub(super) fn from_brief(brief: RepairBrief) -> Self {
        Self { brief }
    }

    pub(super) fn brief(&self) -> &RepairBrief {
        &self.brief
    }
}

impl RepairPlanValidationError {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Rejected(reason) => reason.as_str(),
        }
    }
}

pub(super) fn validate_repair_plan_proposal(
    proposal: &RepairPlanProposal,
    packet: &FailurePacket,
    evidence: &AuthorityEvidence,
) -> Result<AcceptedRepairPlan, RepairPlanValidationError> {
    let action = build_repair_action_with_authority(proposal.brief(), packet, evidence)
        .map_err(RepairPlanValidationError::Rejected)?;
    Ok(AcceptedRepairPlan {
        action,
        proposal_source: proposal.brief().source,
    })
}

#[cfg(test)]
mod tests {
    use super::super::failure_packet::{CandidateArtifact, FailurePacket, ObservedExpectedPair};
    use super::super::repair_brief::{
        AllowedChangeKind, RepairBrief, RepairBriefSource, RepairBriefTarget, SourceOfTruth,
    };
    use super::super::task_contract::ArtifactRole;
    use super::*;

    fn packet(role: ArtifactRole, path: &str) -> FailurePacket {
        FailurePacket::new(
            "pytest",
            "assertion_failure",
            "E       AssertionError: assert 201 == 200",
            Vec::new(),
            vec![ObservedExpectedPair::new("201", "200", "assert_equal")],
            vec![CandidateArtifact::new(role, path, "changed artifact")],
            Vec::new(),
        )
    }

    fn authority() -> AuthorityEvidence {
        AuthorityEvidence {
            user_request_has_explicit_spec: true,
            behavior_contract_present: false,
            observed_expected_pair_count: 1,
            candidate_artifact_count: 1,
        }
    }

    fn brief(
        role: ArtifactRole,
        path: &str,
        source_of_truth: SourceOfTruth,
        allowed_change_kind: AllowedChangeKind,
    ) -> RepairBrief {
        RepairBrief {
            failure_summary: "status mismatch".to_string(),
            root_cause: "implementation differs from expected behavior".to_string(),
            source_of_truth,
            repair_target: Some(RepairBriefTarget {
                role,
                path: path.to_string(),
            }),
            allowed_change_kind,
            must_preserve: Vec::new(),
            concrete_fix_intent: "align behavior".to_string(),
            confidence: 0.8,
            source: RepairBriefSource::DiagnosticLlm,
        }
    }

    #[test]
    fn proposal_becomes_accepted_plan_after_authority_validation() {
        let proposal = RepairPlanProposal::from_brief(brief(
            ArtifactRole::Implementation,
            "app/main.py",
            SourceOfTruth::UserRequest,
            AllowedChangeKind::FixImplementationBehavior,
        ));

        let accepted = validate_repair_plan_proposal(
            &proposal,
            &packet(ArtifactRole::Implementation, "app/main.py"),
            &authority(),
        )
        .expect("valid proposal should be accepted");

        assert_eq!(accepted.action.target_path, "app/main.py");
        assert_eq!(
            accepted.action.allowed_change_kind,
            AllowedChangeKind::FixImplementationBehavior
        );
        assert_eq!(accepted.proposal_source, RepairBriefSource::DiagnosticLlm);
    }

    #[test]
    fn ambiguous_source_of_truth_stays_proposal_only() {
        let proposal = RepairPlanProposal::from_brief(brief(
            ArtifactRole::Implementation,
            "app/main.py",
            SourceOfTruth::Ambiguous,
            AllowedChangeKind::InsufficientEvidence,
        ));

        assert_eq!(
            validate_repair_plan_proposal(
                &proposal,
                &packet(ArtifactRole::Implementation, "app/main.py"),
                &authority(),
            )
            .unwrap_err()
            .as_str(),
            "ambiguous_authority"
        );
    }

    #[test]
    fn generated_test_expectation_accepts_controller_derived_authority() {
        let proposal = RepairPlanProposal::from_brief(brief(
            ArtifactRole::Test,
            "tests/test_main.py",
            SourceOfTruth::ImplementationContract,
            AllowedChangeKind::FixGeneratedTestExpectation,
        ));

        let accepted = validate_repair_plan_proposal(
            &proposal,
            &packet(ArtifactRole::Test, "tests/test_main.py"),
            &authority(),
        )
        .expect("controller-derived implementation contract should authorize generated test fix");

        assert_eq!(accepted.action.target_role, ArtifactRole::Test);
    }
}
