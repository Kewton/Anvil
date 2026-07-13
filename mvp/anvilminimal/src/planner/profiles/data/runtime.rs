use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use super::{checks, manifest};
use crate::minimal_loop::pipeline_probe::{self, PipelineProbeConfig, PipelineProbeReport};
use crate::planner::capability_catalog::{
    DataInternalCheck, InternalCapability, ProbeCapability, ResolvedCapability,
};

pub const DATA_ASSURANCE_EVIDENCE_PATH: &str = "evidence/data-assurance.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DataAssurance {
    Full,
    Partial,
    Static,
    Failed,
}

impl DataAssurance {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Partial => "partial",
            Self::Static => "static",
            Self::Failed => "failed",
        }
    }

    pub const fn behavior_status(self) -> &'static str {
        match self {
            Self::Full => "pass",
            Self::Partial => "partial",
            Self::Static => "static",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataCheckSummary {
    pub status: String,
    pub assurance: DataAssurance,
    pub checks: BTreeMap<String, bool>,
    pub reasons: Vec<String>,
}

pub fn run_manifest_checks(root: &Path) -> anyhow::Result<DataCheckSummary> {
    let adapters = adapters()?;
    let mut checks = BTreeMap::new();
    let mut reasons = Vec::new();

    let pipeline_ok = run_pipeline(root, &adapters, &mut reasons);
    checks.insert("pipeline_probe".to_string(), pipeline_ok);

    let schema = checks::check_results_schema(root)?;
    checks.insert("data_results_schema".to_string(), schema.ok);
    if let Some(error) = schema.error {
        reasons.push(format!("data_results_schema:{error}"));
    }

    let reconciliation = checks::check_reconciliation(root)?;
    checks.insert("data_reconciliation".to_string(), reconciliation.ok);
    reasons.extend(reconciliation.failure_kinds);

    let claims = checks::check_claims_binding(root)?;
    checks.insert("data_claims_binding".to_string(), claims.ok);
    reasons.extend(claims.failure_kinds);

    let rerun_ok = run_rerun(root, &adapters, &mut reasons)?;
    checks.insert("data_rerun_consistency".to_string(), rerun_ok);

    let assurance = classify(&checks, true);
    reasons.sort();
    reasons.dedup();
    let summary = DataCheckSummary {
        status: assurance.as_str().to_string(),
        assurance,
        checks,
        reasons,
    };
    write_summary(root, &summary)?;
    Ok(summary)
}

pub fn assurance_from_evidence(root: &Path) -> DataAssurance {
    if !root.join("pipeline/main.py").is_file() {
        return DataAssurance::Failed;
    }
    let Some(pipeline) =
        read_json::<PipelineProbeReport>(root, pipeline_probe::PIPELINE_RUN_EVIDENCE_PATH)
    else {
        return DataAssurance::Static;
    };
    let mut statuses = BTreeMap::new();
    statuses.insert("pipeline_probe".to_string(), pipeline.ok);
    statuses.insert(
        "data_results_schema".to_string(),
        read_json::<checks::ResultsSchemaEvidence>(root, checks::RESULTS_SCHEMA_EVIDENCE_PATH)
            .is_some_and(|evidence| evidence.ok),
    );
    statuses.insert(
        "data_reconciliation".to_string(),
        read_json::<checks::ReconciliationEvidence>(root, checks::RECONCILIATION_EVIDENCE_PATH)
            .is_some_and(|evidence| evidence.ok),
    );
    statuses.insert(
        "data_claims_binding".to_string(),
        read_json::<checks::ClaimsBindingEvidence>(root, checks::CLAIMS_BINDING_EVIDENCE_PATH)
            .is_some_and(|evidence| evidence.ok),
    );
    statuses.insert(
        "data_rerun_consistency".to_string(),
        read_json::<checks::RerunConsistencyEvidence>(
            root,
            checks::RERUN_CONSISTENCY_EVIDENCE_PATH,
        )
        .is_some_and(|evidence| evidence.ok),
    );
    classify(&statuses, true)
}

fn adapters() -> anyhow::Result<Vec<ResolvedCapability>> {
    Ok(manifest::get()
        .resolve()?
        .into_values()
        .flatten()
        .map(|check| check.capability)
        .collect())
}

fn run_pipeline(root: &Path, adapters: &[ResolvedCapability], reasons: &mut Vec<String>) -> bool {
    let Some((entry, timeout_seconds)) = adapters.iter().find_map(|capability| match capability {
        ResolvedCapability::Probe(ProbeCapability::Pipeline {
            entry,
            timeout_seconds,
        }) => Some((entry.as_str(), *timeout_seconds)),
        _ => None,
    }) else {
        reasons.push("pipeline_probe:binding_missing".to_string());
        return false;
    };
    match pipeline_probe::run(
        root,
        PipelineProbeConfig::new(entry).with_timeout(Duration::from_secs(timeout_seconds.into())),
    ) {
        Ok(report) => {
            reasons.extend(report.failure_kinds.clone());
            report.ok
        }
        Err(error) => {
            reasons.push(format!("pipeline_probe:error:{error}"));
            false
        }
    }
}

fn run_rerun(
    root: &Path,
    adapters: &[ResolvedCapability],
    reasons: &mut Vec<String>,
) -> anyhow::Result<bool> {
    let Some((entry, timeout_seconds)) = adapters.iter().find_map(|capability| match capability {
        ResolvedCapability::Probe(ProbeCapability::DataRerunConsistency {
            entry,
            timeout_seconds,
        }) => Some((entry.as_str(), *timeout_seconds)),
        _ => None,
    }) else {
        reasons.push("data_rerun_consistency:binding_missing".to_string());
        return Ok(false);
    };
    let evidence =
        checks::check_rerun_consistency(root, entry, Duration::from_secs(timeout_seconds.into()))?;
    reasons.extend(evidence.failure_kinds.clone());
    Ok(evidence.ok)
}

fn classify(checks: &BTreeMap<String, bool>, probe_attempted: bool) -> DataAssurance {
    if !probe_attempted {
        return DataAssurance::Static;
    }
    let passed = |id: &str| checks.get(id).copied().unwrap_or(false);
    if !passed("pipeline_probe")
        || !passed("data_reconciliation")
        || !passed("data_rerun_consistency")
    {
        DataAssurance::Failed
    } else if !passed("data_claims_binding") || !passed("data_results_schema") {
        DataAssurance::Partial
    } else {
        DataAssurance::Full
    }
}

fn write_summary(root: &Path, summary: &DataCheckSummary) -> anyhow::Result<()> {
    let path =
        crate::tools::path_guard::resolve_optional_existing(root, DATA_ASSURANCE_EVIDENCE_PATH)
            .context("data assurance evidence path escapes workspace")?;
    let parent = path
        .parent()
        .context("data assurance evidence parent missing")?;
    std::fs::create_dir_all(parent)?;
    let mut file = std::fs::File::create(path)?;
    serde_json::to_writer_pretty(&mut file, summary)?;
    file.write_all(b"\n")?;
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(root: &Path, relative: &str) -> Option<T> {
    let path = crate::tools::path_guard::resolve_existing(root, relative).ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn has_all_manifest_adapters() -> bool {
    adapters().is_ok_and(|adapters| {
        adapters.iter().any(|adapter| {
            matches!(
                adapter,
                ResolvedCapability::Internal(InternalCapability::Data(
                    DataInternalCheck::ResultsSchema
                ))
            )
        }) && adapters.iter().any(|adapter| {
            matches!(
                adapter,
                ResolvedCapability::Internal(InternalCapability::Data(
                    DataInternalCheck::Reconciliation
                ))
            )
        }) && adapters.iter().any(|adapter| {
            matches!(
                adapter,
                ResolvedCapability::Internal(InternalCapability::Data(
                    DataInternalCheck::ClaimsBinding
                ))
            )
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_pipeline(root: &Path, report_number: &str, total_expression: &str) {
        std::fs::create_dir_all(root.join("pipeline")).unwrap();
        std::fs::write(
            root.join("pipeline/main.py"),
            format!(
                r#"import json
from pathlib import Path
Path("output").mkdir(exist_ok=True)
total = {total_expression}
results = {{"reconciliation": {{"input_rows": 3, "used_rows": 2, "excluded": [{{"reason": "missing amount", "rows": 1}}]}}, "values": {{"total": total}}}}
Path("output/results.json").write_text(json.dumps(results, sort_keys=True))
Path("output/report.md").write_text("Total {report_number}")
"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn manifest_dispatch_produces_full_only_after_all_checks_pass() {
        let dir = tempfile::tempdir().unwrap();
        write_pipeline(dir.path(), "12.5", "12.5");

        let summary = run_manifest_checks(dir.path()).unwrap();

        assert_eq!(summary.assurance, DataAssurance::Full);
        assert!(summary.checks.values().all(|ok| *ok), "{summary:?}");
        for path in [
            pipeline_probe::PIPELINE_RUN_EVIDENCE_PATH,
            checks::RESULTS_SCHEMA_EVIDENCE_PATH,
            checks::RECONCILIATION_EVIDENCE_PATH,
            checks::CLAIMS_BINDING_EVIDENCE_PATH,
            checks::RERUN_CONSISTENCY_EVIDENCE_PATH,
            DATA_ASSURANCE_EVIDENCE_PATH,
        ] {
            assert!(dir.path().join(path).is_file(), "missing {path}");
        }
        assert_eq!(assurance_from_evidence(dir.path()), DataAssurance::Full);
    }

    #[test]
    fn fabricated_claim_is_partial_when_execution_e1_and_e3_pass() {
        let dir = tempfile::tempdir().unwrap();
        write_pipeline(dir.path(), "999", "12.5");

        let summary = run_manifest_checks(dir.path()).unwrap();

        assert_eq!(summary.assurance, DataAssurance::Partial);
        assert!(summary.checks["pipeline_probe"]);
        assert!(summary.checks["data_reconciliation"]);
        assert!(summary.checks["data_rerun_consistency"]);
        assert!(!summary.checks["data_claims_binding"]);
    }

    #[test]
    fn generated_script_without_probe_evidence_is_static_never_full() {
        let dir = tempfile::tempdir().unwrap();
        write_pipeline(dir.path(), "12.5", "12.5");

        assert_eq!(assurance_from_evidence(dir.path()), DataAssurance::Static);
    }

    #[test]
    fn changed_rerun_results_are_failed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("pipeline")).unwrap();
        std::fs::write(
            dir.path().join("pipeline/main.py"),
            r#"import json
from pathlib import Path
Path("output").mkdir(exist_ok=True)
counter_path = Path("output/counter.txt")
counter = int(counter_path.read_text()) + 1 if counter_path.exists() else 1
counter_path.write_text(str(counter))
results = {"reconciliation": {"input_rows": 1, "used_rows": 1, "excluded": []}, "values": {"total": counter}}
Path("output/results.json").write_text(json.dumps(results, sort_keys=True))
Path("output/report.md").write_text(f"Total {counter}")
"#,
        )
        .unwrap();

        let summary = run_manifest_checks(dir.path()).unwrap();

        assert_eq!(summary.assurance, DataAssurance::Failed);
        assert!(!summary.checks["data_rerun_consistency"]);
    }

    #[test]
    fn runtime_uses_every_internal_manifest_adapter() {
        assert!(has_all_manifest_adapters());
    }
}
