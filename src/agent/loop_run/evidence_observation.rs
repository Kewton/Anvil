//! Typed evidence observations for terminal-projection diagnostics.
//!
//! This module is intentionally an adapter over existing completion evidence.
//! It does not decide terminal state yet; it gives the controller one closed
//! shape for "what evidence was observed, where it came from, and what it was
//! bound to" before later work packages move projection onto this input.

#![allow(dead_code)] // Extension seam; WP-A pins the shape before broad projection wiring.

use super::completion_evidence::CompletionEvidence;
use super::evidence_runner::{EvidenceRunner, EvidenceRunnerKind, evidence_runner_for_objective};
use super::task_contract::{ObjectiveContract, ObjectiveEvidenceKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EvidenceObservationStatus {
    Passed,
    Failed,
    Missing,
}

impl EvidenceObservationStatus {
    fn as_str(self) -> &'static str {
        match self {
            EvidenceObservationStatus::Passed => "passed",
            EvidenceObservationStatus::Failed => "failed",
            EvidenceObservationStatus::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EvidenceObservationSource {
    CompletionEvidence,
    EvidenceRunner,
    Verifier,
}

impl EvidenceObservationSource {
    fn as_str(self) -> &'static str {
        match self {
            EvidenceObservationSource::CompletionEvidence => "completion_evidence",
            EvidenceObservationSource::EvidenceRunner => "evidence_runner",
            EvidenceObservationSource::Verifier => "verifier",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EvidenceObservation {
    pub(super) kind: ObjectiveEvidenceKind,
    pub(super) status: EvidenceObservationStatus,
    pub(super) bound_artifacts: Vec<String>,
    pub(super) runner: Option<EvidenceRunnerKind>,
    pub(super) source: EvidenceObservationSource,
    pub(super) diagnostic_summary: Option<String>,
}

impl EvidenceObservation {
    pub(super) fn passed(
        kind: ObjectiveEvidenceKind,
        runner: Option<EvidenceRunnerKind>,
        source: EvidenceObservationSource,
    ) -> Self {
        Self {
            kind,
            status: EvidenceObservationStatus::Passed,
            bound_artifacts: Vec::new(),
            runner,
            source,
            diagnostic_summary: None,
        }
    }

    pub(super) fn failed(
        kind: ObjectiveEvidenceKind,
        runner: Option<EvidenceRunnerKind>,
        source: EvidenceObservationSource,
        diagnostic_summary: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            status: EvidenceObservationStatus::Failed,
            bound_artifacts: Vec::new(),
            runner,
            source,
            diagnostic_summary: Some(diagnostic_summary.into()),
        }
    }

    pub(super) fn missing(
        kind: ObjectiveEvidenceKind,
        runner: Option<EvidenceRunnerKind>,
        source: EvidenceObservationSource,
        diagnostic_summary: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            status: EvidenceObservationStatus::Missing,
            bound_artifacts: Vec::new(),
            runner,
            source,
            diagnostic_summary: Some(diagnostic_summary.into()),
        }
    }

    pub(super) fn with_bound_artifacts<I, S>(mut self, artifacts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.bound_artifacts = artifacts.into_iter().map(Into::into).collect();
        self
    }

    pub(super) fn from_completion_evidence_for_objective(
        evidence: &CompletionEvidence,
        objective: &ObjectiveContract,
        source: EvidenceObservationSource,
    ) -> Option<Self> {
        let runner = evidence_runner_for_objective(objective).map(|runner| runner.kind());
        Self::from_completion_evidence(evidence, objective.evidence_kind, runner, source)
    }

    pub(super) fn from_completion_evidence(
        evidence: &CompletionEvidence,
        expected_kind: ObjectiveEvidenceKind,
        runner: Option<EvidenceRunnerKind>,
        source: EvidenceObservationSource,
    ) -> Option<Self> {
        let kind = completion_evidence_observation_kind(evidence, expected_kind)?;
        let mut observation = match evidence {
            CompletionEvidence::CommandObservation {
                exit_status,
                safety_boundary_passed,
                ..
            } if *exit_status != 0 || !*safety_boundary_passed => Self::failed(
                kind,
                runner,
                source,
                format!(
                    "command_observation exit_status={} safety_boundary_passed={}",
                    exit_status, safety_boundary_passed
                ),
            ),
            _ => Self::passed(kind, runner, source),
        };
        observation.bound_artifacts = completion_evidence_bound_artifacts(evidence);
        observation.diagnostic_summary =
            completion_evidence_diagnostic_summary(evidence, observation.diagnostic_summary);
        Some(observation)
    }

    fn to_log_value(&self, turn_index: usize, iter_index: usize) -> serde_json::Value {
        serde_json::json!({
            "turn_index": turn_index,
            "iter_index": iter_index,
            "kind": self.kind.label(),
            "status": self.status.as_str(),
            "bound_artifacts": &self.bound_artifacts,
            "runner": self.runner.map(EvidenceRunnerKind::as_str),
            "source": self.source.as_str(),
            "diagnostic_summary": &self.diagnostic_summary,
        })
    }
}

pub(super) fn log_evidence_observation_observed(
    turn_index: usize,
    iter_index: usize,
    observation: &EvidenceObservation,
) {
    crate::logging::log_llm_event(
        "agent.evidence_observation.observed",
        observation.to_log_value(turn_index, iter_index),
    );
}

fn completion_evidence_observation_kind(
    evidence: &CompletionEvidence,
    expected_kind: ObjectiveEvidenceKind,
) -> Option<ObjectiveEvidenceKind> {
    Some(match evidence {
        CompletionEvidence::VerifierExitZero { .. } => ObjectiveEvidenceKind::TestRun,
        CompletionEvidence::RequiredSectionsPass { .. } => match expected_kind {
            ObjectiveEvidenceKind::ContentAcceptance => ObjectiveEvidenceKind::ContentAcceptance,
            _ => ObjectiveEvidenceKind::ContentCheck,
        },
        CompletionEvidence::StructuredDataPass { .. } => ObjectiveEvidenceKind::SchemaCheck,
        CompletionEvidence::ReportCompletenessPass { .. } => match expected_kind {
            ObjectiveEvidenceKind::ContentCheck
            | ObjectiveEvidenceKind::ContentAcceptance
            | ObjectiveEvidenceKind::SourceFetchEvidence => expected_kind,
            _ => ObjectiveEvidenceKind::ContentAcceptance,
        },
        CompletionEvidence::CommandObservation { .. } => {
            ObjectiveEvidenceKind::SafetyBoundaryEvidence
        }
        CompletionEvidence::AnswerOnly => ObjectiveEvidenceKind::ContentAcceptance,
        CompletionEvidence::RepoEdit { .. } => ObjectiveEvidenceKind::FileLayoutCheck,
    })
}

fn completion_evidence_bound_artifacts(evidence: &CompletionEvidence) -> Vec<String> {
    match evidence {
        CompletionEvidence::RepoEdit {
            path: Some(path), ..
        }
        | CompletionEvidence::RequiredSectionsPass { path: Some(path) }
        | CompletionEvidence::StructuredDataPass {
            path: Some(path), ..
        }
        | CompletionEvidence::ReportCompletenessPass { path: Some(path) } => vec![path.clone()],
        _ => Vec::new(),
    }
}

fn completion_evidence_diagnostic_summary(
    evidence: &CompletionEvidence,
    existing: Option<String>,
) -> Option<String> {
    match evidence {
        CompletionEvidence::VerifierExitZero {
            class,
            bound_test_artifacts_count,
            ..
        } => Some(format!(
            "command_class={} bound_test_artifacts_count={}",
            class.as_str(),
            bound_test_artifacts_count
                .map(|count| count.to_string())
                .unwrap_or_else(|| "unbound".to_string())
        )),
        CompletionEvidence::StructuredDataPass { columns, .. } if !columns.is_empty() => {
            Some(format!("columns={}", columns.join(",")))
        }
        _ => existing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::loop_run::evidence_runner::EvidenceRunnerKind;
    use crate::tools::bash::BashCommandClass;

    #[test]
    fn passed_observation_records_closed_shape() {
        let observation = EvidenceObservation::passed(
            ObjectiveEvidenceKind::TestRun,
            Some(EvidenceRunnerKind::CodingBuildTest),
            EvidenceObservationSource::Verifier,
        );

        assert_eq!(observation.kind, ObjectiveEvidenceKind::TestRun);
        assert_eq!(observation.status, EvidenceObservationStatus::Passed);
        assert_eq!(
            observation.runner,
            Some(EvidenceRunnerKind::CodingBuildTest)
        );
        assert_eq!(observation.source, EvidenceObservationSource::Verifier);
        assert!(observation.bound_artifacts.is_empty());
    }

    #[test]
    fn failed_observation_carries_diagnostic() {
        let observation = EvidenceObservation::failed(
            ObjectiveEvidenceKind::SchemaCheck,
            Some(EvidenceRunnerKind::DataSchemaCheck),
            EvidenceObservationSource::EvidenceRunner,
            "missing required column",
        );

        assert_eq!(observation.status, EvidenceObservationStatus::Failed);
        assert_eq!(
            observation.diagnostic_summary.as_deref(),
            Some("missing required column")
        );
    }

    #[test]
    fn missing_observation_keeps_runner_context() {
        let observation = EvidenceObservation::missing(
            ObjectiveEvidenceKind::ContentCheck,
            Some(EvidenceRunnerKind::DocsContentCheck),
            EvidenceObservationSource::EvidenceRunner,
            "no report artifact",
        );

        assert_eq!(observation.status, EvidenceObservationStatus::Missing);
        assert_eq!(
            observation.runner,
            Some(EvidenceRunnerKind::DocsContentCheck)
        );
    }

    #[test]
    fn observation_can_be_artifact_bound() {
        let observation = EvidenceObservation::passed(
            ObjectiveEvidenceKind::TestRun,
            Some(EvidenceRunnerKind::CodingBuildTest),
            EvidenceObservationSource::Verifier,
        )
        .with_bound_artifacts(["tests/test_sales.py", "src/sales.py"]);

        assert_eq!(
            observation.bound_artifacts,
            vec!["tests/test_sales.py", "src/sales.py"]
        );
    }

    #[test]
    fn python_test_run_completion_evidence_maps_to_observation() {
        let evidence = CompletionEvidence::VerifierExitZero {
            class: BashCommandClass::BuildTest,
            command: "pytest tests/test_sales.py".to_string(),
            bound_test_artifacts_count: Some(1),
        };

        let observation = EvidenceObservation::from_completion_evidence(
            &evidence,
            ObjectiveEvidenceKind::TestRun,
            Some(EvidenceRunnerKind::CodingBuildTest),
            EvidenceObservationSource::Verifier,
        )
        .expect("test evidence maps");

        assert_eq!(observation.kind, ObjectiveEvidenceKind::TestRun);
        assert_eq!(observation.status, EvidenceObservationStatus::Passed);
        assert_eq!(
            observation.diagnostic_summary.as_deref(),
            Some("command_class=build_test bound_test_artifacts_count=1")
        );
    }

    #[test]
    fn toml_test_run_completion_evidence_maps_to_observation() {
        let evidence = CompletionEvidence::VerifierExitZero {
            class: BashCommandClass::BuildTest,
            command: "cargo test".to_string(),
            bound_test_artifacts_count: None,
        };

        let observation = EvidenceObservation::from_completion_evidence(
            &evidence,
            ObjectiveEvidenceKind::TestRun,
            Some(EvidenceRunnerKind::CodingBuildTest),
            EvidenceObservationSource::Verifier,
        )
        .expect("test evidence maps");

        assert_eq!(observation.kind, ObjectiveEvidenceKind::TestRun);
        assert_eq!(
            observation.diagnostic_summary.as_deref(),
            Some("command_class=build_test bound_test_artifacts_count=unbound")
        );
    }

    #[test]
    fn structured_data_pass_maps_to_schema_check_with_bound_path() {
        let evidence = CompletionEvidence::StructuredDataPass {
            path: Some("output/order-summary.csv".to_string()),
            columns: vec!["category".to_string(), "total".to_string()],
        };

        let observation = EvidenceObservation::from_completion_evidence(
            &evidence,
            ObjectiveEvidenceKind::SchemaCheck,
            Some(EvidenceRunnerKind::DataSchemaCheck),
            EvidenceObservationSource::EvidenceRunner,
        )
        .expect("schema evidence maps");

        assert_eq!(observation.kind, ObjectiveEvidenceKind::SchemaCheck);
        assert_eq!(
            observation.bound_artifacts,
            vec!["output/order-summary.csv"]
        );
        assert_eq!(
            observation.diagnostic_summary.as_deref(),
            Some("columns=category,total")
        );
    }

    #[test]
    fn report_completeness_uses_objective_context_for_research() {
        let evidence = CompletionEvidence::ReportCompletenessPass {
            path: Some("research.md".to_string()),
        };

        let observation = EvidenceObservation::from_completion_evidence(
            &evidence,
            ObjectiveEvidenceKind::SourceFetchEvidence,
            Some(EvidenceRunnerKind::ResearchSourceFetch),
            EvidenceObservationSource::EvidenceRunner,
        )
        .expect("report evidence maps");

        assert_eq!(observation.kind, ObjectiveEvidenceKind::SourceFetchEvidence);
        assert_eq!(observation.bound_artifacts, vec!["research.md"]);
    }

    #[test]
    fn failed_command_observation_maps_to_failed_status() {
        let evidence = CompletionEvidence::CommandObservation {
            command: "ls missing".to_string(),
            exit_status: 2,
            safety_boundary_passed: true,
        };

        let observation = EvidenceObservation::from_completion_evidence(
            &evidence,
            ObjectiveEvidenceKind::SafetyBoundaryEvidence,
            Some(EvidenceRunnerKind::OpsCommandObservation),
            EvidenceObservationSource::EvidenceRunner,
        )
        .expect("command evidence maps");

        assert_eq!(observation.status, EvidenceObservationStatus::Failed);
        assert_eq!(
            observation.diagnostic_summary.as_deref(),
            Some("command_observation exit_status=2 safety_boundary_passed=true")
        );
    }
}
