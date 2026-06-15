use std::collections::BTreeSet;

use crate::session::store::SessionSnapshot;

use super::verify::VerificationReport;
use super::{PlanStep, StepPlan, bullet_list, inline_list, required_artifact_contract_prompt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StepProgressReport {
    pub(super) missing_before: Vec<String>,
    pub(super) missing_after: Vec<String>,
    pub(super) write_or_edit_paths: Vec<String>,
    pub(super) repeated_write_or_edit_paths: Vec<String>,
    pub(super) no_expected_path_progress: bool,
}

pub(super) fn build_repair_prompt(
    plan: &StepPlan,
    step: &PlanStep,
    report: &VerificationReport,
    progress: &StepProgressReport,
    turn_error: Option<&str>,
    repair_cycle: usize,
    max_repair_cycles: usize,
) -> String {
    let turn_error = turn_error
        .map(|err| format!("\nPrevious turn stop reason:\n- {err}\n"))
        .unwrap_or_default();
    let progress_note = if progress.no_expected_path_progress {
        format!(
            "\nStep progress warning:\n- Missing expected paths did not decrease.\n- Missing before: {}\n- Missing now: {}\n- Write/Edit paths observed: {}\n- Repeated Write/Edit paths: {}\n\nDo not keep rewriting only the observed existing files. Emit Write/Edit tool calls for the missing expected paths first.\n",
            inline_list(&progress.missing_before),
            inline_list(&progress.missing_after),
            inline_list(&progress.write_or_edit_paths),
            inline_list(&progress.repeated_write_or_edit_paths),
        )
    } else {
        String::new()
    };
    let required_artifacts = required_artifact_contract_prompt(&plan.goal);
    format!(
        "The previous step did not pass deterministic verification.{turn_error}\nOverall goal:\n{goal}\n\n{required_artifacts}Current step id: {id}\nCurrent step instruction:\n{instruction}\n\nExpected paths for this step:\n{paths}\n\nVerification commands for this step:\n{verify}\n\nExpected verification result: {expected_result}\n\nRepair cycle: {repair_cycle}/{max_repair_cycles}\n\nVerification failures:\n{failures}\n{progress_note}\nRepair only this step. Treat verifier failures as actionable development feedback, not as final status. If the verifier actually ran and reported compiler, import, test, or assertion errors, inspect the relevant small file ranges and make a concrete Read/Edit/Write fix before your final answer. Do not declare verification deferred when the failure log identifies a fixable source or config problem. After a file change, rerun the same verification command when possible. Do not move to later steps. If expected_result is fail, do not implement the production fix in this step; make the intended red test fail for the right reason.",
        goal = plan.goal,
        required_artifacts = required_artifacts,
        id = step.id,
        instruction = step.instruction,
        paths = bullet_list(&step.expected_paths),
        verify = bullet_list(&step.verify),
        expected_result = step.expected_result,
        repair_cycle = repair_cycle,
        max_repair_cycles = max_repair_cycles,
        failures = bullet_list(&report.failures),
    )
}

pub(super) fn analyze_step_progress(
    session: &SessionSnapshot,
    turn_start: usize,
    missing_before: Vec<String>,
    report: &VerificationReport,
) -> StepProgressReport {
    let missing_after = report_missing_paths(report);
    let write_or_edit_paths = write_or_edit_paths_since(session, turn_start);
    let repeated_write_or_edit_paths = repeated_items(&write_or_edit_paths);
    let no_expected_path_progress =
        !missing_after.is_empty() && missing_after.len() >= missing_before.len();
    StepProgressReport {
        missing_before,
        missing_after,
        write_or_edit_paths,
        repeated_write_or_edit_paths,
        no_expected_path_progress,
    }
}

pub(super) fn write_or_edit_paths_since(session: &SessionSnapshot, start: usize) -> Vec<String> {
    session
        .messages
        .iter()
        .skip(start)
        .filter(|message| message.role == "assistant")
        .flat_map(|message| message.tool_calls.iter())
        .filter(|call| call.name == "Write" || call.name == "Edit")
        .filter_map(|call| {
            call.arguments
                .get("path")
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
        .collect()
}

pub(super) fn verified_step_stop_reason(turn_error: Option<&str>) -> String {
    match turn_error {
        Some(err) => format!("{}_verified", turn_error_kind(err)),
        None => "completed".to_string(),
    }
}

pub(super) fn repaired_step_stop_reason(
    turn_error: Option<&str>,
    repair_error: Option<&str>,
) -> String {
    match (turn_error, repair_error) {
        (None, None) => "repaired".to_string(),
        (Some(turn), None) => format!("repaired_after_{}", turn_error_kind(turn)),
        (None, Some(repair)) => format!("repaired_after_{}", turn_error_kind(repair)),
        (Some(turn), Some(repair)) => {
            format!(
                "repaired_after_{}_and_{}",
                turn_error_kind(turn),
                turn_error_kind(repair)
            )
        }
    }
}

pub(super) fn failed_step_stop_reason(
    turn_error: Option<&str>,
    repair_error: Option<&str>,
) -> String {
    match (turn_error, repair_error) {
        (None, None) => "verification_failed".to_string(),
        (Some(turn), None) => format!("verification_failed_after_{}", turn_error_kind(turn)),
        (None, Some(repair)) => {
            format!("verification_failed_after_{}", turn_error_kind(repair))
        }
        (Some(turn), Some(repair)) => {
            format!(
                "verification_failed_after_{}_and_{}",
                turn_error_kind(turn),
                turn_error_kind(repair)
            )
        }
    }
}

pub(super) fn build_repair_exhausted_report(
    plan: &StepPlan,
    step: &PlanStep,
    report: &VerificationReport,
    progress: &StepProgressReport,
    turn_error: Option<&str>,
    repair_error: Option<&str>,
    file_change_repairs: usize,
    max_file_change_repairs: usize,
) -> String {
    let next_command = suggested_ultra_plan_run_command(plan, step, report);
    let mut lines = vec![
        "repair attempts exhausted".to_string(),
        format!("step: {}", step.id),
        format!("file-changing repair attempts: {file_change_repairs}/{max_file_change_repairs}"),
        format!(
            "missing expected paths: {}",
            inline_list(&progress.missing_after)
        ),
        format!(
            "changed files during this step: {}",
            inline_list(&progress.write_or_edit_paths)
        ),
        format!(
            "repeated changed files: {}",
            inline_list(&progress.repeated_write_or_edit_paths)
        ),
        format!("verification failures: {}", inline_list(&report.failures)),
    ];
    if !step.verify.is_empty() {
        lines.push(format!("verifier commands: {}", inline_list(&step.verify)));
    }
    if let Some(err) = turn_error {
        lines.push(format!("initial turn stop reason: {err}"));
    }
    if let Some(err) = repair_error {
        lines.push(format!("last repair stop reason: {err}"));
    }
    lines.push(
        "next step: switch from local repair to explicit replanning with /ultra-plan-run"
            .to_string(),
    );
    lines.push(format!("suggested command: {next_command}"));
    lines.join("\n")
}

fn report_missing_paths(report: &VerificationReport) -> Vec<String> {
    report
        .failures
        .iter()
        .filter_map(|failure| failure.strip_prefix("missing expected path: "))
        .map(str::to_string)
        .collect()
}

fn repeated_items(items: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut repeated = BTreeSet::new();
    for item in items {
        if !seen.insert(item.clone()) {
            repeated.insert(item.clone());
        }
    }
    repeated.into_iter().collect()
}

fn turn_error_kind(err: &str) -> &'static str {
    if err.contains("max_iterations") {
        "max_iterations"
    } else {
        "turn_error"
    }
}

fn suggested_ultra_plan_run_command(
    plan: &StepPlan,
    step: &PlanStep,
    report: &VerificationReport,
) -> String {
    let mut goal = format!(
        "Repair failed step {}. Original goal: {}. Step instruction: {}.",
        step.id, plan.goal, step.instruction
    );
    let missing = report_missing_paths(report);
    if !missing.is_empty() {
        goal.push_str(" Missing expected paths: ");
        goal.push_str(&missing.join(", "));
        goal.push('.');
    }
    if !step.verify.is_empty() {
        goal.push_str(" Verification commands: ");
        goal.push_str(&step.verify.join(", "));
        goal.push('.');
    }
    if !report.failures.is_empty() {
        goal.push_str(" Current failures: ");
        goal.push_str(&report.failures.join("; "));
        goal.push('.');
    }
    let goal = goal
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(900)
        .collect::<String>();
    format!("/ultra-plan-run {goal}")
}
