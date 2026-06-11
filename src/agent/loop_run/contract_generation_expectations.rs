//! Typed expectation summaries for contract-bound generation.
//!
//! This module owns the compact source/test/schema/manifest/evidence projection
//! used by `contract_bound_generation`. It stays derived from the sealed
//! execution contract and does not classify raw prompts.

use std::path::{Path, PathBuf};

use super::api_contract_expectation::{ApiContractExpectation, api_contract_summary};
use super::task_contract::{
    ArtifactRole, DeliverableFormat, DeliverableSchema, StructuredRecordSchema,
};
use super::worker_contract::{
    ExecutionDeliverable, ExecutionEvidence, PublicContract, TaskExecutionContract,
};

const MAX_EXPECTATION_ITEMS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ContractGenerationExpectations {
    source_api: Option<String>,
    test_api: Option<String>,
    api_contracts: Option<String>,
    schemas: Vec<String>,
    manifests: Vec<String>,
    evidence: String,
    allowed_files: Vec<String>,
}

impl ContractGenerationExpectations {
    pub(super) fn from_execution(execution: &TaskExecutionContract) -> Self {
        let source_api = source_api_expectation(execution);
        let test_api = test_api_expectation(execution);
        let api_contracts = api_contract_expectations(&execution.api_contract_expectations);
        let schemas = schema_expectations(&execution.deliverables);
        let manifests = manifest_expectations(&execution.deliverables);
        let evidence = evidence_summary(&execution.evidence);
        let allowed_files = allowed_file_expectations(&execution.constraints.allowed_files);
        Self {
            source_api,
            test_api,
            api_contracts,
            schemas,
            manifests,
            evidence,
            allowed_files,
        }
    }

    pub(super) fn summary(&self) -> String {
        [
            self.source_api
                .as_ref()
                .map(|summary| format!("source_api={summary}")),
            self.test_api
                .as_ref()
                .map(|summary| format!("test_api={summary}")),
            self.api_contracts
                .as_ref()
                .map(|summary| format!("api_contracts={summary}")),
            (!self.schemas.is_empty()).then(|| format!("schemas={}", self.schemas.join(","))),
            (!self.manifests.is_empty()).then(|| format!("manifests={}", self.manifests.join(","))),
            Some(self.evidence.clone()),
            (!self.allowed_files.is_empty())
                .then(|| format!("allowed_files={}", self.allowed_files.join("|"))),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(";")
    }
}

pub(super) fn declared_artifacts_summary(deliverables: &[ExecutionDeliverable]) -> String {
    if deliverables.is_empty() {
        return "none".to_string();
    }
    deliverables
        .iter()
        .take(MAX_EXPECTATION_ITEMS)
        .map(declared_artifact_summary)
        .collect::<Vec<_>>()
        .join(",")
}

pub(super) fn declared_expectations_summary(execution: &TaskExecutionContract) -> String {
    ContractGenerationExpectations::from_execution(execution).summary()
}

fn api_contract_expectations(expectations: &[ApiContractExpectation]) -> Option<String> {
    api_contract_summary(expectations)
}

fn source_api_expectation(execution: &TaskExecutionContract) -> Option<String> {
    if !execution
        .deliverables
        .iter()
        .any(|deliverable| deliverable.role == ArtifactRole::Implementation)
    {
        return None;
    }
    let mut parts = Vec::new();
    if let Some(summary) = public_contract_summary(&execution.public_contract) {
        parts.push(summary);
    }
    if let Some(source_paths) =
        role_paths_summary(&execution.deliverables, ArtifactRole::Implementation)
    {
        parts.push(format!("paths={source_paths}"));
    }
    (!parts.is_empty()).then(|| parts.join(","))
}

fn test_api_expectation(execution: &TaskExecutionContract) -> Option<String> {
    if !execution
        .deliverables
        .iter()
        .any(|deliverable| deliverable.role == ArtifactRole::Test)
    {
        return None;
    }
    let mut parts = Vec::new();
    if let Some(source_paths) =
        role_paths_summary(&execution.deliverables, ArtifactRole::Implementation)
    {
        parts.push(format!("targets={source_paths}"));
    }
    if let Some(test_paths) = role_paths_summary(&execution.deliverables, ArtifactRole::Test) {
        parts.push(format!("paths={test_paths}"));
    }
    if let Some(summary) = public_contract_summary(&execution.public_contract) {
        parts.push(format!("contract={summary}"));
    }
    parts.push("assertion_policy=no_exact_diagnostic_text_unless_declared".to_string());
    (!parts.is_empty()).then(|| parts.join(","))
}

fn schema_expectations(deliverables: &[ExecutionDeliverable]) -> Vec<String> {
    deliverables
        .iter()
        .filter_map(|deliverable| {
            let path = deliverable
                .path
                .as_deref()
                .map(display_path)
                .unwrap_or_else(|| "declared".to_string());
            if let Some(schema) = deliverable.schema.as_ref() {
                return Some(format!("{path}:{}", deliverable_schema_summary(schema)));
            }
            if !deliverable.required_sections.is_empty() {
                return Some(format!(
                    "{path}:sections={}",
                    compact_value_list(deliverable.required_sections.iter().map(String::as_str))
                ));
            }
            None
        })
        .take(MAX_EXPECTATION_ITEMS)
        .collect()
}

fn manifest_expectations(deliverables: &[ExecutionDeliverable]) -> Vec<String> {
    deliverables
        .iter()
        .filter(|deliverable| deliverable.role == ArtifactRole::Setup)
        .filter_map(|deliverable| {
            deliverable
                .path
                .as_deref()
                .map(|path| format!("{}:supports_evidence_runtime", display_path(path)))
        })
        .take(MAX_EXPECTATION_ITEMS)
        .collect()
}

fn allowed_file_expectations(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .take(MAX_EXPECTATION_ITEMS)
        .map(|path| display_path(path))
        .collect()
}

fn role_paths_summary(deliverables: &[ExecutionDeliverable], role: ArtifactRole) -> Option<String> {
    let paths = deliverables.iter().filter_map(|deliverable| {
        (deliverable.role == role)
            .then_some(deliverable.path.as_deref())
            .flatten()
            .map(display_path)
    });
    let summary = compact_value_list(paths);
    (summary != "none").then_some(summary)
}

fn declared_artifact_summary(deliverable: &ExecutionDeliverable) -> String {
    let mut details = Vec::new();
    if let Some(kind) = deliverable.kind {
        details.push(format!("kind={}", kind.label()));
    }
    if let Some(format) = deliverable.format.as_ref() {
        details.push(format!("format={}", deliverable_format_label(format)));
    }
    if let Some(schema) = deliverable.schema.as_ref() {
        details.push(deliverable_schema_summary(schema));
    } else if !deliverable.required_sections.is_empty() {
        details.push(format!(
            "sections={}",
            compact_value_list(deliverable.required_sections.iter().map(String::as_str))
        ));
    }
    if !deliverable.acceptance_criteria.is_empty() {
        details.push(format!(
            "criteria={}",
            compact_value_list(deliverable.acceptance_criteria.iter().map(String::as_str))
        ));
    }

    let role = deliverable.role.label();
    let path = deliverable
        .path
        .as_deref()
        .map(display_path)
        .unwrap_or_else(|| "declared".to_string());
    if details.is_empty() {
        format!("{role}@{path}")
    } else {
        format!("{role}@{path}({})", details.join(";"))
    }
}

fn public_contract_summary(public_contract: &PublicContract) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(goal) = public_contract.goal() {
        parts.push(format!("goal={}", mask(goal)));
    }
    if !public_contract.signatures().is_empty() {
        parts.push(format!(
            "signatures={}",
            compact_value_list(public_contract.signatures().iter().map(String::as_str))
        ));
    }
    (!parts.is_empty()).then(|| parts.join(","))
}

fn evidence_summary(evidence: &ExecutionEvidence) -> String {
    let required = if evidence.required {
        "required"
    } else {
        "optional"
    };
    let mut out = format!("evidence={}({required})", evidence.kind.label());
    if let Some(command) = evidence.command.as_deref() {
        out.push_str(&format!(",command={}", mask(command)));
    }
    out
}

fn deliverable_format_label(format: &DeliverableFormat) -> &'static str {
    match format {
        DeliverableFormat::RustSource => "rust_source",
        DeliverableFormat::JavaScriptSource => "javascript_source",
        DeliverableFormat::TypeScriptSource => "typescript_source",
        DeliverableFormat::Markdown => "markdown",
        DeliverableFormat::Toml => "toml",
        DeliverableFormat::Json => "json",
        DeliverableFormat::Csv => "csv",
        DeliverableFormat::Tsv => "tsv",
        DeliverableFormat::JsonLines => "json_lines",
        DeliverableFormat::Text => "text",
    }
}

fn deliverable_schema_summary(schema: &DeliverableSchema) -> String {
    match schema {
        DeliverableSchema::StructuredRecord(record_schema) => {
            structured_record_schema_summary(record_schema)
        }
        DeliverableSchema::JsonFields(fields) => {
            format!(
                "schema=json_fields:{}",
                compact_value_list(fields.iter().map(String::as_str))
            )
        }
        DeliverableSchema::RequiredSections(sections) => format!(
            "schema=required_sections:{}",
            compact_value_list(sections.iter().map(String::as_str))
        ),
    }
}

fn structured_record_schema_summary(schema: &StructuredRecordSchema) -> String {
    let mut parts = vec![format!(
        "columns={}",
        compact_value_list(schema.columns.iter().map(String::as_str))
    )];
    if !schema.expected_rows.is_empty() {
        parts.push(format!(
            "expected_rows={}",
            compact_value_list(schema.expected_rows.iter().map(|row| row.join("|")))
        ));
    }
    format!("schema=structured_record:{}", parts.join(";"))
}

fn compact_value_list<I, S>(values: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out = values
        .into_iter()
        .take(MAX_EXPECTATION_ITEMS)
        .map(|value| mask(value.as_ref()))
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>();
    out.dedup();
    if out.is_empty() {
        "none".to_string()
    } else {
        out.join("|")
    }
}

fn display_path(path: &Path) -> String {
    mask(&path.to_string_lossy())
}

fn mask(value: &str) -> String {
    super::task_contract::mask_and_cap_recovery_field(value)
}

#[cfg(test)]
mod tests {
    use super::super::task_contract::TaskContract;
    use super::super::worker_contract::{RuntimeProfile, TaskExecutionContract};
    use super::*;

    #[test]
    fn coding_expectations_separate_source_and_test_api() {
        let contract = TaskContract::from_request(
            "Create calc.py and tests/test_calc.py with add(a, b). Verify with python -m unittest.",
        );
        let execution = TaskExecutionContract::from_task_contract(&contract)
            .with_runtime_profile(RuntimeProfile::Python)
            .with_public_signature("add(a, b)")
            .with_evidence_command("python -m unittest discover -s tests");
        let expectations = ContractGenerationExpectations::from_execution(&execution);
        let summary = expectations.summary();

        assert!(summary.contains("source_api="), "{summary}");
        assert!(summary.contains("test_api="), "{summary}");
        assert!(summary.contains("paths=calc.py"), "{summary}");
        assert!(summary.contains("targets=calc.py"), "{summary}");
        assert!(summary.contains("paths=tests/test_calc.py"), "{summary}");
        assert!(
            summary.contains("evidence=test_run(required),command=python -m unittest"),
            "{summary}"
        );
        assert!(
            summary.contains("assertion_policy=no_exact_diagnostic_text_unless_declared"),
            "{summary}"
        );
    }

    #[test]
    fn data_expectations_carry_schema_without_source_api() {
        let contract =
            TaskContract::from_request("Create output.csv with columns id,total and row 1,10.");
        let execution = TaskExecutionContract::from_task_contract(&contract);
        let expectations = ContractGenerationExpectations::from_execution(&execution);
        let summary = expectations.summary();

        assert!(!summary.contains("source_api="), "{summary}");
        assert!(!summary.contains("test_api="), "{summary}");
        assert!(summary.contains("schemas="), "{summary}");
        assert!(summary.contains("columns=id|total"), "{summary}");
    }

    #[test]
    fn api_expectations_carry_http_contract_without_framework_specificity() {
        let contract = TaskContract::from_request(
            "Create app.py and tests/test_app.py for an HTTP notes API. Implement GET /notes returning an empty list and POST /notes accepting JSON with title and body, returning the created note with id=1.",
        );
        let execution = TaskExecutionContract::from_task_contract(&contract)
            .with_runtime_profile(RuntimeProfile::Python)
            .with_evidence_command("python -m pytest tests/test_app.py");
        let expectations = ContractGenerationExpectations::from_execution(&execution);
        let summary = expectations.summary();

        assert!(summary.contains("api_contracts="), "{summary}");
        assert!(summary.contains("method=GET,path=/notes"), "{summary}");
        assert!(
            summary.contains("method=POST,path=/notes,request_body=json"),
            "{summary}"
        );
        assert!(
            summary.contains("request_binding=json_body_object"),
            "{summary}"
        );
        assert!(
            summary.contains("request_json_body_fields=title|body"),
            "{summary}"
        );
        assert!(
            summary.contains("response_fields=id|title|body"),
            "{summary}"
        );
        assert!(
            summary.contains("response_shape=empty_collection"),
            "{summary}"
        );
        assert!(summary.contains("expected_status=unspecified"), "{summary}");
        assert!(
            summary.contains("status_assertion_policy=no_exact_http_status"),
            "{summary}"
        );
        assert!(
            summary.contains("assertion_policy=no_exact_diagnostic_text_unless_declared"),
            "{summary}"
        );
    }

    #[test]
    fn setup_deliverable_becomes_manifest_expectation() {
        let contract = TaskContract::from_request(
            "Create a Rust library with Cargo.toml, src/lib.rs, and tests/lib.rs.",
        );
        let execution = TaskExecutionContract::from_task_contract(&contract)
            .with_runtime_profile(RuntimeProfile::Rust)
            .with_evidence_command("cargo test");
        let expectations = ContractGenerationExpectations::from_execution(&execution);
        let summary = expectations.summary();

        assert!(summary.contains("manifests=Cargo.toml:supports_evidence_runtime"));
        assert!(summary.contains("allowed_files="), "{summary}");
        assert!(summary.contains("evidence=test_run(required),command=cargo test"));
    }
}
