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

#[cfg(test)]
mod tests {
    use super::super::completion_evidence::{CompletionEvidence, EvidenceSet, RepoEditCategory};
    use super::super::task_contract::{
        ArtifactExcerpts, ArtifactRecoveryAction, ArtifactRecoveryInputs, ArtifactRole,
        ArtifactState, ObjectiveEvidenceKind, TaskContract, VerifierRepairState,
    };
    use super::*;

    fn repo_edit_path(category: RepoEditCategory, path: &str) -> CompletionEvidence {
        CompletionEvidence::RepoEdit {
            category,
            count: 1,
            path: Some(path.to_string()),
        }
    }

    #[test]
    fn objective_evidence_stage_uses_objective_evidence_kind_for_missing_coding_evidence() {
        let contract = TaskContract::from_request(
            r#"STATE_CONTROL_PACKET
{"objective":"slugify library with passing evidence","next_required_action":"artifact","required_artifacts":[{"path":"Cargo.toml","role":"manifest"},{"path":"src/lib.rs","role":"source"}],"evidence_command":"cargo test --manifest-path Cargo.toml"}"#,
        );
        let mut evidence = EvidenceSet::new();
        evidence.push(repo_edit_path(RepoEditCategory::Setup, "Cargo.toml"));
        evidence.push(repo_edit_path(RepoEditCategory::Impl, "src/lib.rs"));
        let artifacts = vec![
            ArtifactState::exists(ArtifactRole::Setup, "Cargo.toml"),
            ArtifactState::exists(ArtifactRole::Implementation, "src/lib.rs"),
        ];
        let excerpts = ArtifactExcerpts::new();
        let repair_state = VerifierRepairState::None;
        let inputs = ArtifactRecoveryInputs {
            contract: &contract,
            evidence: &evidence,
            artifacts: &artifacts,
            repair_state: &repair_state,
            artifact_excerpts: &excerpts,
            missing_verifier_suppress_retry: false,
            owned_test_artifacts: &[],
        };

        let stage = objective_evidence_stage(&inputs, false, false, true);

        assert_eq!(
            stage,
            ObjectiveEvidenceStage::MissingEvidence {
                runner: ObjectiveEvidenceRunner::Command(ObjectiveEvidenceKind::TestRun)
            }
        );
        assert_eq!(
            stage.clone().into_recovery_action(false),
            Some(ArtifactRecoveryAction::RunVerifier)
        );
        assert_eq!(
            stage.into_recovery_action(true),
            Some(ArtifactRecoveryAction::RepairArtifact { target_hint: None })
        );
    }

    #[test]
    fn objective_evidence_stage_uses_artifact_acceptance_runner_for_docs_and_data() {
        let cases = [
            (
                "Update README.md with installation and usage sections.",
                ObjectiveEvidenceKind::ContentCheck,
            ),
            (
                "Generate output.csv with columns Category and Total.",
                ObjectiveEvidenceKind::SchemaCheck,
            ),
        ];

        for (request, evidence_kind) in cases {
            let contract = TaskContract::from_request(request);
            assert_eq!(contract.objective_contract().evidence_kind, evidence_kind);
            let evidence = EvidenceSet::new();
            let excerpts = ArtifactExcerpts::new();
            let repair_state = VerifierRepairState::None;
            let inputs = ArtifactRecoveryInputs {
                contract: &contract,
                evidence: &evidence,
                artifacts: &[],
                repair_state: &repair_state,
                artifact_excerpts: &excerpts,
                missing_verifier_suppress_retry: false,
                owned_test_artifacts: &[],
            };

            assert_eq!(
                objective_evidence_stage(&inputs, false, false, false),
                ObjectiveEvidenceStage::SatisfiedOrNotRequired {
                    runner: ObjectiveEvidenceRunner::ArtifactAcceptance(evidence_kind)
                },
                "request={request}"
            );
        }
    }

    #[test]
    fn objective_evidence_runner_is_not_required_for_answer_only() {
        let contract = TaskContract::from_request("Explain Rust ownership in one paragraph.");
        let evidence = EvidenceSet::new();
        let excerpts = ArtifactExcerpts::new();
        let repair_state = VerifierRepairState::None;
        let inputs = ArtifactRecoveryInputs {
            contract: &contract,
            evidence: &evidence,
            artifacts: &[],
            repair_state: &repair_state,
            artifact_excerpts: &excerpts,
            missing_verifier_suppress_retry: false,
            owned_test_artifacts: &[],
        };

        assert_eq!(
            objective_evidence_stage(&inputs, false, false, false),
            ObjectiveEvidenceStage::SatisfiedOrNotRequired {
                runner: ObjectiveEvidenceRunner::NotRequired
            }
        );
    }

    #[test]
    fn objective_evidence_runner_action_only_commands_invoke_legacy_verifier() {
        assert_eq!(
            ObjectiveEvidenceRunner::Command(ObjectiveEvidenceKind::TestRun)
                .missing_recovery_action(false),
            Some(ArtifactRecoveryAction::RunVerifier)
        );
        assert_eq!(
            ObjectiveEvidenceRunner::Command(ObjectiveEvidenceKind::TestRun)
                .missing_recovery_action(true),
            Some(ArtifactRecoveryAction::RepairArtifact { target_hint: None })
        );
        assert_eq!(
            ObjectiveEvidenceRunner::ArtifactAcceptance(ObjectiveEvidenceKind::ContentCheck)
                .missing_recovery_action(false),
            None
        );
        assert_eq!(
            ObjectiveEvidenceRunner::NotRequired.missing_recovery_action(false),
            None
        );
    }
}
