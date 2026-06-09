//! Objective evidence runner selection for TaskContract recovery.
//!
//! This module projects typed objective/evidence facts into the next evidence
//! lifecycle stage. It does not inspect raw request text or artifact contents.

use super::task_contract::{
    ArtifactRecoveryAction, ArtifactRecoveryInputs, EvidenceSpec, ObjectiveContract,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ObjectiveEvidenceStage {
    MissingEvidence { runner: ObjectiveEvidenceRunner },
    SatisfiedOrNotRequired { runner: ObjectiveEvidenceRunner },
}

impl ObjectiveEvidenceStage {
    pub(super) fn into_recovery_action(
        self,
        missing_verifier_suppress_retry: bool,
    ) -> Option<ArtifactRecoveryAction> {
        match self {
            ObjectiveEvidenceStage::MissingEvidence { runner } => {
                runner.missing_recovery_action(missing_verifier_suppress_retry)
            }
            ObjectiveEvidenceStage::SatisfiedOrNotRequired { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ObjectiveEvidenceRunner {
    Command(EvidenceSpec),
    ArtifactAcceptance(EvidenceSpec),
    NotRequired,
}

impl ObjectiveEvidenceRunner {
    fn for_objective(objective: &ObjectiveContract) -> Self {
        if objective.requires_evidence() {
            Self::Command(objective.evidence_kind)
        } else if objective.has_required_deliverables() {
            Self::ArtifactAcceptance(objective.evidence_kind)
        } else {
            Self::NotRequired
        }
    }

    pub(super) fn command(evidence_kind: EvidenceSpec) -> Self {
        Self::Command(evidence_kind)
    }

    pub(super) fn missing_recovery_action(
        self,
        missing_verifier_suppress_retry: bool,
    ) -> Option<ArtifactRecoveryAction> {
        match self {
            ObjectiveEvidenceRunner::Command(_) => {
                // Once a MissingVerifierJob is in flight and no in-scope edit
                // has landed, ask for repair instead of re-triggering the same
                // missing-evidence loop.
                if missing_verifier_suppress_retry {
                    Some(ArtifactRecoveryAction::RepairArtifact { target_hint: None })
                } else {
                    Some(ArtifactRecoveryAction::RunVerifier)
                }
            }
            ObjectiveEvidenceRunner::ArtifactAcceptance(_)
            | ObjectiveEvidenceRunner::NotRequired => None,
        }
    }
}

pub(super) fn objective_evidence_stage(
    inputs: &ArtifactRecoveryInputs<'_>,
    objective_evidence_satisfied: bool,
    existing_unverified_used: bool,
    code_or_test_required: bool,
) -> ObjectiveEvidenceStage {
    let objective = inputs.contract.objective_contract();
    let default_runner = ObjectiveEvidenceRunner::for_objective(&objective);
    if objective_evidence_satisfied {
        return ObjectiveEvidenceStage::SatisfiedOrNotRequired {
            runner: default_runner,
        };
    }

    if objective.requires_evidence() {
        return ObjectiveEvidenceStage::MissingEvidence {
            runner: default_runner,
        };
    }

    if existing_unverified_used && code_or_test_required {
        return ObjectiveEvidenceStage::MissingEvidence {
            runner: ObjectiveEvidenceRunner::command(objective.evidence_kind),
        };
    }

    ObjectiveEvidenceStage::SatisfiedOrNotRequired {
        runner: default_runner,
    }
}
