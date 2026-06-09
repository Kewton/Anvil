//! Artifact obligation planning helpers for TaskContract construction.
//!
//! This module owns typed obligation merge/shadow behavior and project-profile
//! obligation projection. It deliberately does not inspect raw request text.

use super::task_contract::{
    ArtifactObligation, ArtifactRole, DeliverableKind, DeliverableSchema, ProjectIntent,
    ProjectLanguage, ProjectShape,
};

pub(super) fn default_readme_required_sections() -> Vec<String> {
    ["setup", "usage", "test"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

pub(super) fn inferred_artifact_obligations_from_project_intent(
    project_intent: &ProjectIntent,
    required_artifacts: &[ArtifactRole],
) -> Vec<ArtifactObligation> {
    if !required_artifacts.contains(&ArtifactRole::Implementation) {
        return Vec::new();
    }
    let shape = project_intent.shape.unwrap_or(ProjectShape::Unknown);
    if !matches!(shape, ProjectShape::Cli | ProjectShape::Library) {
        return Vec::new();
    }
    let mut obligations = Vec::new();
    match project_intent.language.unwrap_or(ProjectLanguage::Unknown) {
        ProjectLanguage::Rust => {
            obligations.push(ArtifactObligation::file(ArtifactRole::Setup, "Cargo.toml"));
            if matches!(shape, ProjectShape::Cli) {
                obligations.push(ArtifactObligation::file(
                    ArtifactRole::Implementation,
                    "src/main.rs",
                ));
                if required_artifacts.contains(&ArtifactRole::Test) {
                    obligations.push(ArtifactObligation::file(ArtifactRole::Test, "tests/cli.rs"));
                }
                if required_artifacts.contains(&ArtifactRole::UsageDocs) {
                    obligations.push(ArtifactObligation::readme(
                        "README.md",
                        default_readme_required_sections(),
                    ));
                }
            } else if matches!(shape, ProjectShape::Library) {
                obligations.push(ArtifactObligation::file(
                    ArtifactRole::Implementation,
                    "src/lib.rs",
                ));
                if required_artifacts.contains(&ArtifactRole::Test) {
                    obligations.push(ArtifactObligation::file(ArtifactRole::Test, "tests/lib.rs"));
                }
                if required_artifacts.contains(&ArtifactRole::UsageDocs) {
                    obligations.push(ArtifactObligation::readme(
                        "README.md",
                        default_readme_required_sections(),
                    ));
                }
            }
        }
        ProjectLanguage::Node => {
            obligations.push(ArtifactObligation::file(
                ArtifactRole::Setup,
                "package.json",
            ));
            if matches!(shape, ProjectShape::Cli) {
                obligations.push(ArtifactObligation::json_field(
                    ArtifactRole::Setup,
                    "package.json",
                    "bin",
                    "package.json declares a bin entry for the CLI",
                ));
                obligations.push(ArtifactObligation::file(
                    ArtifactRole::Implementation,
                    "src/index.js",
                ));
                if required_artifacts.contains(&ArtifactRole::Test) {
                    obligations.push(ArtifactObligation::file(
                        ArtifactRole::Test,
                        "tests/index.test.js",
                    ));
                }
                if required_artifacts.contains(&ArtifactRole::UsageDocs) {
                    obligations.push(ArtifactObligation::readme(
                        "README.md",
                        default_readme_required_sections(),
                    ));
                }
            }
        }
        ProjectLanguage::Python => {
            if matches!(shape, ProjectShape::Cli) {
                obligations.push(ArtifactObligation::file(
                    ArtifactRole::Implementation,
                    "main.py",
                ));
                if required_artifacts.contains(&ArtifactRole::Test) {
                    obligations.push(ArtifactObligation::file(
                        ArtifactRole::Test,
                        "tests/test_main.py",
                    ));
                }
                if required_artifacts.contains(&ArtifactRole::UsageDocs) {
                    obligations.push(ArtifactObligation::readme(
                        "README.md",
                        default_readme_required_sections(),
                    ));
                }
            }
        }
        ProjectLanguage::Docs | ProjectLanguage::Unknown => {}
    }
    obligations
}

pub(super) fn push_or_merge_artifact_obligation(
    obligations: &mut Vec<ArtifactObligation>,
    incoming: ArtifactObligation,
) {
    let Some(existing) = obligations
        .iter_mut()
        .find(|existing| should_merge_artifact_obligations(existing, &incoming))
    else {
        obligations.push(incoming);
        return;
    };
    if existing.kind == DeliverableKind::File && incoming.kind != DeliverableKind::File {
        existing.kind = incoming.kind;
    }
    if existing.required_sections.is_empty() && !incoming.required_sections.is_empty() {
        existing.required_sections = incoming.required_sections;
    }
    if existing.acceptance_criteria.is_empty() && !incoming.acceptance_criteria.is_empty() {
        existing.acceptance_criteria = incoming.acceptance_criteria;
    }
    if existing.structured_record_schema.is_none() {
        existing.structured_record_schema = incoming.structured_record_schema;
    }
    if existing.schema.is_none() {
        existing.schema = incoming.schema;
    }
}

pub(super) fn inferred_obligation_shadowed_by_explicit_identity(
    existing: &[ArtifactObligation],
    incoming: &ArtifactObligation,
) -> bool {
    matches!(
        incoming.role,
        ArtifactRole::Implementation | ArtifactRole::Test
    ) && existing
        .iter()
        .any(|identity| identity.role == incoming.role && identity.path != incoming.path)
}

pub(super) fn profile_obligation_shadowed_by_prior_identity(
    existing: &[ArtifactObligation],
    incoming: &ArtifactObligation,
) -> bool {
    incoming.role == ArtifactRole::DataOutput
        && existing
            .iter()
            .any(|identity| identity.role == incoming.role && identity.path != incoming.path)
}

fn should_merge_artifact_obligations(
    existing: &ArtifactObligation,
    incoming: &ArtifactObligation,
) -> bool {
    if existing.role != incoming.role || existing.path != incoming.path {
        return false;
    }
    if existing.role == ArtifactRole::Setup
        && existing.path == "package.json"
        && (matches!(
            existing.schema.as_ref(),
            Some(DeliverableSchema::JsonFields(_))
        ) || matches!(
            incoming.schema.as_ref(),
            Some(DeliverableSchema::JsonFields(_))
        ))
    {
        return false;
    }
    true
}
