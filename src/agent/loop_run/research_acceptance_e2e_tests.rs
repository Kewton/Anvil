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
    TaskKind, report_intended_research,
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

// PR-003: a read-only input file reference ("summarize notes.txt for me") is
// NOT an output target — it must stay answer-only and NOT gain a report
// obligation. An explicit output target ("...produce a report in report.md")
// does gain one.
#[test]
fn input_file_reference_does_not_force_report_obligation() {
    let input_ref = TaskContract::from_request("Summarize notes.txt for me");
    assert_eq!(input_ref.task_kind, TaskKind::Research);
    assert!(
        input_ref.required_artifacts.is_empty(),
        "a read-only input file reference must not create a report obligation"
    );
    assert_eq!(
        input_ref.completion_policy.project_intent,
        CompletionProjectIntent::AnswerOnly
    );

    let output_target =
        TaskContract::from_request("Investigate the options and produce a report in report.md");
    assert!(
        output_target
            .required_artifacts
            .contains(&ArtifactRole::UsageDocs),
        "an explicit output target must create the report obligation"
    );
}

// PR-002 / DR3-004: the WorkMode consumption-side SSOT (`report_intended_research`)
// that gates the AnswerOnly -> Docs override. Report-intended research (incl.
// Japanese レポート / 出力) is flagged true; genuine answer-only research and a
// read-only input reference are false.
#[test]
fn report_intended_research_ssot() {
    assert!(report_intended_research(
        "調査結果をレポートにまとめて report.md に出力してください"
    ));
    assert!(report_intended_research(
        "Investigate the options and produce a report in report.md"
    ));
    assert!(!report_intended_research(
        "Summarize the latest news for me"
    ));
    assert!(!report_intended_research("Summarize notes.txt for me"));
}

// PR2-001 (#922 remediation): an explicit no-edit / "do not edit files"
// instruction must be honored by the Research capability — the request is still
// classified Research, but it generates NO obligation and stays answer-only
// (the implementation/report artifact is suppressed, matching the WorkMode
// classifier which routes such requests to AnswerOnly). A normal
// research-with-output request (no no-edit clause) is non-regressed.
#[test]
fn explicit_no_edit_research_report_stays_answer_only() {
    // Baseline (no no-edit clause): a Research report request DOES get the
    // obligation and is report-intended — non-regression control.
    let baseline =
        TaskContract::from_request("Investigate the options and produce a report in report.md");
    assert_eq!(baseline.task_kind, TaskKind::Research);
    assert!(
        baseline
            .required_artifacts
            .contains(&ArtifactRole::UsageDocs),
        "a normal research-with-output request must still get the report obligation"
    );
    assert!(report_intended_research(
        "Investigate the options and produce a report in report.md"
    ));

    // Same request + an explicit no-edit / read-only instruction → fails closed:
    // classified Research, NO obligation, not report-intended (so the
    // AnswerOnly->Docs override cannot fire), and WorkMode stays AnswerOnly.
    // Covers both EN ("do not edit any files") and JP ("ファイルは変更しないで").
    for request in [
        "Investigate the options and produce a report in report.md, but do not edit any files",
        "調査結果をレポートにまとめて report.md に出力して。ただしファイルは変更しないで",
    ] {
        let contract = TaskContract::from_request(request);
        assert_eq!(
            contract.task_kind,
            TaskKind::Research,
            "explicit no-edit research must stay Research, not Coding: {request:?}"
        );
        assert!(
            contract.required_artifacts.is_empty(),
            "explicit no-edit must not create any artifact obligation: {request:?}"
        );
        assert!(
            !report_intended_research(request),
            "explicit no-edit must keep the AnswerOnly->Docs override closed: {request:?}"
        );
        assert_eq!(
            crate::modes::plan_act::classify_work_mode_json(request).work_mode,
            crate::modes::plan_act::WorkMode::AnswerOnly,
            "explicit no-edit request must stay WorkMode::AnswerOnly: {request:?}"
        );
    }
}

// PR2-002: token-boundary aware output context. A word merely *containing* a
// preposition substring ("investigate" ⊃ "in") is not an output context, and an
// explicit input verb wins even for an output-looking file name.
#[test]
fn report_output_context_is_token_boundary_aware() {
    // "investigate" must not match the preposition "in" → no obligation.
    let invn = TaskContract::from_request("Investigate notes.txt for me");
    assert!(
        invn.required_artifacts.is_empty(),
        "substring 'in' inside 'investigate' must not create a report obligation"
    );
    // input verb wins even though the file name looks like output.
    let read_report = TaskContract::from_request("Summarize report.md for me");
    assert!(
        read_report.required_artifacts.is_empty(),
        "reading an output-named file is not a report output target"
    );
    // genuine output target still detected.
    let produce =
        TaskContract::from_request("Investigate the options and produce a report in report.md");
    assert!(
        produce
            .required_artifacts
            .contains(&ArtifactRole::UsageDocs)
    );
}

// Codex-High (#922): an output-LOOKING file name (report.md / summary.md /
// findings.md) referenced for reading/comparison — with NO output verb — must
// NOT be treated as an output target. Such requests are answer-only and must
// stay obligation-free, so `classify_confirm_flow` cannot flip AnswerOnly->Docs
// and enable file edits.
#[test]
fn output_looking_input_reference_is_not_an_output_target() {
    for request in [
        "Compare report.md and summary.md",
        "Review findings.md",
        "Summarize report.md and report any discrepancies",
        "What does summary.md conclude?",
    ] {
        let contract = TaskContract::from_request(request);
        assert!(
            contract.required_artifacts.is_empty(),
            "an output-looking file read/compared (no output verb) must not create an obligation: {request:?}"
        );
        assert!(
            !report_intended_research(request),
            "no output context → must not be report-intended (AnswerOnly->Docs stays closed): {request:?}"
        );
    }

    // Contrast: an explicit output verb directed at the same file name DOES
    // create the obligation (non-regression of the genuine output path).
    for request in [
        "Investigate the options and produce a report in report.md",
        "調査結果を report.md にまとめて出力してください",
    ] {
        assert!(
            report_intended_research(request),
            "an explicit output target must be report-intended: {request:?}"
        );
    }
}
