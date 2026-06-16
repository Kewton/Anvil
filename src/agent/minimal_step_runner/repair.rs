use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::session::store::SessionSnapshot;

use super::verify::VerificationReport;
use super::{
    PlanStep, StepPlan, UltraProfile, bullet_list, inline_list, required_artifact_contract_prompt,
    slug,
};

const REPAIR_PROMPT_MAX_CHARS: usize = 3_600;
const REPAIR_GOAL_MAX_CHARS: usize = 800;
const REPAIR_INSTRUCTION_MAX_CHARS: usize = 700;
const REPAIR_FAILURE_MAX_CHARS: usize = 1_800;
const REPAIR_MAX_FAILURES: usize = 6;

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
    work_root: &Path,
    plan: &StepPlan,
    step: &PlanStep,
    report: &VerificationReport,
    progress: &StepProgressReport,
    turn_error: Option<&str>,
    repair_error: Option<&str>,
    file_change_repairs: usize,
    max_file_change_repairs: usize,
) -> String {
    let repair_prompt = build_ultra_repair_prompt(plan, step, report);
    let repair_prompt_save = save_repair_prompt(work_root, step, &repair_prompt);
    let suggested_profile = suggested_repair_profile(plan, step, report);
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
    match repair_prompt_save {
        Ok(path) => {
            let relative = path.to_string_lossy();
            lines.push(format!("repair prompt saved: {relative}"));
            lines.push(format!(
                "suggested command: /ultra-plan-run --profile {suggested_profile} \"$(cat {relative})\""
            ));
        }
        Err(err) => {
            lines.push(format!("repair prompt save failed: {err}"));
            lines.push(format!(
                "suggested command: /ultra-plan-run --profile {suggested_profile} <repair prompt unavailable>"
            ));
        }
    }
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

fn build_ultra_repair_prompt(
    plan: &StepPlan,
    step: &PlanStep,
    report: &VerificationReport,
) -> String {
    let mut lines = vec![
        format!("Repair failed step {}.", step.id),
        String::new(),
        "Original goal:".to_string(),
        truncate_chars(&plan.goal, REPAIR_GOAL_MAX_CHARS),
        String::new(),
        "Step instruction:".to_string(),
        truncate_chars(&step.instruction, REPAIR_INSTRUCTION_MAX_CHARS),
    ];
    let missing = report_missing_paths(report);
    if !missing.is_empty() {
        lines.push(String::new());
        lines.push("Missing expected paths:".to_string());
        lines.extend(missing.iter().map(|path| format!("- {path}")));
    }
    if !step.verify.is_empty() {
        lines.push(String::new());
        lines.push("Verification commands:".to_string());
        lines.extend(step.verify.iter().map(|command| format!("- {command}")));
    }
    if !report.failures.is_empty() {
        lines.push(String::new());
        lines.push("Current failures:".to_string());
        lines.extend(
            report
                .failures
                .iter()
                .take(REPAIR_MAX_FAILURES)
                .map(|failure| format!("- {}", truncate_chars(failure, REPAIR_FAILURE_MAX_CHARS))),
        );
        if report.failures.len() > REPAIR_MAX_FAILURES {
            lines.push(format!(
                "- ... {} more failures omitted; rerun the verifier after focused fixes.",
                report.failures.len() - REPAIR_MAX_FAILURES
            ));
        }
    }
    if report
        .failures
        .iter()
        .any(|failure| failure.contains("dependency_missing"))
    {
        lines.extend([
            String::new(),
            "Dependency setup note:".to_string(),
            "- This failure is a missing dependency/tooling precondition, not an ordinary source-code verifier failure.".to_string(),
            "- Do not keep rewriting existing source or config files if they already satisfy the contract.".to_string(),
            "- Replan with an explicit setup step before the verify step when setup is allowed.".to_string(),
            "- If setup is not allowed or cannot run, stop with dependency_missing; do not claim the verifier passed.".to_string(),
        ]);
    }
    lines.extend([
        String::new(),
        "Repair constraints:".to_string(),
        "- Preserve the existing workspace and current project structure.".to_string(),
        "- Focus on the failed step and its verifier output.".to_string(),
        "- Inspect relevant files before editing.".to_string(),
        "- Use Write/Edit for concrete fixes and rerun the verifier when possible.".to_string(),
    ]);
    truncate_chars(&lines.join("\n"), REPAIR_PROMPT_MAX_CHARS)
}

fn save_repair_prompt(
    work_root: &Path,
    step: &PlanStep,
    repair_prompt: &str,
) -> Result<PathBuf, String> {
    let repair_dir = work_root.join(".anvil").join("repairs");
    std::fs::create_dir_all(&repair_dir)
        .map_err(|err| format!("failed to create {}: {err}", repair_dir.display()))?;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("system time before unix epoch: {err}"))?
        .as_secs();
    let file_name = format!("repair-{}-{ts}.md", slug(&step.id));
    let absolute = repair_dir.join(file_name);
    std::fs::write(&absolute, repair_prompt)
        .map_err(|err| format!("failed to write {}: {err}", absolute.display()))?;
    absolute
        .strip_prefix(work_root)
        .map(PathBuf::from)
        .map_err(|err| format!("failed to relativize repair prompt path: {err}"))
}

fn suggested_repair_profile(
    plan: &StepPlan,
    step: &PlanStep,
    report: &VerificationReport,
) -> UltraProfile {
    if let Some(profile) = explicit_ultra_profile(&plan.goal)
        && profile != UltraProfile::Generic
    {
        return profile;
    }

    let evidence = format!(
        "{}\n{}\n{}\n{}",
        plan.goal,
        step.instruction,
        step.verify.join("\n"),
        report.failures.join("\n")
    )
    .to_ascii_lowercase();

    if evidence.contains("next.js")
        || evidence.contains("nextjs")
        || evidence.contains("next build")
        || evidence.contains("npm run build")
        || evidence.contains("app/page.tsx")
    {
        UltraProfile::Nextjs
    } else if evidence.contains("cargo ") || evidence.contains(".rs") || evidence.contains("rust") {
        UltraProfile::Rust
    } else if evidence.contains("pytest")
        || evidence.contains("python")
        || evidence.contains(".py")
        || evidence.contains("fastapi")
    {
        UltraProfile::Python
    } else {
        explicit_ultra_profile(&plan.goal).unwrap_or(UltraProfile::Generic)
    }
}

fn explicit_ultra_profile(goal: &str) -> Option<UltraProfile> {
    goal.lines()
        .find_map(|line| line.trim().strip_prefix("Ultra profile: "))
        .and_then(|value| value.parse().ok())
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let suffix = "\n[truncated]";
    let keep = max_chars.saturating_sub(suffix.chars().count());
    let mut out = value.chars().take(keep).collect::<String>();
    out.push_str(suffix);
    out
}
