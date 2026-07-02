//! TaskContract deliverable view projection.
//!
//! Artifact-role admission and obligation construction happen before this
//! module. This layer only projects those typed contract parts into the
//! user-visible `TaskDeliverable` list used by prompts, diagnostics, and tests.

use super::task_contract::{
    ArtifactObligation, ArtifactRole, DeliverableKind, TaskDeliverable, TaskKind,
    default_docs_path_from_request, explicit_path_with_data_extension,
    required_doc_sections_from_request, required_ops_sections_from_request,
    required_research_sections_from_request,
};

pub(super) fn deliverables_from_contract_parts(
    request: &str,
    task_kind: TaskKind,
    required_artifacts: &[ArtifactRole],
    required_artifact_identities: &[ArtifactObligation],
    optional_artifacts: &[ArtifactRole],
) -> Vec<TaskDeliverable> {
    let mut deliverables = Vec::new();
    for role in required_artifacts {
        deliverables.push(deliverable_for_role(
            request,
            *role,
            required_artifact_identities,
            false,
        ));
    }
    for role in optional_artifacts {
        deliverables.push(deliverable_for_role(
            request,
            *role,
            required_artifact_identities,
            true,
        ));
    }
    if deliverables.is_empty() {
        deliverables.push(generic_task_deliverable(request, task_kind));
    }
    deliverables
}

fn deliverable_for_role(
    request: &str,
    role: ArtifactRole,
    required_artifact_identities: &[ArtifactObligation],
    optional: bool,
) -> TaskDeliverable {
    // Issue #923: an OpsRunbook obligation rides on the `UsageDocs` role (no
    // dedicated role until #920) but is a runbook deliverable, not docs. Surface
    // it with its `OpsRunbook` kind and carry its canonical `required_sections`
    // so the deliverable view matches the obligation (DR3-002).
    if let Some(identity) = required_artifact_identities
        .iter()
        .find(|identity| identity.role == role && identity.kind == DeliverableKind::OpsRunbook)
    {
        return TaskDeliverable {
            kind: DeliverableKind::OpsRunbook,
            role: Some(role),
            path: Some(identity.path.clone()),
            required_sections: identity.required_sections.clone(),
        };
    }
    if let Some(identity) = required_artifact_identities
        .iter()
        .find(|identity| identity.role == role && !identity.required_sections.is_empty())
    {
        return TaskDeliverable {
            kind: deliverable_kind_for_role(role),
            role: Some(role),
            path: Some(identity.path.clone()),
            required_sections: identity.required_sections.clone(),
        };
    }
    let path = required_artifact_identities
        .iter()
        .find(|identity| identity.role == role)
        .map(|identity| identity.path.clone())
        .or_else(|| default_deliverable_path(role).map(str::to_string));
    TaskDeliverable {
        kind: deliverable_kind_for_role(role),
        role: Some(role),
        path,
        required_sections: if role == ArtifactRole::UsageDocs && !optional {
            required_doc_sections_from_request(request)
        } else {
            Vec::new()
        },
    }
}

fn generic_task_deliverable(request: &str, task_kind: TaskKind) -> TaskDeliverable {
    match task_kind {
        TaskKind::Docs => TaskDeliverable {
            kind: DeliverableKind::UsageDocs,
            role: Some(ArtifactRole::UsageDocs),
            path: Some(default_docs_path_from_request(request)),
            required_sections: required_doc_sections_from_request(request),
        },
        TaskKind::Data => TaskDeliverable {
            kind: DeliverableKind::Data,
            role: None,
            path: explicit_path_with_data_extension(request),
            required_sections: Vec::new(),
        },
        TaskKind::Research => TaskDeliverable {
            kind: DeliverableKind::ResearchNotes,
            role: None,
            path: None,
            required_sections: required_research_sections_from_request(request),
        },
        TaskKind::Ops => TaskDeliverable {
            kind: DeliverableKind::OpsRunbook,
            role: None,
            path: None,
            required_sections: required_ops_sections_from_request(request),
        },
        TaskKind::Coding => TaskDeliverable {
            kind: DeliverableKind::Code,
            role: Some(ArtifactRole::Implementation),
            path: None,
            required_sections: Vec::new(),
        },
        // Issue #919 (Decision #3 site #7): reuse the Docs deliverable shape but
        // with **empty `required_sections`** so the docs section gate is not
        // imposed — Authoring completion is accept-tier only (present + non-empty
        // + min length + softened user-named sections).
        TaskKind::Authoring => TaskDeliverable {
            kind: DeliverableKind::UsageDocs,
            role: Some(ArtifactRole::UsageDocs),
            path: Some(default_docs_path_from_request(request)),
            required_sections: Vec::new(),
        },
    }
}

/// Issue #920: intentional 1:1 decision point — there is no sensible default
/// `DeliverableKind` for an unknown role, so this match stays exhaustive (no
/// `_ =>`). Adding a role MUST compile-error here to force an explicit kind.
pub(super) fn deliverable_kind_for_role(role: ArtifactRole) -> DeliverableKind {
    match role {
        ArtifactRole::Implementation => DeliverableKind::Code,
        ArtifactRole::Test => DeliverableKind::Tests,
        ArtifactRole::UsageDocs => DeliverableKind::UsageDocs,
        ArtifactRole::Setup => DeliverableKind::Setup,
        ArtifactRole::DataOutput => DeliverableKind::Data,
    }
}

pub(super) fn default_deliverable_path(role: ArtifactRole) -> Option<&'static str> {
    match role {
        ArtifactRole::UsageDocs => Some("README.md"),
        ArtifactRole::DataOutput => Some("output.csv"),
        // Issue #920 (Tier A, cascade-free default): roles without a canonical
        // default artifact path (Implementation / Test / Setup today, and any
        // future role) have no deterministic default path. A new role inherits
        // `None` here and only needs a dedicated arm if it gains a convention.
        _ => None,
    }
}
