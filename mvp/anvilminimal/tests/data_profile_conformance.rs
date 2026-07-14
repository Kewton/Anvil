use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anvilminimal::planner::profiles::data::checks;
use anvilminimal::planner::profiles::data::runtime::{
    DataAssurance, assurance_from_evidence, run_manifest_checks,
};
use serde::Deserialize;

const FIXTURE_ROOT: &str = "tests/corpus/apps/test0713_data_profile_contract_v0/fixtures";

#[derive(Debug, Deserialize)]
struct DataFixture {
    case: String,
    expected_assurance: String,
    expected_failed_check: Option<String>,
    files: BTreeMap<String, String>,
}

#[test]
fn full_fixture_runs_end_to_end_and_emits_full_evidence() {
    let (dir, fixture) = materialize("full.jsonl");

    let summary = run_manifest_checks(dir.path()).unwrap();

    assert_eq!(fixture.case, "full");
    assert_eq!(fixture.expected_assurance, "full");
    assert_eq!(summary.assurance, DataAssurance::Full, "{summary:?}");
    assert!(summary.checks.values().all(|passed| *passed));
    for path in [
        "evidence/pipeline-run.json",
        checks::RESULTS_SCHEMA_EVIDENCE_PATH,
        checks::RECONCILIATION_EVIDENCE_PATH,
        checks::CLAIMS_BINDING_EVIDENCE_PATH,
        checks::RERUN_CONSISTENCY_EVIDENCE_PATH,
        "evidence/data-assurance.json",
    ] {
        assert!(dir.path().join(path).is_file(), "missing {path}");
    }
}

#[test]
fn fabricated_number_fails_e2_and_cannot_earn_full() {
    let (dir, fixture) = materialize("fabricated-claim.jsonl");

    let summary = run_manifest_checks(dir.path()).unwrap();

    assert_eq!(summary.assurance.as_str(), fixture.expected_assurance);
    assert!(!summary.checks[fixture.expected_failed_check.as_deref().unwrap()]);
    assert!(summary.checks["data_reconciliation"]);
    assert!(summary.checks["data_rerun_consistency"]);
    let evidence =
        std::fs::read_to_string(dir.path().join(checks::CLAIMS_BINDING_EVIDENCE_PATH)).unwrap();
    assert!(evidence.contains("claims_binding_violation"));
}

#[test]
fn silent_exclusion_fails_e1() {
    let (dir, fixture) = materialize("unaccounted-exclusion.jsonl");

    let summary = run_manifest_checks(dir.path()).unwrap();

    assert_eq!(summary.assurance.as_str(), fixture.expected_assurance);
    assert!(!summary.checks[fixture.expected_failed_check.as_deref().unwrap()]);
    let evidence =
        std::fs::read_to_string(dir.path().join(checks::RECONCILIATION_EVIDENCE_PATH)).unwrap();
    assert!(evidence.contains("reconciliation_violation"));
}

#[test]
fn time_dependent_pipeline_fails_e3() {
    let (dir, fixture) = materialize("time-dependent.jsonl");

    let summary = run_manifest_checks(dir.path()).unwrap();

    assert_eq!(summary.assurance.as_str(), fixture.expected_assurance);
    assert!(!summary.checks[fixture.expected_failed_check.as_deref().unwrap()]);
    let evidence =
        std::fs::read_to_string(dir.path().join(checks::RERUN_CONSISTENCY_EVIDENCE_PATH)).unwrap();
    assert!(evidence.contains("results_changed"));
}

#[test]
fn unexecuted_probe_is_static_and_never_projects_full() {
    let (dir, _) = materialize("full.jsonl");

    assert_eq!(assurance_from_evidence(dir.path()), DataAssurance::Static);
    assert!(!dir.path().join("evidence/pipeline-run.json").exists());
}

#[test]
fn partial_or_full_requires_both_e1_and_e3_pass_evidence() {
    for missing_evidence in [
        checks::RECONCILIATION_EVIDENCE_PATH,
        checks::RERUN_CONSISTENCY_EVIDENCE_PATH,
    ] {
        let (dir, _) = materialize("fabricated-claim.jsonl");
        let summary = run_manifest_checks(dir.path()).unwrap();
        assert_eq!(summary.assurance, DataAssurance::Partial);

        std::fs::remove_file(dir.path().join(missing_evidence)).unwrap();

        assert_eq!(
            assurance_from_evidence(dir.path()),
            DataAssurance::Failed,
            "missing {missing_evidence} must not project partial or full"
        );
    }
}

fn materialize(name: &str) -> (tempfile::TempDir, DataFixture) {
    let fixture = load_fixture(name);
    let dir = tempfile::tempdir().unwrap();
    for (relative, content) in &fixture.files {
        anvilminimal::tools::path_guard::validate_workspace_relative(relative).unwrap();
        let path = dir.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    (dir, fixture)
}

fn load_fixture(name: &str) -> DataFixture {
    let path = PathBuf::from(FIXTURE_ROOT).join(name);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    let line = text.lines().find(|line| !line.trim().is_empty()).unwrap();
    serde_json::from_str(line).unwrap_or_else(|error| {
        panic!(
            "failed to parse {} as JSONL: {error}",
            Path::new(&path).display()
        )
    })
}
