//! Typed projection from an LLM project-profile confirmation into controller
//! contract inputs.
//!
//! `project_profile` owns the prompt/parser boundary. `task_contract` owns the
//! final objective contract. This module is the narrow adapter between them so
//! the contract builder does not accumulate LLM-wire parsing and adoption rules.

use super::project_profile::{
    self, ForbiddenArtifact, ProfileDeliverableKind, ProfileEvidenceKind,
    ProjectProfileConfirmation,
};
use super::task_contract::{
    ArtifactObligation, ArtifactRole, ObjectiveDeliverableKind, ProjectLanguage, ProjectShape,
    TaskContract, TaskKind, VerificationRequirement, preferred_runner_for_language,
};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProjectProfileContractInputs {
    pub(super) task_kind: Option<TaskKind>,
    pub(super) language: Option<ProjectLanguage>,
    pub(super) shape: Option<ProjectShape>,
    pub(super) verification: Option<VerificationRequirement>,
    pub(super) required_role: Option<ArtifactRole>,
    pub(super) artifact_obligations: Vec<ArtifactObligation>,
    pub(super) forbids_implementation: bool,
    pub(super) forbids_tests: bool,
    pub(super) forbids_setup: bool,
    pub(super) confidence: f32,
}

pub(super) fn contract_inputs_from_confirmation(
    profile: Option<&ProjectProfileConfirmation>,
) -> Option<ProjectProfileContractInputs> {
    let profile =
        profile.filter(|profile| project_profile::confirmation_is_authoritative(profile))?;
    let required_role = required_artifact_role(profile);
    Some(ProjectProfileContractInputs {
        task_kind: task_kind_from_profile(profile),
        language: profile.language,
        shape: profile.shape,
        verification: verification_requirement(profile),
        required_role,
        artifact_obligations: artifact_obligations(profile, required_role),
        forbids_implementation: forbids_implementation_artifact(profile),
        forbids_tests: forbids_test_artifacts(profile),
        forbids_setup: forbids_setup_artifact(profile),
        confidence: profile.confidence,
    })
}

pub(super) fn should_confirm_project_profile(first_pass: &TaskContract) -> bool {
    let objective = first_pass.objective_contract();
    if !objective.has_required_deliverables() {
        return false;
    }
    if first_pass.classification().needs_confirm() {
        return true;
    }
    if source_deliverable_has_mixed_objective_roles(&objective.required_deliverables) {
        return true;
    }
    !matches!(
        objective.deliverable_kind,
        ObjectiveDeliverableKind::SourceFiles | ObjectiveDeliverableKind::Answer
    )
}

fn source_deliverable_has_mixed_objective_roles(required: &[ArtifactRole]) -> bool {
    required
        .iter()
        .any(|role| matches!(role, ArtifactRole::UsageDocs | ArtifactRole::DataOutput))
}

fn task_kind_from_profile(profile: &ProjectProfileConfirmation) -> Option<TaskKind> {
    match profile.deliverable_kind? {
        ProfileDeliverableKind::Code => Some(TaskKind::Coding),
        ProfileDeliverableKind::Document => Some(TaskKind::Docs),
        ProfileDeliverableKind::Data => Some(TaskKind::Data),
        ProfileDeliverableKind::ResearchReport => Some(TaskKind::Research),
        ProfileDeliverableKind::CommandObservation => Some(TaskKind::Ops),
        ProfileDeliverableKind::None | ProfileDeliverableKind::Unknown => None,
    }
}

fn required_artifact_role(profile: &ProjectProfileConfirmation) -> Option<ArtifactRole> {
    match profile.deliverable_kind? {
        ProfileDeliverableKind::Code => Some(ArtifactRole::Implementation),
        ProfileDeliverableKind::Document | ProfileDeliverableKind::ResearchReport => {
            Some(ArtifactRole::UsageDocs)
        }
        ProfileDeliverableKind::Data => Some(ArtifactRole::DataOutput),
        ProfileDeliverableKind::CommandObservation
        | ProfileDeliverableKind::None
        | ProfileDeliverableKind::Unknown => None,
    }
}

fn verification_requirement(
    profile: &ProjectProfileConfirmation,
) -> Option<VerificationRequirement> {
    match profile.evidence_kind? {
        ProfileEvidenceKind::TestRun => Some(VerificationRequirement::Required {
            preferred_runner: preferred_runner_from_profile(profile)
                .or_else(|| profile.language.and_then(preferred_runner_for_language)),
        }),
        ProfileEvidenceKind::ContentCheck | ProfileEvidenceKind::SchemaCheck => {
            Some(VerificationRequirement::ArtifactOnly)
        }
        ProfileEvidenceKind::CommandObservation | ProfileEvidenceKind::SourceFetch => {
            let runner = preferred_runner_from_profile(profile)?;
            Some(VerificationRequirement::Required {
                preferred_runner: Some(runner),
            })
        }
        ProfileEvidenceKind::None => Some(VerificationRequirement::NotRequired),
        ProfileEvidenceKind::Unknown => None,
    }
}

fn preferred_runner_from_profile(profile: &ProjectProfileConfirmation) -> Option<&'static str> {
    let runner = profile.preferred_runner.as_deref()?.trim();
    match runner {
        "cargo test" => Some("cargo test"),
        "npm test" => Some("npm test"),
        "pytest" => Some("pytest"),
        _ => None,
    }
}

fn forbids_implementation_artifact(profile: &ProjectProfileConfirmation) -> bool {
    profile
        .forbidden_artifacts
        .iter()
        .any(|artifact| matches!(artifact, ForbiddenArtifact::SourceCode))
        || matches!(
            profile.deliverable_kind,
            Some(
                ProfileDeliverableKind::Document
                    | ProfileDeliverableKind::Data
                    | ProfileDeliverableKind::ResearchReport
                    | ProfileDeliverableKind::CommandObservation
                    | ProfileDeliverableKind::None
            )
        )
}

fn forbids_test_artifacts(profile: &ProjectProfileConfirmation) -> bool {
    profile
        .forbidden_artifacts
        .iter()
        .any(|artifact| matches!(artifact, ForbiddenArtifact::Tests))
        || matches!(
            profile.evidence_kind,
            Some(
                ProfileEvidenceKind::ContentCheck
                    | ProfileEvidenceKind::SchemaCheck
                    | ProfileEvidenceKind::CommandObservation
                    | ProfileEvidenceKind::SourceFetch
                    | ProfileEvidenceKind::None
            )
        )
}

fn forbids_setup_artifact(profile: &ProjectProfileConfirmation) -> bool {
    profile
        .forbidden_artifacts
        .iter()
        .any(|artifact| matches!(artifact, ForbiddenArtifact::Setup))
        || (profile.needs_environment_setup == Some(false)
            && matches!(
                profile.deliverable_kind,
                Some(
                    ProfileDeliverableKind::Document
                        | ProfileDeliverableKind::Data
                        | ProfileDeliverableKind::ResearchReport
                )
            ))
}

fn artifact_obligations(
    profile: &ProjectProfileConfirmation,
    required_role: Option<ArtifactRole>,
) -> Vec<ArtifactObligation> {
    let Some(role) = required_role else {
        return Vec::new();
    };
    profile
        .primary_artifacts
        .iter()
        .map(|path| ArtifactObligation::file(role, path.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::project_profile::parse_project_profile_confirmation;
    use super::*;

    #[test]
    fn low_confidence_confirmation_is_not_projected() {
        let profile = parse_project_profile_confirmation(
            r#"{
                "language":"docs",
                "shape":"documentation",
                "deliverable_kind":"document",
                "primary_artifacts":["README.md"],
                "forbidden_artifacts":["source_code"],
                "evidence_kind":"content_check",
                "needs_environment_setup":false,
                "preferred_runner":null,
                "confidence":0.2,
                "reason":"uncertain"
            }"#,
        )
        .expect("profile");

        assert!(contract_inputs_from_confirmation(Some(&profile)).is_none());
    }

    #[test]
    fn document_confirmation_projects_contract_inputs() {
        let profile = parse_project_profile_confirmation(
            r#"{
                "language":"docs",
                "shape":"documentation",
                "deliverable_kind":"document",
                "primary_artifacts":["README.md"],
                "forbidden_artifacts":["source_code","tests","setup"],
                "evidence_kind":"content_check",
                "needs_environment_setup":false,
                "preferred_runner":null,
                "confidence":0.9,
                "reason":"README only"
            }"#,
        )
        .expect("profile");
        let inputs = contract_inputs_from_confirmation(Some(&profile)).expect("inputs");

        assert_eq!(inputs.task_kind, Some(TaskKind::Docs));
        assert_eq!(inputs.required_role, Some(ArtifactRole::UsageDocs));
        assert_eq!(
            inputs.verification,
            Some(VerificationRequirement::ArtifactOnly)
        );
        assert_eq!(
            inputs.artifact_obligations,
            vec![ArtifactObligation::file(
                ArtifactRole::UsageDocs,
                "README.md"
            )]
        );
        assert!(inputs.forbids_implementation);
        assert!(inputs.forbids_tests);
        assert!(inputs.forbids_setup);
    }

    #[test]
    fn profile_confirm_uses_objective_uncertainty_not_task_kind_gate() {
        let docs = TaskContract::from_request("Write README.md with setup and usage sections.");
        let coding = TaskContract::from_request(
            "Create a Rust library in src/lib.rs with tests and run cargo test.",
        );
        let mixed = TaskContract::from_request(
            "Create a Rust CLI and also write README.md with usage instructions.",
        );

        assert!(should_confirm_project_profile(&docs));
        assert!(!should_confirm_project_profile(&coding));
        assert!(should_confirm_project_profile(&mixed));
    }
}
