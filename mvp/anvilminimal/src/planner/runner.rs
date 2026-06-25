use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::eval_events;
use crate::minimal_loop::loop_run::run_session_with_outcome_with_ui;
use crate::planner::intent::detect_intent;
use crate::planner::lint::{PlanLintReport, lint_step_plan_report, lint_ultra_plan_report};
use crate::planner::profile::{
    profile_after_phase, profile_auto_repair, profile_before_phase, profile_expected_paths,
    profile_guidance, profile_repair_prompt, verify_profile,
};
use crate::planner::repair::{
    RepairContext, build_repair_prompt_with_context, save_repair_report_with_context,
};
use crate::planner::step_plan::{
    PlanStep, StepPlan, parse_step_plan, parse_step_plan_with_default_goal, render_step_plan,
};
use crate::planner::ultra_plan::{UltraPlan, parse_ultra_plan, render_ultra_plan};
use crate::planner::verify::{VerificationReport, verify_step};
use crate::providers::{ChatClient, model_for};
use crate::state::SessionSnapshot;
use crate::tui::status::UiStatus;
use crate::tui::{InteractionUi, NOOP_UI};
use serde_json::json;

const STEP_TURN_MAX_ITERATIONS: usize = 8;
const STEP_REPAIR_MAX_ITERATIONS: usize = 6;
const STEP_REPAIR_MAX_TURNS: usize = 4;
const STEP_REPAIR_MAX_FILE_CHANGING_TURNS: usize = 2;

pub fn generate_step_plan(
    client: &mut dyn ChatClient,
    goal: &str,
    config: &Config,
) -> anyhow::Result<StepPlan> {
    generate_step_plan_with_ui(client, goal, config, &NOOP_UI)
}

pub fn generate_step_plan_with_ui(
    client: &mut dyn ChatClient,
    goal: &str,
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<StepPlan> {
    if ui.interrupted() {
        anyhow::bail!("interrupted by user");
    }
    let mut prompt = format!(
        "Create a YAML StepPlan for this goal. Return only YAML with goal and steps: {goal}"
    );
    if let Some(guidance) = profile_guidance(&config.profile, goal) {
        prompt.push_str("\n\nProfile contract:\n");
        prompt.push_str(&guidance);
        prompt.push_str(
            "\nInclude expected_paths on the final step so deterministic verification can catch missing artifacts.",
        );
    }
    let model = model_for(config, true);
    let mut last_error = None;
    for attempt in 1..=3 {
        let messages = vec![crate::state::ConversationMessage::user(prompt.clone())];
        let reply = {
            let _guard = ui.before_model_call(&format!("planner {} {model}", client.label()));
            client.chat(model, &messages, &[], false)?
        };
        ui.publish_status(UiStatus::for_model_reply(
            config,
            model,
            client.label(),
            reply.prompt_tokens,
            reply.completion_tokens,
        ));
        match parse_step_plan_with_default_goal(&reply.content, goal) {
            Ok((mut plan, repaired)) => {
                if repaired {
                    emit_planner_schema_repaired(
                        config,
                        client.label(),
                        model,
                        "schema",
                        "StepPlan missing goal",
                        attempt,
                    );
                }
                strengthen_step_plan_for_profile(&mut plan, config);
                let lint_report = lint_step_plan_report(&plan);
                if lint_report.is_pass() {
                    return Ok(plan);
                }
                let message = lint_report.primary_message();
                emit_planner_error_for_lint(config, client.label(), model, &lint_report, attempt);
                last_error = Some(message.clone());
                prompt = build_lint_retry_prompt(goal, &lint_report, attempt);
            }
            Err(err) => {
                last_error = Some(err.to_string());
                emit_planner_error(
                    config,
                    client.label(),
                    model,
                    "schema",
                    "planner_schema_error",
                    &err.to_string(),
                    attempt,
                );
                prompt = build_schema_retry_prompt(goal, &err.to_string(), attempt);
            }
        }
    }
    anyhow::bail!(
        "invalid StepPlan after corrective retries: {}",
        last_error.unwrap_or_else(|| "unknown parse error".to_string())
    )
}

pub fn save_step_plan(root: &Path, plan: &StepPlan) -> anyhow::Result<PathBuf> {
    let dir = root.join(".anvil").join("plans");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("plan-{}.yaml", uuid::Uuid::now_v7()));
    std::fs::write(&path, render_step_plan(plan))?;
    Ok(path)
}

pub fn run_plan_file(
    client: &mut dyn ChatClient,
    path: &Path,
    config: &Config,
) -> anyhow::Result<String> {
    run_plan_file_with_ui(client, path, config, &NOOP_UI)
}

pub fn run_plan_file_with_ui(
    client: &mut dyn ChatClient,
    path: &Path,
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<String> {
    let path = resolve_plan_file_path(&config.workspace_root, path)?;
    let text = std::fs::read_to_string(path)?;
    let plan = parse_step_plan(&text)?;
    run_step_plan_with_ui(client, &plan, config, ui)
}

pub fn generate_and_run_step_plan(
    planner: &mut dyn ChatClient,
    execution: &mut dyn ChatClient,
    goal: &str,
    config: &Config,
) -> anyhow::Result<String> {
    generate_and_run_step_plan_with_ui(planner, execution, goal, config, &NOOP_UI)
}

pub fn generate_and_run_step_plan_with_ui(
    planner: &mut dyn ChatClient,
    execution: &mut dyn ChatClient,
    goal: &str,
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<String> {
    let plan = generate_step_plan_with_ui(planner, goal, config, ui)?;
    save_step_plan(&config.workspace_root, &plan)?;
    run_step_plan_with_ui(execution, &plan, config, ui)
}

pub fn run_step_plan(
    client: &mut dyn ChatClient,
    plan: &StepPlan,
    config: &Config,
) -> anyhow::Result<String> {
    run_step_plan_with_ui(client, plan, config, &NOOP_UI)
}

pub fn run_step_plan_with_ui(
    client: &mut dyn ChatClient,
    plan: &StepPlan,
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<String> {
    let report = lint_step_plan_report(plan);
    if !report.is_pass() {
        emit_planner_error_for_lint(config, "plan-file", &config.planner_model, &report, 0);
        anyhow::bail!("{}", report.primary_message());
    }
    let mut session = SessionSnapshot::new();
    for step in &plan.steps {
        if ui.interrupted() {
            anyhow::bail!("interrupted by user");
        }
        run_step(client, &mut session, step, config, ui)?;
    }
    Ok(format!("plan-run complete: {} steps", plan.steps.len()))
}

fn run_step(
    client: &mut dyn ChatClient,
    session: &mut SessionSnapshot,
    step: &PlanStep,
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<()> {
    let instruction = prompt_with_required_paths(&step.instruction, &step.expected_paths);
    let step_config = capped_config(config, STEP_TURN_MAX_ITERATIONS);
    let initial = run_session_with_outcome_with_ui(
        client,
        session,
        &instruction,
        &step.expected_paths,
        &step_config,
        ui,
    )?;
    let report = verify_step(&config.workspace_root, step);
    if report.is_pass() {
        return Ok(());
    }
    let mut context = RepairContext {
        missing_paths: report.missing_paths.clone(),
        changed_files: initial.changed_paths.clone(),
        initial_stop_reason: Some(format!("{:?}", initial.stop_reason)),
        ..RepairContext::default()
    };
    let mut current_report = report;
    let mut previous_missing = current_report.missing_paths.len();
    let mut repair_stop_reason = None;
    let mut file_changing_repairs = 0usize;
    let repair_config = capped_config(config, STEP_REPAIR_MAX_ITERATIONS);
    for _ in 0..STEP_REPAIR_MAX_TURNS {
        let repair_prompt = build_repair_prompt_with_context(&step.id, &current_report, &context);
        let repair_prompt = prompt_with_required_paths(&repair_prompt, &step.expected_paths);
        let repair = run_session_with_outcome_with_ui(
            client,
            session,
            &repair_prompt,
            &step.expected_paths,
            &repair_config,
            ui,
        )?;
        repair_stop_reason = Some(format!("{:?}", repair.stop_reason));
        merge_changed_files(&mut context, &repair.changed_paths);
        if !repair.changed_paths.is_empty() {
            file_changing_repairs += 1;
        }
        let retry = verify_step(&config.workspace_root, step);
        if retry.is_pass() {
            return Ok(());
        }
        let next_missing = retry.missing_paths.len();
        if next_missing >= previous_missing {
            context.progress_warning = Some(format!(
                "Missing expected paths did not decrease after repair. Remaining: {}",
                if retry.missing_paths.is_empty() {
                    "none".to_string()
                } else {
                    retry.missing_paths.join(", ")
                }
            ));
        }
        previous_missing = next_missing;
        current_report = retry;
        if file_changing_repairs >= STEP_REPAIR_MAX_FILE_CHANGING_TURNS {
            break;
        }
    }
    context.repair_stop_reason = repair_stop_reason;
    save_repair_report_with_context(&config.workspace_root, &step.id, &current_report, &context)?;
    anyhow::bail!(
        "step {} failed verification after bounded repair: {}",
        step.id,
        current_report.primary_reason()
    )
}

fn capped_config(config: &Config, cap: usize) -> Config {
    let mut out = config.clone();
    out.max_iterations = out.max_iterations.min(cap);
    out
}

fn merge_changed_files(context: &mut RepairContext, incoming: &[String]) {
    for path in incoming {
        if context.changed_files.contains(path) {
            if !context.repeated_changed_files.contains(path) {
                context.repeated_changed_files.push(path.clone());
            }
        } else {
            context.changed_files.push(path.clone());
        }
    }
}

pub fn generate_ultra_plan(
    client: &mut dyn ChatClient,
    goal: &str,
    config: &Config,
) -> anyhow::Result<UltraPlan> {
    generate_ultra_plan_with_ui(client, goal, config, &NOOP_UI)
}

pub fn generate_ultra_plan_with_ui(
    client: &mut dyn ChatClient,
    goal: &str,
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<UltraPlan> {
    if ui.interrupted() {
        anyhow::bail!("interrupted by user");
    }
    let messages = vec![crate::state::ConversationMessage::user(format!(
        "Create a YAML UltraPlan for profile {} and goal: {goal}",
        config.profile
    ))];
    let model = model_for(config, true);
    let reply = {
        let _guard = ui.before_model_call(&format!("planner {} {model}", client.label()));
        client.chat(model, &messages, &[], false)?
    };
    ui.publish_status(UiStatus::for_model_reply(
        config,
        model,
        client.label(),
        reply.prompt_tokens,
        reply.completion_tokens,
    ));
    match parse_ultra_plan(&reply.content) {
        Ok(plan) => {
            let report = lint_ultra_plan_report(&plan);
            if report.is_pass() {
                Ok(plan)
            } else {
                emit_planner_error_for_lint(config, client.label(), model, &report, 1);
                emit_planner_schema_repaired(
                    config,
                    client.label(),
                    model,
                    "scaffold",
                    &report.primary_message(),
                    1,
                );
                Ok(UltraPlan::deterministic(
                    goal,
                    &config.profile,
                    &config.style,
                    detect_intent(goal),
                ))
            }
        }
        Err(err) => {
            emit_planner_error(
                config,
                client.label(),
                model,
                "schema",
                "planner_schema_error",
                &err.to_string(),
                1,
            );
            emit_planner_schema_repaired(
                config,
                client.label(),
                model,
                "schema",
                &err.to_string(),
                1,
            );
            Ok(UltraPlan::deterministic(
                goal,
                &config.profile,
                &config.style,
                detect_intent(goal),
            ))
        }
    }
}

pub fn save_ultra_plan(root: &Path, plan: &UltraPlan) -> anyhow::Result<PathBuf> {
    let dir = root.join(".anvil").join("plans");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("ultra-plan-{}.yaml", uuid::Uuid::now_v7()));
    std::fs::write(&path, render_ultra_plan(plan))?;
    Ok(path)
}

pub fn run_ultra_plan_file(
    planner: &mut dyn ChatClient,
    execution: &mut dyn ChatClient,
    path: &Path,
    config: &Config,
) -> anyhow::Result<String> {
    run_ultra_plan_file_with_ui(planner, execution, path, config, &NOOP_UI)
}

pub fn run_ultra_plan_file_with_ui(
    planner: &mut dyn ChatClient,
    execution: &mut dyn ChatClient,
    path: &Path,
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<String> {
    let path = resolve_plan_file_path(&config.workspace_root, path)?;
    let text = std::fs::read_to_string(path)?;
    let plan = parse_ultra_plan(&text)?;
    run_ultra_plan_with_ui(planner, execution, &plan, config, ui)
}

pub fn generate_and_run_ultra_plan(
    planner: &mut dyn ChatClient,
    execution: &mut dyn ChatClient,
    goal: &str,
    config: &Config,
) -> anyhow::Result<String> {
    generate_and_run_ultra_plan_with_ui(planner, execution, goal, config, &NOOP_UI)
}

pub fn generate_and_run_ultra_plan_with_ui(
    planner: &mut dyn ChatClient,
    execution: &mut dyn ChatClient,
    goal: &str,
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<String> {
    let plan = generate_ultra_plan_with_ui(planner, goal, config, ui)?;
    save_ultra_plan(&config.workspace_root, &plan)?;
    run_ultra_plan_with_ui(planner, execution, &plan, config, ui)
}

pub fn run_ultra_plan(
    planner: &mut dyn ChatClient,
    execution: &mut dyn ChatClient,
    plan: &UltraPlan,
    config: &Config,
) -> anyhow::Result<String> {
    run_ultra_plan_with_ui(planner, execution, plan, config, &NOOP_UI)
}

pub fn run_ultra_plan_with_ui(
    planner: &mut dyn ChatClient,
    execution: &mut dyn ChatClient,
    plan: &UltraPlan,
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<String> {
    let report = lint_ultra_plan_report(plan);
    if !report.is_pass() {
        emit_planner_error_for_lint(config, "ultra-plan-file", &config.planner_model, &report, 0);
        anyhow::bail!("{}", report.primary_message());
    }
    for (index, phase) in plan.phases.iter().enumerate() {
        if ui.interrupted() {
            anyhow::bail!("interrupted by user");
        }
        let profile_snapshot = profile_before_phase(&config.workspace_root, &plan.profile)?;
        let phase_prompt = ultra_phase_prompt(plan, &phase.prompt, config);
        let step_plan =
            generate_step_plan_with_ui(planner, &phase_prompt, config, ui).map_err(|err| {
                emit_planner_error(
                    config,
                    planner.label(),
                    &config.planner_model,
                    "scaffold",
                    "phase_scaffold_error",
                    &format!("phase scaffold failed: {}", err),
                    index + 1,
                );
                anyhow::anyhow!("phase scaffold failed: {}", err)
            })?;
        save_step_plan(&config.workspace_root, &step_plan)?;
        run_step_plan_with_ui(execution, &step_plan, config, ui)
            .map_err(|err| anyhow::anyhow!("phase {} failed: {err}", phase.id))?;
        let snapshot_report =
            profile_after_phase(&config.workspace_root, &plan.profile, &profile_snapshot);
        if !snapshot_report.is_pass() {
            return Err(anyhow::anyhow!(
                "phase {} profile snapshot verification failed: {}",
                phase.id,
                snapshot_report.primary_reason()
            ));
        }
        let profile_report = verify_profile(&config.workspace_root, &plan.profile, &plan.goal);
        let final_phase = index + 1 == plan.phases.len();
        if !profile_report.is_pass() {
            if final_phase
                && profile_auto_repair(
                    &config.workspace_root,
                    &plan.profile,
                    &plan.goal,
                    &profile_report,
                )?
            {
                let retry = verify_profile(&config.workspace_root, &plan.profile, &plan.goal);
                if retry.is_pass() {
                    continue;
                }
            }
            if final_phase
                && let Some(repair_prompt) = profile_repair_prompt(
                    &config.workspace_root,
                    &plan.profile,
                    &plan.goal,
                    &profile_report,
                )
            {
                let expected_paths =
                    profile_expected_paths(&config.workspace_root, &plan.profile, &plan.goal);
                let mut repair_session = SessionSnapshot::new();
                let repair_config = capped_config(config, STEP_REPAIR_MAX_ITERATIONS);
                run_session_with_outcome_with_ui(
                    execution,
                    &mut repair_session,
                    &repair_prompt,
                    &expected_paths,
                    &repair_config,
                    ui,
                )
                .map_err(|err| {
                    anyhow::anyhow!("phase {} profile repair failed: {err}", phase.id)
                })?;
                let retry = verify_profile(&config.workspace_root, &plan.profile, &plan.goal);
                if retry.is_pass() {
                    continue;
                }
                return Err(anyhow::anyhow!(
                    "phase {} profile verification failed after repair: {:?}",
                    phase.id,
                    retry.status
                ));
            }
            return Err(anyhow::anyhow!(
                "phase {} profile verification failed: {:?}",
                phase.id,
                profile_report.status
            ));
        }
    }
    Ok(format!(
        "ultra-plan-run complete: {} phases",
        plan.phases.len()
    ))
}

fn emit_planner_error_for_lint(
    config: &Config,
    provider: &str,
    model: &str,
    report: &PlanLintReport,
    attempt: usize,
) {
    let (stage, kind) = planner_stage_and_kind_for_lint(report);
    emit_planner_error(
        config,
        provider,
        model,
        stage,
        kind,
        &report.primary_message(),
        attempt,
    );
}

fn emit_planner_error(
    config: &Config,
    provider: &str,
    model: &str,
    stage: &str,
    kind: &str,
    message: &str,
    attempt: usize,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "planner_error",
            "planner_stage": stage,
            "planner_error_kind": kind,
            "planner_error_message": eval_events::body_snippet(message),
            "planner_provider": provider,
            "planner_model": model,
            "repair_attempt": attempt,
        }),
    );
}

fn emit_planner_schema_repaired(
    config: &Config,
    provider: &str,
    model: &str,
    stage: &str,
    message: &str,
    attempt: usize,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "planner_schema_repaired",
            "planner_stage": stage,
            "planner_error_kind": "planner_schema_repaired",
            "planner_error_message": eval_events::body_snippet(message),
            "planner_provider": provider,
            "planner_model": model,
            "repair_attempt": attempt,
        }),
    );
}

fn planner_stage_and_kind_for_lint(report: &PlanLintReport) -> (&'static str, &'static str) {
    if report.has_category("verify_policy") {
        ("verify_policy", "verify_command_policy_error")
    } else if report.has_category("scaffold") {
        ("scaffold", "phase_scaffold_error")
    } else {
        ("lint", "planner_lint_error")
    }
}

fn build_schema_retry_prompt(goal: &str, error: &str, attempt: usize) -> String {
    format!(
        "Your previous StepPlan output failed schema validation on attempt {attempt}/3: {error}.\n\
Return only YAML. Include top-level `goal` and non-empty `steps` exactly like this:\n\
goal: \"{goal}\"\nsteps:\n  - id: \"step-1\"\n    kind: \"implement\"\n    expected_result: \"pass\"\n    instruction: \"Create the required files for the goal.\"\n    expected_paths:\n      - \"relative/path\"\n    verify:\n      - \"command\"\n\nGoal: {goal}"
    )
}

fn build_lint_retry_prompt(goal: &str, report: &PlanLintReport, attempt: usize) -> String {
    let guidance = if report.has_category("verify_policy") {
        "Do not use shell control syntax such as &&, ||, |, or ; in verify commands. Split each verify command into a separate YAML list item."
    } else if report.has_category("path_ownership") {
        "Do not assign the same expected path to multiple steps. Each output path must have exactly one owning implement step."
    } else if report.has_category("dependency_order") {
        "Put package manifest and dependency setup before build/test verify steps. Do not verify before the files needed by that command exist."
    } else {
        "Implement steps must declare concrete workspace-relative expected_paths. Inspect/report steps must not declare expected paths or verify commands."
    };
    let errors = report
        .errors
        .iter()
        .map(|err| format!("- [{}] {}", err.category, err.message))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Your previous StepPlan failed deterministic lint on attempt {attempt}/3.\n\
Fix these issues without weakening safety rules:\n{errors}\n\n\
{guidance}\n\n\
Return only YAML with goal and steps. Goal: {goal}"
    )
}

fn strengthen_step_plan_for_profile(plan: &mut StepPlan, config: &Config) {
    let Some(last) = plan.steps.last_mut() else {
        return;
    };
    if plan.goal.to_ascii_lowercase().contains("scaffold") {
        for path in profile_expected_paths(&config.workspace_root, &config.profile, &plan.goal) {
            if path.ends_with("package.json") && !last.expected_paths.contains(&path) {
                last.expected_paths.push(path);
            }
        }
        if !last.expected_paths.is_empty() && last.kind == "report" {
            last.kind = "implement".to_string();
        }
    }
    if let Some(guidance) = profile_guidance(&config.profile, &plan.goal) {
        last.instruction = format!("{}\n\nProfile contract:\n{}", last.instruction, guidance);
    }
}

fn prompt_with_required_paths(instruction: &str, paths: &[String]) -> String {
    if paths.is_empty() || instruction.contains("Required final artifacts:") {
        return instruction.to_string();
    }
    format!(
        "{}\n\nRequired final artifacts:\n{}",
        instruction,
        paths
            .iter()
            .map(|path| format!("- {path}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

fn ultra_phase_prompt(plan: &UltraPlan, phase_prompt: &str, config: &Config) -> String {
    let expected_paths = profile_expected_paths(&config.workspace_root, &plan.profile, &plan.goal);
    let required = if expected_paths.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nRequired final artifacts:\n{}",
            expected_paths
                .iter()
                .map(|path| format!("- {path}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    format!(
        "Original ultra goal: {}\nProfile: {}\nStyle: {}\nIntent: {}\nPhase task: {}{}",
        plan.goal, plan.profile, plan.style, plan.intent, phase_prompt, required
    )
}

#[allow(dead_code)]
fn _format_report(report: &VerificationReport) -> String {
    format!("{:?}", report.status)
}

fn resolve_plan_file_path(root: &Path, path: &Path) -> anyhow::Result<PathBuf> {
    let root = root.canonicalize()?;
    let canonical = if path.is_absolute() {
        path.canonicalize()?
    } else {
        let raw = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("plan path is not valid UTF-8"))?;
        crate::tools::path_guard::resolve_existing(&root, raw)?
    };
    if !canonical.starts_with(&root) {
        anyhow::bail!("plan path escapes workspace");
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{AssistantReply, ChatClient};
    use crate::state::ConversationMessage;
    use crate::tools::registry::ToolSpec;

    #[test]
    fn plan_artifact_saved() {
        let dir = tempfile::tempdir().unwrap();
        let plan = StepPlan::single("goal");
        let path = save_step_plan(dir.path(), &plan).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn run_plan_accepts_absolute_path_inside_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let plan = StepPlan::single("goal");
        let path = save_step_plan(dir.path(), &plan).unwrap();
        let mut fake = FakeClient::new(vec![AssistantReply::text("done")]);
        let result = run_plan_file(&mut fake, &path, &config(dir.path().to_path_buf())).unwrap();
        assert_eq!(result, "plan-run complete: 1 steps");
    }

    #[test]
    fn run_plan_path_confinement_rejects_absolute_escape() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let path = outside.path().join("plan.yaml");
        std::fs::write(&path, render_step_plan(&StepPlan::single("goal"))).unwrap();
        let mut fake = FakeClient::new(vec![AssistantReply::text("done")]);
        let err = run_plan_file(&mut fake, &path, &config(workspace.path().to_path_buf()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("escapes workspace"));
    }

    #[test]
    fn invalid_planner_output_gets_corrective_retry() {
        let dir = tempfile::tempdir().unwrap();
        let valid = render_step_plan(&StepPlan::single("goal"));
        let mut planner = FakeClient::new(vec![
            AssistantReply::text("not yaml"),
            AssistantReply::text(valid),
        ]);
        let plan =
            generate_step_plan(&mut planner, "goal", &config(dir.path().to_path_buf())).unwrap();
        assert_eq!(plan.goal, "goal");
        assert_eq!(plan.steps.len(), 1);
    }

    #[test]
    fn missing_goal_is_repaired_and_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let mut planner = FakeClient::new(vec![AssistantReply::text(
            r#"steps:
  - id: "s1"
    kind: "implement"
    expected_result: "pass"
    instruction: "Create file"
    expected_paths:
      - "out.txt"
"#,
        )]);
        let plan = generate_step_plan(&mut planner, "goal", &cfg).unwrap();
        assert_eq!(plan.goal, "goal");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("planner_schema_repaired"));
    }

    #[test]
    fn verify_policy_error_gets_corrective_retry() {
        let dir = tempfile::tempdir().unwrap();
        let invalid = r#"goal: "goal"
steps:
  - id: "s1"
    kind: "implement"
    expected_result: "pass"
    instruction: "Create app"
    expected_paths:
      - "package.json"
    verify:
      - "node check.js && node check2.js"
"#;
        let valid = r#"goal: "goal"
steps:
  - id: "s1"
    kind: "implement"
    expected_result: "pass"
    instruction: "Create app"
    expected_paths:
      - "package.json"
    verify:
      - "node check.js"
      - "node check2.js"
"#;
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(invalid),
            AssistantReply::text(valid),
        ]);
        let plan =
            generate_step_plan(&mut planner, "goal", &config(dir.path().to_path_buf())).unwrap();
        assert_eq!(
            plan.steps[0].verify,
            vec!["node check.js".to_string(), "node check2.js".to_string()]
        );
    }

    #[test]
    fn required_final_artifacts_are_preserved_in_step_prompt() {
        let prompt = prompt_with_required_paths(
            "Create the app",
            &["package.json".to_string(), "src/app/page.tsx".to_string()],
        );
        assert!(prompt.contains("Required final artifacts:"));
        assert!(prompt.contains("- package.json"));
        assert!(prompt.contains("- src/app/page.tsx"));
    }

    #[test]
    fn required_final_artifacts_are_preserved_in_ultra_phase_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let plan = UltraPlan {
            goal: "3011 port app".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "scaffold".to_string(),
                    prompt: "Scaffold project".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "finish".to_string(),
                    prompt: "Finish project".to_string(),
                },
            ],
        };
        let prompt = ultra_phase_prompt(&plan, "Finish project", &config(dir.path().to_path_buf()));
        assert!(prompt.contains("Original ultra goal: 3011 port app"));
        assert!(prompt.contains("Profile: nextjs"));
        assert!(prompt.contains("Required final artifacts:"));
        assert!(prompt.contains("- package.json"));
        assert!(prompt.contains("- src/app/page.tsx"));
    }

    #[test]
    fn ultra_plan_final_profile_failure_runs_repair() {
        let dir = tempfile::tempdir().unwrap();
        let step_yaml = render_step_plan(&StepPlan::single("Scaffold project"));
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(step_yaml.clone()),
            AssistantReply::text(step_yaml),
        ]);
        let good_package = r#"{"dependencies":{"next":"x","react":"x","react-dom":"x"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#;
        let bad_package =
            r#"{"dependencies":{},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#;
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"package.json","content":good_package}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/page.tsx","content":"export default function Page(){return <main>Space Invaders</main>;}"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/layout.tsx","content":"export default function Layout({children}:{children:React.ReactNode}){return <html><body>{children}</body></html>;}"}),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("done"),
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"package.json","content":bad_package}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("done"),
        ]);
        let plan = UltraPlan {
            goal: "3011 port app".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "scaffold".to_string(),
                    prompt: "Scaffold project".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "verify".to_string(),
                    prompt: "Scaffold project".to_string(),
                },
            ],
        };
        let result = run_ultra_plan(
            &mut planner,
            &mut execution,
            &plan,
            &config(dir.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        assert!(dir.path().join("src/app/page.tsx").is_file());
    }

    #[test]
    fn ultra_plan_non_final_profile_failure_stops() {
        let dir = tempfile::tempdir().unwrap();
        let step_yaml = render_step_plan(&StepPlan::single("Scaffold project"));
        let mut planner = FakeClient::new(vec![AssistantReply::text(step_yaml)]);
        let package = r#"{"dependencies":{"next":"x","react":"x","react-dom":"x"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#;
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"package.json","content":package}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("done"),
        ]);
        let plan = UltraPlan {
            goal: "3011 port app".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "scaffold".to_string(),
                    prompt: "Scaffold project".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "finish".to_string(),
                    prompt: "Finish project".to_string(),
                },
            ],
        };
        let err = run_ultra_plan(
            &mut planner,
            &mut execution,
            &plan,
            &config(dir.path().to_path_buf()),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("phase scaffold profile verification failed"));
    }

    #[test]
    fn ultra_phase_profile_snapshot_runs_before_and_after_phase() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("input")).unwrap();
        std::fs::write(dir.path().join("input/source.csv"), "1234").unwrap();
        let step_yaml = render_step_plan(&StepPlan::single("mutate data"));
        let mut planner = FakeClient::new(vec![AssistantReply::text(step_yaml)]);
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Bash",
                    serde_json::json!({"command":"printf '5678' > input/source.csv"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("done"),
        ]);
        let plan = UltraPlan {
            goal: "analyze data".to_string(),
            profile: "data".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "phase-1".to_string(),
                    prompt: "Mutate data".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "phase-2".to_string(),
                    prompt: "Report".to_string(),
                },
            ],
        };
        let err = run_ultra_plan(
            &mut planner,
            &mut execution,
            &plan,
            &config(dir.path().to_path_buf()),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("profile snapshot verification failed"));
        assert!(err.contains("content changed"));
    }

    #[test]
    fn step_loop_uses_step_iteration_cap() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.max_iterations = 20;
        let plan = StepPlan {
            goal: "goal".to_string(),
            steps: vec![PlanStep {
                id: "s1".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create missing file".to_string(),
                expected_paths: vec!["missing.txt".to_string()],
                verify: Vec::new(),
            }],
        };
        let replies = (0..10)
            .map(|_| AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Bash",
                    serde_json::json!({"command":"true"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            })
            .collect();
        let mut fake = FakeClient::new(replies);
        let err = run_step_plan(&mut fake, &plan, &cfg)
            .unwrap_err()
            .to_string();
        assert!(err.contains("max_iterations (8)"));
    }

    #[test]
    fn repair_loop_uses_repair_iteration_cap() {
        let mut cfg = config(PathBuf::from("/tmp/work"));
        cfg.max_iterations = 20;
        assert_eq!(
            capped_config(&cfg, STEP_REPAIR_MAX_ITERATIONS).max_iterations,
            6
        );
    }

    struct FakeClient {
        replies: Vec<AssistantReply>,
    }

    impl FakeClient {
        fn new(replies: Vec<AssistantReply>) -> Self {
            Self { replies }
        }
    }

    impl ChatClient for FakeClient {
        fn label(&self) -> &str {
            "fake"
        }

        fn chat(
            &mut self,
            _model: &str,
            _messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _native_tools_enabled: bool,
        ) -> anyhow::Result<AssistantReply> {
            if self.replies.is_empty() {
                anyhow::bail!("fake client exhausted")
            }
            Ok(self.replies.remove(0))
        }
    }

    fn config(root: PathBuf) -> Config {
        Config {
            workspace_root: root,
            state_dir: PathBuf::from("state"),
            eval_events_path: None,
            completion_contract_path: None,
            yes: true,
            offline: false,
            context_budget: 1000,
            model: "m".to_string(),
            provider: crate::config::Provider::Ollama,
            planner_model: "m".to_string(),
            planner_provider: crate::config::Provider::Ollama,
            ollama_host: "http://localhost:11434".to_string(),
            num_predict: 100,
            max_iterations: 4,
            chat_timeout_secs: 1,
            chat_retries: 1,
            resume: None,
            fresh_session: false,
            no_footer: false,
            profile: "generic".to_string(),
            style: "default".to_string(),
            action: crate::config::Action::Repl,
        }
    }
}
