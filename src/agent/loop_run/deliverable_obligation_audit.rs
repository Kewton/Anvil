//! Read-only audit projection for deliverable obligations.
//!
//! This module does not infer request semantics and does not decide completion.
//! It projects the existing `TaskContract` obligations into a compact typed
//! record so non-coding leakage and fresh-edit pressure can be inspected without
//! adding task-kind branches in the controller loop.

use super::task_contract::{
    ArtifactRole, DeliverableKind, ObjectiveAuthority, TaskContract, TaskKind,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DeliverableObligationSource {
    RequiredArtifactIdentity,
    RequiredArtifactRole,
}

impl DeliverableObligationSource {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::RequiredArtifactIdentity => "required_artifact_identity",
            Self::RequiredArtifactRole => "required_artifact_role",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DeliverableObligationAuditEntry {
    pub(super) task_kind: TaskKind,
    pub(super) role: ArtifactRole,
    pub(super) kind: DeliverableKind,
    pub(super) path: Option<String>,
    pub(super) source: DeliverableObligationSource,
    pub(super) authority: ObjectiveAuthority,
    pub(super) fresh_repo_edit_required: bool,
}

impl DeliverableObligationAuditEntry {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "task_kind": self.task_kind.as_str(),
            "role": self.role.label(),
            "kind": self.kind.label(),
            "path": self.path,
            "source": self.source.label(),
            "authority": self.authority.label(),
            "fresh_repo_edit_required": self.fresh_repo_edit_required,
        })
    }
}

pub(super) fn audit_deliverable_obligations(
    contract: &TaskContract,
) -> Vec<DeliverableObligationAuditEntry> {
    let objective = contract.objective_contract();
    let fresh_roles = if contract.requires_fresh_repo_edit_before_done() {
        contract.fresh_repo_edit_missing_roles()
    } else {
        Vec::new()
    };
    let mut entries = contract
        .required_artifact_identities
        .iter()
        .map(|identity| DeliverableObligationAuditEntry {
            task_kind: contract.task_kind,
            role: identity.role,
            kind: identity.kind,
            path: Some(identity.path.clone()),
            source: DeliverableObligationSource::RequiredArtifactIdentity,
            authority: objective.authority,
            fresh_repo_edit_required: fresh_roles.contains(&identity.role),
        })
        .collect::<Vec<_>>();

    for role in &contract.required_artifacts {
        if entries.iter().any(|entry| entry.role == *role) {
            continue;
        }
        entries.push(DeliverableObligationAuditEntry {
            task_kind: contract.task_kind,
            role: *role,
            kind: deliverable_kind_for_role(*role),
            path: None,
            source: DeliverableObligationSource::RequiredArtifactRole,
            authority: objective.authority,
            fresh_repo_edit_required: fresh_roles.contains(role),
        });
    }
    entries.sort_by(|a, b| {
        (
            a.role,
            a.path.as_deref().unwrap_or_default(),
            a.kind.label(),
        )
            .cmp(&(
                b.role,
                b.path.as_deref().unwrap_or_default(),
                b.kind.label(),
            ))
    });
    entries
}

pub(super) fn deliverable_obligation_audit_payload(contract: &TaskContract) -> serde_json::Value {
    serde_json::Value::Array(
        audit_deliverable_obligations(contract)
            .into_iter()
            .map(|entry| entry.to_json())
            .collect(),
    )
}

fn deliverable_kind_for_role(role: ArtifactRole) -> DeliverableKind {
    match role {
        ArtifactRole::Implementation => DeliverableKind::Code,
        ArtifactRole::Test => DeliverableKind::Tests,
        ArtifactRole::UsageDocs => DeliverableKind::UsageDocs,
        ArtifactRole::Setup => DeliverableKind::Setup,
        ArtifactRole::DataOutput => DeliverableKind::Data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::loop_run::task_contract::TaskKind;

    fn entries_for(request: &str) -> Vec<DeliverableObligationAuditEntry> {
        audit_deliverable_obligations(&TaskContract::from_request(request))
    }

    #[test]
    fn research_report_requires_report_artifact_not_fresh_source_edit() {
        let entries =
            entries_for("Investigate the notes in source_report.md and produce findings.md");

        assert!(entries.iter().any(|entry| {
            entry.task_kind == TaskKind::Research
                && entry.role == ArtifactRole::UsageDocs
                && entry.kind == DeliverableKind::ResearchNotes
                && entry.path.as_deref() == Some("findings.md")
        }));
        assert!(entries.iter().all(|entry| !entry.fresh_repo_edit_required));
    }

    #[test]
    fn ops_command_report_requires_command_observation_not_source_edit() {
        let contract = TaskContract::from_request_with_kind(
            "Run pwd and ls, then create ops/observation.md summarizing the observed current directory and file list. Do not create source code, package manifests, or tests.",
            Some(TaskKind::Ops),
        );
        let entries = audit_deliverable_obligations(&contract);

        assert!(entries.iter().any(|entry| {
            entry.task_kind == TaskKind::Ops
                && entry.role == ArtifactRole::UsageDocs
                && entry.kind == DeliverableKind::CommandOutput
        }));
        assert!(entries.iter().all(|entry| !entry.fresh_repo_edit_required));
    }

    #[test]
    fn data_output_requires_output_artifact_not_source_edit() {
        let entries =
            entries_for("Create output/order-summary.csv with columns Category and Total.");

        assert!(entries.iter().any(|entry| {
            entry.task_kind == TaskKind::Data
                && entry.role == ArtifactRole::DataOutput
                && entry.kind == DeliverableKind::StructuredRecord
                && entry.path.as_deref() == Some("output/order-summary.csv")
        }));
        assert!(entries.iter().all(|entry| !entry.fresh_repo_edit_required));
    }

    #[test]
    fn coding_feature_requires_fresh_source_or_test_edit() {
        let entries =
            entries_for("Implement calculator.py and tests/test_calculator.py for addition.");

        assert!(entries.iter().any(|entry| {
            entry.task_kind == TaskKind::Coding
                && entry.role == ArtifactRole::Implementation
                && entry.fresh_repo_edit_required
        }));
        assert!(entries.iter().any(|entry| {
            entry.task_kind == TaskKind::Coding
                && entry.role == ArtifactRole::Test
                && entry.fresh_repo_edit_required
        }));
    }

    #[test]
    fn verification_only_coding_does_not_require_fresh_edit() {
        let contract = TaskContract::from_request_with_kind(
            "Run the existing Python test suite and report the result without editing files.",
            Some(TaskKind::Coding),
        );
        let entries = audit_deliverable_obligations(&contract);

        assert_eq!(contract.task_kind, TaskKind::Coding);
        assert!(entries.iter().all(|entry| !entry.fresh_repo_edit_required));
    }
}
