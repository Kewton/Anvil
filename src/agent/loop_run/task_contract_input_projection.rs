//! Request-to-contract input projection.
//!
//! This module is intentionally small: it turns the raw request scan plus the
//! already-inferred project intent into typed booleans consumed by TaskContract
//! construction. It does not decide terminal completion or recovery.

use super::task_contract::{
    OutputContextScan, ProjectIntent, ProjectLanguage, ProjectShape, VerificationRequirement,
    request_asks_for_data_output_artifact_with_scan, request_asks_for_implementation_artifact,
    request_asks_for_setup, request_asks_for_test_artifact, request_asks_for_usage_docs,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ContractRequestInputs {
    pub(super) asks_for_tests: bool,
    pub(super) asks_for_usage_docs: bool,
    pub(super) asks_for_setup: bool,
    pub(super) asks_for_data_output: bool,
    pub(super) asks_for_implementation: bool,
    pub(super) project_intent_implies_implementation: bool,
}

impl ContractRequestInputs {
    pub(super) fn collect(
        scan: &OutputContextScan,
        request_for_inference: &str,
        lower: &str,
        project_intent: &ProjectIntent,
    ) -> Self {
        let asks_for_tests = request_asks_for_test_artifact(request_for_inference, lower);
        let asks_for_usage_docs = request_asks_for_usage_docs(request_for_inference, lower);
        let asks_for_setup = request_asks_for_setup(request_for_inference, lower);
        let asks_for_data_output =
            request_asks_for_data_output_artifact_with_scan(scan, request_for_inference);
        let asks_for_implementation = request_asks_for_implementation_artifact(
            request_for_inference,
            lower,
            asks_for_tests,
            asks_for_usage_docs,
            asks_for_setup,
        );
        let project_intent_implies_implementation =
            project_intent_implies_implementation_artifact(project_intent)
                && !test_only_without_implementation_signal(
                    asks_for_tests,
                    asks_for_usage_docs,
                    asks_for_setup,
                    asks_for_data_output,
                    asks_for_implementation,
                );

        Self {
            asks_for_tests,
            asks_for_usage_docs,
            asks_for_setup,
            asks_for_data_output,
            asks_for_implementation,
            project_intent_implies_implementation,
        }
    }
}

fn project_intent_implies_implementation_artifact(project_intent: &ProjectIntent) -> bool {
    matches!(
        project_intent.language,
        Some(ProjectLanguage::Rust | ProjectLanguage::Node | ProjectLanguage::Python)
    ) && matches!(
        project_intent.shape,
        Some(ProjectShape::Cli | ProjectShape::Library | ProjectShape::Api | ProjectShape::WebApp)
    ) && matches!(
        project_intent.verification,
        VerificationRequirement::Required { .. }
    )
}

fn test_only_without_implementation_signal(
    asks_for_tests: bool,
    asks_for_usage_docs: bool,
    asks_for_setup: bool,
    asks_for_data_output: bool,
    asks_for_implementation: bool,
) -> bool {
    asks_for_tests
        && !asks_for_usage_docs
        && !asks_for_setup
        && !asks_for_data_output
        && !asks_for_implementation
}

#[cfg(test)]
mod tests {
    use super::*;

    fn python_cli_with_required_verification() -> ProjectIntent {
        ProjectIntent {
            intent: super::super::task_contract::TaskIntent::Build,
            language: Some(ProjectLanguage::Python),
            shape: Some(ProjectShape::Cli),
            verification: VerificationRequirement::Required {
                preferred_runner: Some("pytest"),
            },
            confidence: 0.9,
        }
    }

    #[test]
    fn test_only_request_does_not_promote_project_intent_to_implementation() {
        let request = "Run pytest for the existing tests.";
        let lower = request.to_ascii_lowercase();
        let scan = OutputContextScan::new(request);
        let inputs = ContractRequestInputs::collect(
            &scan,
            request,
            &lower,
            &python_cli_with_required_verification(),
        );

        assert!(inputs.asks_for_tests);
        assert!(!inputs.asks_for_implementation);
        assert!(!inputs.project_intent_implies_implementation);
    }

    #[test]
    fn data_output_request_projects_data_without_coding_artifacts() {
        let request = "Create data/summary.json with fields topic, status, count.";
        let lower = request.to_ascii_lowercase();
        let scan = OutputContextScan::new(request);
        let intent = ProjectIntent::from_request(request);
        let inputs = ContractRequestInputs::collect(&scan, request, &lower, &intent);

        assert!(inputs.asks_for_data_output);
        assert!(!inputs.asks_for_tests);
        assert!(!inputs.asks_for_setup);
    }
}
