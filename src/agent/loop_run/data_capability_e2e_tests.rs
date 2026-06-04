//! Issue #921 (P4) — Data capability E2E suite (in-crate `#[cfg(test)]`).
//!
//! Pattern lifted from `safe_stop_e2e_tests.rs` / `pam_advisory_e2e_tests.rs`
//! (CB-001 / DR3-001 precedent): an in-crate `#[cfg(test)] mod` that drives the
//! PRODUCTION completion spine (`task_contract::plan_artifact_recovery`, the
//! `verifier` diagnostic side, and the `capability_for` spine) without an
//! `Agent`, Ollama, or any process spawn. Production binary excludes this
//! module.
//!
//! These tests assert the OR-tolerant + accept-tier acceptance contract end to
//! end through the same two-gate flow `plan_artifact_recovery` evaluates
//! (`required_role_satisfied` diagnostic gate first, then
//! `structured_record_excerpt_satisfies_obligations`), so the S7-001 control
//! flow is exercised — not the predicates in isolation.

use super::completion_evidence::{CompletionEvidence, EvidenceSet, RepoEditCategory};
use super::repair_job::VerifierRepairState;
use super::task_contract::{
    ArtifactExcerpts, ArtifactRecoveryAction, ArtifactRecoveryInputs, ArtifactRole, ArtifactState,
    TaskContract, TaskKind,
};
use super::verifier::{
    DocsVerifier, OpsVerifier, ResearchVerifier, StructuredDataTier, VerifierArtifact,
    VerifierDiagnosticCode, assess_structured_data, capability_for,
};

// ---------------------------------------------------------------------------
// Helpers (self-contained; the parent module's `#[cfg(test)] mod tests`
// helpers are not visible from this sibling test module).
// ---------------------------------------------------------------------------

fn data_repo_edit(path: &str) -> CompletionEvidence {
    CompletionEvidence::RepoEdit {
        category: RepoEditCategory::Data,
        count: 1,
        path: Some(path.to_string()),
    }
}

fn excerpts(pairs: &[(ArtifactRole, &str)]) -> ArtifactExcerpts {
    pairs
        .iter()
        .map(|(role, body)| (*role, (*body).to_string()))
        .collect()
}

/// Drive the production recovery planner for a single-DataOutput contract whose
/// `output.csv` deliverable was observed with `excerpt`.
fn plan_data_output(contract: &TaskContract, path: &str, excerpt: &str) -> ArtifactRecoveryAction {
    let mut evidence = EvidenceSet::new();
    evidence.push(data_repo_edit(path));
    let repair_state = VerifierRepairState::None;
    let excerpts = excerpts(&[(ArtifactRole::DataOutput, excerpt)]);
    super::task_contract::plan_artifact_recovery(ArtifactRecoveryInputs {
        contract,
        evidence: &evidence,
        artifacts: &[ArtifactState::exists(ArtifactRole::DataOutput, path)],
        repair_state: &repair_state,
        artifact_excerpts: &excerpts,
        missing_verifier_suppress_retry: false,
        owned_test_artifacts: &[],
    })
}

// ---------------------------------------------------------------------------
// Case 1 — CSV with no declared columns → accept-tier completion.
// ---------------------------------------------------------------------------

#[test]
fn csv_with_no_declared_columns_completes() {
    // A data request that declares an output path but no specific columns.
    let contract = TaskContract::from_request("Generate report output.csv from the input data.");
    assert_eq!(contract.task_kind, TaskKind::Data);
    assert_eq!(contract.required_artifacts, vec![ArtifactRole::DataOutput]);

    let action = plan_data_output(&contract, "output.csv", "any,header\nrow,value\n");
    assert_eq!(
        action,
        ArtifactRecoveryAction::Done,
        "no-column-schema parse-ready CSV must complete"
    );
}

// ---------------------------------------------------------------------------
// Case 2 — declared-but-missing column but parse-ready → completes (no dead-end).
// ---------------------------------------------------------------------------

#[test]
fn declared_missing_column_but_parse_ready_completes() {
    let contract = TaskContract::from_request(
        "Generate output.csv with columns Category and Total from the input CSV.",
    );
    assert_eq!(contract.task_kind, TaskKind::Data);

    // "Total" is declared but absent from the header; the artifact is parse-ready
    // and above the char floor → AcceptTier → completion (the old conjunctive AND
    // dead-end is resolved).
    let action = plan_data_output(&contract, "output.csv", "Category,Amount\nA,1\n");
    assert_eq!(action, ArtifactRecoveryAction::Done);
}

// ---------------------------------------------------------------------------
// Case 3 — unparseable / empty → rejected (Continue, blocking).
// ---------------------------------------------------------------------------

#[test]
fn unparseable_or_empty_data_is_rejected() {
    let contract = TaskContract::from_request(
        "Generate output.csv with columns Category and Total from the input CSV.",
    );

    // Below the char floor and missing the declared columns → Insufficient.
    let below_floor = plan_data_output(&contract, "output.csv", "x,y\n1");
    assert!(
        matches!(
            below_floor,
            ArtifactRecoveryAction::Continue { ref missing, .. }
                if missing == &vec![ArtifactRole::DataOutput]
        ),
        "below-floor missing-column data must keep dead-ending, got {below_floor:?}"
    );

    // Empty excerpt → Insufficient.
    let empty = plan_data_output(&contract, "output.csv", "   \n");
    assert!(
        matches!(
            empty,
            ArtifactRecoveryAction::Continue { ref missing, .. }
                if missing == &vec![ArtifactRole::DataOutput]
        ),
        "empty data excerpt must keep dead-ending, got {empty:?}"
    );

    // Malformed JSONL for a .jsonl deliverable → parse-error → Insufficient.
    let jsonl_contract =
        TaskContract::from_request("Generate data/results.jsonl with columns id and score.");
    assert_eq!(
        jsonl_contract.task_kind,
        TaskKind::Data,
        "fixture must classify as Data so the malformed-JSONL branch is non-vacuous"
    );
    let malformed = plan_data_output(&jsonl_contract, "data/results.jsonl", r#"{"id": broken"#);
    assert!(
        matches!(malformed, ArtifactRecoveryAction::Continue { .. }),
        "malformed JSONL data must keep dead-ending, got {malformed:?}"
    );
}

// ---------------------------------------------------------------------------
// Case 4 — Data completes without a coding/executable verifier.
// ---------------------------------------------------------------------------

#[test]
fn data_requires_no_executable_verifier() {
    // verifier_free_document_task is irrelevant for non-coding kinds; both
    // values must be false (1:1 with the historical `coding_verifier_required`
    // gate for non-coding).
    assert!(!capability_for(TaskKind::Data).requires_executable_verifier(false));
    assert!(!capability_for(TaskKind::Data).requires_executable_verifier(true));
}

// ---------------------------------------------------------------------------
// Case 5 — Data manifest-named path → no InvalidManifest / Setup leak.
//
// DD4 reachability conclusion: a Data flow only synthesizes `DataOutput`
// obligations (never `ArtifactRole::Setup`), so `obligation_requires_manifest_parse`
// (role==Setup && Json/Toml) cannot fire. We pin BOTH that the manifest-named
// Data path is classified as DataOutput (not Setup) AND that it never receives
// an InvalidManifest diagnostic — i.e. the leak is structurally unreachable in
// Data flows. No blanket `task_kind==Data` manifest suppression is added (it
// could weaken real Setup validation / DR4-002).
// ---------------------------------------------------------------------------

#[test]
fn data_manifest_named_path_does_not_leak_invalid_manifest() {
    // A Data task whose declared output is structured JSON-family data.
    let contract =
        TaskContract::from_request("Generate data/manifest.jsonl with columns key and val.");
    assert_eq!(
        contract.task_kind,
        TaskKind::Data,
        "fixture must classify as Data so the Setup-role assertion is non-vacuous"
    );

    // Every DataOutput identity must be DataOutput-role, never Setup, so the
    // role==Setup manifest gate (`obligation_requires_manifest_parse`) is
    // structurally unreachable for this flow.
    assert!(
        !contract.required_artifact_identities.is_empty(),
        "Data flow must declare at least one obligation: {contract:?}"
    );
    for identity in &contract.required_artifact_identities {
        assert_ne!(
            identity.role,
            ArtifactRole::Setup,
            "Data flow must not synthesize a Setup obligation: {identity:?}"
        );
    }

    // Independently: the Data diagnostic arm (`data_artifact_diagnostic`,
    // DataOutput role) can only ever emit SchemaMismatch — never
    // InvalidManifest, which is reserved for the role==Setup manifest gate. A
    // malformed JSON-family artifact is SchemaMismatch on the DataOutput role.
    use super::verifier::DataVerifier;
    use super::verifier::Verifier;
    let required = vec!["key".to_string(), "val".to_string()];
    let malformed = DataVerifier
        .diagnostic(VerifierArtifact {
            path: Some("data/manifest.jsonl"),
            excerpt: r#"{"key": broken"#,
            required_columns: &required,
            required_sections: &[],
        })
        .expect("malformed JSONL yields a blocking diagnostic");
    assert_eq!(
        malformed.code,
        VerifierDiagnosticCode::SchemaMismatch,
        "Data malformed manifest-named path must be SchemaMismatch, not InvalidManifest"
    );
    assert_ne!(malformed.code, VerifierDiagnosticCode::InvalidManifest);
    assert_eq!(malformed.role, ArtifactRole::DataOutput);
}

// ---------------------------------------------------------------------------
// Case 6 — Docs / Research / Ops acceptance unchanged (non-regression).
//
// `assess_structured_data` is Data-scoped; it must not perturb the other
// kinds' diagnostic predicates. We exercise each verifier's `.diagnostic()`
// with known-passing content and assert it stays `None`.
// ---------------------------------------------------------------------------

#[test]
fn docs_research_ops_acceptance_is_unchanged() {
    use super::verifier::Verifier;

    assert_eq!(
        DocsVerifier.diagnostic(VerifierArtifact {
            path: Some("README.md"),
            excerpt: "## Setup\nInstall it.\n## Usage\nRun it.\n",
            required_columns: &[],
            required_sections: &[],
        }),
        None,
        "docs acceptance regressed"
    );

    assert_eq!(
        ResearchVerifier.diagnostic(VerifierArtifact {
            path: Some("research.md"),
            excerpt: "## Summary\nFinding: cadence changed.\nSource: https://example.test/r\nLimitation: confidence is medium.\n",
            required_columns: &[],
            required_sections: &[],
        }),
        None,
        "research acceptance regressed"
    );

    assert_eq!(
        OpsVerifier.diagnostic(VerifierArtifact {
            path: Some("runbook.md"),
            excerpt: "## Checklist\n[x] deploy\n## Validation\nVerify health.\n## Rollback\nRevert the deploy.\n## Risk\nImpact is low.\n",
            required_columns: &[],
            required_sections: &[],
        }),
        None,
        "ops acceptance regressed"
    );
}

// ---------------------------------------------------------------------------
// Case 7 — Data no-spawn: the capability spine forbids process exec for Data
// (the gate both `auto_test::run_structured` and the repair-pass cheap-check
// honor). DR4-003 / DR2: this is the type-level no-spawn guarantee.
// ---------------------------------------------------------------------------

#[test]
fn data_capability_forbids_process_exec() {
    assert!(
        !capability_for(TaskKind::Data).allows_process_exec(),
        "Data must not be permitted to spawn a verifier process"
    );
    // Coding stays the only kind that may spawn (coding-unchanged invariant).
    assert!(capability_for(TaskKind::Coding).allows_process_exec());
}

// ---------------------------------------------------------------------------
// Regression-free pin (DR1-002): a short artifact whose declared columns are
// all observed must still pass (SchemaSatisfied carries NO floor), exactly as
// the historical `structured_data_pass` accepted it.
// ---------------------------------------------------------------------------

#[test]
fn short_column_matching_artifact_still_passes() {
    let cols = vec!["a".to_string(), "b".to_string()];
    // "a,b\n1" is below the char floor but all declared columns are observed.
    assert_eq!(
        assess_structured_data(Some("output.csv"), "a,b\n1", &cols),
        StructuredDataTier::SchemaSatisfied
    );

    let contract = TaskContract::from_request("Generate output.csv with columns a and b.");
    assert_eq!(
        contract.task_kind,
        TaskKind::Data,
        "fixture must classify as Data so the completion assertion is non-vacuous"
    );
    let action = plan_data_output(&contract, "output.csv", "a,b\n1");
    assert_eq!(
        action,
        ArtifactRecoveryAction::Done,
        "short but column-matching data must still complete"
    );
}

// ---------------------------------------------------------------------------
// CB-002 — an explicit `.json` DataOutput request must synthesize a DataOutput
// obligation (previously the structured-format keyword gate omitted `.json`),
// so the SSOT validates it: malformed JSON blocks, valid JSON completes.
// ---------------------------------------------------------------------------

#[test]
fn explicit_json_data_output_is_obligated_and_validated() {
    let contract =
        TaskContract::from_request("Generate data/results.json with columns id and score.");
    assert_eq!(
        contract.task_kind,
        TaskKind::Data,
        "explicit .json data request must classify as Data"
    );
    assert!(
        contract
            .required_artifacts
            .contains(&ArtifactRole::DataOutput),
        "explicit .json output must synthesize a DataOutput obligation (CB-002): {contract:?}"
    );

    // Malformed JSON → parse-error → Insufficient → blocked (Continue).
    let malformed = plan_data_output(&contract, "data/results.json", r#"{"id": broken"#);
    assert!(
        matches!(malformed, ArtifactRecoveryAction::Continue { .. }),
        "malformed .json data must block, got {malformed:?}"
    );

    // Valid JSON containing both declared columns → SchemaSatisfied → completes.
    let valid = plan_data_output(&contract, "data/results.json", r#"{"id": 1, "score": 9.5}"#);
    assert_eq!(
        valid,
        ArtifactRecoveryAction::Done,
        "valid .json with declared columns must complete"
    );
}

// ---------------------------------------------------------------------------
// PR-001 — `.parquet` is binary/columnar and unverifiable from a text excerpt,
// so it must NOT be inferred as a schema-validated DataOutput obligation (which
// would "complete" on any non-empty text with no parse-readiness check). It may
// still classify as a Data task, but no structured_record DataOutput identity
// is synthesized for it.
// ---------------------------------------------------------------------------

#[test]
fn parquet_output_is_not_a_structured_data_obligation() {
    let contract = TaskContract::from_request("Generate output.parquet with columns id and score.");
    let parquet_schema_obligation = contract
        .required_artifact_identities
        .iter()
        .any(|identity| {
            identity.role == ArtifactRole::DataOutput && identity.structured_record_schema.is_some()
        });
    assert!(
        !parquet_schema_obligation,
        "unparseable .parquet must not become a schema-validated DataOutput obligation: {contract:?}"
    );
}

// ===========================================================================
// Issue #937: robust output-context detection — data surface pins.
// ===========================================================================

fn data_output_roles(contract: &TaskContract) -> Vec<String> {
    contract
        .required_artifact_identities
        .iter()
        .filter(|o| o.role == ArtifactRole::DataOutput)
        .map(|o| o.path.clone())
        .collect()
}

/// R3 (input reference, output-looking filename). `Summarize the trends in
/// output_data.csv` reads/summarizes the file; today the `file_output_name`
/// override fabricated a DataOutput obligation. After the demotion (no directed
/// output verb/prep/JP near the path), it stays obligation-free.
#[test]
fn r3_input_reference_with_output_looking_filename_creates_no_obligation() {
    let contract = TaskContract::from_request("Summarize the trends in output_data.csv");
    assert!(
        !contract
            .required_artifacts
            .contains(&ArtifactRole::DataOutput),
        "an input-reference (no directed output) must not create a DataOutput obligation: {:?}",
        data_output_roles(&contract)
    );
    assert!(data_output_roles(&contract).is_empty());
}

/// R4 (fifth/default surface). `What columns are in output_data.csv, a CSV
/// file?` must NOT synthesize a default `output.csv`. The masked `output_action`
/// gate drops it (the filename `output` is masked); `columns`/`csv file` alone
/// cannot drive the standalone default.
#[test]
fn r4_question_about_columns_creates_no_default_output() {
    let contract = TaskContract::from_request("What columns are in output_data.csv, a CSV file?");
    assert!(
        data_output_roles(&contract).is_empty(),
        "a question about an existing file must not synthesize a default output.csv: {:?}",
        data_output_roles(&contract)
    );
    // No default `output.csv` and no explicit `output_data.csv` obligation.
    assert!(
        !data_output_roles(&contract)
            .iter()
            .any(|p| p == "output.csv" || p == "output_data.csv")
    );
}

/// R6 (JP false-positive). `この文書を要約して output_data.csv の列を確認`:
/// the bare `書` inside `文書` (document) must not be a data output marker (it is
/// isolated to the research-only after-window vocabulary). No DataOutput.
#[test]
fn r6_jp_document_substring_is_not_a_data_output_marker() {
    let contract = TaskContract::from_request("この文書を要約して output_data.csv の列を確認");
    assert!(
        data_output_roles(&contract).is_empty(),
        "`文書` must not be read as a data output cue: {:?}",
        data_output_roles(&contract)
    );
}

/// N3 (#921 central risk). `idとtotalの列を持つoutput.csvを生成してください` keeps
/// its DataOutput obligation with identity `output.csv` via the JP `生成`
/// after-window marker (survives the `file_output_name` demotion).
#[test]
fn n3_jp_generate_output_csv_keeps_obligation() {
    let contract = TaskContract::from_request("idとtotalの列を持つoutput.csvを生成してください");
    assert!(
        data_output_roles(&contract).contains(&"output.csv".to_string()),
        "JP genuine output must keep DataOutput identity output.csv: {:?}",
        data_output_roles(&contract)
    );
}

/// N4 (no-path default) + N5 (explicit / input.jsonl-dropped) non-regression.
#[test]
fn n4_n5_genuine_data_output_obligations_preserved() {
    // N4: no explicit path → default output.csv.
    let n4 = TaskContract::from_request("Generate a CSV file with columns id and total");
    assert!(
        data_output_roles(&n4).contains(&"output.csv".to_string()),
        "N4 default output.csv must be synthesized: {:?}",
        data_output_roles(&n4)
    );

    // N5a: explicit adjacent-verb output.
    let n5a = TaskContract::from_request("Generate output.csv with columns id and score");
    assert!(
        data_output_roles(&n5a).contains(&"output.csv".to_string()),
        "N5a explicit output.csv must be obligated: {:?}",
        data_output_roles(&n5a)
    );

    // N5b: explicit output path; the `input.jsonl` source is dropped as input.
    let n5b = TaskContract::from_request(
        "Generate data/results.jsonl with columns id and score from input.jsonl",
    );
    let n5b_paths = data_output_roles(&n5b);
    assert!(
        n5b_paths.contains(&"data/results.jsonl".to_string()),
        "N5b output path must be obligated: {n5b_paths:?}"
    );
    assert!(
        !n5b_paths.contains(&"input.jsonl".to_string()),
        "N5b input.jsonl must be dropped as input, not obligated: {n5b_paths:?}"
    );

    // Genuine output whose stem looks like output AND has a downstream `from ...
    // input` phrase must still be obligated (auxiliary stem guard).
    let aux = TaskContract::from_request("Generate report output.csv from the input data.");
    assert!(
        data_output_roles(&aux).contains(&"output.csv".to_string()),
        "an output-looking stem with a directed verb must stay obligated: {:?}",
        data_output_roles(&aux)
    );
}
