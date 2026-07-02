//! Objective-level evidence satisfaction and binding.

use super::completion_evidence::{CompletionEvidence, EvidenceSet};
use super::task_contract::{
    ArtifactRole, ObjectiveContract, ObjectiveEvidenceKind, TaskContract, role_from_repo_edit,
};
use crate::tools::bash::BashCommandClass;

pub(super) fn objective_evidence_satisfied_for_contract(
    evidence: &EvidenceSet,
    contract: &TaskContract,
) -> bool {
    objective_evidence_satisfied(evidence, &contract.objective_contract())
}

pub(super) fn command_observation_evidence_collected_for_contract(
    evidence: &EvidenceSet,
    contract: &TaskContract,
) -> bool {
    let objective = contract.objective_contract();
    objective.evidence_kind == ObjectiveEvidenceKind::SafetyBoundaryEvidence
        && successful_command_observation_requirements_satisfied(evidence, &objective)
}

pub(super) fn objective_evidence_satisfied(
    evidence: &EvidenceSet,
    objective: &ObjectiveContract,
) -> bool {
    match objective.evidence_kind {
        ObjectiveEvidenceKind::TestRun => has_build_test_verifier(evidence),
        ObjectiveEvidenceKind::SafetyBoundaryEvidence => {
            successful_command_observation_requirements_satisfied(evidence, objective)
                && command_observation_deliverable_bound(evidence, objective)
        }
        ObjectiveEvidenceKind::ContentCheck | ObjectiveEvidenceKind::ContentAcceptance => {
            evidence.iter().any(|item| {
                matches!(
                    item,
                    CompletionEvidence::RequiredSectionsPass { .. }
                        | CompletionEvidence::ReportCompletenessPass { .. }
                        | CompletionEvidence::AnswerOnly
                )
            })
        }
        ObjectiveEvidenceKind::SchemaCheck => evidence
            .iter()
            .any(|item| matches!(item, CompletionEvidence::StructuredDataPass { .. })),
        ObjectiveEvidenceKind::SourceFetchEvidence => evidence
            .iter()
            .any(|item| matches!(item, CompletionEvidence::ReportCompletenessPass { .. })),
        ObjectiveEvidenceKind::FileLayoutCheck => false,
    }
}

fn has_build_test_verifier(evidence: &EvidenceSet) -> bool {
    evidence.iter().any(|item| {
        matches!(
            item,
            CompletionEvidence::VerifierExitZero {
                class: BashCommandClass::BuildTest,
                ..
            }
        )
    })
}

fn has_successful_command_observation(evidence: &EvidenceSet) -> bool {
    evidence.iter().any(|item| {
        matches!(
            item,
            CompletionEvidence::CommandObservation {
                exit_status: 0,
                safety_boundary_passed: true,
                ..
            }
        )
    })
}

fn has_successful_command_observation_for(evidence: &EvidenceSet, required: &str) -> bool {
    evidence.iter().any(|item| {
        let CompletionEvidence::CommandObservation {
            command,
            exit_status: 0,
            safety_boundary_passed: true,
        } = item
        else {
            return false;
        };
        command_observation_matches_required(command, required)
    })
}

fn command_observation_matches_required(actual: &str, required: &str) -> bool {
    let required_tokens = required.split_whitespace().collect::<Vec<_>>();
    if required_tokens.is_empty() {
        return false;
    }
    let actual_tokens = actual.split_whitespace().collect::<Vec<_>>();
    if actual_tokens.is_empty() {
        return false;
    }
    actual_tokens == required_tokens
        || (required_tokens.len() == 1
            && observed_command_heads(actual)
                .iter()
                .any(|head| *head == required_tokens[0]))
        || observed_command_segments(actual)
            .iter()
            .any(|segment| shell_wrapper_segment_runs_required(segment, &required_tokens))
}

fn observed_command_heads(command: &str) -> Vec<&str> {
    observed_command_segments(command)
        .into_iter()
        .filter_map(|segment| {
            let head = segment.split_whitespace().next()?;
            (!head.is_empty()).then_some(head)
        })
        .collect()
}

fn observed_command_segments(command: &str) -> Vec<&str> {
    command
        .split(['\n', ';'])
        .flat_map(|line| line.split("&&"))
        .flat_map(|segment| segment.split("||"))
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn shell_wrapper_segment_runs_required(segment: &str, required_tokens: &[&str]) -> bool {
    let tokens = segment.split_whitespace().collect::<Vec<_>>();
    let Some(wrapper) = tokens.first().copied() else {
        return false;
    };
    if !matches!(wrapper, "bash" | "sh" | "zsh") || tokens.len() <= required_tokens.len() {
        return false;
    }
    tokens[1..]
        .windows(required_tokens.len())
        .any(|window| window == required_tokens)
}

fn successful_command_observation_requirements_satisfied(
    evidence: &EvidenceSet,
    objective: &ObjectiveContract,
) -> bool {
    if objective.required_evidence_commands.is_empty() {
        has_successful_command_observation(evidence)
    } else {
        objective
            .required_evidence_commands
            .iter()
            .all(|command| has_successful_command_observation_for(evidence, command))
    }
}

fn command_observation_deliverable_bound(
    evidence: &EvidenceSet,
    objective: &ObjectiveContract,
) -> bool {
    if objective.required_deliverables.is_empty() {
        return true;
    }
    let Some(last_command_index) = last_successful_command_observation_index(evidence, objective)
    else {
        return false;
    };
    evidence
        .iter()
        .enumerate()
        .skip(last_command_index.saturating_add(1))
        .any(|(_, item)| {
            evidence_item_satisfies_observed_deliverable(item, objective.required_deliverables())
        })
        || command_observation_validates_prior_deliverable(evidence, objective, last_command_index)
}

fn command_observation_validates_prior_deliverable(
    evidence: &EvidenceSet,
    objective: &ObjectiveContract,
    command_index: usize,
) -> bool {
    if !objective
        .required_deliverables()
        .contains(&ArtifactRole::DataOutput)
    {
        return false;
    }
    evidence.iter().take(command_index).any(|item| {
        evidence_item_satisfies_observed_deliverable(item, objective.required_deliverables())
    })
}

fn last_successful_command_observation_index(
    evidence: &EvidenceSet,
    objective: &ObjectiveContract,
) -> Option<usize> {
    let mut last_index = None;
    for (index, item) in evidence.iter().enumerate() {
        let CompletionEvidence::CommandObservation {
            command,
            exit_status: 0,
            safety_boundary_passed: true,
        } = item
        else {
            continue;
        };
        if objective.required_evidence_commands.is_empty()
            || objective
                .required_evidence_commands
                .iter()
                .any(|required| command_observation_matches_required(command.as_str(), required))
        {
            last_index = Some(index);
        }
    }
    last_index
}

fn evidence_item_satisfies_observed_deliverable(
    evidence: &CompletionEvidence,
    required_deliverables: &[ArtifactRole],
) -> bool {
    let Some(role) = observed_deliverable_role(evidence) else {
        return false;
    };
    required_deliverables.contains(&role)
}

fn observed_deliverable_role(evidence: &CompletionEvidence) -> Option<ArtifactRole> {
    match evidence {
        CompletionEvidence::RepoEdit { category, .. } => role_from_repo_edit(*category),
        CompletionEvidence::RequiredSectionsPass { .. }
        | CompletionEvidence::ReportCompletenessPass { .. } => Some(ArtifactRole::UsageDocs),
        CompletionEvidence::StructuredDataPass { .. } => Some(ArtifactRole::DataOutput),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::completion_evidence::RepoEditCategory;
    use super::super::task_contract::{
        ObjectiveAuthority, ObjectiveAuxiliaryContext, ObjectiveDeliverableKind, ObjectiveKind,
        TaskKind,
    };
    use super::*;

    #[test]
    fn command_observation_matches_required_inside_shell_chain() {
        assert!(command_observation_matches_required("pwd && ls -la", "pwd"));
        assert!(command_observation_matches_required("pwd && ls -la", "ls"));
        assert!(command_observation_matches_required("pwd; ls -la", "ls"));
        assert!(command_observation_matches_required("pwd\nls -la", "ls"));
    }

    #[test]
    fn command_observation_does_not_match_non_head_arguments() {
        assert!(!command_observation_matches_required(
            "printf ls && pwd",
            "ls"
        ));
        assert!(!command_observation_matches_required("echo pwd", "pwd"));
    }

    #[test]
    fn command_observation_accepts_required_command_run_through_shell_wrapper() {
        assert!(command_observation_matches_required(
            "bash ./scripts/health.sh 2>reports/stderr.txt",
            "./scripts/health.sh"
        ));
        assert!(command_observation_matches_required(
            "sh -c ./scripts/health.sh",
            "./scripts/health.sh"
        ));
        assert!(!command_observation_matches_required(
            "echo ./scripts/health.sh",
            "./scripts/health.sh"
        ));
    }

    #[test]
    fn command_observation_collection_accepts_shell_chain_for_required_commands() {
        let objective = ObjectiveContract {
            authority: ObjectiveAuthority::CurrentUserRequest,
            auxiliary_context: ObjectiveAuxiliaryContext::SessionContext,
            task_kind: TaskKind::Ops,
            objective_kind: ObjectiveKind::Ops,
            deliverable_kind: ObjectiveDeliverableKind::CommandObservation,
            evidence_kind: ObjectiveEvidenceKind::SafetyBoundaryEvidence,
            required_deliverables: Vec::new(),
            evidence_required: true,
            required_evidence_commands: vec!["ls".to_string(), "pwd".to_string()],
        };

        let mut evidence = EvidenceSet::new();
        evidence.push(CompletionEvidence::CommandObservation {
            command: "pwd && ls -la".to_string(),
            exit_status: 0,
            safety_boundary_passed: true,
        });

        assert!(successful_command_observation_requirements_satisfied(
            &evidence, &objective
        ));
    }

    #[test]
    fn safety_boundary_data_output_can_be_validated_by_later_command() {
        let objective = ObjectiveContract {
            authority: ObjectiveAuthority::CurrentUserRequest,
            auxiliary_context: ObjectiveAuxiliaryContext::SessionContext,
            task_kind: TaskKind::Data,
            objective_kind: ObjectiveKind::Data,
            deliverable_kind: ObjectiveDeliverableKind::OutputFile,
            evidence_kind: ObjectiveEvidenceKind::SafetyBoundaryEvidence,
            required_deliverables: vec![ArtifactRole::DataOutput],
            evidence_required: true,
            required_evidence_commands: Vec::new(),
        };

        let mut evidence = EvidenceSet::new();
        evidence.push(CompletionEvidence::RepoEdit {
            category: RepoEditCategory::Data,
            count: 1,
            path: Some("summary.json".to_string()),
        });
        evidence.push(CompletionEvidence::CommandObservation {
            command: "python3 -c 'validate summary.json'".to_string(),
            exit_status: 0,
            safety_boundary_passed: true,
        });

        assert!(objective_evidence_satisfied(&evidence, &objective));
    }

    #[test]
    fn safety_boundary_ops_document_still_requires_artifact_after_command() {
        let objective = ObjectiveContract {
            authority: ObjectiveAuthority::CurrentUserRequest,
            auxiliary_context: ObjectiveAuxiliaryContext::SessionContext,
            task_kind: TaskKind::Ops,
            objective_kind: ObjectiveKind::Ops,
            deliverable_kind: ObjectiveDeliverableKind::DocumentSections,
            evidence_kind: ObjectiveEvidenceKind::SafetyBoundaryEvidence,
            required_deliverables: vec![ArtifactRole::UsageDocs],
            evidence_required: true,
            required_evidence_commands: Vec::new(),
        };

        let mut stale_document = EvidenceSet::new();
        stale_document.push(CompletionEvidence::RepoEdit {
            category: RepoEditCategory::Docs,
            count: 1,
            path: Some("ops-observation.md".to_string()),
        });
        stale_document.push(CompletionEvidence::CommandObservation {
            command: "pwd".to_string(),
            exit_status: 0,
            safety_boundary_passed: true,
        });

        assert!(!objective_evidence_satisfied(&stale_document, &objective));

        let mut bound_document = EvidenceSet::new();
        bound_document.push(CompletionEvidence::CommandObservation {
            command: "pwd".to_string(),
            exit_status: 0,
            safety_boundary_passed: true,
        });
        bound_document.push(CompletionEvidence::RepoEdit {
            category: RepoEditCategory::Docs,
            count: 1,
            path: Some("ops-observation.md".to_string()),
        });

        assert!(objective_evidence_satisfied(&bound_document, &objective));
    }
}
