use std::path::Path;
use std::time::Duration;

use serde_json::json;

use super::{checks, manifest};
use crate::eval_events;
use crate::minimal_loop::pipeline_probe::{self, PipelineProbeConfig};
use crate::planner::capability_catalog::{
    DataInternalCheck, InternalCapability, ProbeCapability, ResolvedCapability,
};
use crate::planner::step_plan::{PlanStep, StepKind, StepPlan};

pub(crate) const CATALOG_CHECK_PREFIX: &str = "anvil-catalog-check:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CatalogCheckOutcome {
    pub id: String,
    pub ok: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DataSetupStepChecks {
    pub expected_paths: Vec<String>,
    pub verify_commands: Vec<String>,
}

pub(crate) fn canonicalize_step_plan(
    plan: &mut StepPlan,
    eval_events_path: Option<&Path>,
) -> usize {
    plan.steps
        .iter_mut()
        .map(|step| canonicalize_step(step, eval_events_path))
        .sum()
}

pub(crate) fn catalog_check_command(id: &str) -> String {
    format!("{CATALOG_CHECK_PREFIX}{id}")
}

pub(crate) fn catalog_check_id(command: &str) -> Option<&str> {
    let id = command.trim().strip_prefix(CATALOG_CHECK_PREFIX)?;
    (!id.is_empty() && !id.chars().any(char::is_whitespace) && is_bound_check_id(id)).then_some(id)
}

pub(crate) fn execute_catalog_check(
    root: &Path,
    command: &str,
) -> Option<anyhow::Result<CatalogCheckOutcome>> {
    let id = catalog_check_id(command)?.to_string();
    Some(execute_bound_check(root, id))
}

pub(crate) fn run_step_catalog_checks(
    root: &Path,
    profile: Option<&str>,
    step: &PlanStep,
    report: &mut crate::planner::verify::VerificationReport,
) {
    for command in &step.verify {
        let Some(execution) = execute_catalog_check(root, command) else {
            continue;
        };
        if profile
            .is_none_or(|profile| crate::planner::profile::domain_profile(profile).id() != "data")
        {
            report.push_command_failure(
                command.clone(),
                "data catalog check is invalid outside the active data profile",
            );
            continue;
        }
        match execution {
            Ok(outcome) if outcome.ok => {}
            Ok(outcome) => report.push_profile_failure(format!(
                "{}:{}",
                outcome.id,
                outcome.reasons.join("; ")
            )),
            Err(error) => report.push_profile_failure(format!(
                "{}:catalog_check_error:{error}",
                catalog_check_id(command).unwrap_or("data_check")
            )),
        }
    }
}

pub(crate) fn setup_step_checks(step: &PlanStep) -> Option<DataSetupStepChecks> {
    let text = format!("{} {}", step.id, step.instruction).replace('\\', "/");
    let lower = text.to_ascii_lowercase();
    let mut expected_paths = step
        .expected_paths
        .iter()
        .filter(|path| manifest_owned_path(path))
        .cloned()
        .collect::<Vec<_>>();
    for path in [
        "pipeline/main.py",
        "output/inspection.json",
        "output/results.json",
        "output/report.md",
    ] {
        if lower.contains(path)
            || (path == "output/inspection.json" && lower.contains("inspection"))
        {
            push_unique(&mut expected_paths, path.to_string());
        }
    }
    let mut verify_commands = Vec::new();
    if expected_paths.iter().any(|path| path == "pipeline/main.py") {
        push_unique(
            &mut verify_commands,
            catalog_check_command("pipeline_probe"),
        );
    }
    if expected_paths
        .iter()
        .any(|path| path == "output/inspection.json")
    {
        push_unique(
            &mut verify_commands,
            "test -f output/inspection.json".to_string(),
        );
    }
    if expected_paths
        .iter()
        .any(|path| path == "output/results.json")
    {
        for id in ["data_results_schema", "data_reconciliation"] {
            push_unique(&mut verify_commands, catalog_check_command(id));
        }
    }
    if expected_paths.iter().any(|path| path == "output/report.md") {
        push_unique(
            &mut verify_commands,
            catalog_check_command("data_claims_binding"),
        );
    }
    (!expected_paths.is_empty() || !verify_commands.is_empty()).then_some(DataSetupStepChecks {
        expected_paths,
        verify_commands,
    })
}

pub(crate) fn owns_declared_paths(step: &PlanStep) -> bool {
    step.expected_paths
        .iter()
        .all(|path| manifest_owned_path(path))
}

pub(crate) fn preset_phase_supports_conversion(phase_id: &str) -> bool {
    manifest::get()
        .plan
        .phases
        .iter()
        .any(|phase| phase.id == phase_id)
}

pub(crate) fn supports_verify_conversion(step: &PlanStep) -> bool {
    if matches!(
        step.step_kind(),
        StepKind::Setup | StepKind::Inspect | StepKind::Verify
    ) {
        return true;
    }
    step.id
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|token| matches!(token, "setup" | "inspect" | "inspection"))
}

fn canonicalize_step(step: &mut PlanStep, eval_events_path: Option<&Path>) -> usize {
    let mut changes = canonicalize_expected_paths(step, eval_events_path);
    changes += canonicalize_instruction(step, eval_events_path);
    changes + canonicalize_verify_commands(step, eval_events_path)
}

fn canonicalize_expected_paths(step: &mut PlanStep, eval_events_path: Option<&Path>) -> usize {
    let step_id = step.id.clone();
    let original_paths = std::mem::take(&mut step.expected_paths);
    let mut canonical = Vec::with_capacity(original_paths.len());
    let mut changes = 0;
    for original in original_paths {
        let replacement = canonical_artifact_path(&original).unwrap_or_else(|| original.clone());
        if replacement != original {
            step.instruction = step.instruction.replace(&original, &replacement);
            emit_canonicalized(
                eval_events_path,
                &step_id,
                "expected_path",
                &original,
                &replacement,
                "canonical",
            );
            changes += 1;
        }
        push_unique(&mut canonical, replacement);
    }
    step.expected_paths = canonical;
    changes
}

fn canonicalize_instruction(step: &mut PlanStep, eval_events_path: Option<&Path>) -> usize {
    if !invented_results_schema(&step.instruction) {
        return 0;
    }
    let original = step.instruction.clone();
    let requirement = &manifest::get().guidance.contracts.state_requirement;
    step.instruction = format!(
        "{original}\n\nThe preceding schema example is invalid and must not be used. Canonical data contract: {requirement}"
    );
    let schema_check = catalog_check_command("data_results_schema");
    push_unique(&mut step.verify, schema_check.clone());
    emit_canonicalized(
        eval_events_path,
        &step.id,
        "instruction",
        &original,
        &step.instruction,
        "canonical",
    );
    1
}

fn canonicalize_verify_commands(step: &mut PlanStep, eval_events_path: Option<&Path>) -> usize {
    let original_commands = std::mem::take(&mut step.verify);
    let mut canonical = Vec::with_capacity(original_commands.len());
    let mut changes = 0;
    for original in original_commands {
        if catalog_check_id(&original).is_some() || !invented_verify_command(&original) {
            push_unique(&mut canonical, original);
            continue;
        }
        let replacements = inferred_catalog_checks(step, &original);
        if replacements.is_empty() {
            emit_canonicalized(
                eval_events_path,
                &step.id,
                "verify",
                &original,
                "advisory",
                "advisory",
            );
        } else {
            for id in replacements {
                let replacement = catalog_check_command(id);
                push_unique(&mut canonical, replacement.clone());
                emit_canonicalized(
                    eval_events_path,
                    &step.id,
                    "verify",
                    &original,
                    &replacement,
                    "canonical",
                );
            }
        }
        changes += 1;
    }
    step.verify = canonical;
    changes
}

fn invented_verify_command(command: &str) -> bool {
    invented_workspace_python_check(command) || inspection_literal_check(command)
}

fn invented_workspace_python_check(command: &str) -> bool {
    let mut fields = command.split_whitespace();
    let Some(program) = fields.next() else {
        return false;
    };
    if !matches!(program, "python" | "python3") {
        return false;
    }
    let Some(script) = fields.next() else {
        return false;
    };
    let script = script.trim_matches(['\'', '"']);
    if script == "pipeline/main.py" || script.starts_with('-') || !script.ends_with(".py") {
        return false;
    }
    crate::tools::path_guard::validate_workspace_relative(script).is_ok()
}

fn inspection_literal_check(command: &str) -> bool {
    let lower = command.trim().to_ascii_lowercase();
    (lower.starts_with("grep ") || lower.starts_with("rg ")) && lower.contains("inspection")
}

fn inferred_catalog_checks<'a>(step: &PlanStep, command: &'a str) -> Vec<&'a str> {
    let lower = format!("{} {} {command}", step.id, step.instruction).to_ascii_lowercase();
    let mut ids = Vec::new();
    if [
        "schema",
        "results.json",
        "excluded_rows",
        "aggregations",
        "summary.input_rows",
    ]
    .iter()
    .any(|token| lower.contains(token))
    {
        ids.push("data_results_schema");
    }
    if ["reconciliation", "input_rows", "used_rows", "excluded"]
        .iter()
        .any(|token| lower.contains(token))
    {
        ids.push("data_reconciliation");
    }
    if [
        "claims binding",
        "claims_binding",
        "numeric claim",
        "report claim",
    ]
    .iter()
    .any(|token| lower.contains(token))
    {
        ids.push("data_claims_binding");
    }
    if ["rerun", "reproduc", "determin"]
        .iter()
        .any(|token| lower.contains(token))
    {
        ids.push("data_rerun_consistency");
    }
    if [
        "verify_pipeline",
        "smoke-check",
        "smoke_check",
        "pipeline probe",
    ]
    .iter()
    .any(|token| lower.contains(token))
    {
        ids.push("pipeline_probe");
    }
    ids.retain(|id| is_bound_check_id(id));
    ids
}

fn invented_results_schema(instruction: &str) -> bool {
    let lower = instruction.to_ascii_lowercase();
    lower.contains("results.json")
        && ["excluded_rows", "aggregations", "summary.input_rows"]
            .iter()
            .any(|token| lower.contains(token))
        && !(lower.contains("\"reconciliation\"") && lower.contains("\"values\""))
}

fn canonical_artifact_path(path: &str) -> Option<String> {
    let lower = path.replace('\\', "/").to_ascii_lowercase();
    if lower == "output/report.html" {
        return Some("output/report.md".to_string());
    }
    if lower.contains("inspection") && (lower.ends_with(".json") || lower.ends_with(".md")) {
        return Some("output/inspection.json".to_string());
    }
    if lower.contains("report") && (lower.ends_with(".md") || lower.ends_with(".html")) {
        return Some("output/report.md".to_string());
    }
    if lower.contains("result") && lower.ends_with(".json") {
        return Some("output/results.json".to_string());
    }
    if lower.contains("pipeline") && lower.ends_with(".py") {
        return Some("pipeline/main.py".to_string());
    }
    None
}

fn manifest_owned_path(path: &str) -> bool {
    let lower = path.replace('\\', "/").to_ascii_lowercase();
    lower.starts_with("data/")
        || manifest::get()
            .step_templates
            .ownership
            .template_owned_artifacts
            .artifact_path_suffixes
            .iter()
            .any(|suffix| lower.ends_with(suffix))
}

fn is_bound_check_id(id: &str) -> bool {
    manifest::check_ids().iter().any(|bound| bound == id)
}

fn execute_bound_check(root: &Path, id: String) -> anyhow::Result<CatalogCheckOutcome> {
    let resolved = manifest::get()
        .resolve()?
        .into_values()
        .flatten()
        .find(|check| check.id == id)
        .ok_or_else(|| anyhow::anyhow!("data manifest check `{id}` is not bound"))?
        .capability;
    let (ok, reasons) = match resolved {
        ResolvedCapability::Internal(InternalCapability::Data(
            DataInternalCheck::ResultsSchema,
        )) => {
            let evidence = checks::check_results_schema(root)?;
            (evidence.ok, evidence.error.into_iter().collect())
        }
        ResolvedCapability::Internal(InternalCapability::Data(
            DataInternalCheck::Reconciliation,
        )) => {
            let evidence = checks::check_reconciliation(root)?;
            (evidence.ok, evidence.failure_kinds)
        }
        ResolvedCapability::Internal(InternalCapability::Data(
            DataInternalCheck::ClaimsBinding,
        )) => {
            let evidence = checks::check_claims_binding(root)?;
            (evidence.ok, evidence.failure_kinds)
        }
        ResolvedCapability::Probe(ProbeCapability::Pipeline {
            entry,
            timeout_seconds,
        }) => {
            let evidence = pipeline_probe::run(
                root,
                PipelineProbeConfig::new(entry)
                    .with_timeout(Duration::from_secs(timeout_seconds.into())),
            )?;
            (evidence.ok, evidence.failure_kinds)
        }
        ResolvedCapability::Probe(ProbeCapability::DataRerunConsistency {
            entry,
            timeout_seconds,
        }) => {
            let evidence = checks::check_rerun_consistency(
                root,
                &entry,
                Duration::from_secs(timeout_seconds.into()),
            )?;
            (evidence.ok, evidence.failure_kinds)
        }
        capability => anyhow::bail!("unsupported data catalog check adapter: {capability:?}"),
    };
    Ok(CatalogCheckOutcome { id, ok, reasons })
}

fn emit_canonicalized(
    eval_events_path: Option<&Path>,
    step_id: &str,
    field: &str,
    original: &str,
    replacement: &str,
    disposition: &str,
) {
    eval_events::emit(
        eval_events_path,
        json!({
            "event": "verify_canonicalized",
            "step_id": step_id,
            "field": field,
            "original": original,
            "replacement": replacement,
            "disposition": disposition,
        }),
    );
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_with(step: PlanStep) -> StepPlan {
        StepPlan {
            goal: "Analyze sales".to_string(),
            steps: vec![step],
        }
    }

    #[test]
    fn invented_schema_script_and_instruction_use_bound_catalog_checks() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut plan = plan_with(PlanStep {
            id: "verify-invented-schema".to_string(),
            kind: "verify".to_string(),
            expected_result: "pass".to_string(),
            instruction: "Require {excluded_rows:[{reason,count}], aggregations:[...]} in output/results.json.".to_string(),
            expected_paths: Vec::new(),
            verify: vec!["python3 tests/verify_pipeline.py".to_string()],
        });

        assert!(canonicalize_step_plan(&mut plan, Some(&events)) > 0);
        assert!(
            plan.steps[0]
                .verify
                .contains(&catalog_check_command("data_results_schema"))
        );
        assert!(
            !plan.steps[0]
                .verify
                .iter()
                .any(|command| { command.contains("verify_pipeline.py") })
        );
        assert!(plan.steps[0].instruction.contains("\"reconciliation\""));
        assert!(plan.steps[0].instruction.contains("\"values\""));
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"verify_canonicalized\""));
        assert!(event_text.contains("\"original\":\"python3 tests/verify_pipeline.py\""));
        assert!(event_text.contains("anvil-catalog-check:data_results_schema"));
    }

    #[test]
    fn unknown_invented_check_is_advisory_but_contract_checks_are_not_demoted() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let contract_marker = catalog_check_command("data_reconciliation");
        let mut plan = plan_with(PlanStep {
            id: "verify-custom-rule".to_string(),
            kind: "verify".to_string(),
            expected_result: "pass".to_string(),
            instruction: "Run a bespoke business-rule verifier.".to_string(),
            expected_paths: Vec::new(),
            verify: vec![
                "python3 tests/custom_guard.py".to_string(),
                "test -f output/results.json".to_string(),
                contract_marker.clone(),
            ],
        });

        assert_eq!(canonicalize_step_plan(&mut plan, Some(&events)), 1);
        assert_eq!(
            plan.steps[0].verify,
            ["test -f output/results.json".to_string(), contract_marker]
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"replacement\":\"advisory\""));
        assert!(event_text.contains("\"disposition\":\"advisory\""));
    }

    #[test]
    fn data_setup_checks_cover_canonical_artifacts_and_preset_phases() {
        let step = PlanStep {
            id: "setup-data-outputs".to_string(),
            kind: "setup".to_string(),
            expected_result: "pass".to_string(),
            instruction:
                "Prepare output/inspection.json, output/results.json, and output/report.md."
                    .to_string(),
            expected_paths: Vec::new(),
            verify: Vec::new(),
        };

        let checks = setup_step_checks(&step).expect("manifest-owned data checks");
        assert_eq!(
            checks.expected_paths,
            [
                "output/inspection.json",
                "output/results.json",
                "output/report.md",
            ]
        );
        assert!(
            checks
                .verify_commands
                .contains(&catalog_check_command("data_results_schema"))
        );
        assert!(
            checks
                .verify_commands
                .contains(&catalog_check_command("data_reconciliation"))
        );
        assert!(
            checks
                .verify_commands
                .contains(&catalog_check_command("data_claims_binding"))
        );
        assert!(owns_declared_paths(&step));
        assert!(preset_phase_supports_conversion("data-inspection"));
        assert!(preset_phase_supports_conversion("data-validation"));
        assert!(!preset_phase_supports_conversion("core-implementation"));
        assert!(supports_verify_conversion(&step));
        let mut report_step = step;
        report_step.id = "write-report".to_string();
        report_step.kind = "report".to_string();
        assert!(!supports_verify_conversion(&report_step));
    }

    #[test]
    fn bound_catalog_marker_executes_in_data_profile_and_never_demotes_cross_profile() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("output")).unwrap();
        std::fs::write(
            dir.path().join("output/results.json"),
            r#"{"reconciliation":{"input_rows":1,"used_rows":1,"excluded":[]},"values":{"total":1}}"#,
        )
        .unwrap();
        let step = PlanStep {
            id: "verify-results-schema".to_string(),
            kind: "verify".to_string(),
            expected_result: "pass".to_string(),
            instruction: "Run the bound results schema check.".to_string(),
            expected_paths: Vec::new(),
            verify: vec![catalog_check_command("data_results_schema")],
        };

        let (data_report, _) =
            crate::planner::verify::verify_step_with_profile_setup_observed_with_offline(
                dir.path(),
                &step,
                Some("data"),
                crate::minimal_loop::dependency_setup::NodeDependencySetupAuthority::None,
                true,
            );
        assert!(data_report.is_pass(), "{}", data_report.primary_reason());

        let (nextjs_report, _) =
            crate::planner::verify::verify_step_with_profile_setup_observed_with_offline(
                dir.path(),
                &step,
                Some("nextjs"),
                crate::minimal_loop::dependency_setup::NodeDependencySetupAuthority::None,
                true,
            );
        assert!(!nextjs_report.is_pass());
        assert!(
            nextjs_report.command_failures[0]
                .reason
                .contains("invalid outside the active data profile")
        );
    }
}
