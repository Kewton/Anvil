use std::path::Path;

use crate::planner::verify::VerificationReport;

#[derive(Debug, Clone, Default)]
pub struct RepairContext {
    pub overall_goal: Option<String>,
    pub required_final_artifacts: Vec<String>,
    pub step_instruction: Option<String>,
    pub expected_paths: Vec<String>,
    pub verify_commands: Vec<String>,
    pub expected_result: Option<String>,
    pub repair_attempt: Option<usize>,
    pub max_repair_turns: Option<usize>,
    pub missing_paths: Vec<String>,
    pub changed_files: Vec<String>,
    pub repeated_changed_files: Vec<String>,
    pub initial_stop_reason: Option<String>,
    pub repair_stop_reason: Option<String>,
    pub progress_warning: Option<String>,
}

pub fn build_repair_prompt(step_id: &str, report: &VerificationReport) -> String {
    build_repair_prompt_with_context(step_id, report, &RepairContext::default())
}

pub fn build_repair_prompt_with_context(
    step_id: &str,
    report: &VerificationReport,
    context: &RepairContext,
) -> String {
    let mut prompt = format!(
        "Repair step `{step_id}`. Verification failed: {}.\n\
Make the smallest bounded change, then stop.",
        report.primary_reason()
    );
    if let Some(goal) = &context.overall_goal {
        prompt.push_str("\n\nOverall goal:\n");
        prompt.push_str(goal);
    }
    if let (Some(attempt), Some(max)) = (context.repair_attempt, context.max_repair_turns) {
        prompt.push_str("\n\nRepair budget:\n");
        prompt.push_str(&format!("- attempt {attempt}/{max}\n"));
    }
    if !context.required_final_artifacts.is_empty() {
        prompt.push_str("\n\nRequired final artifacts:\n");
        prompt.push_str(&bullet_list(&context.required_final_artifacts));
    }
    if let Some(instruction) = &context.step_instruction {
        prompt.push_str("\n\nCurrent step instruction:\n");
        prompt.push_str(instruction);
    }
    if !context.expected_paths.is_empty() {
        prompt.push_str("\n\nExpected paths after this step:\n");
        prompt.push_str(&bullet_list(&context.expected_paths));
    }
    if !context.verify_commands.is_empty() {
        prompt.push_str("\n\nVerification commands for this step:\n");
        prompt.push_str(&bullet_list(&context.verify_commands));
    }
    if let Some(expected) = &context.expected_result {
        prompt.push_str("\n\nExpected verification result:\n");
        prompt.push_str(expected);
    }
    if !report.missing_paths.is_empty() {
        prompt.push_str("\n\nMissing expected paths:\n");
        prompt.push_str(&bullet_list(&report.missing_paths));
    }
    if !report.command_failures.is_empty() {
        prompt.push_str("\n\nCommand failures:\n");
        let failures = report
            .command_failures
            .iter()
            .map(|failure| format!("{}: {}", failure.command, failure.reason))
            .collect::<Vec<_>>();
        prompt.push_str(&bullet_list(&failures));
    }
    if !context.changed_files.is_empty() {
        prompt.push_str("\n\nFiles already changed in this step:\n");
        prompt.push_str(&bullet_list(&context.changed_files));
    }
    if let Some(warning) = &context.progress_warning {
        prompt.push_str("\n\nProgress warning:\n");
        prompt.push_str(warning);
    }
    prompt.push_str(
        "\n\nRepair rules:\n\
- Work only on this step's missing or failed artifacts.\n\
- Treat verifier output as actionable feedback.\n\
- If this is an expected failing red test step, preserve the expected failure instead of implementing the feature.\n\
- Re-run only the declared deterministic verification mentally or via tools needed for this step.\n\
- Stop after the smallest bounded repair.",
    );
    prompt
}

pub fn save_repair_report(
    root: &Path,
    step_id: &str,
    report: &VerificationReport,
) -> anyhow::Result<std::path::PathBuf> {
    save_repair_report_with_context(root, step_id, report, &RepairContext::default())
}

pub fn save_repair_report_with_context(
    root: &Path,
    step_id: &str,
    report: &VerificationReport,
    context: &RepairContext,
) -> anyhow::Result<std::path::PathBuf> {
    let dir = root.join(".anvil").join("repairs");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("repair-{step_id}.md"));
    std::fs::write(&path, render_repair_report(step_id, report, context))?;
    Ok(path)
}

fn render_repair_report(
    step_id: &str,
    report: &VerificationReport,
    context: &RepairContext,
) -> String {
    format!(
        "# Repair exhausted\n\n\
Step: `{step_id}`\n\n\
Primary failure: {}\n\n\
## Missing Paths\n{}\n\n\
## Command Failures\n{}\n\n\
## Dependency Missing\n{}\n\n\
## Profile Failures\n{}\n\n\
## Changed Files\n{}\n\n\
## Repeated Changed Files\n{}\n\n\
## Step Contract\n\
- overall goal: {}\n\
- expected result: {}\n\
- expected paths: {}\n\
- verify commands: {}\n\n\
## Stop Reasons\n\
- initial: {}\n\
- repair: {}\n\n\
## Suggested Replan\n\
Run `/plan-run` again with the original goal and include the missing paths above as required artifacts.\n",
        report.primary_reason(),
        list_or_none(&report.missing_paths),
        list_or_none(
            &report
                .command_failures
                .iter()
                .map(|failure| format!("{}: {}", failure.command, failure.reason))
                .collect::<Vec<_>>()
        ),
        list_or_none(&report.dependency_missing),
        list_or_none(&report.profile_failures),
        list_or_none(&context.changed_files),
        list_or_none(&context.repeated_changed_files),
        context.overall_goal.as_deref().unwrap_or("unknown"),
        context.expected_result.as_deref().unwrap_or("unknown"),
        list_or_none(&context.expected_paths),
        list_or_none(&context.verify_commands),
        context.initial_stop_reason.as_deref().unwrap_or("unknown"),
        context.repair_stop_reason.as_deref().unwrap_or("unknown")
    )
}

fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "- none".to_string()
    } else {
        bullet_list(items)
    }
}

fn bullet_list(items: &[String]) -> String {
    items
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repair_exhausted_report_contains_missing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let report = VerificationReport::missing_path("src/app/page.tsx");
        let path = save_repair_report(dir.path(), "step-1", &report).unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("src/app/page.tsx"));
        assert!(text.contains("Suggested Replan"));
    }

    #[test]
    fn repair_exhausted_report_contains_changed_files() {
        let dir = tempfile::tempdir().unwrap();
        let report = VerificationReport::missing_path("package.json");
        let context = RepairContext {
            changed_files: vec!["src/app/page.tsx".to_string()],
            repeated_changed_files: vec!["src/app/page.tsx".to_string()],
            initial_stop_reason: Some("AssistantFinal".to_string()),
            repair_stop_reason: Some("AssistantFinal".to_string()),
            ..RepairContext::default()
        };
        let path =
            save_repair_report_with_context(dir.path(), "step-1", &report, &context).unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("src/app/page.tsx"));
        assert!(text.contains("initial: AssistantFinal"));
    }

    #[test]
    fn repair_prompt_includes_source_contract() {
        let report = VerificationReport::missing_path("src/app/page.tsx");
        let context = RepairContext {
            overall_goal: Some("Build a game".to_string()),
            required_final_artifacts: vec![
                "package.json".to_string(),
                "src/app/page.tsx".to_string(),
            ],
            step_instruction: Some("Create the page".to_string()),
            expected_paths: vec!["src/app/page.tsx".to_string()],
            verify_commands: vec!["npm run build".to_string()],
            expected_result: Some("pass".to_string()),
            repair_attempt: Some(1),
            max_repair_turns: Some(4),
            ..RepairContext::default()
        };
        let prompt = build_repair_prompt_with_context("page", &report, &context);
        assert!(prompt.contains("Overall goal:"));
        assert!(prompt.contains("Build a game"));
        assert!(prompt.contains("Current step instruction:"));
        assert!(prompt.contains("Verification commands for this step:"));
        assert!(prompt.contains("Expected verification result:"));
        assert!(prompt.contains("attempt 1/4"));
    }
}
