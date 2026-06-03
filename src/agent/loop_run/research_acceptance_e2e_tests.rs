//! Issue #922 (P5): Research capability E2E suite (in-crate `#[cfg(test)]`).
//!
//! Covers the Full-scope deliverables (DD1 predicate SSOT, DD3 obligation
//! bridge, DD4 answer-only routing) end-to-end at the `TaskContract` /
//! verifier-diagnostic level, plus the answer-only and Docs/Ops
//! non-regression guards. Production binary excludes this module (DR3-001 /
//! CB-001 pattern, mirroring `safe_stop_e2e_tests.rs`).

use super::completion_evidence::{CompletionEvidence, EvidenceSet};
use super::task_contract::{
    ArtifactRole, CompletionDecision, CompletionProjectIntent, DeliverableSchema, TaskContract,
    TaskKind,
};
use super::verifier::{VerifierDiagnosticCode, verifier_diagnostic_for_obligation};

// DD3 (bridge): a report-intended research request now carries a required
// `UsageDocs` obligation with a `RequiredSections` schema, and is no longer
// short-circuited to AnswerOnly — the report can flow through acceptance.
#[test]
fn research_report_intended_creates_usage_docs_required_sections_obligation() {
    let contract = TaskContract::from_request(
        "Investigate the deployment options and produce a report in report.md",
    );
    assert_eq!(contract.task_kind, TaskKind::Research);
    assert!(
        contract
            .required_artifacts
            .contains(&ArtifactRole::UsageDocs),
        "report-intended research must require a UsageDocs report obligation"
    );
    let obligation = contract
        .required_artifact_identities
        .iter()
        .find(|o| o.role == ArtifactRole::UsageDocs)
        .expect("research report obligation present");
    assert!(
        matches!(
            obligation.schema,
            Some(DeliverableSchema::RequiredSections(_))
        ),
        "research obligation must carry a RequiredSections schema"
    );
    assert!(!obligation.required_sections.is_empty());
    assert_ne!(
        contract.completion_policy.project_intent,
        CompletionProjectIntent::AnswerOnly,
        "report-intended research must not be answer-only"
    );
}

// DD3 reachability: a research report observed at the obligation path reaches
// `Done` — previously this completed trivially / answer-only without the
// research predicate ever gating.
#[test]
fn research_report_completes_via_report_completeness_pass() {
    let contract = TaskContract::from_request(
        "Investigate the deployment options and produce a report in report.md",
    );
    assert_eq!(contract.task_kind, TaskKind::Research);
    assert!(!contract.verification_required);

    let mut evidence = EvidenceSet::new();
    evidence.push(CompletionEvidence::ReportCompletenessPass {
        path: Some("report.md".to_string()),
    });
    assert_eq!(
        contract.evaluate_with_owned_test_artifacts(&evidence, &[]),
        CompletionDecision::Done
    );
}

// DD1 (OR-tolerant) at the production diagnostic path: a research obligation
// passes when its sections are covered and fails (diagnostic) when they are
// not — with NO docs-shaped surface gate (DR3-002) and NO required uncertainty
// phrase (the old 3-way AND).
#[test]
fn research_obligation_section_gating_via_diagnostic() {
    let contract = TaskContract::from_request(
        "Investigate the deployment options and produce a report in report.md",
    );
    let obligation = contract
        .required_artifact_identities
        .iter()
        .find(|o| o.role == ArtifactRole::UsageDocs)
        .expect("research report obligation present");

    // Sections covered, no uncertainty phrase → accepted (no diagnostic).
    let covered = "## Findings\nrelease cadence changed.\n## Sources\nhttps://example.test\n";
    assert!(
        verifier_diagnostic_for_obligation(TaskKind::Research, obligation, Some(covered), true)
            .is_none(),
        "covered research report must be accepted without an uncertainty phrase"
    );

    // A required section absent → diagnostic (evidence missing).
    let thin = "just a single sentence with no sections.";
    let diag = verifier_diagnostic_for_obligation(TaskKind::Research, obligation, Some(thin), true);
    assert_eq!(
        diag.map(|d| d.code),
        Some(VerifierDiagnosticCode::EvidenceMissing)
    );
}

// DD4 non-regression: a genuine answer-only research request (no report / file
// signal) is NOT pushed into a file-edit obligation and stays answer-only.
#[test]
fn genuine_answer_only_research_stays_answer_only() {
    let contract = TaskContract::from_request("Summarize the latest news for me");
    assert_eq!(contract.task_kind, TaskKind::Research);
    assert!(
        contract.required_artifacts.is_empty(),
        "answer-only research must not be forced into a file-edit obligation"
    );
    assert_eq!(
        contract.completion_policy.project_intent,
        CompletionProjectIntent::AnswerOnly
    );
    assert_eq!(
        contract.evaluate_with_owned_test_artifacts(&EvidenceSet::new(), &[]),
        CompletionDecision::Done
    );
}

// DD6 non-regression: a Docs request still completes via its existing
// `UsageDocs` fast-path — the research-scoped shortcut narrowing (DR3-001) does
// not touch Docs.
#[test]
fn docs_completion_unchanged_by_research_changes() {
    let contract =
        TaskContract::from_request("Update README.md with setup, usage, and test sections.");
    assert_eq!(contract.task_kind, TaskKind::Docs);

    let mut evidence = EvidenceSet::new();
    evidence.push(CompletionEvidence::ReportCompletenessPass {
        path: Some("README.md".to_string()),
    });
    assert_eq!(
        contract.evaluate_with_owned_test_artifacts(&evidence, &[]),
        CompletionDecision::Done
    );
}
