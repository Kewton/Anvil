use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use super::claims_binding::{ClaimBinding, bind_report_claims, claim_limit_exceeded};
use super::results_schema::{self, ExcludedRows, ResultsDocument};
use crate::minimal_loop::pipeline_probe::{self, PipelineProbeConfig};

pub const RESULTS_SCHEMA_EVIDENCE_PATH: &str = "evidence/results-schema.json";
pub const RECONCILIATION_EVIDENCE_PATH: &str = "evidence/reconciliation.json";
pub const CLAIMS_BINDING_EVIDENCE_PATH: &str = "evidence/claims-binding.json";
pub const RERUN_CONSISTENCY_EVIDENCE_PATH: &str = "evidence/rerun-consistency.json";
const MAX_REPORT_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultsSchemaEvidence {
    pub capability_id: String,
    pub status: String,
    pub ok: bool,
    pub results_path: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReconciliationEvidence {
    pub capability_id: String,
    pub status: String,
    pub ok: bool,
    pub input_rows: Option<u64>,
    pub used_rows: Option<u64>,
    pub excluded: Vec<ExcludedRows>,
    pub excluded_rows: Option<u64>,
    pub equation: Option<String>,
    pub failure_kinds: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClaimsBindingEvidence {
    pub capability_id: String,
    pub status: String,
    pub ok: bool,
    pub report_paths: Vec<String>,
    pub claims: Vec<ClaimBinding>,
    pub failure_kinds: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RerunConsistencyEvidence {
    pub capability_id: String,
    pub status: String,
    pub ok: bool,
    pub entry: String,
    pub pipeline_run_ok: bool,
    pub baseline_results: Option<ResultsDocument>,
    pub rerun_results: Option<ResultsDocument>,
    pub failure_kinds: Vec<String>,
}

pub fn check_results_schema(root: &Path) -> anyhow::Result<ResultsSchemaEvidence> {
    let error = results_schema::load(root)
        .err()
        .map(|error| error.to_string());
    let ok = error.is_none();
    let evidence = ResultsSchemaEvidence {
        capability_id: "data_results_schema".to_string(),
        status: status(ok),
        ok,
        results_path: results_schema::RESULTS_RELATIVE_PATH.to_string(),
        error,
    };
    write_evidence(root, RESULTS_SCHEMA_EVIDENCE_PATH, &evidence)?;
    Ok(evidence)
}

pub fn check_reconciliation(root: &Path) -> anyhow::Result<ReconciliationEvidence> {
    let mut evidence = ReconciliationEvidence {
        capability_id: "data_reconciliation".to_string(),
        status: "failed".to_string(),
        ok: false,
        input_rows: None,
        used_rows: None,
        excluded: Vec::new(),
        excluded_rows: None,
        equation: None,
        failure_kinds: Vec::new(),
    };
    match results_schema::load(root) {
        Ok(results) => evaluate_reconciliation(&mut evidence, results),
        Err(error) => evidence.failure_kinds.push(format!(
            "reconciliation_violation:invalid_results_schema:{error}"
        )),
    }
    evidence.ok = evidence.failure_kinds.is_empty();
    evidence.status = status(evidence.ok);
    write_evidence(root, RECONCILIATION_EVIDENCE_PATH, &evidence)?;
    Ok(evidence)
}

pub fn check_claims_binding(root: &Path) -> anyhow::Result<ClaimsBindingEvidence> {
    let mut evidence = ClaimsBindingEvidence {
        capability_id: "data_claims_binding".to_string(),
        status: "failed".to_string(),
        ok: false,
        report_paths: Vec::new(),
        claims: Vec::new(),
        failure_kinds: Vec::new(),
    };
    let results = match results_schema::load(root) {
        Ok(results) => Some(results),
        Err(error) => {
            evidence.failure_kinds.push(format!(
                "claims_binding_violation:invalid_results_schema:{error}"
            ));
            None
        }
    };
    if let Some(results) = results {
        evaluate_reports(root, &results, &mut evidence);
    }
    evidence.ok = evidence.failure_kinds.is_empty();
    evidence.status = status(evidence.ok);
    write_evidence(root, CLAIMS_BINDING_EVIDENCE_PATH, &evidence)?;
    Ok(evidence)
}

pub fn check_rerun_consistency(
    root: &Path,
    entry: &str,
    timeout: Duration,
) -> anyhow::Result<RerunConsistencyEvidence> {
    let mut evidence = RerunConsistencyEvidence {
        capability_id: "data_rerun_consistency".to_string(),
        status: "failed".to_string(),
        ok: false,
        entry: entry.to_string(),
        pipeline_run_ok: false,
        baseline_results: None,
        rerun_results: None,
        failure_kinds: Vec::new(),
    };
    match results_schema::load(root) {
        Ok(results) => evidence.baseline_results = Some(results),
        Err(error) => evidence.failure_kinds.push(format!(
            "rerun_consistency_violation:baseline_results:{error}"
        )),
    }
    if evidence.baseline_results.is_some() {
        match pipeline_probe::run(root, PipelineProbeConfig::new(entry).with_timeout(timeout)) {
            Ok(report) => {
                evidence.pipeline_run_ok = report.ok;
                if !report.ok {
                    evidence.failure_kinds.push(format!(
                        "rerun_consistency_violation:pipeline_run:{}",
                        report.failure_kinds.join(",")
                    ));
                }
            }
            Err(error) => evidence.failure_kinds.push(format!(
                "rerun_consistency_violation:pipeline_run_error:{error}"
            )),
        }
        match results_schema::load(root) {
            Ok(results) => evidence.rerun_results = Some(results),
            Err(error) => evidence
                .failure_kinds
                .push(format!("rerun_consistency_violation:rerun_results:{error}")),
        }
    }
    if let (Some(baseline), Some(rerun)) = (&evidence.baseline_results, &evidence.rerun_results)
        && baseline != rerun
    {
        evidence
            .failure_kinds
            .push("rerun_consistency_violation:results_changed".to_string());
    }
    evidence.ok = evidence.failure_kinds.is_empty()
        && evidence.pipeline_run_ok
        && evidence.rerun_results.is_some();
    evidence.status = status(evidence.ok);
    write_evidence(root, RERUN_CONSISTENCY_EVIDENCE_PATH, &evidence)?;
    Ok(evidence)
}

fn evaluate_reconciliation(evidence: &mut ReconciliationEvidence, results: ResultsDocument) {
    let reconciliation = results.reconciliation;
    evidence.input_rows = Some(reconciliation.input_rows);
    evidence.used_rows = Some(reconciliation.used_rows);
    evidence.excluded = reconciliation.excluded;
    for (index, excluded) in evidence.excluded.iter().enumerate() {
        if excluded.reason.trim().is_empty() {
            evidence.failure_kinds.push(format!(
                "reconciliation_violation:excluded_reason_empty:index={index}"
            ));
        }
    }
    let excluded_rows = evidence
        .excluded
        .iter()
        .try_fold(0u64, |total, excluded| total.checked_add(excluded.rows));
    let Some(excluded_rows) = excluded_rows else {
        evidence
            .failure_kinds
            .push("reconciliation_violation:excluded_rows_overflow".to_string());
        return;
    };
    evidence.excluded_rows = Some(excluded_rows);
    evidence.equation = Some(format!(
        "{} = {} + {}",
        reconciliation.input_rows, reconciliation.used_rows, excluded_rows
    ));
    if reconciliation.used_rows.checked_add(excluded_rows) != Some(reconciliation.input_rows) {
        evidence.failure_kinds.push(format!(
            "reconciliation_violation:input_rows={} used_rows={} excluded_rows={excluded_rows}",
            reconciliation.input_rows, reconciliation.used_rows
        ));
    }
}

fn evaluate_reports(root: &Path, results: &ResultsDocument, evidence: &mut ClaimsBindingEvidence) {
    for report_path in ["output/report.html", "output/report.md"] {
        let candidate = root.join(report_path);
        if !candidate.exists() {
            continue;
        }
        evidence.report_paths.push(report_path.to_string());
        let resolved = match crate::tools::path_guard::resolve_existing(root, report_path) {
            Ok(path) if path.is_file() => path,
            Ok(_) => {
                evidence.failure_kinds.push(format!(
                    "claims_binding_violation:report_not_file:{report_path}"
                ));
                continue;
            }
            Err(error) => {
                evidence.failure_kinds.push(format!(
                    "claims_binding_violation:report_path:{report_path}:{error}"
                ));
                continue;
            }
        };
        let metadata = match resolved.metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                evidence.failure_kinds.push(format!(
                    "claims_binding_violation:report_metadata:{report_path}:{error}"
                ));
                continue;
            }
        };
        if metadata.len() > MAX_REPORT_BYTES {
            evidence.failure_kinds.push(format!(
                "claims_binding_violation:report_size_limit:{report_path}"
            ));
            continue;
        }
        let text = match std::fs::read_to_string(&resolved) {
            Ok(text) => text,
            Err(error) => {
                evidence.failure_kinds.push(format!(
                    "claims_binding_violation:report_unreadable:{report_path}:{error}"
                ));
                continue;
            }
        };
        if claim_limit_exceeded(report_path, &text) {
            evidence.failure_kinds.push(format!(
                "claims_binding_violation:claim_count_limit:{report_path}"
            ));
        }
        evidence
            .claims
            .extend(bind_report_claims(report_path, &text, &results.values));
    }
    if evidence.report_paths.is_empty() {
        evidence
            .failure_kinds
            .push("claims_binding_violation:report_missing".to_string());
    }
    for claim in evidence.claims.iter().filter(|claim| !claim.ok) {
        evidence.failure_kinds.push(format!(
            "claims_binding_violation:{}:{}:{}",
            claim.report_path, claim.byte_offset, claim.raw
        ));
    }
}

fn write_evidence<T: Serialize>(root: &Path, relative: &str, value: &T) -> anyhow::Result<()> {
    let path = crate::tools::path_guard::resolve_optional_existing(root, relative)
        .with_context(|| format!("evidence path escapes workspace: {relative}"))?;
    let parent = path.parent().context("evidence parent missing")?;
    std::fs::create_dir_all(parent)?;
    let mut file = std::fs::File::create(path)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    Ok(())
}

fn status(ok: bool) -> String {
    if ok { "pass" } else { "failed" }.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_results(root: &Path, value: serde_json::Value) {
        std::fs::create_dir_all(root.join("output")).unwrap();
        std::fs::write(
            root.join(results_schema::RESULTS_RELATIVE_PATH),
            format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
        )
        .unwrap();
    }

    fn valid_results() -> serde_json::Value {
        json!({
            "reconciliation": {
                "input_rows": 5,
                "used_rows": 3,
                "excluded": [{"reason": "missing amount", "rows": 2}]
            },
            "values": {"total": 1234.567, "rate_percent": 40.0, "count": 3.2}
        })
    }

    #[test]
    fn schema_check_records_missing_required_key() {
        let dir = tempfile::tempdir().unwrap();
        write_results(dir.path(), json!({"values": {}}));

        let evidence = check_results_schema(dir.path()).unwrap();

        assert!(!evidence.ok);
        assert!(evidence.error.unwrap().contains("reconciliation"));
        assert!(dir.path().join(RESULTS_SCHEMA_EVIDENCE_PATH).is_file());
    }

    #[test]
    fn reconciliation_requires_balanced_rows_and_nonempty_reasons() {
        let dir = tempfile::tempdir().unwrap();
        write_results(dir.path(), valid_results());
        assert!(check_reconciliation(dir.path()).unwrap().ok);

        let mut invalid = valid_results();
        invalid["reconciliation"]["excluded"][0]["reason"] = json!("  ");
        invalid["reconciliation"]["used_rows"] = json!(4);
        write_results(dir.path(), invalid);
        let evidence = check_reconciliation(dir.path()).unwrap();

        assert!(!evidence.ok);
        assert!(
            evidence
                .failure_kinds
                .iter()
                .any(|failure| failure.contains("excluded_reason_empty"))
        );
        assert!(
            evidence
                .failure_kinds
                .iter()
                .any(|failure| failure.contains("input_rows=5"))
        );
    }

    #[test]
    fn claims_binding_accepts_only_results_rounded_to_printed_precision() {
        let dir = tempfile::tempdir().unwrap();
        write_results(dir.path(), valid_results());
        std::fs::write(
            dir.path().join("output/report.md"),
            "Total 1,234.57 USD; rate 40%; count 3 件.",
        )
        .unwrap();

        let evidence = check_claims_binding(dir.path()).unwrap();

        assert!(evidence.ok, "{evidence:?}");
        assert_eq!(evidence.claims.len(), 3);
        assert!(dir.path().join(CLAIMS_BINDING_EVIDENCE_PATH).is_file());
    }

    #[test]
    fn claims_binding_rejects_fabricated_report_number() {
        let dir = tempfile::tempdir().unwrap();
        write_results(dir.path(), valid_results());
        std::fs::write(dir.path().join("output/report.md"), "Fabricated 999").unwrap();

        let evidence = check_claims_binding(dir.path()).unwrap();

        assert!(!evidence.ok);
        assert!(
            evidence
                .failure_kinds
                .iter()
                .any(|failure| failure.starts_with("claims_binding_violation"))
        );
    }

    #[test]
    fn rerun_consistency_compares_the_entire_results_document() {
        let dir = tempfile::tempdir().unwrap();
        write_results(dir.path(), valid_results());
        std::fs::create_dir_all(dir.path().join("pipeline")).unwrap();
        let serialized = serde_json::to_string(&valid_results()).unwrap();
        std::fs::write(
            dir.path().join("pipeline/main.py"),
            format!(
                "from pathlib import Path\nPath('output/results.json').write_text({serialized:?})\n"
            ),
        )
        .unwrap();

        let evidence =
            check_rerun_consistency(dir.path(), "pipeline/main.py", Duration::from_secs(2))
                .unwrap();

        assert!(evidence.ok, "{evidence:?}");
        assert_eq!(evidence.baseline_results, evidence.rerun_results);
    }

    #[test]
    fn rerun_consistency_rejects_changed_values() {
        let dir = tempfile::tempdir().unwrap();
        write_results(dir.path(), valid_results());
        std::fs::create_dir_all(dir.path().join("pipeline")).unwrap();
        let mut changed = valid_results();
        changed["values"]["total"] = json!(999.0);
        let serialized = serde_json::to_string(&changed).unwrap();
        std::fs::write(
            dir.path().join("pipeline/main.py"),
            format!(
                "from pathlib import Path\nPath('output/results.json').write_text({serialized:?})\n"
            ),
        )
        .unwrap();

        let evidence =
            check_rerun_consistency(dir.path(), "pipeline/main.py", Duration::from_secs(2))
                .unwrap();

        assert!(!evidence.ok);
        assert!(
            evidence
                .failure_kinds
                .contains(&"rerun_consistency_violation:results_changed".to_string())
        );
    }
}
