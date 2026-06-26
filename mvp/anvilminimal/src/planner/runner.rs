use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::eval_events;
use crate::minimal_loop::loop_run::run_session_with_outcome_with_ui;
use crate::planner::intent::detect_intent;
use crate::planner::lint::{
    PlanLintReport, PlanQualityContext, PlanQualityReport, lint_step_plan_report,
    lint_ultra_plan_report, step_plan_quality_report, step_plan_quality_warnings,
};
use crate::planner::profile::{
    profile_after_phase, profile_auto_repair, profile_before_phase, profile_expected_paths,
    profile_guidance, profile_quality_expectations, profile_repair_prompt, verify_profile,
};
use crate::planner::repair::{
    RepairContext, build_repair_prompt_with_context, save_repair_report_with_context,
};
use crate::planner::step_plan::{
    PlanStep, StepKind, StepPlan, extract_json_object, parse_generated_step_plan_json,
    parse_step_plan, render_step_plan, repair_generated_step_plan_contract,
};
use crate::planner::ultra_plan::{UltraPhase, UltraPlan, parse_ultra_plan, render_ultra_plan};
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
    let mut prompt = build_step_plan_user_prompt(goal, config);
    if let Some(guidance) = profile_guidance(&config.profile, goal) {
        prompt.push_str("\n\nProfile contract:\n");
        prompt.push_str(&guidance);
        prompt.push_str(
            "\nInclude expected_paths on the final step so deterministic verification can catch missing artifacts.",
        );
    }
    let model = model_for(config, true);
    let mut last_error = None;
    let mut last_valid_plan: Option<StepPlan> = None;
    let mut lint_categories_seen = BTreeSet::new();
    for attempt in 1..=3 {
        let messages = step_plan_messages(&prompt);
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
        emit_planner_raw_output_shape(config, client.label(), model, attempt, &reply.content);
        match parse_generated_step_plan_json(&reply.content, goal) {
            Ok(mut plan) => {
                repair_generated_step_plan_contract(&mut plan);
                strengthen_step_plan_for_profile(&mut plan, config);
                let lint_report = lint_step_plan_report(&plan);
                if lint_report.is_pass() {
                    let quality_context = plan_quality_context(config, goal);
                    let quality_report = step_plan_quality_report(&plan, &quality_context);
                    emit_planner_quality_warnings(config, client.label(), model, attempt, &plan);
                    emit_planner_quality_issues(
                        config,
                        client.label(),
                        model,
                        attempt,
                        &quality_report,
                    );
                    if quality_report.has_retryable_quality() {
                        last_valid_plan = Some(plan.clone());
                        if attempt < 3 {
                            emit_planner_quality_retry(
                                config,
                                client.label(),
                                model,
                                attempt,
                                &quality_report,
                            );
                            prompt = build_quality_retry_prompt(goal, &quality_report, attempt);
                            continue;
                        }
                        emit_planner_quality_retry_exhausted(
                            config,
                            client.label(),
                            model,
                            attempt,
                            &quality_report,
                        );
                    }
                    return Ok(plan);
                }
                if let Some(plan) = last_valid_plan.clone() {
                    emit_planner_quality_retry_degraded(
                        config,
                        client.label(),
                        model,
                        attempt,
                        "lint",
                        &lint_report.primary_message(),
                    );
                    if attempt >= 3 {
                        return Ok(plan);
                    }
                    let message = lint_report.primary_message();
                    last_error = Some(message.clone());
                    for err in &lint_report.errors {
                        lint_categories_seen.insert(err.category.clone());
                    }
                    prompt =
                        build_lint_retry_prompt(goal, &lint_report, attempt, &lint_categories_seen);
                    continue;
                }
                let message = lint_report.primary_message();
                emit_planner_error_for_lint(config, client.label(), model, &lint_report, attempt);
                last_error = Some(message.clone());
                for err in &lint_report.errors {
                    lint_categories_seen.insert(err.category.clone());
                }
                prompt =
                    build_lint_retry_prompt(goal, &lint_report, attempt, &lint_categories_seen);
            }
            Err(err) => {
                if let Some(plan) = last_valid_plan.clone() {
                    emit_planner_quality_retry_degraded(
                        config,
                        client.label(),
                        model,
                        attempt,
                        "schema",
                        &err.to_string(),
                    );
                    if attempt >= 3 {
                        return Ok(plan);
                    }
                    last_error = Some(err.to_string());
                    prompt = build_schema_retry_prompt(goal, &err.to_string(), attempt);
                    continue;
                }
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
    if let Some(plan) = last_valid_plan {
        return Ok(plan);
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
    let required_final_artifacts = required_final_artifacts(plan);
    let mut prior_expected_paths = Vec::new();
    for step in &plan.steps {
        if ui.interrupted() {
            anyhow::bail!("interrupted by user");
        }
        let prompt_context = StepPromptContext {
            required_final_artifacts: required_final_artifacts.clone(),
            prior_expected_paths: prior_expected_paths.clone(),
        };
        run_step(
            client,
            &mut session,
            plan,
            step,
            &prompt_context,
            config,
            ui,
        )?;
        merge_unique_strings(&mut prior_expected_paths, &step.expected_paths);
    }
    Ok(format!("plan-run complete: {} steps", plan.steps.len()))
}

#[derive(Debug, Clone, Default)]
struct StepPromptContext {
    required_final_artifacts: Vec<String>,
    prior_expected_paths: Vec<String>,
}

fn run_step(
    client: &mut dyn ChatClient,
    session: &mut SessionSnapshot,
    plan: &StepPlan,
    step: &PlanStep,
    prompt_context: &StepPromptContext,
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<()> {
    let instruction = build_step_prompt(plan, step, prompt_context);
    emit_step_prompt_contract(config, step, prompt_context, &instruction);
    if step.step_kind() == StepKind::Report
        && step.expected_paths.is_empty()
        && step.verify.is_empty()
    {
        return Ok(());
    }
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
        overall_goal: Some(plan.goal.clone()),
        required_final_artifacts: prompt_context.required_final_artifacts.clone(),
        step_instruction: Some(step.instruction.clone()),
        expected_paths: step.expected_paths.clone(),
        verify_commands: step.verify.clone(),
        expected_result: Some(step_expected_result(step).to_string()),
        max_repair_turns: Some(STEP_REPAIR_MAX_TURNS),
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
    for attempt in 1..=STEP_REPAIR_MAX_TURNS {
        context.repair_attempt = Some(attempt);
        let repair_prompt = build_repair_prompt_with_context(&step.id, &current_report, &context);
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

fn required_final_artifacts(plan: &StepPlan) -> Vec<String> {
    let mut out = Vec::new();
    for step in &plan.steps {
        merge_unique_strings(&mut out, &step.expected_paths);
    }
    out
}

fn merge_unique_strings(out: &mut Vec<String>, incoming: &[String]) {
    for item in incoming {
        if !out.contains(item) {
            out.push(item.clone());
        }
    }
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
        let phase_prompt = ultra_phase_prompt(plan, phase, config);
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

fn emit_planner_raw_output_shape(
    config: &Config,
    provider: &str,
    model: &str,
    attempt: usize,
    content: &str,
) {
    let json_extract_status = match extract_json_object(content) {
        Ok(_) => "ok",
        Err(_) => "missing",
    };
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "planner_raw_output_shape",
            "planner_provider": provider,
            "planner_model": model,
            "attempt": attempt,
            "content_len": content.chars().count(),
            "has_json_object": json_extract_status == "ok",
            "has_yaml_fence": content.contains("```yaml") || content.contains("```yml"),
            "contains_goal_key": content.contains("\"goal\"") || content.contains("goal:"),
            "contains_steps_key": content.contains("\"steps\"") || content.contains("steps:"),
            "json_extract_status": json_extract_status,
            "preview": eval_events::body_snippet(content),
        }),
    );
}

fn emit_planner_quality_warnings(
    config: &Config,
    provider: &str,
    model: &str,
    attempt: usize,
    plan: &StepPlan,
) {
    for message in step_plan_quality_warnings(plan) {
        eval_events::emit(
            config.eval_events_path.as_deref(),
            json!({
                "event": "planner_quality_warning",
                "planner_stage": "quality",
                "planner_error_kind": "planner_quality_warning",
                "planner_error_message": eval_events::body_snippet(&message),
                "planner_provider": provider,
                "planner_model": model,
                "repair_attempt": attempt,
            }),
        );
    }
}

fn emit_planner_quality_issues(
    config: &Config,
    provider: &str,
    model: &str,
    attempt: usize,
    report: &PlanQualityReport,
) {
    for issue in &report.issues {
        eval_events::emit(
            config.eval_events_path.as_deref(),
            json!({
                "event": "planner_quality_issue",
                "planner_stage": "quality",
                "planner_error_kind": "planner_quality_issue",
                "planner_quality_category": issue.category,
                "planner_quality_severity": issue.severity.as_str(),
                "planner_error_message": eval_events::body_snippet(&issue.message),
                "planner_quality_step_id": issue.step_id,
                "planner_quality_evidence": issue.evidence,
                "planner_provider": provider,
                "planner_model": model,
                "repair_attempt": attempt,
            }),
        );
    }
}

fn emit_planner_quality_retry(
    config: &Config,
    provider: &str,
    model: &str,
    attempt: usize,
    report: &PlanQualityReport,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "planner_quality_retry",
            "planner_stage": "quality",
            "planner_error_kind": "planner_quality_retry",
            "planner_error_message": eval_events::body_snippet(&report.primary_message()),
            "planner_provider": provider,
            "planner_model": model,
            "repair_attempt": attempt,
            "planner_quality_issue_count": report.issues.len(),
        }),
    );
}

fn emit_planner_quality_retry_degraded(
    config: &Config,
    provider: &str,
    model: &str,
    attempt: usize,
    stage: &str,
    message: &str,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "planner_quality_retry_degraded",
            "planner_stage": stage,
            "planner_error_kind": "planner_quality_retry_degraded",
            "planner_error_message": eval_events::body_snippet(message),
            "planner_provider": provider,
            "planner_model": model,
            "repair_attempt": attempt,
        }),
    );
}

fn emit_planner_quality_retry_exhausted(
    config: &Config,
    provider: &str,
    model: &str,
    attempt: usize,
    report: &PlanQualityReport,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "planner_quality_retry_exhausted",
            "planner_stage": "quality",
            "planner_error_kind": "planner_quality_retry_exhausted",
            "planner_error_message": eval_events::body_snippet(&report.primary_message()),
            "planner_provider": provider,
            "planner_model": model,
            "repair_attempt": attempt,
            "planner_quality_issue_count": report.issues.len(),
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
Return only one JSON object and no markdown fences.\n\
Required JSON shape:\n\
{{\n  \"goal\": \"{goal}\",\n  \"steps\": [\n    {{\n      \"id\": \"kebab-id\",\n      \"kind\": \"implement\",\n      \"expected_result\": \"pass\",\n      \"instruction\": \"Create the required files for the goal.\",\n      \"expected_paths\": [\"relative/path\"],\n      \"verify\": [\"command\"]\n    }}\n  ]\n}}\n\n\
Rules:\n- Include top-level goal and non-empty steps.\n- Step id must be a quoted string, not a number.\n- expected_result must be exactly \"pass\" or \"fail\", not prose.\n- Keep expected_paths workspace-relative.\n- Use deterministic verify commands only.\n\nGoal: {goal}"
    )
}

fn build_lint_retry_prompt(
    goal: &str,
    report: &PlanLintReport,
    attempt: usize,
    categories_seen: &BTreeSet<String>,
) -> String {
    let guidance = lint_retry_hard_constraints(report, categories_seen).join("\n");
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
Return only one JSON object with top-level goal and steps. Do not use markdown fences.\n\
Step id must be a quoted string. expected_result must be exactly \"pass\" or \"fail\".\n\
Goal: {goal}"
    )
}

fn build_quality_retry_prompt(goal: &str, report: &PlanQualityReport, attempt: usize) -> String {
    let issues = report
        .issues
        .iter()
        .filter(|issue| issue.severity.as_str() == "retryable_quality")
        .map(|issue| {
            let step = issue
                .step_id
                .as_ref()
                .map(|value| format!(" step={value}"))
                .unwrap_or_default();
            let evidence = issue
                .evidence
                .as_ref()
                .map(|value| format!(" evidence={value}"))
                .unwrap_or_default();
            format!(
                "- [{}{}] {}{}",
                issue.category, step, issue.message, evidence
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Your previous StepPlan was schema-valid and passed safety lint, but it had retryable quality issues on attempt {attempt}/3.\n\
Improve the plan without weakening safety rules:\n{issues}\n\n\
Hard constraints:\n\
- Keep the original top-level goal unchanged.\n\
- Keep expected_paths workspace-relative and owned by one setup/implement step.\n\
- Keep verify commands deterministic, single commands, and free of shell control syntax.\n\
- Do not start dev servers or perform dependency installation in verify.\n\
- Prefer tests, builds, smoke checks, or content assertions over file existence checks.\n\
- For Next.js, put package.json and app entrypoints before npm run build.\n\n\
- Do not add a final report step for normal success; report is only for explicit blockers.\n\
- In a fresh create/scaffold workspace, start with setup/implement work that owns an artifact instead of an empty inspect wrapper.\n\
- Keep verify-only steps connected to prior artifacts through the verify command or step instruction.\n\n\
Return only one JSON object with top-level goal and steps. Do not use markdown fences.\n\
Goal: {goal}"
    )
}

fn lint_retry_hard_constraints(
    report: &PlanLintReport,
    categories_seen: &BTreeSet<String>,
) -> Vec<&'static str> {
    let mut categories = categories_seen.clone();
    for err in &report.errors {
        categories.insert(err.category.clone());
    }
    let mut out = vec![
        "Hard constraints to preserve in the corrected plan:",
        "- Keep the original top-level goal unchanged.",
        "- Use setup, implement, verify, and report responsibilities separately.",
        "- Implement/setup steps own expected_paths; verify-only steps should normally have empty expected_paths.",
        "- Each implementation-owned expected path must have exactly one owner step.",
    ];
    if categories.contains("verify_policy") {
        out.push("- Verify commands must be single commands without &&, ||, |, ;, backticks, or command substitution.");
        out.push(
            "- Split multiple checks into multiple verify steps or multiple verify list items.",
        );
        out.push("- If a Node smoke check needs multiple statements, create a smoke-check.js artifact in an implement step and verify with a single command such as node smoke-check.js.");
        out.push("- If documentation needs multiple assertions, prefer separate grep -q commands in the verify list, each checking one phrase in one file.");
    }
    if categories.contains("path_ownership") {
        out.push("- Do not duplicate expected_paths across steps; move shared validation into verify commands instead.");
    }
    if categories.contains("dependency_order") {
        out.push("- Put package manifest and dependency setup before npm/pnpm/yarn build or test verification.");
        out.push("- Python stdlib unittest does not require dependency setup by itself.");
    }
    if categories.contains("contract") {
        out.push("- Implement steps must declare concrete workspace-relative expected_paths.");
        out.push("- Inspect and report steps must not declare expected_paths or verify commands.");
        out.push("- Verify steps must not request file changes in their instruction.");
    }
    out
}

fn step_plan_messages(prompt: &str) -> Vec<crate::state::ConversationMessage> {
    vec![
        crate::state::ConversationMessage::system(plan_generation_system_prompt()),
        crate::state::ConversationMessage::user(prompt.to_string()),
    ]
}

fn plan_generation_system_prompt() -> String {
    [
        "You are a deterministic planning component for a local coding agent.",
        "Return only one JSON object. Do not include markdown fences, prose, comments, or XML.",
        "The JSON object must have top-level keys: goal, steps.",
        "Each step must include id, kind, instruction, expected_paths, verify, and expected_result.",
        "Step id must be a quoted string. Use kebab-case such as inspect-workspace.",
        "expected_result must be exactly \"pass\" or \"fail\". Do not put prose in expected_result.",
        "Use 2 to 8 steps for normal implementation tasks, with a maximum of 12 steps.",
        "Allowed step kinds: inspect, setup, implement, verify, report.",
        "Use inspect only for reading or assessing state; it must not declare expected_paths or verify commands.",
        "Use setup for dependency manifests or setup files; do not run build/test verification in setup.",
        "Use implement/create/edit/work/repair semantics as implement steps, with concrete expected_paths.",
        "Use verify only for deterministic checks. Do not request file changes in verify steps.",
        "Use report only for explicit blockers such as dependency_missing, unavailable external service, required user input, or unfixable local blocker. Report is not success.",
        "Normal success plans must end with a verify step or an artifact-owning setup/implement step, not a final summary report.",
        "Expected paths must be workspace-relative, exact, and owned by one implement/setup step.",
        "Prefer tests, builds, smoke checks, or content assertions over file existence checks.",
        "Use file existence checks only as a fallback when no stronger deterministic verification fits.",
        "Implement/setup instructions should say what each expected artifact will contain.",
        "Verify commands must not use shell control syntax such as &&, ||, |, or ;.",
        "For multi-statement Node checks, create a smoke-check.js artifact and verify with node smoke-check.js.",
        "Do not put dev server startup or network setup in verify.",
        "For Next.js, create package.json and app entrypoints before npm run build.",
        "Do not use node --check for .ts or .tsx files.",
    ]
    .join("\n")
}

fn build_step_plan_user_prompt(goal: &str, config: &Config) -> String {
    let mut prompt = format!("Create a step plan for this task:\n{goal}");
    let expected_paths = profile_expected_paths(&config.workspace_root, &config.profile, goal);
    if !expected_paths.is_empty() {
        prompt.push_str("\n\nRequired final artifacts:\n");
        for path in expected_paths {
            prompt.push_str("- ");
            prompt.push_str(&path);
            prompt.push('\n');
        }
    }
    let expectations = profile_quality_expectations(&config.workspace_root, &config.profile, goal);
    if !expectations.preferred_verify.is_empty()
        || !expectations
            .dependency_order_hint
            .as_deref()
            .unwrap_or("")
            .is_empty()
    {
        prompt.push_str("\nProfile verification expectations:\n");
        if !expectations.preferred_verify.is_empty() {
            prompt.push_str("- Preferred deterministic verify commands:\n");
            for command in expectations.preferred_verify {
                prompt.push_str("  - ");
                prompt.push_str(&command);
                prompt.push('\n');
            }
        }
        if let Some(hint) = expectations.dependency_order_hint {
            prompt.push_str("- Order: ");
            prompt.push_str(&hint);
            prompt.push('\n');
        }
        if !expectations.forbidden_verify.is_empty() {
            prompt.push_str("- Do not use these in verify:\n");
            for command in expectations.forbidden_verify {
                prompt.push_str("  - ");
                prompt.push_str(&command);
                prompt.push('\n');
            }
        }
    }
    prompt
}

fn plan_quality_context(config: &Config, goal: &str) -> PlanQualityContext {
    let expectations = profile_quality_expectations(&config.workspace_root, &config.profile, goal);
    let workspace = workspace_quality_snapshot(&config.workspace_root);
    PlanQualityContext {
        profile: config.profile.clone(),
        required_artifacts: expectations.required_artifacts,
        preferred_verify: expectations.preferred_verify,
        dependency_order_hint: expectations.dependency_order_hint,
        task_intent: detect_intent(goal).to_string(),
        workspace_context_known: workspace.context_known,
        workspace_snapshot_class: workspace.snapshot_class,
        has_user_seed_files: workspace.has_user_seed_files,
        has_only_agent_metadata: workspace.has_only_agent_metadata,
    }
}

#[derive(Debug, Clone)]
struct WorkspaceQualitySnapshot {
    context_known: bool,
    snapshot_class: String,
    has_user_seed_files: bool,
    has_only_agent_metadata: bool,
}

fn workspace_quality_snapshot(root: &Path) -> WorkspaceQualitySnapshot {
    let Ok(entries) = std::fs::read_dir(root) else {
        return WorkspaceQualitySnapshot {
            context_known: false,
            snapshot_class: "unknown".to_string(),
            has_user_seed_files: false,
            has_only_agent_metadata: false,
        };
    };
    let mut user_entries = 0usize;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if is_agent_metadata_entry(&name) {
            continue;
        }
        user_entries += 1;
    }
    let has_only_agent_metadata = user_entries == 0;
    WorkspaceQualitySnapshot {
        context_known: true,
        snapshot_class: if has_only_agent_metadata {
            "metadata_only".to_string()
        } else {
            "user_files".to_string()
        },
        has_user_seed_files: user_entries > 0,
        has_only_agent_metadata,
    }
}

fn is_agent_metadata_entry(name: &str) -> bool {
    matches!(
        name,
        ".git" | ".anvil" | ".codex" | ".agents" | "target" | ".DS_Store"
    ) || name.starts_with("anvilminimal-eval-")
}

fn strengthen_step_plan_for_profile(plan: &mut StepPlan, config: &Config) {
    let is_scaffold = plan.goal.to_ascii_lowercase().contains("scaffold");
    let Some(target_index) = plan
        .steps
        .iter()
        .rposition(|step| matches!(step.step_kind(), StepKind::Setup | StepKind::Implement))
        .or_else(|| {
            is_scaffold.then(|| {
                plan.steps
                    .iter()
                    .rposition(|step| step.step_kind() == StepKind::Report)
            })?
        })
    else {
        return;
    };
    let target = &mut plan.steps[target_index];
    if is_scaffold {
        for path in profile_expected_paths(&config.workspace_root, &config.profile, &plan.goal) {
            if path.ends_with("package.json") && !target.expected_paths.contains(&path) {
                target.expected_paths.push(path);
            }
        }
        if !target.expected_paths.is_empty() && target.kind == "report" {
            target.kind = "implement".to_string();
        }
    }
    if let Some(guidance) = profile_guidance(&config.profile, &plan.goal) {
        target.instruction = format!("{}\n\nProfile contract:\n{}", target.instruction, guidance);
    }
}

fn build_step_prompt(plan: &StepPlan, step: &PlanStep, context: &StepPromptContext) -> String {
    let mut prompt = String::new();
    prompt.push_str("Execute exactly one StepPlan step.\n\n");
    prompt.push_str("Overall goal:\n");
    prompt.push_str(&plan.goal);
    prompt.push_str("\n\nCurrent step id:\n");
    prompt.push_str(&step.id);
    prompt.push_str("\n\nCurrent step kind:\n");
    prompt.push_str(&step.kind);
    prompt.push_str("\n\nCurrent step instruction:\n");
    prompt.push_str(&step.instruction);

    prompt.push_str("\n\nRequired final artifacts:\n");
    append_bullets_or_none(&mut prompt, &context.required_final_artifacts);

    prompt.push_str("\n\nArtifacts available from previous steps:\n");
    append_bullets_or_none(&mut prompt, &context.prior_expected_paths);

    prompt.push_str("\n\nExpected paths after this step:\n");
    append_bullets_or_none(&mut prompt, &step.expected_paths);

    prompt.push_str("\n\nVerification commands for this step:\n");
    append_bullets_or_none(&mut prompt, &step.verify);

    prompt.push_str("\n\nExpected verification result:\n");
    prompt.push_str(step_expected_result(step));

    prompt.push_str(
        "\n\nStep execution rules:\n\
- Work only on the current step unless a prior artifact is required to verify this step.\n\
- Prefer Write/Edit/MultiEdit for declared expected paths.\n\
- If this step has verification commands, use them as the deterministic success contract.\n\
- If verification fails, make a bounded step-local repair and re-check the declared contract.\n\
- Do not claim the plan is complete until this step's expected paths and verification contract are satisfied.\n\
- Report an explicit blocker only when the blocker cannot be resolved locally.",
    );
    prompt
}

fn append_bullets_or_none(prompt: &mut String, items: &[String]) {
    if items.is_empty() {
        prompt.push_str("- none\n");
        return;
    }
    for item in items {
        prompt.push_str("- ");
        prompt.push_str(item);
        prompt.push('\n');
    }
}

fn step_expected_result(step: &PlanStep) -> &str {
    let trimmed = step.expected_result.trim();
    if trimmed.is_empty() { "pass" } else { trimmed }
}

fn emit_step_prompt_contract(
    config: &Config,
    step: &PlanStep,
    context: &StepPromptContext,
    prompt: &str,
) {
    let prior_context_applicable =
        step.step_kind() == StepKind::Verify && !context.prior_expected_paths.is_empty();
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "step_prompt_contract",
            "prompt_contract_version": 1,
            "step_id": step.id,
            "step_kind": step.kind,
            "has_overall_goal": prompt.contains("Overall goal:"),
            "has_required_final_artifacts": prompt.contains("Required final artifacts:"),
            "has_expected_paths": prompt.contains("Expected paths after this step:"),
            "has_verify_commands": prompt.contains("Verification commands for this step:"),
            "has_expected_result": prompt.contains("Expected verification result:"),
            "has_prior_artifact_context": !context.prior_expected_paths.is_empty()
                && prompt.contains("Artifacts available from previous steps:"),
            "prior_artifact_context_applicable": prior_context_applicable,
            "has_bounded_repair_policy": prompt.contains("bounded step-local repair"),
            "prompt_body_saved": false,
        }),
    );
}

#[allow(dead_code)]
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

fn ultra_phase_prompt(plan: &UltraPlan, phase: &UltraPhase, config: &Config) -> String {
    let expected_paths = profile_expected_paths(&config.workspace_root, &plan.profile, &plan.goal);
    let expectations =
        profile_quality_expectations(&config.workspace_root, &plan.profile, &plan.goal);
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
    let preferred_verify = if expectations.preferred_verify.is_empty() {
        "- none".to_string()
    } else {
        expectations
            .preferred_verify
            .iter()
            .map(|command| format!("- {command}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let workspace_snapshot = compact_workspace_snapshot(&config.workspace_root);
    format!(
        "Original ultra goal: {}\nProfile: {}\nStyle: {}\nIntent: {}\nPhase id: {}\nPhase task: {}\n\nWorkspace snapshot:\n{}\n\nProfile runtime contract:\n- keep work inside the workspace\n- satisfy required profile artifacts before final phase completion\n- use deterministic verification; preferred commands:\n{}\n{}",
        plan.goal,
        plan.profile,
        plan.style,
        plan.intent,
        phase.id,
        phase.prompt,
        workspace_snapshot,
        preferred_verify,
        required
    )
}

fn compact_workspace_snapshot(root: &Path) -> String {
    let Ok(entries) = std::fs::read_dir(root) else {
        return "- unavailable".to_string();
    };
    let mut names = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| !matches!(name.as_str(), ".git" | ".anvil" | "target" | ".DS_Store"))
        .take(12)
        .collect::<Vec<_>>();
    names.sort();
    if names.is_empty() {
        "- empty or metadata-only".to_string()
    } else {
        names
            .into_iter()
            .map(|name| format!("- {name}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
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
    fn run_plan_rejects_invalid_yaml_without_repair() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.yaml");
        std::fs::write(
            &path,
            r#"steps:
  - id: "s1"
    instruction: "do it"
"#,
        )
        .unwrap();
        let mut fake = FakeClient::new(vec![AssistantReply::text("done")]);
        let err = run_plan_file(&mut fake, &path, &config(dir.path().to_path_buf()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("StepPlan missing goal"));
    }

    #[test]
    fn source_generated_json_saves_yaml_readable_by_run_plan() {
        let dir = tempfile::tempdir().unwrap();
        let json = include_str!("../../eval/fixtures/plans/source-step-plan.json");
        let plan = parse_generated_step_plan_json(
            json,
            "Create a Next.js Space Invaders app on port 3011.",
        )
        .unwrap();
        let path = save_step_plan(dir.path(), &plan).unwrap();
        let parsed = parse_step_plan(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(parsed, plan);
        assert!(parsed.steps.iter().any(|step| step.kind == "implement"));
        assert!(parsed.steps.iter().any(|step| step.kind == "verify"));
    }

    #[test]
    fn run_plan_accepts_existing_and_generated_yaml() {
        let existing = include_str!("../../eval/fixtures/plans/existing-mvp-step-plan.yaml");
        let parsed_existing = parse_step_plan(existing).unwrap();
        assert_eq!(
            parsed_existing.goal,
            "Create a small markdown heading linter."
        );

        let generated = include_str!("../../eval/fixtures/plans/source-step-plan.expected.yaml");
        let parsed_generated = parse_step_plan(generated).unwrap();
        assert_eq!(
            parsed_generated.goal,
            "Create a Next.js Space Invaders app on port 3011."
        );
        assert_eq!(parsed_generated.steps.len(), 5);
    }

    #[test]
    fn invalid_planner_output_gets_corrective_retry() {
        let dir = tempfile::tempdir().unwrap();
        let valid = generated_step_plan_json("goal");
        let mut planner = FakeClient::new(vec![
            AssistantReply::text("not json"),
            AssistantReply::text(valid),
        ]);
        let plan =
            generate_step_plan(&mut planner, "goal", &config(dir.path().to_path_buf())).unwrap();
        assert_eq!(plan.goal, "goal");
        assert_eq!(plan.steps.len(), 1);
    }

    #[test]
    fn missing_goal_gets_corrective_retry_and_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(r#"{"steps":[{"id":"s1","instruction":"Create file"}]}"#),
            AssistantReply::text(generated_step_plan_json("goal")),
        ]);
        let plan = generate_step_plan(&mut planner, "goal", &cfg).unwrap();
        assert_eq!(plan.goal, "goal");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("planner_error"));
        assert!(event_text.contains("planner_raw_output_shape"));
    }

    #[test]
    fn verify_policy_error_gets_corrective_retry() {
        let dir = tempfile::tempdir().unwrap();
        let invalid = r#"{"goal":"goal","steps":[{"id":"s1","kind":"implement","expected_result":"pass","instruction":"Create app","expected_paths":["package.json"],"verify":["node check.js && node check2.js"]}]}"#;
        let valid = r#"{"goal":"goal","steps":[{"id":"s1","kind":"implement","expected_result":"pass","instruction":"Create app","expected_paths":["package.json"],"verify":["node check.js","node check2.js"]}]}"#;
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
    fn retry_prompt_accumulates_lint_categories() {
        let mut report = PlanLintReport::pass();
        report.push(
            "verify_policy",
            "verify command may not use shell control syntax",
        );
        let mut categories = BTreeSet::new();
        categories.insert("dependency_order".to_string());
        categories.insert("path_ownership".to_string());
        let prompt = build_lint_retry_prompt("goal", &report, 2, &categories);
        assert!(prompt.contains("without &&, ||, |, ;"));
        assert!(prompt.contains("smoke-check.js"));
        assert!(prompt.contains("grep -q"));
        assert!(prompt.contains("Do not duplicate expected_paths"));
        assert!(prompt.contains("Python stdlib unittest does not require dependency setup"));
        assert!(prompt.contains("Keep the original top-level goal unchanged"));
        for provider in ["OpenAI", "Gemini", "Ollama"] {
            assert!(!prompt.contains(provider), "{provider}: {prompt}");
        }
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
    fn step_execution_prompt_includes_source_contract() {
        let plan = StepPlan {
            goal: "Build a game".to_string(),
            steps: vec![PlanStep {
                id: "create-page".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create the page".to_string(),
                expected_paths: vec!["src/app/page.tsx".to_string()],
                verify: vec!["npm run build".to_string()],
            }],
        };
        let context = StepPromptContext {
            required_final_artifacts: vec!["src/app/page.tsx".to_string()],
            prior_expected_paths: vec!["package.json".to_string()],
        };
        let prompt = build_step_prompt(&plan, &plan.steps[0], &context);
        assert!(prompt.contains("Overall goal:"));
        assert!(prompt.contains("Build a game"));
        assert!(prompt.contains("Current step id:"));
        assert!(prompt.contains("create-page"));
        assert!(prompt.contains("Verification commands for this step:"));
        assert!(prompt.contains("npm run build"));
        assert!(prompt.contains("Expected verification result:"));
        assert!(prompt.contains("Artifacts available from previous steps:"));
        assert!(prompt.contains("bounded step-local repair"));
    }

    #[test]
    fn run_plan_passes_step_contract_to_execution_client() {
        let dir = tempfile::tempdir().unwrap();
        let plan = StepPlan {
            goal: "Create app.py".to_string(),
            steps: vec![PlanStep {
                id: "code".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create app.py".to_string(),
                expected_paths: vec!["app.py".to_string()],
                verify: Vec::new(),
            }],
        };
        let mut fake = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: vec![crate::state::ToolCall::new(
                "Write",
                serde_json::json!({"path":"app.py","content":"print('ok')"}),
            )],
            prompt_tokens: None,
            completion_tokens: None,
        }]);
        let result = run_step_plan(&mut fake, &plan, &config(dir.path().to_path_buf())).unwrap();
        assert_eq!(result, "plan-run complete: 1 steps");
        let messages = fake.messages.first().expect("execution prompt");
        let prompt = messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(prompt.contains("Overall goal:"));
        assert!(prompt.contains("Current step id:"));
        assert!(prompt.contains("Expected paths after this step:"));
        assert!(prompt.contains("Verification commands for this step:"));
    }

    #[test]
    fn planner_prompt_report_is_blocker_not_success() {
        let prompt = plan_generation_system_prompt();
        assert!(prompt.contains("Report is not success"));
        assert!(prompt.contains("explicit blockers"));
        assert!(!prompt.contains("Use report only for final summary"));
    }

    #[test]
    fn invalid_planner_json_does_not_save_plan_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut planner = FakeClient::new(vec![
            AssistantReply::text("not json"),
            AssistantReply::text("still not json"),
            AssistantReply::text("nope"),
        ]);
        let err = generate_step_plan(&mut planner, "goal", &config(dir.path().to_path_buf()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid StepPlan after corrective retries"));
        assert!(!dir.path().join(".anvil/plans").exists());
    }

    #[test]
    fn invalid_planner_lint_does_not_save_plan_file() {
        let dir = tempfile::tempdir().unwrap();
        let invalid = r#"{"goal":"goal","steps":[{"id":"s1","kind":"implement","instruction":"Create app","expected_paths":["package.json"],"verify":["node check.js && node check2.js"]}]}"#;
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(invalid),
            AssistantReply::text(invalid),
            AssistantReply::text(invalid),
        ]);
        let err = generate_step_plan(&mut planner, "goal", &config(dir.path().to_path_buf()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("verify command"));
        assert!(!dir.path().join(".anvil/plans").exists());
    }

    #[test]
    fn planner_prompt_provider_request_contract() {
        let dir = tempfile::tempdir().unwrap();
        let mut planner =
            FakeClient::new(vec![AssistantReply::text(generated_step_plan_json("goal"))]);
        let _ =
            generate_step_plan(&mut planner, "goal", &config(dir.path().to_path_buf())).unwrap();
        let messages = planner.messages.first().expect("messages");
        assert_eq!(messages[0].role, "system");
        assert!(messages[0].content.contains("Return only one JSON object"));
        assert_eq!(messages[1].role, "user");
        assert!(messages[1].content.contains("Create a step plan"));
    }

    #[test]
    fn planner_prompt_ollama_request_contract() {
        let messages = step_plan_messages(&build_step_plan_user_prompt(
            "goal",
            &config(PathBuf::from("/tmp/work")),
        ));
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "system");
        assert!(messages[0].content.contains("Allowed step kinds"));
        assert_eq!(messages[1].role, "user");
    }

    #[test]
    fn step_plan_quality_warning_does_not_change_exit_status() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let plan_json = r#"{"goal":"Build a Next.js game app","steps":[{"id":"s1","kind":"implement","instruction":"Create the app","expected_paths":["package.json","src/app/page.tsx"],"verify":[]}]}"#;
        let mut planner = FakeClient::new(vec![AssistantReply::text(plan_json)]);
        let plan = generate_step_plan(&mut planner, "Build a Next.js game app", &cfg).unwrap();
        assert_eq!(plan.steps.len(), 1);
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("planner_quality_warning"));
    }

    #[test]
    fn retryable_quality_issue_gets_corrective_retry() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let weak = r#"{"goal":"Build a Next.js game app","steps":[{"id":"make-app","kind":"implement","expected_result":"pass","instruction":"Create package.json and src/app/page.tsx for the game app","expected_paths":["package.json","src/app/page.tsx"],"verify":[]}]}"#;
        let strong = r#"{"goal":"Build a Next.js game app","steps":[{"id":"setup","kind":"setup","expected_result":"pass","instruction":"Create package.json with next, react, and react-dom dependencies","expected_paths":["package.json"],"verify":[]},{"id":"page","kind":"implement","expected_result":"pass","instruction":"Create src/app/page.tsx game page","expected_paths":["src/app/page.tsx"],"verify":[]},{"id":"build","kind":"verify","expected_result":"pass","instruction":"Run deterministic Next.js build","expected_paths":[],"verify":["npm run build"]}]}"#;
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(weak),
            AssistantReply::text(strong),
        ]);
        let plan = generate_step_plan(&mut planner, "Build a Next.js game app", &cfg).unwrap();
        assert_eq!(planner.messages.len(), 2);
        assert!(
            plan.steps
                .iter()
                .flat_map(|step| step.verify.iter())
                .any(|command| command == "npm run build")
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("planner_quality_issue"));
        assert!(event_text.contains("planner_quality_retry"));
    }

    #[test]
    fn quality_retry_degradation_keeps_last_valid_plan() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let weak = r#"{"goal":"Build a Next.js game app","steps":[{"id":"make-app","kind":"implement","expected_result":"pass","instruction":"Create package.json and src/app/page.tsx for the game app","expected_paths":["package.json","src/app/page.tsx"],"verify":[]}]}"#;
        let degraded = r#"{"goal":"Build a Next.js game app","steps":[{"id":"bad","kind":"implement","expected_result":"pass","instruction":"Create app","expected_paths":["package.json"],"verify":["node check.js && node check2.js"]}]}"#;
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(weak),
            AssistantReply::text(degraded),
            AssistantReply::text(degraded),
        ]);
        let plan = generate_step_plan(&mut planner, "Build a Next.js game app", &cfg).unwrap();
        assert_eq!(planner.messages.len(), 3);
        assert_eq!(plan.steps[0].id, "make-app");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("planner_quality_retry_degraded"));
    }

    #[test]
    fn advisory_quality_issue_does_not_retry() {
        let dir = tempfile::tempdir().unwrap();
        let plan_json = r#"{"goal":"Update README heading","steps":[{"id":"docs","kind":"implement","expected_result":"pass","instruction":"Update README.md","expected_paths":["README.md"],"verify":["test -f README.md"]}]}"#;
        let mut planner = FakeClient::new(vec![AssistantReply::text(plan_json)]);
        let plan = generate_step_plan(
            &mut planner,
            "Update README heading",
            &config(dir.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(planner.messages.len(), 1);
        assert_eq!(plan.steps[0].id, "docs");
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
        let prompt = ultra_phase_prompt(
            &plan,
            &crate::planner::ultra_plan::UltraPhase {
                id: "finish".to_string(),
                prompt: "Finish project".to_string(),
            },
            &config(dir.path().to_path_buf()),
        );
        assert!(prompt.contains("Original ultra goal: 3011 port app"));
        assert!(prompt.contains("Profile: nextjs"));
        assert!(prompt.contains("Phase id: finish"));
        assert!(prompt.contains("Workspace snapshot:"));
        assert!(prompt.contains("Profile runtime contract:"));
        assert!(prompt.contains("Required final artifacts:"));
        assert!(prompt.contains("- package.json"));
        assert!(prompt.contains("- src/app/page.tsx"));
    }

    #[test]
    fn ultra_plan_final_profile_failure_runs_repair() {
        let dir = tempfile::tempdir().unwrap();
        let step_json = generated_nextjs_artifact_plan_json("Scaffold project");
        let mut planner = FakeClient::new(
            (0..6)
                .map(|_| AssistantReply::text(step_json.clone()))
                .collect(),
        );
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
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/global.d.ts","content":"declare module \"*.css\";"}),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
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
        let step_json = generated_nextjs_artifact_plan_json("Scaffold project");
        let mut planner = FakeClient::new(
            (0..3)
                .map(|_| AssistantReply::text(step_json.clone()))
                .collect(),
        );
        let package = r#"{"dependencies":{"next":"x","react":"x","react-dom":"x"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#;
        let bad_package = r#"{"dependencies":{"next":"x"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#;
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"package.json","content":bad_package}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/page.tsx","content":"export default function Page(){return <main>App</main>;}"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/layout.tsx","content":"export default function Layout({children}:{children:React.ReactNode}){return <html><body>{children}</body></html>;}"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/global.d.ts","content":"declare module \"*.css\";"}),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("done"),
        ]);
        let _ = package;
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
        let step_json = generated_data_mutation_plan_json("mutate data");
        let mut planner = FakeClient::new(vec![AssistantReply::text(step_json)]);
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"input/source.csv","content":"5678"}),
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
        messages: Vec<Vec<ConversationMessage>>,
    }

    impl FakeClient {
        fn new(replies: Vec<AssistantReply>) -> Self {
            Self {
                replies,
                messages: Vec::new(),
            }
        }
    }

    impl ChatClient for FakeClient {
        fn label(&self) -> &str {
            "fake"
        }

        fn chat(
            &mut self,
            _model: &str,
            messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _native_tools_enabled: bool,
        ) -> anyhow::Result<AssistantReply> {
            self.messages.push(messages.to_vec());
            if self.replies.is_empty() {
                anyhow::bail!("fake client exhausted")
            }
            Ok(self.replies.remove(0))
        }
    }

    fn generated_step_plan_json(goal: &str) -> String {
        serde_json::to_string(&StepPlan::single(goal)).unwrap()
    }

    fn generated_nextjs_artifact_plan_json(goal: &str) -> String {
        serde_json::to_string(&StepPlan {
            goal: goal.to_string(),
            steps: vec![PlanStep {
                id: "create-nextjs-artifacts".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create package.json, src/app/page.tsx, src/app/layout.tsx, and src/app/global.d.ts".to_string(),
                expected_paths: vec![
                    "package.json".to_string(),
                    "src/app/page.tsx".to_string(),
                    "src/app/layout.tsx".to_string(),
                    "src/app/global.d.ts".to_string(),
                ],
                verify: Vec::new(),
            }],
        })
        .unwrap()
    }

    fn generated_data_mutation_plan_json(goal: &str) -> String {
        serde_json::to_string(&StepPlan {
            goal: goal.to_string(),
            steps: vec![PlanStep {
                id: "mutate-input".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Mutate input/source.csv".to_string(),
                expected_paths: vec!["input/source.csv".to_string()],
                verify: Vec::new(),
            }],
        })
        .unwrap()
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
