//! Post-tool artifact/evidence reconciliation projection.
//!
//! This module is deliberately read-only in WP17. It classifies the existing
//! `ArtifactRecoveryAction` into typed post-tool statuses so later slices can
//! move actor-loop routing behind this boundary without changing behavior now.

use super::Agent;
use super::deliverable_obligation_audit::deliverable_obligation_audit_payload;
use super::evidence_runner::{EvidenceRunner, EvidenceRunnerKind, evidence_runner_for_task_kind};
use super::task_contract::{
    ArtifactRecoveryAction, ArtifactRole, ObjectiveEvidenceKind, RecoveryTargetHint,
    SafeStopReason, TaskContract, TaskKind,
};
use crate::logging::log_llm_event;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PostToolReconciliation {
    pub(super) task_kind: TaskKind,
    pub(super) evidence_kind: ObjectiveEvidenceKind,
    pub(super) evidence_required: bool,
    pub(super) runner_kind: Option<EvidenceRunnerKind>,
    pub(super) status: PostToolReconciliationStatus,
}

impl PostToolReconciliation {
    fn label(&self) -> &'static str {
        self.status.label()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PostToolReconciliationStatus {
    MissingDeliverable {
        missing: Vec<ArtifactRole>,
        target_hint: Option<RecoveryTargetHint>,
    },
    EvidenceReady,
    EvidenceFailed {
        target_hint: Option<RecoveryTargetHint>,
    },
    Done,
    Blocked {
        reason: SafeStopReason,
    },
}

impl PostToolReconciliationStatus {
    fn label(&self) -> &'static str {
        match self {
            Self::MissingDeliverable { .. } => "missing_deliverable",
            Self::EvidenceReady => "evidence_ready",
            Self::EvidenceFailed { .. } => "evidence_failed",
            Self::Done => "done",
            Self::Blocked { .. } => "blocked",
        }
    }
}

pub(super) fn reconcile_post_tool_action(
    contract: &TaskContract,
    action: &ArtifactRecoveryAction,
) -> PostToolReconciliation {
    let objective = contract.objective_contract();
    let runner_kind =
        evidence_runner_for_task_kind(objective.task_kind).map(|runner| runner.kind());
    let status = match action {
        ArtifactRecoveryAction::Continue {
            missing,
            target_hint,
        } => PostToolReconciliationStatus::MissingDeliverable {
            missing: missing.clone(),
            target_hint: target_hint.clone(),
        },
        ArtifactRecoveryAction::RunVerifier => PostToolReconciliationStatus::EvidenceReady,
        ArtifactRecoveryAction::RepairArtifact { target_hint } => {
            PostToolReconciliationStatus::EvidenceFailed {
                target_hint: target_hint.clone(),
            }
        }
        ArtifactRecoveryAction::Done => PostToolReconciliationStatus::Done,
        ArtifactRecoveryAction::SafeStop { reason } => {
            PostToolReconciliationStatus::Blocked { reason: *reason }
        }
    };

    PostToolReconciliation {
        task_kind: objective.task_kind,
        evidence_kind: objective.evidence_kind,
        evidence_required: objective.evidence_required,
        runner_kind,
        status,
    }
}

pub(super) fn emit_post_tool_reconciliation(
    agent: &Agent,
    contract: &TaskContract,
    action: &ArtifactRecoveryAction,
) {
    let reconciliation = reconcile_post_tool_action(contract, action);
    log_llm_event(
        "agent.post_tool_reconciliation.context",
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "turn_index": agent.current_turn_index,
            "task_kind": reconciliation.task_kind.as_str(),
            "evidence_kind": reconciliation.evidence_kind.label(),
            "evidence_required": reconciliation.evidence_required,
            "runner_kind": reconciliation.runner_kind.map(EvidenceRunnerKind::as_str),
            "status": reconciliation.label(),
            "authority": "projection_only",
            "deliverable_obligations": deliverable_obligation_audit_payload(contract),
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::super::task_contract::RecoveryTargetHint;
    use super::*;

    #[test]
    fn continue_action_projects_missing_deliverable_status() {
        let contract = TaskContract::from_request("Create README.md with Usage section.");
        let action = ArtifactRecoveryAction::Continue {
            missing: vec![ArtifactRole::UsageDocs],
            target_hint: Some(RecoveryTargetHint {
                role: ArtifactRole::UsageDocs,
                path: "README.md".to_string(),
                reason: "missing docs".to_string(),
            }),
        };

        let projected = reconcile_post_tool_action(&contract, &action);

        assert_eq!(projected.task_kind, TaskKind::Docs);
        assert_eq!(projected.label(), "missing_deliverable");
        assert_eq!(
            projected.runner_kind,
            Some(EvidenceRunnerKind::DocsContentCheck)
        );
    }

    #[test]
    fn run_verifier_action_projects_evidence_ready_status() {
        let contract = TaskContract::from_request(
            "Generate output.csv with columns Category and Total from the input CSV.",
        );
        let projected = reconcile_post_tool_action(&contract, &ArtifactRecoveryAction::RunVerifier);

        assert_eq!(projected.task_kind, TaskKind::Data);
        assert_eq!(projected.label(), "evidence_ready");
        assert_eq!(
            projected.runner_kind,
            Some(EvidenceRunnerKind::DataSchemaCheck)
        );
    }

    #[test]
    fn repair_action_projects_evidence_failed_status() {
        let contract = TaskContract::from_request("Create a Python function with tests.");
        let action = ArtifactRecoveryAction::RepairArtifact {
            target_hint: Some(RecoveryTargetHint {
                role: ArtifactRole::Implementation,
                path: "main.py".to_string(),
                reason: "test failure".to_string(),
            }),
        };

        let projected = reconcile_post_tool_action(&contract, &action);

        assert_eq!(projected.task_kind, TaskKind::Coding);
        assert_eq!(projected.label(), "evidence_failed");
        assert_eq!(
            projected.runner_kind,
            Some(EvidenceRunnerKind::CodingBuildTest)
        );
    }
}
