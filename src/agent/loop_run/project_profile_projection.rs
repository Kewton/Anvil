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
    ArtifactObligation, ArtifactRole, ObjectiveDeliverableKind, ObjectiveEvidenceKind,
    ProjectLanguage, ProjectShape, TaskContract, TaskKind, VerificationRequirement,
    preferred_runner_for_language,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProjectProfileAdoptionDecision {
    Adopt,
    RejectLowConfidence,
    RejectContradictoryObjective,
}

impl ProjectProfileAdoptionDecision {
    pub(super) fn is_adopted(self) -> bool {
        matches!(self, Self::Adopt)
    }

    pub(super) fn fallback_reason(self) -> Option<&'static str> {
        match self {
            Self::Adopt => None,
            Self::RejectLowConfidence => Some("unusable_or_low_confidence"),
            Self::RejectContradictoryObjective => Some("objective_contract_conflict"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProjectProfileContractInputs {
    pub(super) task_kind: Option<TaskKind>,
    pub(super) language: Option<ProjectLanguage>,
    pub(super) shape: Option<ProjectShape>,
    pub(super) verification: Option<VerificationRequirement>,
    pub(super) evidence_kind: Option<ObjectiveEvidenceKind>,
    pub(super) required_role: Option<ArtifactRole>,
    pub(super) artifact_obligations: Vec<ArtifactObligation>,
    pub(super) forbids_implementation: bool,
    pub(super) forbids_tests: bool,
    pub(super) forbids_setup: bool,
    pub(super) confidence: f32,
}

pub(super) fn project_profile_adoption_decision(
    profile: Option<&ProjectProfileConfirmation>,
    first_pass: &TaskContract,
) -> ProjectProfileAdoptionDecision {
    let Some(profile) = profile else {
        return ProjectProfileAdoptionDecision::RejectLowConfidence;
    };
    if !project_profile::confirmation_is_authoritative(profile) {
        return ProjectProfileAdoptionDecision::RejectLowConfidence;
    }
    if profile_conflicts_with_first_pass_objective(profile, first_pass) {
        return ProjectProfileAdoptionDecision::RejectContradictoryObjective;
    }
    ProjectProfileAdoptionDecision::Adopt
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
        evidence_kind: objective_evidence_kind_from_profile(profile),
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

fn profile_conflicts_with_first_pass_objective(
    profile: &ProjectProfileConfirmation,
    first_pass: &TaskContract,
) -> bool {
    let Some(profile_deliverable) = objective_deliverable_kind_from_profile(profile) else {
        return false;
    };
    let objective = first_pass.objective_contract();
    if !objective.has_required_deliverables() {
        return false;
    }
    if profile_deliverable == ObjectiveDeliverableKind::SourceFiles {
        if source_deliverable_has_mixed_objective_roles(&objective.required_deliverables) {
            return true;
        }
        return !matches!(
            objective.deliverable_kind,
            ObjectiveDeliverableKind::SourceFiles
        );
    }

    if profile_deliverable != ObjectiveDeliverableKind::SourceFiles
        && matches!(
            objective.deliverable_kind,
            ObjectiveDeliverableKind::SourceFiles
        )
        && first_pass_requires_code_setup_or_test(first_pass)
    {
        return true;
    }

    false
}

fn first_pass_requires_code_setup_or_test(first_pass: &TaskContract) -> bool {
    first_pass.required_artifacts.iter().any(|role| {
        matches!(
            role,
            ArtifactRole::Implementation | ArtifactRole::Test | ArtifactRole::Setup
        )
    }) || first_pass
        .required_artifact_identities
        .iter()
        .any(|identity| {
            matches!(
                identity.role,
                ArtifactRole::Implementation | ArtifactRole::Test | ArtifactRole::Setup
            )
        })
}

fn objective_deliverable_kind_from_profile(
    profile: &ProjectProfileConfirmation,
) -> Option<ObjectiveDeliverableKind> {
    match profile.deliverable_kind? {
        ProfileDeliverableKind::Code => Some(ObjectiveDeliverableKind::SourceFiles),
        ProfileDeliverableKind::Document => Some(ObjectiveDeliverableKind::DocumentSections),
        ProfileDeliverableKind::Data => Some(ObjectiveDeliverableKind::OutputFile),
        ProfileDeliverableKind::ResearchReport => Some(ObjectiveDeliverableKind::ResearchNotes),
        ProfileDeliverableKind::CommandObservation => {
            Some(ObjectiveDeliverableKind::CommandObservation)
        }
        ProfileDeliverableKind::None | ProfileDeliverableKind::Unknown => None,
    }
}

fn objective_evidence_kind_from_profile(
    profile: &ProjectProfileConfirmation,
) -> Option<ObjectiveEvidenceKind> {
    match profile.evidence_kind? {
        ProfileEvidenceKind::TestRun => Some(ObjectiveEvidenceKind::TestRun),
        ProfileEvidenceKind::ContentCheck => Some(ObjectiveEvidenceKind::ContentCheck),
        ProfileEvidenceKind::SchemaCheck => Some(ObjectiveEvidenceKind::SchemaCheck),
        ProfileEvidenceKind::CommandObservation => {
            Some(ObjectiveEvidenceKind::SafetyBoundaryEvidence)
        }
        ProfileEvidenceKind::SourceFetch => Some(ObjectiveEvidenceKind::SourceFetchEvidence),
        ProfileEvidenceKind::None | ProfileEvidenceKind::Unknown => None,
    }
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
            Some(VerificationRequirement::Required {
                preferred_runner: preferred_runner_from_profile(profile),
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
        || (matches!(
            profile.deliverable_kind,
            Some(
                ProfileDeliverableKind::Document
                    | ProfileDeliverableKind::Data
                    | ProfileDeliverableKind::ResearchReport
            )
        ) && !matches!(profile.evidence_kind, Some(ProfileEvidenceKind::TestRun)))
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

    fn node_csv_markdown_prompt() -> &'static str {
        r#"# CSV to JSON CLI Tool

A Node.js CLI tool that converts CSV input to JSON array output.

## Features
- Support stdin or file path as input
- Parse quoted commas and escaped quotes correctly
- Fail clearly on malformed rows
- Output JSON array to stdout

## Usage

### From file path
```bash
node index.js data.csv
```

### From stdin
```bash
cat data.csv | node index.js
```

### Example
Given `data.csv`:
```csv
name,age,city
"Alice",30,"New York"
"Bob",25,"Los Angeles"
```

Running `node index.js data.csv` outputs:
```json
[
  {"name": "Alice", "age": "30", "city": "New York"},
  {"name": "Bob", "age": "25", "city": "Los Angeles"}
]
```

## Installation
```bash
npm install
```

## Testing
```bash
npm test
```"#
    }

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
            inputs.evidence_kind,
            Some(ObjectiveEvidenceKind::ContentCheck)
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
    fn document_with_command_observation_keeps_independent_evidence_kind() {
        let profile = parse_project_profile_confirmation(
            r#"{
                "language":"unknown",
                "shape":"cli",
                "deliverable_kind":"document",
                "primary_artifacts":["ops-observation.md"],
                "forbidden_artifacts":["source_code","tests","setup"],
                "evidence_kind":"command_observation",
                "needs_environment_setup":false,
                "preferred_runner":null,
                "confidence":1.0,
                "reason":"document must be grounded in a command observation"
            }"#,
        )
        .expect("profile");
        let inputs = contract_inputs_from_confirmation(Some(&profile)).expect("inputs");

        assert_eq!(inputs.task_kind, Some(TaskKind::Docs));
        assert_eq!(
            inputs.evidence_kind,
            Some(ObjectiveEvidenceKind::SafetyBoundaryEvidence)
        );
        assert_eq!(
            inputs.verification,
            Some(VerificationRequirement::Required {
                preferred_runner: None
            })
        );
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

    #[test]
    fn code_profile_conflicting_with_research_objective_is_not_adopted() {
        let first_pass = TaskContract::from_request(
            "Read source.md and write a concise research report to report.md. Do not create source code or tests.",
        );
        let profile = parse_project_profile_confirmation(
            r#"{
                "language":"rust",
                "shape":"library",
                "deliverable_kind":"code",
                "primary_artifacts":["src/main.rs"],
                "forbidden_artifacts":[],
                "evidence_kind":"test_run",
                "needs_environment_setup":true,
                "preferred_runner":"cargo test",
                "confidence":0.95,
                "reason":"first pass suggested Rust"
            }"#,
        )
        .expect("profile");

        assert_eq!(
            project_profile_adoption_decision(Some(&profile), &first_pass),
            ProjectProfileAdoptionDecision::RejectContradictoryObjective
        );
    }

    #[test]
    fn document_profile_conflicting_with_readme_formatted_node_cli_is_not_adopted() {
        let first_pass = TaskContract::from_request(node_csv_markdown_prompt());
        assert_eq!(first_pass.task_kind, TaskKind::Coding);
        assert!(
            first_pass
                .required_artifacts
                .contains(&ArtifactRole::Implementation)
        );
        assert!(first_pass.required_artifacts.contains(&ArtifactRole::Setup));
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
                "confidence":0.95,
                "reason":"markdown prompt looked like docs"
            }"#,
        )
        .expect("profile");

        assert_eq!(
            project_profile_adoption_decision(Some(&profile), &first_pass),
            ProjectProfileAdoptionDecision::RejectContradictoryObjective
        );
    }

    #[test]
    fn command_observation_profile_conflicting_with_node_cli_is_not_adopted() {
        let first_pass = TaskContract::from_request(
            "Create a small Node.js CSV summarizer CLI. Implement the CLI, package.json, and tests. Run npm test before finishing.",
        );
        assert_eq!(first_pass.task_kind, TaskKind::Coding);
        assert!(
            first_pass
                .required_artifacts
                .contains(&ArtifactRole::Implementation)
        );
        assert!(first_pass.required_artifacts.contains(&ArtifactRole::Test));
        let profile = parse_project_profile_confirmation(
            r#"{
                "language":"rust",
                "shape":"cli",
                "deliverable_kind":"command_observation",
                "primary_artifacts":[],
                "forbidden_artifacts":[],
                "evidence_kind":"test_run",
                "needs_environment_setup":true,
                "preferred_runner":null,
                "confidence":1.0,
                "reason":"misread run npm test as the deliverable"
            }"#,
        )
        .expect("profile");

        assert_eq!(
            project_profile_adoption_decision(Some(&profile), &first_pass),
            ProjectProfileAdoptionDecision::RejectContradictoryObjective
        );
    }

    #[test]
    fn data_profile_can_override_first_pass_setup_objective() {
        let first_pass = TaskContract::from_request(
            "Read inventory.csv and write summary.json containing total_count and total_value. Do not create source code, tests, scripts, Cargo.toml, package.json, or setup files.",
        );
        let profile = parse_project_profile_confirmation(
            r#"{
                "language":"unknown",
                "shape":"unknown",
                "deliverable_kind":"data",
                "primary_artifacts":["summary.json"],
                "forbidden_artifacts":["source_code","tests","setup"],
                "evidence_kind":"schema_check",
                "needs_environment_setup":false,
                "preferred_runner":null,
                "confidence":0.95,
                "reason":"structured JSON output"
            }"#,
        )
        .expect("profile");

        assert_eq!(
            project_profile_adoption_decision(Some(&profile), &first_pass),
            ProjectProfileAdoptionDecision::Adopt
        );
    }

    #[test]
    fn adopted_data_profile_without_primary_artifacts_keeps_request_output_path_not_setup() {
        let request = "Read inventory.csv and write summary.json containing total_count and total_value. Do not create source code, tests, scripts, Cargo.toml, package.json, or setup files.";
        let profile = parse_project_profile_confirmation(
            r#"{
                "language":"unknown",
                "shape":"unknown",
                "deliverable_kind":"data",
                "primary_artifacts":[],
                "forbidden_artifacts":["source_code","tests","setup"],
                "evidence_kind":"schema_check",
                "needs_environment_setup":false,
                "preferred_runner":null,
                "confidence":0.95,
                "reason":"structured JSON output"
            }"#,
        )
        .expect("profile");
        let contract =
            TaskContract::from_request_with_kind_and_project_profile(request, None, Some(&profile));

        assert_eq!(contract.task_kind, TaskKind::Data);
        assert!(
            contract
                .required_artifacts
                .contains(&ArtifactRole::DataOutput)
        );
        assert_eq!(
            contract.required_identities_for_role(ArtifactRole::DataOutput),
            vec![&ArtifactObligation::file(
                ArtifactRole::DataOutput,
                "summary.json"
            )]
        );
        assert!(
            contract
                .required_identities_for_role(ArtifactRole::Setup)
                .is_empty()
        );
    }

    #[test]
    fn adopted_data_profile_filters_primary_input_paths_through_output_context() {
        let request = "Read inventory.csv and write summary.json containing total_count and total_value. Do not create source code, tests, scripts, Cargo.toml, package.json, or setup files.";
        let profile = parse_project_profile_confirmation(
            r#"{
                "language":"unknown",
                "shape":"unknown",
                "deliverable_kind":"data",
                "primary_artifacts":["inventory.csv"],
                "forbidden_artifacts":["source_code","tests","setup"],
                "evidence_kind":"schema_check",
                "needs_environment_setup":false,
                "preferred_runner":null,
                "confidence":0.95,
                "reason":"structured JSON output"
            }"#,
        )
        .expect("profile");
        let contract =
            TaskContract::from_request_with_kind_and_project_profile(request, None, Some(&profile));

        assert_eq!(
            contract.required_identities_for_role(ArtifactRole::DataOutput),
            vec![&ArtifactObligation::file(
                ArtifactRole::DataOutput,
                "summary.json"
            )]
        );
    }

    #[test]
    fn non_source_profile_without_test_run_forbids_setup_even_when_setup_flag_drifts_true() {
        let profile = parse_project_profile_confirmation(
            r#"{
                "language":"unknown",
                "shape":"unknown",
                "deliverable_kind":"data",
                "primary_artifacts":["summary.json"],
                "forbidden_artifacts":[],
                "evidence_kind":"schema_check",
                "needs_environment_setup":true,
                "preferred_runner":null,
                "confidence":0.95,
                "reason":"data output"
            }"#,
        )
        .expect("profile");
        let inputs = contract_inputs_from_confirmation(Some(&profile)).expect("inputs");

        assert_eq!(inputs.task_kind, Some(TaskKind::Data));
        assert!(inputs.forbids_setup);
    }
}
