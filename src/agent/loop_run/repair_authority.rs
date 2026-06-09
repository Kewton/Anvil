//! v0.4.15 MVP: authority evidence boundary for verifier repair.
//!
//! Diagnostic LLM output is useful as a proposal, but it is not authority.
//! This module keeps the first production boundary deliberately small:
//! gather bounded provenance signals, validate a `RepairBrief`, and only
//! admit it as a controller `RepairAction` when the proposal is consistent
//! with those signals and the existing path/role safety checks.

#![allow(dead_code)]

use super::failure_packet::FailurePacket;
use super::repair_action::{RepairAction, RepairActionRejection, build_repair_action};
use super::repair_brief::{AllowedChangeKind, RepairBrief, SourceOfTruth};
use super::task_contract::ArtifactRole;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AuthorityEvidence {
    pub(super) user_request_has_explicit_spec: bool,
    pub(super) behavior_contract_present: bool,
    pub(super) observed_expected_pair_count: usize,
    pub(super) candidate_artifact_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RepairPlanRejection {
    Action(RepairActionRejection),
    AmbiguousAuthority,
    TestExpectationWithoutAuthority,
    TestExpectationContradictsAuthority,
}

impl AuthorityEvidence {
    pub(super) fn from_packet_and_context(
        packet: &FailurePacket,
        active_request: &str,
        behavior_contract_present: bool,
    ) -> Self {
        Self {
            user_request_has_explicit_spec:
                super::spec_authority::detect_explicit_spec_in_user_request(active_request),
            behavior_contract_present,
            observed_expected_pair_count: packet.observed_expected_pairs.len(),
            candidate_artifact_count: packet.candidate_artifacts.len(),
        }
    }

    pub(super) fn has_explicit_spec_authority(&self) -> bool {
        self.user_request_has_explicit_spec || self.behavior_contract_present
    }

    pub(super) fn to_json_value(&self) -> serde_json::Value {
        serde_json::json!({
            "user_request_has_explicit_spec": self.user_request_has_explicit_spec,
            "behavior_contract_present": self.behavior_contract_present,
            "observed_expected_pair_count": self.observed_expected_pair_count,
            "candidate_artifact_count": self.candidate_artifact_count,
            "verifier_observation_is_authority": false,
        })
    }
}

impl RepairPlanRejection {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Action(reason) => reason.as_str(),
            Self::AmbiguousAuthority => "ambiguous_authority",
            Self::TestExpectationWithoutAuthority => "test_expectation_without_authority",
            Self::TestExpectationContradictsAuthority => "test_expectation_contradicts_authority",
        }
    }
}

pub(super) fn build_repair_action_with_authority(
    brief: &RepairBrief,
    packet: &FailurePacket,
    evidence: &AuthorityEvidence,
) -> Result<RepairAction, RepairPlanRejection> {
    let admitted_brief = controller_authorized_generated_test_brief(brief, evidence);
    validate_authority_consistency(&admitted_brief, packet, evidence)?;
    build_repair_action(&admitted_brief, packet).map_err(RepairPlanRejection::Action)
}

fn controller_authorized_generated_test_brief(
    brief: &RepairBrief,
    evidence: &AuthorityEvidence,
) -> RepairBrief {
    if brief.allowed_change_kind != AllowedChangeKind::FixGeneratedTestExpectation
        || evidence.observed_expected_pair_count == 0
        || !evidence.has_explicit_spec_authority()
        || !matches!(
            brief.source_of_truth,
            SourceOfTruth::Unknown | SourceOfTruth::LlmGeneratedTest
        )
    {
        return brief.clone();
    }
    let mut admitted = brief.clone();
    admitted.source_of_truth = if evidence.user_request_has_explicit_spec {
        SourceOfTruth::UserRequest
    } else {
        SourceOfTruth::BehaviorContract
    };
    admitted
}

fn validate_authority_consistency(
    brief: &RepairBrief,
    packet: &FailurePacket,
    evidence: &AuthorityEvidence,
) -> Result<(), RepairPlanRejection> {
    validate_claimed_authority_source(brief, evidence)?;
    validate_dependency_or_config_target(brief)?;
    validate_generated_test_expectation_authority(brief)?;
    validate_observed_value_implementation_authority(brief, packet, evidence)?;

    Ok(())
}

fn validate_claimed_authority_source(
    brief: &RepairBrief,
    evidence: &AuthorityEvidence,
) -> Result<(), RepairPlanRejection> {
    if matches!(brief.source_of_truth, SourceOfTruth::Ambiguous)
        || brief.allowed_change_kind == AllowedChangeKind::InsufficientEvidence
    {
        return Err(RepairPlanRejection::AmbiguousAuthority);
    }
    if brief.source_of_truth == SourceOfTruth::UserRequest
        && !evidence.user_request_has_explicit_spec
    {
        return Err(RepairPlanRejection::AmbiguousAuthority);
    }
    if brief.source_of_truth == SourceOfTruth::BehaviorContract
        && !evidence.behavior_contract_present
    {
        return Err(RepairPlanRejection::AmbiguousAuthority);
    }

    Ok(())
}

fn validate_dependency_or_config_target(brief: &RepairBrief) -> Result<(), RepairPlanRejection> {
    if brief.allowed_change_kind == AllowedChangeKind::FixDependencyOrConfig
        && let Some(target) = brief.repair_target.as_ref()
        && target.role != ArtifactRole::Setup
    {
        return Err(RepairPlanRejection::Action(
            RepairActionRejection::RoleMismatch,
        ));
    }

    Ok(())
}

fn validate_generated_test_expectation_authority(
    brief: &RepairBrief,
) -> Result<(), RepairPlanRejection> {
    if brief.allowed_change_kind == AllowedChangeKind::FixGeneratedTestExpectation
        && matches!(
            brief.source_of_truth,
            SourceOfTruth::Unknown | SourceOfTruth::Ambiguous | SourceOfTruth::LlmGeneratedTest
        )
    {
        return Err(RepairPlanRejection::TestExpectationWithoutAuthority);
    }

    Ok(())
}

fn validate_observed_value_implementation_authority(
    brief: &RepairBrief,
    packet: &FailurePacket,
    evidence: &AuthorityEvidence,
) -> Result<(), RepairPlanRejection> {
    // Exact observed/expected assertion pairs come from verifier output, not
    // from the user's specification. For implementation edits that try to
    // satisfy such values, require an external authority signal; otherwise a
    // generated test literal can drag implementation behavior toward a
    // hallucinated or typo-like expectation.
    if !packet.observed_expected_pairs.is_empty()
        && brief.allowed_change_kind == AllowedChangeKind::FixImplementationBehavior
        && !evidence.has_explicit_spec_authority()
        && !matches!(
            brief.source_of_truth,
            SourceOfTruth::UserRequest
                | SourceOfTruth::BehaviorContract
                | SourceOfTruth::VerifiedPublicInterface
                | SourceOfTruth::UsageDocs
        )
    {
        return Err(RepairPlanRejection::AmbiguousAuthority);
    }

    Ok(())
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

    fn implementation_brief(path: &str) -> RepairBrief {
        RepairBrief {
            failure_summary: "status mismatch".to_string(),
            root_cause: "implementation returned a different status".to_string(),
            source_of_truth: SourceOfTruth::BehaviorContract,
            repair_target: Some(RepairBriefTarget {
                role: ArtifactRole::Implementation,
                path: path.to_string(),
            }),
            allowed_change_kind: AllowedChangeKind::FixImplementationBehavior,
            must_preserve: Vec::new(),
            concrete_fix_intent: "align implementation behavior".to_string(),
            confidence: 0.8,
            source: RepairBriefSource::DiagnosticLlm,
        }
    }

    fn test_brief(path: &str, source_of_truth: SourceOfTruth) -> RepairBrief {
        RepairBrief {
            failure_summary: "status mismatch".to_string(),
            root_cause: "generated test expectation is stale".to_string(),
            source_of_truth,
            repair_target: Some(RepairBriefTarget {
                role: ArtifactRole::Test,
                path: path.to_string(),
            }),
            allowed_change_kind: AllowedChangeKind::FixGeneratedTestExpectation,
            must_preserve: Vec::new(),
            concrete_fix_intent: "align generated expectation".to_string(),
            confidence: 0.8,
            source: RepairBriefSource::DiagnosticLlm,
        }
    }

    #[test]
    fn authority_evidence_allows_implementation_fix_with_behavior_contract() {
        let packet = packet(ArtifactRole::Implementation, "app/main.py");
        let evidence = AuthorityEvidence {
            user_request_has_explicit_spec: false,
            behavior_contract_present: true,
            observed_expected_pair_count: 1,
            candidate_artifact_count: 1,
        };

        let action = build_repair_action_with_authority(
            &implementation_brief("app/main.py"),
            &packet,
            &evidence,
        )
        .unwrap();

        assert_eq!(action.target_path, "app/main.py");
        assert_eq!(
            action.allowed_change_kind,
            AllowedChangeKind::FixImplementationBehavior
        );
    }

    #[test]
    fn behavior_contract_self_claim_requires_controller_evidence() {
        let packet = packet(ArtifactRole::Implementation, "app/main.py");
        let evidence = AuthorityEvidence {
            user_request_has_explicit_spec: false,
            behavior_contract_present: false,
            observed_expected_pair_count: 1,
            candidate_artifact_count: 1,
        };

        assert_eq!(
            build_repair_action_with_authority(
                &implementation_brief("app/main.py"),
                &packet,
                &evidence,
            ),
            Err(RepairPlanRejection::AmbiguousAuthority)
        );
    }

    #[test]
    fn generated_assertion_literal_cannot_drive_unknown_implementation_repair() {
        let packet = packet(ArtifactRole::Implementation, "app/main.py");
        let evidence = AuthorityEvidence {
            user_request_has_explicit_spec: false,
            behavior_contract_present: false,
            observed_expected_pair_count: 1,
            candidate_artifact_count: 1,
        };
        let mut brief = implementation_brief("app/main.py");
        brief.source_of_truth = SourceOfTruth::Unknown;

        assert_eq!(
            build_repair_action_with_authority(&brief, &packet, &evidence),
            Err(RepairPlanRejection::AmbiguousAuthority)
        );
    }

    #[test]
    fn implementation_contract_can_authorize_generated_test_expectation_fix() {
        let packet = packet(ArtifactRole::Test, "tests/test_main.py");
        let evidence = AuthorityEvidence {
            user_request_has_explicit_spec: false,
            behavior_contract_present: false,
            observed_expected_pair_count: 1,
            candidate_artifact_count: 1,
        };

        let action = build_repair_action_with_authority(
            &test_brief("tests/test_main.py", SourceOfTruth::ImplementationContract),
            &packet,
            &evidence,
        )
        .unwrap();

        assert_eq!(action.target_role, ArtifactRole::Test);
        assert_eq!(
            action.allowed_change_kind,
            AllowedChangeKind::FixGeneratedTestExpectation
        );
    }

    #[test]
    fn behavior_contract_can_authorize_generated_test_expectation_fix() {
        let packet = packet(ArtifactRole::Test, "tests/test_main.py");
        let evidence = AuthorityEvidence {
            user_request_has_explicit_spec: false,
            behavior_contract_present: true,
            observed_expected_pair_count: 1,
            candidate_artifact_count: 1,
        };

        let action = build_repair_action_with_authority(
            &test_brief("tests/test_main.py", SourceOfTruth::BehaviorContract),
            &packet,
            &evidence,
        )
        .unwrap();

        assert_eq!(action.target_role, ArtifactRole::Test);
    }

    #[test]
    fn controller_user_request_evidence_authorizes_unknown_generated_test_expectation_fix() {
        let packet = packet(ArtifactRole::Test, "tests/test_main.py");
        let evidence = AuthorityEvidence {
            user_request_has_explicit_spec: true,
            behavior_contract_present: false,
            observed_expected_pair_count: 1,
            candidate_artifact_count: 1,
        };

        let action = build_repair_action_with_authority(
            &test_brief("tests/test_main.py", SourceOfTruth::Unknown),
            &packet,
            &evidence,
        )
        .unwrap();

        assert_eq!(action.target_role, ArtifactRole::Test);
        assert_eq!(
            action.allowed_change_kind,
            AllowedChangeKind::FixGeneratedTestExpectation
        );
    }

    #[test]
    fn controller_behavior_contract_evidence_authorizes_llm_generated_test_expectation_fix() {
        let packet = packet(ArtifactRole::Test, "tests/test_main.py");
        let evidence = AuthorityEvidence {
            user_request_has_explicit_spec: false,
            behavior_contract_present: true,
            observed_expected_pair_count: 1,
            candidate_artifact_count: 1,
        };

        let action = build_repair_action_with_authority(
            &test_brief("tests/test_main.py", SourceOfTruth::LlmGeneratedTest),
            &packet,
            &evidence,
        )
        .unwrap();

        assert_eq!(action.target_role, ArtifactRole::Test);
    }

    #[test]
    fn controller_evidence_does_not_authorize_ambiguous_generated_test_expectation_fix() {
        let packet = packet(ArtifactRole::Test, "tests/test_main.py");
        let evidence = AuthorityEvidence {
            user_request_has_explicit_spec: true,
            behavior_contract_present: true,
            observed_expected_pair_count: 1,
            candidate_artifact_count: 1,
        };

        assert_eq!(
            build_repair_action_with_authority(
                &test_brief("tests/test_main.py", SourceOfTruth::Ambiguous),
                &packet,
                &evidence,
            ),
            Err(RepairPlanRejection::AmbiguousAuthority)
        );
    }

    #[test]
    fn controller_evidence_requires_observed_expected_pairs_for_unknown_generated_test_fix() {
        let packet = FailurePacket::new(
            "pytest",
            "assertion_failure",
            "assertion failed without structured pair",
            Vec::new(),
            Vec::new(),
            vec![CandidateArtifact::new(
                ArtifactRole::Test,
                "tests/test_main.py",
                "changed test",
            )],
            Vec::new(),
        );
        let evidence = AuthorityEvidence {
            user_request_has_explicit_spec: true,
            behavior_contract_present: false,
            observed_expected_pair_count: 0,
            candidate_artifact_count: 1,
        };

        assert_eq!(
            build_repair_action_with_authority(
                &test_brief("tests/test_main.py", SourceOfTruth::Unknown),
                &packet,
                &evidence,
            ),
            Err(RepairPlanRejection::TestExpectationWithoutAuthority)
        );
    }

    #[test]
    fn unknown_source_cannot_authorize_generated_test_expectation_fix() {
        let packet = packet(ArtifactRole::Test, "tests/test_main.py");
        let evidence =
            AuthorityEvidence::from_packet_and_context(&packet, "verifier says edit tests", false);

        assert!(!evidence.has_explicit_spec_authority());
        assert_eq!(
            build_repair_action_with_authority(
                &test_brief("tests/test_main.py", SourceOfTruth::Unknown),
                &packet,
                &evidence,
            ),
            Err(RepairPlanRejection::TestExpectationWithoutAuthority)
        );
    }

    #[test]
    fn path_and_role_safety_still_come_from_repair_action_validator() {
        let packet = packet(ArtifactRole::Implementation, "app/main.py");
        let evidence = AuthorityEvidence {
            user_request_has_explicit_spec: false,
            behavior_contract_present: true,
            observed_expected_pair_count: 1,
            candidate_artifact_count: 1,
        };

        assert_eq!(
            build_repair_action_with_authority(
                &implementation_brief("other.py"),
                &packet,
                &evidence,
            ),
            Err(RepairPlanRejection::Action(
                RepairActionRejection::PathNotCandidate
            ))
        );
    }

    #[test]
    fn dependency_or_config_repair_cannot_target_usage_docs() {
        let packet = FailurePacket::new(
            "python3 -m pytest",
            "dependency_missing",
            "No module named 'pytest'",
            Vec::new(),
            Vec::new(),
            vec![CandidateArtifact::new(
                ArtifactRole::UsageDocs,
                "README.md",
                "changed docs",
            )],
            Vec::new(),
        );
        let evidence = AuthorityEvidence::from_packet_and_context(&packet, "", false);
        let mut brief = test_brief("README.md", SourceOfTruth::UsageDocs);
        brief.repair_target.as_mut().unwrap().role = ArtifactRole::UsageDocs;
        brief.allowed_change_kind = AllowedChangeKind::FixDependencyOrConfig;

        assert_eq!(
            build_repair_action_with_authority(&brief, &packet, &evidence),
            Err(RepairPlanRejection::Action(
                RepairActionRejection::RoleMismatch
            ))
        );
    }
}
