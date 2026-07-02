use std::path::Path;

use crate::eval_events;
use crate::minimal_loop::repair_target::classify_repair_target;
use crate::planner::ultra_plan::{UltraPhase, UltraPlan, parse_ultra_plan, render_ultra_plan};
use crate::planner::verify::VerificationReport;

#[derive(Debug, Clone, Default)]
pub struct RepairContext {
    pub profile: Option<String>,
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

#[derive(Debug, Clone, Default)]
pub struct RecoveryHandoff {
    pub profile: String,
    pub original_goal: String,
    pub failed_phase: Option<String>,
    pub failed_step: Option<String>,
    pub failure_kind: String,
    pub failure_evidence: Vec<String>,
    pub missing_paths: Vec<String>,
    pub missing_capabilities: Vec<String>,
    pub verify_commands: Vec<String>,
    pub changed_paths: Vec<String>,
    pub repair_targets: Vec<String>,
}

pub fn build_repair_prompt(step_id: &str, report: &VerificationReport) -> String {
    build_repair_prompt_with_context(step_id, report, &RepairContext::default())
}

pub fn build_repair_prompt_with_context(
    step_id: &str,
    report: &VerificationReport,
    context: &RepairContext,
) -> String {
    let repair_target = classify_repair_target(report);
    let mut prompt = format!(
        "Repair step `{step_id}`. Verification failed: {}.\n\
Repair target: {}. {}\n\
Make the smallest bounded change, then stop.",
        report.primary_reason(),
        repair_target.as_str(),
        repair_target.guidance()
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
    let path = dir.join(format!("repair-{step_id}-{}.md", uuid::Uuid::now_v7()));
    std::fs::write(&path, render_repair_report(step_id, report, context))?;
    Ok(path)
}

pub fn save_ultra_recovery_prompt(
    root: &Path,
    scope: &str,
    handoff: &RecoveryHandoff,
) -> anyhow::Result<std::path::PathBuf> {
    let dir = root.join(".anvil").join("repairs");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("repair-{scope}-{}.md", uuid::Uuid::now_v7()));
    std::fs::write(&path, render_ultra_recovery_prompt(handoff))?;
    Ok(path)
}

pub fn build_recovery_ultra_plan(handoff: &RecoveryHandoff) -> UltraPlan {
    let failed_phase = handoff.failed_phase.as_deref().unwrap_or("unknown");
    let failed_step = handoff.failed_step.as_deref().unwrap_or("unknown");
    let missing_signals = recovery_missing_signals(handoff);
    let verify_preference = recovery_verify_preference(handoff);
    UltraPlan {
        goal: handoff.original_goal.clone(),
        profile: handoff.profile.clone(),
        style: "recovery".to_string(),
        intent: "recover".to_string(),
        phases: vec![
            UltraPhase {
                id: "inspect-current-state".to_string(),
                prompt: format!(
                    "Inspect the current workspace before changing files. Original goal: {}. Failed acceptance layer or phase: {failed_phase}. Failed step: {failed_step}. Failure kind: {}. Preserve useful existing artifacts and identify the smallest remaining implementation gap.",
                    handoff.original_goal, handoff.failure_kind
                ),
            },
            UltraPhase {
                id: format!("repair-{}", recovery_plan_phase_token(failed_phase)),
                prompt: format!(
                    "Repair the incomplete work for the failed phase without restarting from scratch.\nOriginal goal: {}\nFailed acceptance layer or phase: {failed_phase}\nFailed step: {failed_step}\nMissing capability or artifact signals:\n{}\nFailure evidence:\n{}\nRepair targets:\n{}\nCreate or update the task-specific implementation artifacts needed to satisfy the original goal. Do not treat scaffold-only, setup-only, style-only, or build-only output as complete.",
                    handoff.original_goal,
                    list_or_none(&missing_signals),
                    list_or_none(&redacted_list(&handoff.failure_evidence)),
                    list_or_none(&handoff.repair_targets),
                ),
            },
            UltraPhase {
                id: "verify-recovery".to_string(),
                prompt: format!(
                    "Verify the recovered output with deterministic checks and repair only targeted failures.\nOriginal goal: {}\nFailed acceptance layer or phase: {failed_phase}\nPreferred verify/browser check:\n{}\nVerify preference: use the preferred checks above.\nExpected recovery result: runnable task-specific output, not only a saved plan or diagnostic report.",
                    handoff.original_goal,
                    list_or_none(&verify_preference),
                ),
            },
        ],
    }
}

pub fn save_recovery_ultra_plan(
    root: &Path,
    scope: &str,
    handoff: &RecoveryHandoff,
) -> anyhow::Result<std::path::PathBuf> {
    let plan = build_recovery_ultra_plan(handoff);
    let rendered = render_ultra_plan(&plan);
    let parsed = parse_ultra_plan(&rendered)?;
    if parsed != plan {
        anyhow::bail!("recovery UltraPlan failed render/parse roundtrip");
    }
    let dir = root.join(".anvil").join("plans");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!(
        "recovery-ultra-plan-{scope}-{}.yaml",
        uuid::Uuid::now_v7()
    ));
    std::fs::write(&path, rendered)?;
    Ok(path)
}

pub fn suggested_ultra_recovery_command(path: &Path, profile: &str) -> String {
    format!(
        "/ultra-plan-run --profile {} \"$(cat {})\"",
        shell_quote_token(profile),
        shell_quote_path(path)
    )
}

pub fn suggested_recovery_ultra_plan_command(path: &Path) -> String {
    format!("/run-ultra-plan {}", shell_quote_path(path))
}

fn shell_quote_path(path: &Path) -> String {
    let display = workspace_relative_handoff_path(path);
    if display
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/'))
    {
        display
    } else {
        format!("{display:?}")
    }
}

fn workspace_relative_handoff_path(path: &Path) -> String {
    let components = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    if let Some(index) = components.iter().position(|part| part == ".anvil") {
        return components[index..].join("/");
    }
    path.to_string_lossy().replace('\\', "/")
}

fn recovery_missing_signals(handoff: &RecoveryHandoff) -> Vec<String> {
    let mut signals = Vec::new();
    signals.extend(
        handoff
            .missing_capabilities
            .iter()
            .map(|value| format!("missing capability: {value}")),
    );
    signals.extend(
        handoff
            .missing_paths
            .iter()
            .map(|value| format!("missing artifact: {value}")),
    );
    if signals.is_empty() {
        signals.extend(
            handoff
                .repair_targets
                .iter()
                .map(|value| format!("repair target: {value}")),
        );
    }
    signals
}

fn recovery_verify_preference(handoff: &RecoveryHandoff) -> Vec<String> {
    if handoff.verify_commands.is_empty() {
        vec![
            "use deterministic file existence, build, route, and capability evidence checks"
                .to_string(),
            "avoid shell control syntax and interactive dev-server-only verification".to_string(),
        ]
    } else {
        handoff.verify_commands.clone()
    }
}

fn recovery_plan_phase_token(value: &str) -> String {
    let token = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    if token.trim_matches('-').is_empty() {
        "failed-phase".to_string()
    } else {
        token
    }
}

pub fn render_ultra_recovery_prompt(handoff: &RecoveryHandoff) -> String {
    format!(
        "Recover this failed run by producing and executing a focused ultra plan.\n\n\
Original goal:\n{}\n\n\
Profile: {}\n\n\
Failure scope:\n- phase: {}\n- step: {}\n- kind: {}\n\n\
Failure evidence:\n{}\n\n\
Missing paths:\n{}\n\n\
Missing capabilities:\n{}\n\n\
Verification commands:\n{}\n\n\
Changed paths:\n{}\n\n\
Repair targets:\n{}\n\n\
Required recovery action:\n\
- Inspect the current workspace state first.\n\
- Preserve already useful artifacts.\n\
- Create or repair the missing implementation artifacts.\n\
- Use deterministic verification.\n\
- Do not treat scaffold-only or build-only output as complete.\n",
        handoff.original_goal,
        handoff.profile,
        handoff.failed_phase.as_deref().unwrap_or("unknown"),
        handoff.failed_step.as_deref().unwrap_or("unknown"),
        handoff.failure_kind,
        list_or_none(&redacted_list(&handoff.failure_evidence)),
        list_or_none(&handoff.missing_paths),
        list_or_none(&handoff.missing_capabilities),
        list_or_none(&handoff.verify_commands),
        list_or_none(&handoff.changed_paths),
        list_or_none(&handoff.repair_targets),
    )
}

fn render_repair_report(
    step_id: &str,
    report: &VerificationReport,
    context: &RepairContext,
) -> String {
    let profile = context.profile.as_deref().unwrap_or("generic");
    let recovery = RecoveryHandoff {
        profile: profile.to_string(),
        original_goal: context.overall_goal.clone().unwrap_or_default(),
        failed_phase: None,
        failed_step: Some(step_id.to_string()),
        failure_kind: classify_repair_target(report).as_str().to_string(),
        failure_evidence: std::iter::once(report.primary_reason())
            .chain(context.progress_warning.iter().cloned())
            .chain(
                report
                    .command_failures
                    .iter()
                    .map(|failure| format!("{}: {}", failure.command, failure.reason)),
            )
            .collect(),
        missing_paths: report.missing_paths.clone(),
        missing_capabilities: Vec::new(),
        verify_commands: context.verify_commands.clone(),
        changed_paths: context.changed_files.clone(),
        repair_targets: vec![classify_repair_target(report).as_str().to_string()],
    };
    format!(
        "# Repair exhausted\n\n\
Step: `{step_id}`\n\n\
Primary failure: {}\n\n\
Repair target: {}\n\n\
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
Next step: switch from local repair to explicit replanning with `/ultra-plan-run`.\n\n\
Suggested command:\n\
`/ultra-plan-run --profile {profile} \"$(cat .anvil/repairs/repair-...)\"`\n\n\
## Ultra Recovery Prompt\n\
{}\n",
        report.primary_reason(),
        classify_repair_target(report).as_str(),
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
        context.repair_stop_reason.as_deref().unwrap_or("unknown"),
        render_ultra_recovery_prompt(&recovery)
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

fn redacted_list(items: &[String]) -> Vec<String> {
    items
        .iter()
        .map(|item| eval_events::body_snippet(item))
        .collect()
}

fn shell_quote_token(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        value.to_string()
    } else {
        "generic".to_string()
    }
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
        assert!(prompt.contains("Repair target:"));
    }

    #[test]
    fn recovery_ultra_plan_roundtrips_and_contains_recovery_contract() {
        let dir = tempfile::tempdir().unwrap();
        let handoff = RecoveryHandoff {
            profile: "nextjs".to_string(),
            original_goal: "Build an interactive web game".to_string(),
            failed_phase: Some("web-audio-synth-and-ui".to_string()),
            failed_step: None,
            failure_kind: "phase_scaffold_error".to_string(),
            failure_evidence: vec!["verify command may not use shell control syntax".to_string()],
            missing_paths: vec!["src/app/page.tsx".to_string()],
            missing_capabilities: vec!["interactive_ui".to_string()],
            verify_commands: vec!["npm run build".to_string()],
            changed_paths: Vec::new(),
            repair_targets: vec!["phase_scaffold".to_string()],
        };
        let path = save_recovery_ultra_plan(dir.path(), "phase-web-audio", &handoff).unwrap();
        assert!(path.starts_with(dir.path().join(".anvil").join("plans")));
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("recovery-ultra-plan-phase-web-audio-")
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("Build an interactive web game"));
        assert!(text.contains("web-audio-synth-and-ui"));
        assert!(text.contains("interactive_ui"));
        assert!(text.contains("Failed acceptance layer or phase"));
        assert!(text.contains("Preferred verify/browser check"));
        assert!(text.contains("Verify preference"));
        let parsed = parse_ultra_plan(&text).unwrap();
        assert_eq!(parsed, build_recovery_ultra_plan(&handoff));
        assert_eq!(
            parse_ultra_plan(&render_ultra_plan(&parsed)).unwrap(),
            parsed
        );
    }

    #[test]
    fn suggested_recovery_commands_use_workspace_relative_anvil_paths() {
        let path =
            std::path::Path::new("/tmp/workspace/.anvil/plans/recovery-ultra-plan-phase-x.yaml");
        assert_eq!(
            suggested_recovery_ultra_plan_command(path),
            "/run-ultra-plan .anvil/plans/recovery-ultra-plan-phase-x.yaml"
        );
        let prompt = std::path::Path::new("/tmp/workspace/.anvil/repairs/repair-phase-x.md");
        assert_eq!(
            suggested_ultra_recovery_command(prompt, "nextjs"),
            "/ultra-plan-run --profile nextjs \"$(cat .anvil/repairs/repair-phase-x.md)\""
        );
    }
}
