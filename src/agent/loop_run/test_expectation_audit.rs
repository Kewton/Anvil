//! LLM-side test expectation audit prompt.
//!
//! This is a small prompt boundary for coding contracts with both source and
//! test deliverables. It asks the model to justify test expectations from the
//! sealed contract instead of inventing behavior while writing tests.

use super::task_contract::ArtifactRole;
use super::worker_contract::{TaskExecutionContract, WorkerKind};
use crate::session::store::ConversationMessage;

pub(super) fn test_expectation_audit_message_for_execution(
    execution: &TaskExecutionContract,
) -> Option<ConversationMessage> {
    TestExpectationAudit::from_execution_contract(execution)
        .map(|audit| ConversationMessage::system(audit.policy_message()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestExpectationAudit {
    source_path: String,
    test_path: String,
}

impl TestExpectationAudit {
    fn from_execution_contract(execution: &TaskExecutionContract) -> Option<Self> {
        if execution.constraints.read_only {
            return None;
        }
        let source = execution
            .deliverables
            .iter()
            .find(|deliverable| deliverable.role == ArtifactRole::Implementation)?;
        let test = execution
            .deliverables
            .iter()
            .find(|deliverable| deliverable.role == ArtifactRole::Test)?;
        Some(Self {
            source_path: target_label(source.path.as_deref(), "source"),
            test_path: target_label(test.path.as_deref(), "test"),
        })
    }

    fn policy_message(&self) -> String {
        format!(
            "[Test Expectation Audit] worker={}; source={}; test={}; before writing or repairing tests, classify each assertion expectation as explicit_user_request, declared_public_contract, existing_required_behavior, language_runtime_fact, or unsupported_assumption. Keep exact product expectations only when supported by explicit_user_request, declared_public_contract, or existing_required_behavior. Use language_runtime_fact only for mechanics, not to turn incidental implementation behavior into product assertions. For HTTP APIs, do not invent exact status-code assertions when the contract status is unspecified; success/non-error is enough unless the user declared a status. For underspecified dimensions such as tie-breaking, ordering, rounding, randomness, filesystem order, or locale, use non-ambiguous fixtures or property assertions instead of exact literals. Preserve existing required behavior and keep tests aligned with the source API.",
            WorkerKind::TestAuthor.label(),
            self.source_path,
            self.test_path
        )
    }
}

fn target_label(path: Option<&std::path::Path>, fallback: &'static str) -> String {
    path.map(|path| super::task_contract::mask_and_cap_recovery_field(&path.to_string_lossy()))
        .unwrap_or_else(|| fallback.to_string())
}

#[cfg(test)]
mod tests {
    use super::super::task_contract::TaskContract;
    use super::super::worker_contract::TaskExecutionContract;
    use super::*;

    #[test]
    fn coding_source_and_test_contract_gets_expectation_audit_message() {
        let contract = TaskContract::from_request(
            "Implement longest_word(text) in math_words.py and add tests/test_math_words.py tests.",
        );
        let execution = TaskExecutionContract::from_task_contract(&contract);
        let message = test_expectation_audit_message_for_execution(&execution)
            .expect("source plus test contract should produce audit prompt");

        assert_eq!(message.role, "system");
        assert!(message.content.contains("[Test Expectation Audit]"));
        assert!(message.content.contains("unsupported_assumption"));
        assert!(
            message
                .content
                .contains("incidental implementation behavior")
        );
        assert!(message.content.contains("tie-breaking"));
        assert!(message.content.contains("do not invent exact status-code"));
        assert!(message.content.contains("property assertions"));
        assert!(message.content.contains("test_author"));
    }

    #[test]
    fn non_coding_data_contract_has_no_test_expectation_audit() {
        let contract = TaskContract::from_request("Create summary.json with total and row_count.");
        let execution = TaskExecutionContract::from_task_contract(&contract);

        assert!(test_expectation_audit_message_for_execution(&execution).is_none());
    }
}
