//! v0.4.13 Phase 4: controller-owned repair action.
//!
//! A `RepairBrief` is an LLM-proposed semantic diagnosis. `RepairAction` is
//! the controller-admitted next step after path, role, source-of-truth, and
//! budget checks.

#![allow(dead_code)]

use super::failure_packet::FailurePacket;
use super::repair_brief::{AllowedChangeKind, RepairBrief, SourceOfTruth};
use super::task_contract::ArtifactRole;

const DEFAULT_ACTION_BUDGET: u8 = 2;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct RepairAction {
    pub(super) target_role: ArtifactRole,
    pub(super) target_path: String,
    pub(super) allowed_change_kind: AllowedChangeKind,
    pub(super) source_of_truth: SourceOfTruth,
    pub(super) budget: u8,
    pub(super) brief_confidence: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RepairActionRejection {
    MissingTarget,
    InsufficientEvidence,
    PathNotCandidate,
    RoleMismatch,
    TestExpectationBlockedByUserRequest,
    AmbiguousSpec,
    TestExpectationWithoutAuthority,
}

impl RepairActionRejection {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::MissingTarget => "missing_target",
            Self::InsufficientEvidence => "insufficient_evidence",
            Self::PathNotCandidate => "path_not_candidate",
            Self::RoleMismatch => "role_mismatch",
            Self::TestExpectationBlockedByUserRequest => "test_expectation_blocked_by_user_request",
            Self::AmbiguousSpec => "ambiguous_spec",
            Self::TestExpectationWithoutAuthority => "test_expectation_without_authority",
        }
    }
}

pub(super) fn build_repair_action(
    brief: &RepairBrief,
    packet: &FailurePacket,
) -> Result<RepairAction, RepairActionRejection> {
    if brief.allowed_change_kind == AllowedChangeKind::InsufficientEvidence {
        return Err(RepairActionRejection::InsufficientEvidence);
    }
    if matches!(brief.source_of_truth, SourceOfTruth::Ambiguous) {
        return Err(RepairActionRejection::AmbiguousSpec);
    }
    let target = brief
        .repair_target
        .as_ref()
        .ok_or(RepairActionRejection::MissingTarget)?;
    if !packet.has_candidate_path(&target.path) {
        return Err(RepairActionRejection::PathNotCandidate);
    }
    let packet_role = packet
        .candidate_role_for_path(&target.path)
        .ok_or(RepairActionRejection::PathNotCandidate)?;
    if packet_role != target.role {
        return Err(RepairActionRejection::RoleMismatch);
    }
    if !allowed_change_kind_allows_target_role(brief.allowed_change_kind, target.role) {
        return Err(RepairActionRejection::RoleMismatch);
    }
    if brief.allowed_change_kind == AllowedChangeKind::FixGeneratedTestExpectation
        && brief.source_of_truth == SourceOfTruth::UserRequest
    {
        return Err(RepairActionRejection::TestExpectationBlockedByUserRequest);
    }
    if brief.allowed_change_kind == AllowedChangeKind::FixGeneratedTestExpectation
        && matches!(
            brief.source_of_truth,
            SourceOfTruth::Unknown | SourceOfTruth::Ambiguous
        )
    {
        return Err(RepairActionRejection::TestExpectationWithoutAuthority);
    }

    Ok(RepairAction {
        target_role: target.role,
        target_path: target.path.clone(),
        allowed_change_kind: brief.allowed_change_kind,
        source_of_truth: brief.source_of_truth,
        budget: DEFAULT_ACTION_BUDGET,
        brief_confidence: brief.confidence,
    })
}

/// Whether `kind` is permitted to target an artifact of `role`.
///
/// Issue #920: this is a predicate (equality / `matches!`), not an exhaustive
/// `match` over `ArtifactRole`, so a *new* role does NOT compile-error here —
/// it silently fails CLOSED (no change kind matches it ⇒ `false`). That is the
/// intentional safe default: an unrecognised role is denied as a repair target
/// until an explicit arm grants it. `data_output` is already in this state and
/// the `data_output_role_is_denied_by_every_change_kind` regression test pins it.
pub(super) fn allowed_change_kind_allows_target_role(
    kind: AllowedChangeKind,
    role: ArtifactRole,
) -> bool {
    match kind {
        AllowedChangeKind::FixImplementationBehavior => role == ArtifactRole::Implementation,
        AllowedChangeKind::FixGeneratedTestExpectation | AllowedChangeKind::FixTestIsolation => {
            role == ArtifactRole::Test
        }
        AllowedChangeKind::FixTestImportOrSetup => {
            matches!(role, ArtifactRole::Test | ArtifactRole::Setup)
        }
        AllowedChangeKind::ConnectExistingTestSetupToSut => matches!(
            role,
            ArtifactRole::Implementation | ArtifactRole::Test | ArtifactRole::Setup
        ),
        AllowedChangeKind::FixDependencyOrConfig | AllowedChangeKind::FixVerifierCommand => {
            role == ArtifactRole::Setup
        }
        AllowedChangeKind::InsufficientEvidence => false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::failure_packet::{CandidateArtifact, FailurePacket};
    use super::super::repair_brief::{
        AllowedChangeKind, RepairBrief, RepairBriefSource, RepairBriefTarget, SourceOfTruth,
    };
    use super::super::task_contract::ArtifactRole;
    use super::*;

    // Issue #920 (AC: repair role-policy fail-closed is intentional and tested):
    // no `AllowedChangeKind` targets `DataOutput` today, and a future role would
    // likewise fall through to `false`. This pins that silent fail-closed as the
    // deliberate safe default (an unrecognised role is never an allowed target).
    #[test]
    fn data_output_role_is_denied_by_every_change_kind() {
        let kinds = [
            AllowedChangeKind::FixImplementationBehavior,
            AllowedChangeKind::FixGeneratedTestExpectation,
            AllowedChangeKind::FixTestIsolation,
            AllowedChangeKind::FixTestImportOrSetup,
            AllowedChangeKind::ConnectExistingTestSetupToSut,
            AllowedChangeKind::FixDependencyOrConfig,
            AllowedChangeKind::FixVerifierCommand,
            AllowedChangeKind::InsufficientEvidence,
        ];
        for kind in kinds {
            assert!(
                !allowed_change_kind_allows_target_role(kind, ArtifactRole::DataOutput),
                "DataOutput must be denied as a repair target by {kind:?} (intentional fail-closed)"
            );
        }
    }

    fn packet() -> FailurePacket {
        FailurePacket::new(
            "pytest",
            "assertion_failure",
            "failed",
            Vec::new(),
            Vec::new(),
            vec![CandidateArtifact::new(
                ArtifactRole::Implementation,
                "app/main.py",
                "changed implementation",
            )],
            Vec::new(),
        )
    }

    fn brief() -> RepairBrief {
        RepairBrief {
            failure_summary: "summary".to_string(),
            root_cause: "cause".to_string(),
            source_of_truth: SourceOfTruth::UserRequest,
            repair_target: Some(RepairBriefTarget {
                role: ArtifactRole::Implementation,
                path: "app/main.py".to_string(),
            }),
            allowed_change_kind: AllowedChangeKind::FixImplementationBehavior,
            must_preserve: Vec::new(),
            concrete_fix_intent: "fix".to_string(),
            confidence: 0.9,
            source: RepairBriefSource::DiagnosticLlm,
        }
    }

    #[test]
    fn repair_action_admits_candidate_path_and_role() {
        let action = build_repair_action(&brief(), &packet()).unwrap();

        assert_eq!(action.target_path, "app/main.py");
        assert_eq!(action.target_role, ArtifactRole::Implementation);
        assert_eq!(
            action.allowed_change_kind,
            AllowedChangeKind::FixImplementationBehavior
        );
    }

    #[test]
    fn repair_action_rejects_non_candidate_path() {
        let mut brief = brief();
        brief.repair_target.as_mut().unwrap().path = "other.py".to_string();

        assert_eq!(
            build_repair_action(&brief, &packet()),
            Err(RepairActionRejection::PathNotCandidate)
        );
    }

    #[test]
    fn repair_action_rejects_change_kind_target_role_mismatch() {
        let test_packet = FailurePacket::new(
            "pytest",
            "assertion_failure",
            "failed",
            Vec::new(),
            Vec::new(),
            vec![CandidateArtifact::new(
                ArtifactRole::Test,
                "tests/test_main.py",
                "changed test",
            )],
            Vec::new(),
        );
        let mut brief = brief();
        brief.repair_target = Some(RepairBriefTarget {
            role: ArtifactRole::Test,
            path: "tests/test_main.py".to_string(),
        });
        brief.allowed_change_kind = AllowedChangeKind::FixImplementationBehavior;

        assert_eq!(
            build_repair_action(&brief, &test_packet),
            Err(RepairActionRejection::RoleMismatch)
        );
    }

    #[test]
    fn repair_action_blocks_test_expectation_under_user_request() {
        let test_packet = FailurePacket::new(
            "pytest",
            "assertion_failure",
            "failed",
            Vec::new(),
            Vec::new(),
            vec![CandidateArtifact::new(
                ArtifactRole::Test,
                "tests/test_main.py",
                "changed test",
            )],
            Vec::new(),
        );
        let mut brief = brief();
        brief.repair_target = Some(RepairBriefTarget {
            role: ArtifactRole::Test,
            path: "tests/test_main.py".to_string(),
        });
        brief.allowed_change_kind = AllowedChangeKind::FixGeneratedTestExpectation;

        assert_eq!(
            build_repair_action(&brief, &test_packet),
            Err(RepairActionRejection::TestExpectationBlockedByUserRequest)
        );
    }

    #[test]
    fn repair_action_rejects_ambiguous_spec() {
        let mut brief = brief();
        brief.source_of_truth = SourceOfTruth::Ambiguous;

        assert_eq!(
            build_repair_action(&brief, &packet()),
            Err(RepairActionRejection::AmbiguousSpec)
        );
    }

    #[test]
    fn repair_action_rejects_test_expectation_without_authority() {
        let test_packet = FailurePacket::new(
            "pytest",
            "assertion_failure",
            "failed",
            Vec::new(),
            Vec::new(),
            vec![CandidateArtifact::new(
                ArtifactRole::Test,
                "tests/test_main.py",
                "changed test",
            )],
            Vec::new(),
        );
        let mut brief = brief();
        brief.source_of_truth = SourceOfTruth::Unknown;
        brief.repair_target = Some(RepairBriefTarget {
            role: ArtifactRole::Test,
            path: "tests/test_main.py".to_string(),
        });
        brief.allowed_change_kind = AllowedChangeKind::FixGeneratedTestExpectation;

        assert_eq!(
            build_repair_action(&brief, &test_packet),
            Err(RepairActionRejection::TestExpectationWithoutAuthority)
        );
    }
}
