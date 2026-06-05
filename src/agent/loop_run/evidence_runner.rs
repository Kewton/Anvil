//! Issue #949: local evidence runner adapters.
//!
//! This module is intentionally thin. It does not introduce a provider layer or
//! new process execution path; it selects the existing verifier/content checks
//! and represents shell/ops observations in the same local lifecycle vocabulary.

#![allow(dead_code)] // Extension seam; focused tests pin the shape before wiring broad callers.

use super::completion_evidence::CompletionEvidence;
use super::summary::GenericTerminalState;
use super::task_contract::{ObjectiveEvidenceKind, TaskKind};
use super::verifier::{VerifierArtifact, verifier_for_task_kind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EvidenceRunnerKind {
    CodingBuildTest,
    DocsContentCheck,
    DataSchemaCheck,
    ResearchSourceFetch,
    OpsCommandObservation,
    AuthoringContentCheck,
}

impl EvidenceRunnerKind {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            EvidenceRunnerKind::CodingBuildTest => "coding_build_test",
            EvidenceRunnerKind::DocsContentCheck => "docs_content_check",
            EvidenceRunnerKind::DataSchemaCheck => "data_schema_check",
            EvidenceRunnerKind::ResearchSourceFetch => "research_source_fetch",
            EvidenceRunnerKind::OpsCommandObservation => "ops_command_observation",
            EvidenceRunnerKind::AuthoringContentCheck => "authoring_content_check",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CommandObservationEvidence {
    pub(super) command: String,
    pub(super) exit_status: i32,
    pub(super) safety_boundary_passed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SourceFetchEvidence {
    pub(super) source: String,
    pub(super) cited_artifact_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum EvidenceRunnerOutput {
    Completion(CompletionEvidence),
    CommandObservation(CommandObservationEvidence),
    SourceFetch(SourceFetchEvidence),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EvidenceRunnerError {
    MissingRunner,
    BindingFailed,
    ExecutionFailed,
}

impl EvidenceRunnerError {
    pub(super) fn generic_terminal_state(self) -> GenericTerminalState {
        match self {
            EvidenceRunnerError::MissingRunner => GenericTerminalState::EvidenceRunnerMissing,
            EvidenceRunnerError::BindingFailed => GenericTerminalState::EvidenceBindingFailed,
            EvidenceRunnerError::ExecutionFailed => GenericTerminalState::EvidenceFailed,
        }
    }
}

pub(super) trait EvidenceRunner {
    fn task_kind(&self) -> TaskKind;
    fn kind(&self) -> EvidenceRunnerKind;
    fn evidence_kind(&self) -> ObjectiveEvidenceKind;

    fn observe_command(
        &self,
        command: &str,
        exit_status: i32,
        safety_boundary_passed: bool,
        bound_artifacts_count: Option<usize>,
    ) -> Option<EvidenceRunnerOutput>;

    fn observe_source_fetch(
        &self,
        source: &str,
        fetched: bool,
        cited_artifact_path: Option<&str>,
    ) -> Option<EvidenceRunnerOutput>;

    fn artifact_evidence(&self, artifact: VerifierArtifact<'_>) -> Option<EvidenceRunnerOutput>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TaskEvidenceRunner {
    task_kind: TaskKind,
    runner_kind: EvidenceRunnerKind,
    evidence_kind: ObjectiveEvidenceKind,
}

impl TaskEvidenceRunner {
    const fn new(
        task_kind: TaskKind,
        runner_kind: EvidenceRunnerKind,
        evidence_kind: ObjectiveEvidenceKind,
    ) -> Self {
        Self {
            task_kind,
            runner_kind,
            evidence_kind,
        }
    }
}

impl EvidenceRunner for TaskEvidenceRunner {
    fn task_kind(&self) -> TaskKind {
        self.task_kind
    }

    fn kind(&self) -> EvidenceRunnerKind {
        self.runner_kind
    }

    fn evidence_kind(&self) -> ObjectiveEvidenceKind {
        self.evidence_kind
    }

    fn observe_command(
        &self,
        command: &str,
        exit_status: i32,
        safety_boundary_passed: bool,
        bound_artifacts_count: Option<usize>,
    ) -> Option<EvidenceRunnerOutput> {
        match self.runner_kind {
            EvidenceRunnerKind::CodingBuildTest => (exit_status == 0 && safety_boundary_passed)
                .then(|| {
                    EvidenceRunnerOutput::Completion(
                        verifier_for_task_kind(TaskKind::Coding)
                            .pass_evidence(command, bound_artifacts_count),
                    )
                }),
            EvidenceRunnerKind::OpsCommandObservation => Some(
                EvidenceRunnerOutput::CommandObservation(CommandObservationEvidence {
                    command: command.to_string(),
                    exit_status,
                    safety_boundary_passed,
                }),
            ),
            EvidenceRunnerKind::DocsContentCheck
            | EvidenceRunnerKind::DataSchemaCheck
            | EvidenceRunnerKind::ResearchSourceFetch
            | EvidenceRunnerKind::AuthoringContentCheck => None,
        }
    }

    fn observe_source_fetch(
        &self,
        source: &str,
        fetched: bool,
        cited_artifact_path: Option<&str>,
    ) -> Option<EvidenceRunnerOutput> {
        (self.runner_kind == EvidenceRunnerKind::ResearchSourceFetch && fetched).then(|| {
            EvidenceRunnerOutput::SourceFetch(SourceFetchEvidence {
                source: source.to_string(),
                cited_artifact_path: cited_artifact_path.map(str::to_string),
            })
        })
    }

    fn artifact_evidence(&self, artifact: VerifierArtifact<'_>) -> Option<EvidenceRunnerOutput> {
        match self.runner_kind {
            EvidenceRunnerKind::CodingBuildTest | EvidenceRunnerKind::OpsCommandObservation => None,
            EvidenceRunnerKind::DocsContentCheck
            | EvidenceRunnerKind::DataSchemaCheck
            | EvidenceRunnerKind::ResearchSourceFetch
            | EvidenceRunnerKind::AuthoringContentCheck => verifier_for_task_kind(self.task_kind)
                .artifact_evidence(artifact)
                .map(EvidenceRunnerOutput::Completion),
        }
    }
}

pub(super) fn evidence_runner_for_task_kind(task_kind: TaskKind) -> Option<TaskEvidenceRunner> {
    Some(match task_kind {
        TaskKind::Coding => TaskEvidenceRunner::new(
            task_kind,
            EvidenceRunnerKind::CodingBuildTest,
            ObjectiveEvidenceKind::TestRun,
        ),
        TaskKind::Docs => TaskEvidenceRunner::new(
            task_kind,
            EvidenceRunnerKind::DocsContentCheck,
            ObjectiveEvidenceKind::ContentCheck,
        ),
        TaskKind::Data => TaskEvidenceRunner::new(
            task_kind,
            EvidenceRunnerKind::DataSchemaCheck,
            ObjectiveEvidenceKind::SchemaCheck,
        ),
        TaskKind::Research => TaskEvidenceRunner::new(
            task_kind,
            EvidenceRunnerKind::ResearchSourceFetch,
            ObjectiveEvidenceKind::SourceFetchEvidence,
        ),
        TaskKind::Ops => TaskEvidenceRunner::new(
            task_kind,
            EvidenceRunnerKind::OpsCommandObservation,
            ObjectiveEvidenceKind::SafetyBoundaryEvidence,
        ),
        TaskKind::Authoring => TaskEvidenceRunner::new(
            task_kind,
            EvidenceRunnerKind::AuthoringContentCheck,
            ObjectiveEvidenceKind::ContentAcceptance,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::bash::BashCommandClass;

    const ALL_KINDS: [TaskKind; 6] = [
        TaskKind::Coding,
        TaskKind::Docs,
        TaskKind::Data,
        TaskKind::Research,
        TaskKind::Ops,
        TaskKind::Authoring,
    ];

    #[test]
    fn runner_selection_covers_current_task_kinds() {
        for kind in ALL_KINDS {
            let runner = evidence_runner_for_task_kind(kind).expect("runner exists");
            assert_eq!(runner.task_kind(), kind);
        }

        let cases = [
            (
                TaskKind::Coding,
                EvidenceRunnerKind::CodingBuildTest,
                ObjectiveEvidenceKind::TestRun,
            ),
            (
                TaskKind::Docs,
                EvidenceRunnerKind::DocsContentCheck,
                ObjectiveEvidenceKind::ContentCheck,
            ),
            (
                TaskKind::Data,
                EvidenceRunnerKind::DataSchemaCheck,
                ObjectiveEvidenceKind::SchemaCheck,
            ),
            (
                TaskKind::Research,
                EvidenceRunnerKind::ResearchSourceFetch,
                ObjectiveEvidenceKind::SourceFetchEvidence,
            ),
            (
                TaskKind::Ops,
                EvidenceRunnerKind::OpsCommandObservation,
                ObjectiveEvidenceKind::SafetyBoundaryEvidence,
            ),
            (
                TaskKind::Authoring,
                EvidenceRunnerKind::AuthoringContentCheck,
                ObjectiveEvidenceKind::ContentAcceptance,
            ),
        ];

        for (kind, expected_runner, expected_evidence) in cases {
            let runner = evidence_runner_for_task_kind(kind).expect("runner exists");
            assert_eq!(runner.kind(), expected_runner);
            assert_eq!(runner.evidence_kind(), expected_evidence);
        }
    }

    #[test]
    fn coding_runner_reuses_verifier_exit_zero_evidence() {
        let runner = evidence_runner_for_task_kind(TaskKind::Coding).expect("runner exists");
        let output = runner
            .observe_command("cargo test", 0, true, Some(2))
            .expect("successful coding command emits evidence");

        assert_eq!(
            output,
            EvidenceRunnerOutput::Completion(CompletionEvidence::VerifierExitZero {
                class: BashCommandClass::BuildTest,
                command: "cargo test".to_string(),
                bound_test_artifacts_count: Some(2),
            })
        );

        assert_eq!(runner.observe_command("cargo test", 1, true, Some(2)), None);
        assert_eq!(
            runner.observe_command("cargo test", 0, false, Some(2)),
            None
        );
    }

    #[test]
    fn docs_and_data_artifact_runners_emit_existing_completion_evidence() {
        let docs = evidence_runner_for_task_kind(TaskKind::Docs).expect("docs runner");
        let docs_output = docs
            .artifact_evidence(VerifierArtifact {
                path: Some("README.md"),
                excerpt: "## Setup\nInstall it.\n## Usage\nRun it.\n",
                required_columns: &[],
                required_sections: &[],
            })
            .expect("docs evidence");
        assert_eq!(
            docs_output,
            EvidenceRunnerOutput::Completion(CompletionEvidence::RequiredSectionsPass {
                path: Some("README.md".to_string()),
            })
        );

        let data = evidence_runner_for_task_kind(TaskKind::Data).expect("data runner");
        let required_columns = vec!["Category".to_string(), "Total".to_string()];
        let data_output = data
            .artifact_evidence(VerifierArtifact {
                path: Some("output.csv"),
                excerpt: "Category,Total\nA,1\nB,2\n",
                required_columns: &required_columns,
                required_sections: &[],
            })
            .expect("data evidence");
        assert_eq!(
            data_output,
            EvidenceRunnerOutput::Completion(CompletionEvidence::StructuredDataPass {
                path: Some("output.csv".to_string()),
                columns: required_columns,
            })
        );
    }

    #[test]
    fn research_and_ops_runners_cover_non_coding_evidence_shapes() {
        let research = evidence_runner_for_task_kind(TaskKind::Research).expect("research runner");
        assert_eq!(
            research.observe_source_fetch("https://example.test/report", true, Some("research.md")),
            Some(EvidenceRunnerOutput::SourceFetch(SourceFetchEvidence {
                source: "https://example.test/report".to_string(),
                cited_artifact_path: Some("research.md".to_string()),
            }))
        );
        assert_eq!(
            research.observe_source_fetch("https://example.test/report", false, None),
            None
        );

        let research_output = research
            .artifact_evidence(VerifierArtifact {
                path: Some("research.md"),
                excerpt: "## Summary\nFinding: release cadence changed.\nSource: https://example.test/report\nLimitation: confidence is medium.\n",
                required_columns: &[],
                required_sections: &[],
            })
            .expect("research evidence");
        assert_eq!(
            research_output,
            EvidenceRunnerOutput::Completion(CompletionEvidence::ReportCompletenessPass {
                path: Some("research.md".to_string()),
            })
        );

        let ops = evidence_runner_for_task_kind(TaskKind::Ops).expect("ops runner");
        assert_eq!(
            ops.observe_command("printf ok", 0, true, None),
            Some(EvidenceRunnerOutput::CommandObservation(
                CommandObservationEvidence {
                    command: "printf ok".to_string(),
                    exit_status: 0,
                    safety_boundary_passed: true,
                },
            ))
        );
    }

    #[test]
    fn runner_failures_project_to_generic_terminal_states() {
        assert_eq!(
            EvidenceRunnerError::MissingRunner.generic_terminal_state(),
            GenericTerminalState::EvidenceRunnerMissing
        );
        assert_eq!(
            EvidenceRunnerError::BindingFailed.generic_terminal_state(),
            GenericTerminalState::EvidenceBindingFailed
        );
        assert_eq!(
            EvidenceRunnerError::ExecutionFailed.generic_terminal_state(),
            GenericTerminalState::EvidenceFailed
        );
    }
}
