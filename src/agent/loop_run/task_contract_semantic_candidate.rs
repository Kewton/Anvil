//! Shadow semantic-candidate vocabulary for future contract admission.
//!
//! This module is intentionally read-only: candidates can be constructed and
//! compared with the sealed `TaskContract`, but they do not influence
//! completion, recovery, or tool policy. WP2 uses this as the typed shape that
//! later LLM-assisted interpretation can fill before deterministic admission.

#![allow(dead_code)]

use super::authoring_style::AuthoringStyleDecision;
use super::task_contract::{
    ArtifactObligation, ArtifactRole, DeliverableFormat, DeliverableKind, DeliverableSchema,
    ObjectiveDeliverableKind, ObjectiveEvidenceKind, ObjectiveKind, ProjectLanguage, ProjectShape,
    StructuredRecordSchema, TaskContract, TaskKind,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SemanticCandidateOrigin {
    DeterministicShadow,
    LlmShadow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SemanticDeliverableCandidate {
    pub(super) role: ArtifactRole,
    pub(super) path: Option<String>,
    pub(super) kind: DeliverableKind,
    pub(super) format: Option<DeliverableFormat>,
    pub(super) schema: Option<DeliverableSchema>,
}

impl SemanticDeliverableCandidate {
    fn from_obligation(obligation: &ArtifactObligation) -> Self {
        Self {
            role: obligation.role,
            path: Some(obligation.path.clone()),
            kind: obligation.kind,
            format: obligation.format.clone(),
            schema: obligation.schema.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SemanticProjectProfile {
    pub(super) language: Option<ProjectLanguage>,
    pub(super) shape: Option<ProjectShape>,
    pub(super) runtime_expectations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SemanticExpectation {
    pub(super) name: String,
    pub(super) value: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct SemanticAmbiguity {
    pub(super) confidence: f32,
    pub(super) reasons: Vec<String>,
}

impl SemanticAmbiguity {
    fn from_contract(contract: &TaskContract) -> Self {
        Self {
            confidence: contract.classification().confidence,
            reasons: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct SemanticCandidate {
    pub(super) origin: SemanticCandidateOrigin,
    pub(super) objective_kind: ObjectiveKind,
    pub(super) deliverable_kind: ObjectiveDeliverableKind,
    pub(super) evidence_kind: ObjectiveEvidenceKind,
    pub(super) deliverable_candidates: Vec<SemanticDeliverableCandidate>,
    pub(super) artifact_identities: Vec<ArtifactObligation>,
    pub(super) project_profile: SemanticProjectProfile,
    pub(super) api_expectations: Vec<SemanticExpectation>,
    pub(super) schema_expectations: Vec<SemanticExpectation>,
    pub(super) authoring_style: AuthoringStyleDecision,
    pub(super) compatibility_risks: Vec<String>,
    pub(super) ambiguity: SemanticAmbiguity,
}

impl SemanticCandidate {
    pub(super) fn deterministic_shadow_from_contract(contract: &TaskContract) -> Self {
        let objective = contract.objective_contract();
        let artifact_identities = contract.required_artifact_identities.clone();
        let schema_expectations = schema_expectations_from_obligations(&artifact_identities);
        let api_expectations = api_expectations_from_contract(contract);
        Self {
            origin: SemanticCandidateOrigin::DeterministicShadow,
            objective_kind: objective.objective_kind,
            deliverable_kind: objective.deliverable_kind,
            evidence_kind: objective.evidence_kind,
            deliverable_candidates: artifact_identities
                .iter()
                .map(SemanticDeliverableCandidate::from_obligation)
                .collect(),
            artifact_identities,
            project_profile: SemanticProjectProfile {
                language: None,
                shape: None,
                runtime_expectations: Vec::new(),
            },
            api_expectations,
            schema_expectations,
            authoring_style: contract.authoring_style_decision,
            compatibility_risks: compatibility_risks_from_contract(contract),
            ambiguity: SemanticAmbiguity::from_contract(contract),
        }
    }

    pub(super) fn disagreements_with_contract(
        &self,
        contract: &TaskContract,
    ) -> Vec<CandidateContractDisagreement> {
        let objective = contract.objective_contract();
        let mut disagreements = Vec::new();
        push_disagreement_if(
            &mut disagreements,
            "objective_kind",
            self.objective_kind.label(),
            objective.objective_kind.label(),
        );
        push_disagreement_if(
            &mut disagreements,
            "deliverable_kind",
            self.deliverable_kind.label(),
            objective.deliverable_kind.label(),
        );
        push_disagreement_if(
            &mut disagreements,
            "evidence_kind",
            self.evidence_kind.label(),
            objective.evidence_kind.label(),
        );
        push_disagreement_if(
            &mut disagreements,
            "artifact_identities",
            &artifact_identity_signature(&self.artifact_identities),
            &artifact_identity_signature(&contract.required_artifact_identities),
        );
        disagreements
    }
}

pub(super) fn limited_semantic_candidate_adoption_enabled(contract: &TaskContract) -> bool {
    matches!(
        contract.task_kind,
        TaskKind::Docs | TaskKind::Data | TaskKind::Ops
    )
}

fn compatibility_risks_from_contract(contract: &TaskContract) -> Vec<String> {
    let mut risks = Vec::new();
    if contract.verification_required && contract.evidence_command_hint().is_none() {
        risks.push("evidence_required_without_explicit_runner_hint".to_string());
    }
    if contract.required_artifact_identities.is_empty() && !contract.required_artifacts.is_empty() {
        risks.push("required_roles_without_explicit_artifact_identity".to_string());
    }
    risks
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CandidateContractDisagreement {
    pub(super) field: &'static str,
    pub(super) candidate: String,
    pub(super) contract: String,
}

fn api_expectations_from_contract(contract: &TaskContract) -> Vec<SemanticExpectation> {
    contract
        .api_contract_expectations
        .iter()
        .map(|expectation| SemanticExpectation {
            name: format!(
                "{} {}",
                expectation.method.label(),
                super::task_contract::mask_and_cap_recovery_field(&expectation.path)
            ),
            value: expectation.summary(),
        })
        .collect()
}

fn schema_expectations_from_obligations(
    obligations: &[ArtifactObligation],
) -> Vec<SemanticExpectation> {
    obligations
        .iter()
        .filter_map(|obligation| {
            obligation
                .schema
                .as_ref()
                .map(|schema| SemanticExpectation {
                    name: format!("{}:{}", obligation.role.label(), obligation.path),
                    value: schema_signature(schema),
                })
        })
        .collect()
}

fn schema_signature(schema: &DeliverableSchema) -> String {
    match schema {
        DeliverableSchema::StructuredRecord(StructuredRecordSchema {
            columns,
            expected_rows,
            column_policy,
        }) => format!(
            "structured_record columns={} expected_rows={} column_policy={}",
            columns.join("|"),
            expected_rows.len(),
            column_policy.label()
        ),
        DeliverableSchema::JsonFields(fields) => format!("json_fields {}", fields.join("|")),
        DeliverableSchema::RequiredSections(sections) => {
            format!("required_sections {}", sections.join("|"))
        }
    }
}

fn push_disagreement_if(
    disagreements: &mut Vec<CandidateContractDisagreement>,
    field: &'static str,
    candidate: &str,
    contract: &str,
) {
    if candidate != contract {
        disagreements.push(CandidateContractDisagreement {
            field,
            candidate: candidate.to_string(),
            contract: contract.to_string(),
        });
    }
}

fn artifact_identity_signature(obligations: &[ArtifactObligation]) -> String {
    let mut parts = obligations
        .iter()
        .map(|obligation| format!("{}:{}", obligation.role.label(), obligation.path))
        .collect::<Vec<_>>();
    parts.sort();
    parts.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_shadow_candidate_matches_current_contract() {
        let contract = TaskContract::from_request(
            "Create a Rust CLI. Include Cargo.toml, implementation, tests, and README.md.",
        );
        let candidate = SemanticCandidate::deterministic_shadow_from_contract(&contract);

        assert_eq!(
            candidate.origin,
            SemanticCandidateOrigin::DeterministicShadow
        );
        assert_eq!(candidate.objective_kind, ObjectiveKind::Coding);
        assert!(
            candidate
                .artifact_identities
                .iter()
                .any(|identity| identity.path == "Cargo.toml")
        );
        assert!(candidate.disagreements_with_contract(&contract).is_empty());
        assert_eq!(candidate.authoring_style, contract.authoring_style_decision);
    }

    #[test]
    fn standalone_data_candidate_carries_schema_expectation() {
        let contract = TaskContract::from_request("Generate output.csv with columns id and total.");
        let candidate = SemanticCandidate::deterministic_shadow_from_contract(&contract);

        assert_eq!(candidate.objective_kind, ObjectiveKind::Data);
        assert_eq!(candidate.evidence_kind, ObjectiveEvidenceKind::SchemaCheck);
        assert!(
            candidate
                .schema_expectations
                .iter()
                .any(|expectation| expectation.value.contains("id|total"))
        );
    }

    #[test]
    fn api_candidate_carries_http_contract_expectation() {
        let contract = TaskContract::from_request(
            "Create an HTTP API with POST /notes accepting JSON with title and body returning id.",
        );
        let candidate = SemanticCandidate::deterministic_shadow_from_contract(&contract);

        assert_eq!(candidate.api_expectations.len(), 1);
        assert_eq!(candidate.api_expectations[0].name, "POST /notes");
        assert!(
            candidate.api_expectations[0]
                .value
                .contains("request_json_body_fields=title|body"),
            "{:?}",
            candidate.api_expectations[0]
        );
    }

    #[test]
    fn coding_candidate_records_missing_explicit_runner_risk() {
        let contract = TaskContract::from_request("Create main.py and tests/test_main.py.");
        let candidate = SemanticCandidate::deterministic_shadow_from_contract(&contract);

        assert!(
            candidate
                .compatibility_risks
                .iter()
                .any(|risk| risk == "evidence_required_without_explicit_runner_hint")
        );
    }

    #[test]
    fn shadow_admission_logs_candidate_contract_disagreement() {
        use super::super::task_contract_admission::{
            SemanticCandidateAdmissionInput, SemanticCandidateAdmissionStatus,
            admit_semantic_candidate,
        };

        let contract = TaskContract::from_request("Generate output.csv with columns id and total.");
        let mut candidate = SemanticCandidate::deterministic_shadow_from_contract(&contract);
        candidate.objective_kind = ObjectiveKind::Coding;

        let decision = admit_semantic_candidate(SemanticCandidateAdmissionInput {
            candidate: &candidate,
            contract: &contract,
            allow_equivalent_current_behavior: false,
        });

        assert_eq!(decision.status, SemanticCandidateAdmissionStatus::Rejected);
        assert_eq!(decision.disagreements.len(), 1);
        assert_eq!(decision.disagreements[0].field, "objective_kind");
        assert!(
            decision
                .log_lines()
                .iter()
                .any(|line| line.contains("semantic_candidate_disagreement"))
        );
    }

    #[test]
    fn limited_adoption_accepts_equivalent_docs_candidate() {
        let contract = TaskContract::from_request("Create README.md with Usage and Validation.");
        let candidate = SemanticCandidate::deterministic_shadow_from_contract(&contract);

        let decision = super::super::task_contract_admission::admit_semantic_candidate(
            super::super::task_contract_admission::SemanticCandidateAdmissionInput {
                candidate: &candidate,
                contract: &contract,
                allow_equivalent_current_behavior: limited_semantic_candidate_adoption_enabled(
                    &contract,
                ),
            },
        );

        assert!(limited_semantic_candidate_adoption_enabled(&contract));
        assert!(decision.is_authoritative());
        assert_eq!(
            decision.reason_labels(),
            vec!["equivalent_stable_task_kind"]
        );
    }

    #[test]
    fn limited_adoption_accepts_equivalent_data_and_ops_candidates() {
        for kind in [TaskKind::Data, TaskKind::Ops] {
            let contract =
                TaskContract::from_request_with_kind("Create the requested artifact.", Some(kind));
            let candidate = SemanticCandidate::deterministic_shadow_from_contract(&contract);

            let decision = super::super::task_contract_admission::admit_semantic_candidate(
                super::super::task_contract_admission::SemanticCandidateAdmissionInput {
                    candidate: &candidate,
                    contract: &contract,
                    allow_equivalent_current_behavior: limited_semantic_candidate_adoption_enabled(
                        &contract,
                    ),
                },
            );

            assert_eq!(contract.task_kind, kind);
            assert!(limited_semantic_candidate_adoption_enabled(&contract));
            assert!(decision.is_authoritative());
        }
    }

    #[test]
    fn limited_adoption_keeps_research_authoring_and_coding_shadow_only() {
        for kind in [TaskKind::Research, TaskKind::Authoring, TaskKind::Coding] {
            let contract =
                TaskContract::from_request_with_kind("Create the requested artifact.", Some(kind));
            let candidate = SemanticCandidate::deterministic_shadow_from_contract(&contract);

            let decision = super::super::task_contract_admission::admit_semantic_candidate(
                super::super::task_contract_admission::SemanticCandidateAdmissionInput {
                    candidate: &candidate,
                    contract: &contract,
                    allow_equivalent_current_behavior: limited_semantic_candidate_adoption_enabled(
                        &contract,
                    ),
                },
            );

            assert_eq!(contract.task_kind, kind);
            assert!(!limited_semantic_candidate_adoption_enabled(&contract));
            assert!(!decision.is_authoritative());
            assert_eq!(decision.reason_labels(), vec!["shadow_only"]);
        }
    }

    #[test]
    fn limited_adoption_does_not_accept_unstyled_coding_candidate() {
        let contract = TaskContract::from_request("Implement feature X and add tests.");
        let candidate = SemanticCandidate::deterministic_shadow_from_contract(&contract);

        let decision = super::super::task_contract_admission::admit_semantic_candidate(
            super::super::task_contract_admission::SemanticCandidateAdmissionInput {
                candidate: &candidate,
                contract: &contract,
                allow_equivalent_current_behavior: limited_semantic_candidate_adoption_enabled(
                    &contract,
                ),
            },
        );

        assert_eq!(contract.task_kind, TaskKind::Coding);
        assert!(!limited_semantic_candidate_adoption_enabled(&contract));
        assert!(!decision.is_authoritative());
    }
}
