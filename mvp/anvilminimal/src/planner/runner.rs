use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::eval_events;
use crate::minimal_loop::completion::CompletionContract;
use crate::minimal_loop::dependency_setup::NodeDependencySetupAuthority;
use crate::minimal_loop::evidence::verify_runtime_acceptance;
use crate::minimal_loop::loop_run::{
    RunSessionOptions, RunSessionStepKind, extract_requested_artifact_paths,
    run_session_with_outcome_with_options,
};
use crate::minimal_loop::repair_target::{classify_repair_target, repair_target_followed};
use crate::planner::intent::detect_intent;
use crate::planner::lint::{
    PlanLintReport, PlanQualityContext, PlanQualityReport, lint_step_plan_report,
    lint_ultra_plan_report, step_plan_quality_report, step_plan_quality_warnings,
};
use crate::planner::profile::{
    PhaseVerificationMode, profile_auto_repair, profile_before_phase, profile_expected_paths,
    profile_generation_rules, profile_guidance, profile_post_step_repair,
    profile_quality_expectations, profile_repair_prompt, verify_profile_final,
    verify_profile_invariant,
};
use crate::planner::repair::{
    RecoveryHandoff, RepairContext, build_repair_prompt_with_context,
    save_repair_report_with_context, save_ultra_recovery_prompt, suggested_ultra_recovery_command,
};
use crate::planner::step_plan::{
    PlanStep, StepKind, StepPlan, extract_json_object, parse_generated_step_plan_json,
    parse_step_plan, render_step_plan, repair_generated_step_plan_contract,
};
use crate::planner::ultra_plan::{UltraPhase, UltraPlan, parse_ultra_plan, render_ultra_plan};
use crate::planner::verify::{VerificationReport, verify_step_with_setup};
use crate::providers::{ChatClient, model_for};
use crate::state::SessionSnapshot;
use crate::tools::path_guard::resolve_existing;
use crate::tui::status::UiStatus;
use crate::tui::{InteractionUi, NOOP_UI};
use serde_json::json;

const STEP_TURN_MAX_ITERATIONS: usize = 8;
const STEP_REPAIR_MAX_ITERATIONS: usize = 6;
const STEP_REPAIR_MAX_TURNS: usize = 4;
const STEP_REPAIR_MAX_FILE_CHANGING_TURNS: usize = 2;
const ULTRA_PLAN_GENERATION_ATTEMPTS: usize = 3;

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
                repair_generated_step_plan_contract(&mut plan);
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
    let mut session = SessionSnapshot::new();
    run_step_plan_with_session_with_ui(client, &mut session, plan, config, ui, true)
        .map(|outcome| outcome.summary)
        .map_err(|err| anyhow::anyhow!("{}", err.message))
}

#[derive(Debug, Clone, Default)]
struct StepPlanRunOutcome {
    summary: String,
    completed_steps: usize,
    total_steps: usize,
    changed_paths: Vec<String>,
    verify_failures: Vec<String>,
    primary_failure: Option<String>,
    repair_targets: Vec<String>,
    command_failures: Vec<String>,
    repair_attempts: usize,
    repair_changed_paths: Vec<String>,
    stop_reason: Option<String>,
    partial: bool,
}

#[derive(Debug, Clone, Default)]
struct UltraRunContext {
    completed_phases: Vec<String>,
    created_or_changed_paths: Vec<String>,
    last_failed_phase: Option<String>,
    last_verify_failures: Vec<String>,
    last_repair_changed_paths: Vec<String>,
    pending_final_artifacts: Vec<String>,
    unresolved_repair_targets: Vec<String>,
    truncated: bool,
}

const ULTRA_CONTEXT_MAX_PHASES: usize = 12;
const ULTRA_CONTEXT_MAX_PATHS: usize = 24;
const ULTRA_CONTEXT_MAX_MESSAGES: usize = 10;

impl UltraRunContext {
    fn new(pending_final_artifacts: Vec<String>) -> Self {
        Self {
            pending_final_artifacts,
            ..Self::default()
        }
    }

    fn update_after_phase(
        &mut self,
        phase: &UltraPhase,
        outcome: &StepPlanRunOutcome,
        pending_final_artifacts: Vec<String>,
    ) {
        self.last_failed_phase = None;
        self.pending_final_artifacts = pending_final_artifacts;
        push_context_unique_capped(
            &mut self.completed_phases,
            format!(
                "{} ({}/{})",
                phase.id, outcome.completed_steps, outcome.total_steps
            ),
            ULTRA_CONTEXT_MAX_PHASES,
            &mut self.truncated,
        );
        push_context_items_capped(
            &mut self.created_or_changed_paths,
            &outcome.changed_paths,
            ULTRA_CONTEXT_MAX_PATHS,
            &mut self.truncated,
        );
        self.last_verify_failures.clear();
        push_context_items_capped(
            &mut self.last_repair_changed_paths,
            &outcome.repair_changed_paths,
            ULTRA_CONTEXT_MAX_PATHS,
            &mut self.truncated,
        );
        self.unresolved_repair_targets.clear();
        push_context_items_capped(
            &mut self.unresolved_repair_targets,
            &outcome.repair_targets,
            ULTRA_CONTEXT_MAX_MESSAGES,
            &mut self.truncated,
        );
    }

    fn update_after_failure(
        &mut self,
        phase: &UltraPhase,
        outcome: &StepPlanRunOutcome,
        pending_final_artifacts: Vec<String>,
    ) {
        self.last_failed_phase = Some(phase.id.clone());
        self.pending_final_artifacts = pending_final_artifacts;
        push_context_items_capped(
            &mut self.created_or_changed_paths,
            &outcome.changed_paths,
            ULTRA_CONTEXT_MAX_PATHS,
            &mut self.truncated,
        );
        push_context_items_capped(
            &mut self.last_verify_failures,
            &outcome.verify_failures,
            ULTRA_CONTEXT_MAX_MESSAGES,
            &mut self.truncated,
        );
        if let Some(primary) = &outcome.primary_failure {
            push_context_unique_capped(
                &mut self.last_verify_failures,
                primary.clone(),
                ULTRA_CONTEXT_MAX_MESSAGES,
                &mut self.truncated,
            );
        }
        push_context_items_capped(
            &mut self.last_repair_changed_paths,
            &outcome.repair_changed_paths,
            ULTRA_CONTEXT_MAX_PATHS,
            &mut self.truncated,
        );
        push_context_items_capped(
            &mut self.unresolved_repair_targets,
            &outcome.repair_targets,
            ULTRA_CONTEXT_MAX_MESSAGES,
            &mut self.truncated,
        );
    }

    fn update_after_profile_failure(
        &mut self,
        phase: &UltraPhase,
        reason: &str,
        pending_final_artifacts: Vec<String>,
    ) {
        self.last_failed_phase = Some(phase.id.clone());
        self.pending_final_artifacts = pending_final_artifacts;
        push_context_unique_capped(
            &mut self.last_verify_failures,
            eval_events::body_snippet(reason),
            ULTRA_CONTEXT_MAX_MESSAGES,
            &mut self.truncated,
        );
        push_context_unique_capped(
            &mut self.unresolved_repair_targets,
            "profile_contract".to_string(),
            ULTRA_CONTEXT_MAX_MESSAGES,
            &mut self.truncated,
        );
    }

    fn render_prompt_section(&self) -> String {
        if self.completed_phases.is_empty()
            && self.created_or_changed_paths.is_empty()
            && self.last_failed_phase.is_none()
            && self.pending_final_artifacts.is_empty()
        {
            return "Prior ultra context:\n- none yet".to_string();
        }
        let mut lines = vec!["Prior ultra context:".to_string()];
        append_context_list(&mut lines, "Completed phases", &self.completed_phases);
        append_context_list(
            &mut lines,
            "Created or changed paths",
            &self.created_or_changed_paths,
        );
        if let Some(phase) = &self.last_failed_phase {
            lines.push(format!("- Last failed phase: {phase}"));
        }
        append_context_list(
            &mut lines,
            "Recent verify failures",
            &self.last_verify_failures,
        );
        append_context_list(
            &mut lines,
            "Recent repair changed paths",
            &self.last_repair_changed_paths,
        );
        append_context_list(
            &mut lines,
            "Pending final artifacts",
            &self.pending_final_artifacts,
        );
        append_context_list(
            &mut lines,
            "Unresolved repair targets",
            &self.unresolved_repair_targets,
        );
        if self.truncated {
            lines.push("- Context was truncated to bounded path/failure summaries".to_string());
        }
        lines.join("\n")
    }
}

impl StepPlanRunOutcome {
    fn for_plan(plan: &StepPlan) -> Self {
        Self {
            total_steps: plan.steps.len(),
            summary: format!("plan-run complete: {} steps", plan.steps.len()),
            ..Self::default()
        }
    }

    fn merge_step(&mut self, step: &StepRunOutcome) {
        merge_unique_strings(&mut self.changed_paths, &step.changed_paths);
        merge_unique_strings(&mut self.verify_failures, &step.verify_failures);
        merge_unique_strings(&mut self.repair_targets, &step.repair_targets);
        merge_unique_strings(&mut self.command_failures, &step.command_failures);
        merge_unique_strings(&mut self.repair_changed_paths, &step.repair_changed_paths);
        self.repair_attempts += step.repair_attempts;
        if let Some(primary) = &step.primary_failure {
            self.primary_failure = Some(primary.clone());
        }
        if let Some(stop) = &step.stop_reason {
            self.stop_reason = Some(stop.clone());
        }
        self.partial |= step.partial;
    }

    fn mark_failure(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.primary_failure = Some(message.clone());
        self.stop_reason = Some(message);
        self.partial = true;
    }
}

#[derive(Debug, Clone, Default)]
struct StepRunOutcome {
    changed_paths: Vec<String>,
    verify_failures: Vec<String>,
    primary_failure: Option<String>,
    repair_targets: Vec<String>,
    command_failures: Vec<String>,
    repair_attempts: usize,
    repair_changed_paths: Vec<String>,
    stop_reason: Option<String>,
    partial: bool,
}

#[derive(Debug, Clone)]
struct StepRunError {
    message: String,
    outcome: StepRunOutcome,
}

#[derive(Debug, Clone)]
struct StepPlanRunError {
    message: String,
    partial_outcome: StepPlanRunOutcome,
}

impl StepPlanRunError {
    fn from_error(message: impl Into<String>, mut partial_outcome: StepPlanRunOutcome) -> Self {
        let message = message.into();
        partial_outcome.mark_failure(message.clone());
        Self {
            message,
            partial_outcome,
        }
    }
}

fn run_step_plan_with_session_with_ui(
    client: &mut dyn ChatClient,
    session: &mut SessionSnapshot,
    plan: &StepPlan,
    config: &Config,
    ui: &dyn InteractionUi,
    verify_final_contract: bool,
) -> Result<StepPlanRunOutcome, StepPlanRunError> {
    let mut outcome = StepPlanRunOutcome::for_plan(plan);
    let report = lint_step_plan_report(plan);
    if !report.is_pass() {
        emit_planner_error_for_lint(config, "plan-file", &config.planner_model, &report, 0);
        return Err(StepPlanRunError::from_error(
            report.primary_message(),
            outcome,
        ));
    }
    let required_final_artifacts = required_final_artifacts(plan, &config.workspace_root);
    let external_contract = CompletionContract::load_for_config(config)
        .map_err(|err| StepPlanRunError::from_error(err.to_string(), outcome.clone()))?;
    let final_required_capabilities = external_contract
        .as_ref()
        .map(|contract| contract.required_capabilities.clone())
        .unwrap_or_default();
    let final_required_evidence = external_contract
        .as_ref()
        .map(|contract| contract.required_evidence.clone())
        .unwrap_or_default();
    let mut prior_expected_paths = Vec::new();
    for step in &plan.steps {
        if ui.interrupted() {
            return Err(StepPlanRunError::from_error("interrupted by user", outcome));
        }
        let prompt_context = StepPromptContext {
            required_final_artifacts: required_final_artifacts.clone(),
            prior_expected_paths: prior_expected_paths.clone(),
            final_required_capabilities: final_required_capabilities.clone(),
            final_required_evidence: final_required_evidence.clone(),
        };
        match run_step(client, session, plan, step, &prompt_context, config, ui) {
            Ok(step_outcome) => {
                outcome.completed_steps += 1;
                outcome.merge_step(&step_outcome);
            }
            Err(err) => {
                outcome.merge_step(&err.outcome);
                return Err(StepPlanRunError::from_error(err.message, outcome));
            }
        }
        merge_unique_strings(&mut prior_expected_paths, &step.expected_paths);
    }
    if verify_final_contract {
        if let Err(err) = verify_plan_final_contract(plan, &required_final_artifacts, config) {
            return Err(StepPlanRunError::from_error(err.to_string(), outcome));
        }
    }
    outcome.summary = format!("plan-run complete: {} steps", plan.steps.len());
    Ok(outcome)
}

#[derive(Debug, Clone, Default)]
struct StepPromptContext {
    required_final_artifacts: Vec<String>,
    prior_expected_paths: Vec<String>,
    final_required_capabilities: Vec<String>,
    final_required_evidence: Vec<String>,
}

fn run_step(
    client: &mut dyn ChatClient,
    session: &mut SessionSnapshot,
    plan: &StepPlan,
    step: &PlanStep,
    prompt_context: &StepPromptContext,
    config: &Config,
    ui: &dyn InteractionUi,
) -> Result<StepRunOutcome, StepRunError> {
    let instruction = build_step_prompt(plan, step, prompt_context);
    emit_step_prompt_contract(config, step, prompt_context, &instruction);
    if step.step_kind() == StepKind::Report
        && step.expected_paths.is_empty()
        && step.verify.is_empty()
    {
        return Ok(StepRunOutcome::default());
    }
    let step_config = capped_config(config, STEP_TURN_MAX_ITERATIONS);
    let step_options = step_run_session_options(step);
    let initial = run_session_with_outcome_with_options(
        client,
        session,
        &instruction,
        &step.expected_paths,
        &step_config,
        ui,
        step_options,
    )
    .map_err(|err| StepRunError {
        message: err.to_string(),
        outcome: StepRunOutcome {
            primary_failure: Some(err.to_string()),
            stop_reason: Some("initial_turn_error".to_string()),
            partial: true,
            ..StepRunOutcome::default()
        },
    })?;
    let mut outcome = StepRunOutcome {
        changed_paths: initial.changed_paths.clone(),
        stop_reason: Some(format!("{:?}", initial.stop_reason)),
        ..StepRunOutcome::default()
    };
    if let Err(err) = profile_post_step_repair(&config.workspace_root, &config.profile, &plan.goal)
    {
        outcome.primary_failure = Some(err.to_string());
        outcome.stop_reason = Some("profile_post_step_repair_error".to_string());
        outcome.partial = true;
        return Err(StepRunError {
            message: err.to_string(),
            outcome,
        });
    }
    let setup_authority = step_verify_setup_authority(plan, step);
    let report = verify_step_with_setup(&config.workspace_root, step, setup_authority);
    if report.is_pass() {
        return Ok(outcome);
    }
    let first_target = classify_repair_target(&report).as_str().to_string();
    outcome.primary_failure = Some(report.primary_reason());
    outcome.verify_failures.push(report.primary_reason());
    outcome.repair_targets.push(first_target.clone());
    outcome
        .command_failures
        .extend(command_failure_summaries(&report));
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "step_verify_failure",
            "step_id": step.id,
            "repair_target": first_target,
            "primary_reason": eval_events::body_snippet(&report.primary_reason()),
            "missing_paths": report.missing_paths.clone(),
            "command_failures": report.command_failures.len(),
            "dependency_missing": report.dependency_missing.clone(),
            "profile_failures": report.profile_failures.clone(),
            "dependency_setup_authority": setup_authority.as_str(),
        }),
    );
    let mut context = RepairContext {
        profile: Some(config.profile.clone()),
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
        let repair = run_session_with_outcome_with_options(
            client,
            session,
            &repair_prompt,
            &step.expected_paths,
            &repair_config,
            ui,
            step_options,
        )
        .map_err(|err| {
            outcome.primary_failure = Some(err.to_string());
            outcome.stop_reason = Some("repair_turn_error".to_string());
            outcome.repair_attempts = attempt;
            outcome.partial = true;
            StepRunError {
                message: err.to_string(),
                outcome: outcome.clone(),
            }
        })?;
        outcome.repair_attempts = attempt;
        repair_stop_reason = Some(format!("{:?}", repair.stop_reason));
        merge_changed_files(&mut context, &repair.changed_paths);
        merge_unique_strings(&mut outcome.changed_paths, &repair.changed_paths);
        merge_unique_strings(&mut outcome.repair_changed_paths, &repair.changed_paths);
        if !repair.changed_paths.is_empty() {
            file_changing_repairs += 1;
        }
        match profile_post_step_repair(&config.workspace_root, &config.profile, &plan.goal) {
            Ok(true) => {
                let package_path = "package.json".to_string();
                merge_changed_files(&mut context, std::slice::from_ref(&package_path));
                merge_unique_strings(
                    &mut outcome.changed_paths,
                    std::slice::from_ref(&package_path),
                );
                merge_unique_strings(&mut outcome.repair_changed_paths, &[package_path]);
            }
            Ok(false) => {}
            Err(err) => {
                outcome.primary_failure = Some(err.to_string());
                outcome.stop_reason = Some("profile_post_step_repair_error".to_string());
                outcome.partial = true;
                return Err(StepRunError {
                    message: err.to_string(),
                    outcome,
                });
            }
        }
        let retry = verify_step_with_setup(&config.workspace_root, step, setup_authority);
        let retry_target = classify_repair_target(&retry);
        let previous_target = classify_repair_target(&current_report);
        let repair_target_followed = repair_target_followed(previous_target, &repair.changed_paths);
        merge_unique_strings(
            &mut outcome.repair_targets,
            &[retry_target.as_str().to_string()],
        );
        if !retry.is_pass() {
            merge_unique_strings(&mut outcome.verify_failures, &[retry.primary_reason()]);
            merge_unique_strings(
                &mut outcome.command_failures,
                &command_failure_summaries(&retry),
            );
            outcome.primary_failure = Some(retry.primary_reason());
        }
        eval_events::emit(
            config.eval_events_path.as_deref(),
            json!({
                "event": "step_verify_repair",
                "step_id": step.id,
                "attempt": attempt,
                "ok": retry.is_pass(),
                "repair_target": retry_target.as_str(),
                "previous_repair_target": previous_target.as_str(),
                "repair_target_followed": repair_target_followed,
                "primary_reason": eval_events::body_snippet(&retry.primary_reason()),
                "changed_paths": repair.changed_paths.clone(),
                "repair_stop_reason": repair_stop_reason.clone().unwrap_or_default(),
                "dependency_setup_authority": setup_authority.as_str(),
            }),
        );
        if retry.is_pass() {
            outcome.primary_failure = None;
            outcome.stop_reason = repair_stop_reason.clone();
            return Ok(outcome);
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
    let repair_report_path = match save_repair_report_with_context(
        &config.workspace_root,
        &step.id,
        &current_report,
        &context,
    ) {
        Ok(path) => path,
        Err(err) => {
            outcome.primary_failure = Some(err.to_string());
            outcome.stop_reason = Some("repair_report_save_error".to_string());
            outcome.partial = true;
            return Err(StepRunError {
                message: err.to_string(),
                outcome,
            });
        }
    };
    let suggested_command = suggested_ultra_recovery_command(&repair_report_path, &config.profile);
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "recovery_prompt_saved",
            "recovery_handoff_kind": "step_repair_exhausted",
            "step_id": step.id,
            "recovery_prompt_path": repair_report_path.display().to_string(),
            "suggested_recovery_command": suggested_command,
            "recovery_profile": config.profile,
            "local_repair_exhausted": true,
        }),
    );
    let message = format!(
        "step {} failed verification after bounded repair: {}; repair prompt saved: {}; suggested command: {}",
        step.id,
        current_report.primary_reason(),
        repair_report_path.display(),
        suggested_ultra_recovery_command(&repair_report_path, &config.profile)
    );
    outcome.primary_failure = Some(current_report.primary_reason());
    outcome.stop_reason = Some("bounded_repair_exhausted".to_string());
    outcome.partial = true;
    Err(StepRunError { message, outcome })
}

fn step_verify_setup_authority(plan: &StepPlan, step: &PlanStep) -> NodeDependencySetupAuthority {
    if step.step_kind() == StepKind::Setup {
        return NodeDependencySetupAuthority::PlanSetupStep;
    }
    if step.step_kind() != StepKind::Verify {
        return NodeDependencySetupAuthority::None;
    }
    let prior_setup_exists = plan
        .steps
        .iter()
        .take_while(|candidate| candidate.id != step.id)
        .any(|candidate| candidate.step_kind() == StepKind::Setup);
    if prior_setup_exists {
        NodeDependencySetupAuthority::PlanSetupStep
    } else {
        NodeDependencySetupAuthority::None
    }
}

fn step_run_session_options(step: &PlanStep) -> RunSessionOptions {
    RunSessionOptions::plan_step(run_session_step_kind(step))
}

fn run_session_step_kind(step: &PlanStep) -> RunSessionStepKind {
    match step.step_kind() {
        StepKind::Inspect => RunSessionStepKind::Inspect,
        StepKind::Setup => RunSessionStepKind::Setup,
        StepKind::Implement => RunSessionStepKind::Implement,
        StepKind::Verify => RunSessionStepKind::Verify,
        StepKind::Report => RunSessionStepKind::Report,
        StepKind::Unknown(_) => RunSessionStepKind::Unknown,
    }
}

fn capped_config(config: &Config, cap: usize) -> Config {
    let mut out = config.clone();
    out.max_iterations = out.max_iterations.min(cap);
    out
}

fn required_final_artifacts(plan: &StepPlan, root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    merge_unique_strings(
        &mut out,
        &extract_requested_artifact_paths(root, &plan.goal),
    );
    for step in &plan.steps {
        merge_unique_strings(&mut out, &step.expected_paths);
    }
    out
}

fn verify_plan_final_contract(
    plan: &StepPlan,
    required_final_artifacts: &[String],
    config: &Config,
) -> anyhow::Result<()> {
    let external_contract = CompletionContract::load_for_config(config)?;
    let mut required_paths = required_final_artifacts.to_vec();
    if let Some(contract) = external_contract.as_ref() {
        merge_unique_strings(&mut required_paths, &contract.required_paths);
    }
    let missing_final_artifacts = missing_final_artifacts(&config.workspace_root, &required_paths);
    let external_report = external_contract
        .as_ref()
        .map(|contract| contract.verify_with_goal(&config.workspace_root, &plan.goal));
    let runtime_acceptance = external_contract
        .as_ref()
        .map(|contract| contract.runtime_acceptance_report(&config.workspace_root));
    let external_ok = external_report
        .as_ref()
        .is_none_or(|report| report.is_pass());
    let ok = missing_final_artifacts.is_empty() && external_ok;
    let primary_reason = if !missing_final_artifacts.is_empty() {
        format!(
            "missing final artifacts: {}",
            missing_final_artifacts.join(", ")
        )
    } else if let Some(report) = external_report.as_ref().filter(|report| !report.is_pass()) {
        report.primary_reason()
    } else {
        "ok".to_string()
    };
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "plan_final_contract",
            "required_final_artifacts": required_paths,
            "missing_final_artifacts": missing_final_artifacts,
            "external_contract_checked": external_contract.is_some(),
            "external_contract_ok": external_ok,
            "required_capabilities": external_contract
                .as_ref()
                .map(|contract| contract.required_capabilities.clone())
                .unwrap_or_default(),
            "required_evidence": external_contract
                .as_ref()
                .map(|contract| contract.required_evidence.clone())
                .unwrap_or_default(),
            "missing_capabilities": runtime_acceptance
                .as_ref()
                .map(|report| report.missing_capabilities.clone())
                .unwrap_or_default(),
            "missing_evidence": runtime_acceptance
                .as_ref()
                .map(|report| report.missing_evidence.clone())
                .unwrap_or_default(),
            "weak_evidence": runtime_acceptance
                .as_ref()
                .map(|report| report.weak_evidence.clone())
                .unwrap_or_default(),
            "inconclusive_reasons": runtime_acceptance
                .as_ref()
                .map(|report| report.inconclusive_reasons.clone())
                .unwrap_or_default(),
            "runtime_acceptance_inconclusive": runtime_acceptance
                .as_ref()
                .map(|report| report.inconclusive)
                .unwrap_or(false),
            "ok": ok,
            "primary_reason": eval_events::body_snippet(&primary_reason),
        }),
    );
    if ok {
        return Ok(());
    }
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "loop_stop",
            "reason": "plan_final_contract_failure",
            "primary_reason": eval_events::body_snippet(&primary_reason),
        }),
    );
    anyhow::bail!("plan final contract failed: {primary_reason}")
}

fn missing_final_artifacts(root: &Path, required_paths: &[String]) -> Vec<String> {
    required_paths
        .iter()
        .filter(|path| resolve_existing(root, path).is_err())
        .cloned()
        .collect()
}

fn merge_unique_strings(out: &mut Vec<String>, incoming: &[String]) {
    for item in incoming {
        if !out.contains(item) {
            out.push(item.clone());
        }
    }
}

fn command_failure_summaries(report: &VerificationReport) -> Vec<String> {
    report
        .command_failures
        .iter()
        .map(|failure| {
            format!(
                "{}: {}",
                failure.command,
                eval_events::body_snippet(&failure.reason)
            )
        })
        .collect()
}

fn push_context_items_capped(
    out: &mut Vec<String>,
    incoming: &[String],
    cap: usize,
    truncated: &mut bool,
) {
    for item in incoming {
        push_context_unique_capped(out, item.clone(), cap, truncated);
    }
}

fn push_context_unique_capped(
    out: &mut Vec<String>,
    item: String,
    cap: usize,
    truncated: &mut bool,
) {
    if out.contains(&item) {
        return;
    }
    if out.len() >= cap {
        *truncated = true;
        return;
    }
    out.push(item);
}

fn append_context_list(lines: &mut Vec<String>, label: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }
    lines.push(format!("- {label}:"));
    for value in values {
        lines.push(format!("  - {value}"));
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
    let model = model_for(config, true);
    let intent = detect_intent(goal);
    let mut messages = ultra_plan_generation_messages(goal, config);
    let mut last_error = None;
    for attempt in 1..=ULTRA_PLAN_GENERATION_ATTEMPTS {
        emit_ultra_plan_generation_attempt(
            config,
            client.label(),
            model,
            attempt,
            &config.profile,
            &config.style,
            intent,
        );
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
        emit_ultra_plan_raw_output_shape(config, client.label(), model, attempt, &reply.content);
        if !reply.tool_calls.is_empty() {
            let message = format!(
                "ultra plan generation must not emit tool calls: {}",
                tool_call_names(&reply.tool_calls)
            );
            last_error = Some(message.clone());
            emit_ultra_plan_generation_tool_call_rejected(
                config,
                client.label(),
                model,
                attempt,
                &message,
            );
            emit_planner_error(
                config,
                client.label(),
                model,
                "schema",
                "planner_schema_error",
                &message,
                attempt,
            );
            if attempt < ULTRA_PLAN_GENERATION_ATTEMPTS {
                emit_ultra_plan_generation_retry(
                    config,
                    client.label(),
                    model,
                    attempt,
                    "planner_schema_error",
                    &message,
                );
                messages.push(crate::state::ConversationMessage::user(
                    build_ultra_plan_tool_call_retry_prompt(goal, attempt),
                ));
                continue;
            }
            break;
        }
        match parse_ultra_plan(&reply.content) {
            Ok(mut plan) => {
                let normalized =
                    normalize_ultra_plan_metadata(&mut plan, goal, &config.profile, &config.style);
                if !normalized.is_empty() {
                    emit_ultra_plan_generation_metadata_normalized(
                        config,
                        client.label(),
                        model,
                        attempt,
                        &normalized,
                    );
                }
                let report = lint_ultra_plan_report(&plan);
                if report.is_pass() {
                    emit_ultra_plan_generation_succeeded(
                        config,
                        client.label(),
                        model,
                        attempt,
                        plan.phases.len(),
                    );
                    return Ok(plan);
                } else {
                    emit_planner_error_for_lint(config, client.label(), model, &report, attempt);
                    let message = report.primary_message();
                    last_error = Some(message.clone());
                    if attempt < ULTRA_PLAN_GENERATION_ATTEMPTS {
                        let (_stage, kind) = planner_stage_and_kind_for_lint(&report);
                        emit_ultra_plan_generation_retry(
                            config,
                            client.label(),
                            model,
                            attempt,
                            kind,
                            &message,
                        );
                        messages.push(crate::state::ConversationMessage::user(
                            build_ultra_plan_lint_retry_prompt(goal, &report, attempt),
                        ));
                        continue;
                    }
                    break;
                }
            }
            Err(err) => {
                let message = err.to_string();
                last_error = Some(message.clone());
                emit_planner_error(
                    config,
                    client.label(),
                    model,
                    "schema",
                    "planner_schema_error",
                    &message,
                    attempt,
                );
                if attempt < ULTRA_PLAN_GENERATION_ATTEMPTS {
                    emit_ultra_plan_generation_retry(
                        config,
                        client.label(),
                        model,
                        attempt,
                        "planner_schema_error",
                        &message,
                    );
                    messages.push(crate::state::ConversationMessage::user(
                        build_ultra_plan_schema_retry_prompt(goal, &message, attempt),
                    ));
                    continue;
                }
                break;
            }
        }
    }
    let message = last_error.unwrap_or_else(|| "unknown UltraPlan generation error".to_string());
    emit_ultra_plan_generation_failed(config, client.label(), model, &message);
    anyhow::bail!("invalid generated UltraPlan after corrective retries: {message}")
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
    let final_expected_paths =
        profile_expected_paths(&config.workspace_root, &plan.profile, &plan.goal);
    let mut ultra_context = UltraRunContext::new(missing_final_artifacts(
        &config.workspace_root,
        &final_expected_paths,
    ));
    let mut ultra_session = SessionSnapshot::new();
    emit_ultra_context_initialized(config, plan, &ultra_context, ultra_session.messages.len());
    for (index, phase) in plan.phases.iter().enumerate() {
        if ui.interrupted() {
            anyhow::bail!("interrupted by user");
        }
        emit_ultra_phase_event(
            config,
            "ultra_phase_start",
            plan,
            phase,
            index,
            "start",
            None,
            None,
            None,
        );
        let profile_snapshot = profile_before_phase(&config.workspace_root, &plan.profile)?;
        emit_ultra_phase_context_attached(
            config,
            plan,
            phase,
            index,
            &ultra_context,
            ultra_session.messages.len(),
        );
        let phase_prompt = ultra_phase_prompt(plan, phase, config, &ultra_context);
        let step_plan =
            generate_step_plan_with_ui(planner, &phase_prompt, config, ui).map_err(|err| {
                let message = err.to_string();
                emit_ultra_phase_event(
                    config,
                    "ultra_phase_failed",
                    plan,
                    phase,
                    index,
                    "scaffold",
                    Some(false),
                    Some(&message),
                    None,
                );
                emit_planner_error(
                    config,
                    planner.label(),
                    &config.planner_model,
                    "scaffold",
                    "phase_scaffold_error",
                    &format!("phase scaffold failed: {}", message),
                    index + 1,
                );
                let handoff = save_ultra_phase_recovery_handoff(
                    config,
                    plan,
                    phase,
                    "phase_scaffold_error",
                    &message,
                    &missing_final_artifacts(&config.workspace_root, &final_expected_paths),
                    &["phase_scaffold".to_string()],
                )
                .unwrap_or_default();
                anyhow::anyhow!("phase scaffold failed: {}{}", message, handoff)
            })?;
        emit_ultra_phase_event(
            config,
            "ultra_phase_scaffold_complete",
            plan,
            phase,
            index,
            "scaffold",
            Some(true),
            None,
            Some(step_plan.steps.len()),
        );
        emit_ultra_phase_event(
            config,
            "ultra_phase_plan_validated",
            plan,
            phase,
            index,
            "lint",
            Some(true),
            None,
            Some(step_plan.steps.len()),
        );
        save_step_plan(&config.workspace_root, &step_plan)?;
        let final_phase = index + 1 == plan.phases.len();
        let step_outcome = match run_step_plan_with_session_with_ui(
            execution,
            &mut ultra_session,
            &step_plan,
            config,
            ui,
            final_phase,
        ) {
            Ok(outcome) => outcome,
            Err(err) => {
                let message = err.message.clone();
                ultra_context.update_after_failure(
                    phase,
                    &err.partial_outcome,
                    missing_final_artifacts(&config.workspace_root, &final_expected_paths),
                );
                emit_ultra_phase_context_updated(
                    config,
                    plan,
                    phase,
                    index,
                    &ultra_context,
                    ultra_session.messages.len(),
                    true,
                );
                emit_ultra_phase_event(
                    config,
                    "ultra_phase_failed",
                    plan,
                    phase,
                    index,
                    "execute",
                    Some(false),
                    Some(&message),
                    None,
                );
                let handoff = save_ultra_phase_recovery_handoff(
                    config,
                    plan,
                    phase,
                    "phase_execute_error",
                    &message,
                    &missing_final_artifacts(&config.workspace_root, &final_expected_paths),
                    &err.partial_outcome.repair_targets,
                )
                .unwrap_or_default();
                return Err(anyhow::anyhow!(
                    "phase {} failed: {message}{handoff}",
                    phase.id
                ));
            }
        };
        ultra_context.update_after_phase(
            phase,
            &step_outcome,
            missing_final_artifacts(&config.workspace_root, &final_expected_paths),
        );
        emit_ultra_phase_context_updated(
            config,
            plan,
            phase,
            index,
            &ultra_context,
            ultra_session.messages.len(),
            step_outcome.partial,
        );
        emit_ultra_phase_event(
            config,
            "ultra_phase_execute_complete",
            plan,
            phase,
            index,
            "execute",
            Some(true),
            None,
            None,
        );
        let invariant_report = verify_profile_invariant(
            &config.workspace_root,
            &plan.profile,
            &plan.goal,
            &profile_snapshot,
        );
        if !invariant_report.is_pass() {
            ultra_context.update_after_profile_failure(
                phase,
                &invariant_report.primary_reason(),
                missing_final_artifacts(&config.workspace_root, &final_expected_paths),
            );
            emit_ultra_phase_context_updated(
                config,
                plan,
                phase,
                index,
                &ultra_context,
                ultra_session.messages.len(),
                true,
            );
            emit_ultra_phase_event(
                config,
                if final_phase {
                    "ultra_phase_profile_check"
                } else {
                    "ultra_phase_failed"
                },
                plan,
                phase,
                index,
                "profile_invariant",
                Some(false),
                Some(&invariant_report.primary_reason()),
                None,
            );
            emit_phase_verification_event(
                config,
                plan,
                phase,
                index,
                PhaseVerificationMode::IntermediateInvariant,
                false,
                Some(&invariant_report.primary_reason()),
            );
            if !final_phase {
                let handoff = save_ultra_phase_recovery_handoff(
                    config,
                    plan,
                    phase,
                    "profile_invariant_failure",
                    &invariant_report.primary_reason(),
                    &invariant_report.missing_paths,
                    &["profile_contract".to_string()],
                );
                return Err(anyhow::anyhow!(
                    "phase {} profile invariant verification failed: {}{}",
                    phase.id,
                    invariant_report.primary_reason(),
                    handoff.unwrap_or_default()
                ));
            }
        } else {
            emit_phase_verification_event(
                config,
                plan,
                phase,
                index,
                PhaseVerificationMode::IntermediateInvariant,
                true,
                None,
            );
        }
        if !final_phase {
            emit_ultra_phase_event(
                config,
                "ultra_phase_profile_check",
                plan,
                phase,
                index,
                "profile_invariant",
                Some(true),
                None,
                None,
            );
            emit_ultra_phase_event(
                config,
                "ultra_phase_complete",
                plan,
                phase,
                index,
                "complete",
                Some(true),
                None,
                None,
            );
            continue;
        }
        let profile_report = {
            let final_report =
                verify_profile_final(&config.workspace_root, &plan.profile, &plan.goal);
            if final_report.is_pass() && !invariant_report.is_pass() {
                invariant_report.clone()
            } else {
                final_report
            }
        };
        if !profile_report.is_pass() {
            ultra_context.update_after_profile_failure(
                phase,
                &profile_report.primary_reason(),
                missing_final_artifacts(&config.workspace_root, &final_expected_paths),
            );
            emit_ultra_phase_context_updated(
                config,
                plan,
                phase,
                index,
                &ultra_context,
                ultra_session.messages.len(),
                true,
            );
            emit_ultra_phase_event(
                config,
                "ultra_phase_profile_check",
                plan,
                phase,
                index,
                "profile",
                Some(false),
                Some(&profile_report.primary_reason()),
                None,
            );
            emit_phase_verification_event(
                config,
                plan,
                phase,
                index,
                PhaseVerificationMode::FinalAcceptance,
                false,
                Some(&profile_report.primary_reason()),
            );
            if final_phase
                && profile_auto_repair(
                    &config.workspace_root,
                    &plan.profile,
                    &plan.goal,
                    &profile_report,
                )?
            {
                let retry = verify_profile_final(&config.workspace_root, &plan.profile, &plan.goal);
                if retry.is_pass() {
                    emit_ultra_phase_event(
                        config,
                        "ultra_phase_profile_check",
                        plan,
                        phase,
                        index,
                        "profile_auto_repair",
                        Some(true),
                        None,
                        None,
                    );
                    emit_ultra_phase_event(
                        config,
                        "ultra_phase_complete",
                        plan,
                        phase,
                        index,
                        "complete",
                        Some(true),
                        None,
                        None,
                    );
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
                run_session_with_outcome_with_options(
                    execution,
                    &mut repair_session,
                    &repair_prompt,
                    &expected_paths,
                    &repair_config,
                    ui,
                    RunSessionOptions::plan_step(RunSessionStepKind::Implement),
                )
                .map_err(|err| {
                    anyhow::anyhow!("phase {} profile repair failed: {err}", phase.id)
                })?;
                let retry = verify_profile_final(&config.workspace_root, &plan.profile, &plan.goal);
                if retry.is_pass() {
                    emit_ultra_phase_event(
                        config,
                        "ultra_phase_profile_check",
                        plan,
                        phase,
                        index,
                        "profile_repair",
                        Some(true),
                        None,
                        None,
                    );
                    emit_ultra_phase_event(
                        config,
                        "ultra_phase_complete",
                        plan,
                        phase,
                        index,
                        "complete",
                        Some(true),
                        None,
                        None,
                    );
                    continue;
                }
                emit_ultra_phase_event(
                    config,
                    "ultra_phase_failed",
                    plan,
                    phase,
                    index,
                    "profile_repair",
                    Some(false),
                    Some(&format!("{:?}", retry.status)),
                    None,
                );
                return Err(anyhow::anyhow!(
                    "phase {} profile verification failed after repair: {:?}{}",
                    phase.id,
                    retry.status,
                    save_ultra_phase_recovery_handoff(
                        config,
                        plan,
                        phase,
                        "profile_repair_failed",
                        &format!("{:?}", retry.status),
                        &retry.missing_paths,
                        &["profile_contract".to_string()],
                    )
                    .unwrap_or_default()
                ));
            }
            emit_ultra_phase_event(
                config,
                "ultra_phase_failed",
                plan,
                phase,
                index,
                "profile",
                Some(false),
                Some(&format!("{:?}", profile_report.status)),
                None,
            );
            let handoff = save_ultra_phase_recovery_handoff(
                config,
                plan,
                phase,
                "profile_final_failure",
                &format!("{:?}", profile_report.status),
                &profile_report.missing_paths,
                &["profile_contract".to_string()],
            );
            return Err(anyhow::anyhow!(
                "phase {} profile verification failed: {:?}{}",
                phase.id,
                profile_report.status,
                handoff.unwrap_or_default()
            ));
        }
        emit_phase_verification_event(
            config,
            plan,
            phase,
            index,
            PhaseVerificationMode::FinalAcceptance,
            true,
            None,
        );
        emit_ultra_phase_event(
            config,
            "ultra_phase_profile_check",
            plan,
            phase,
            index,
            "profile",
            Some(true),
            None,
            None,
        );
        emit_ultra_phase_event(
            config,
            "ultra_phase_complete",
            plan,
            phase,
            index,
            "complete",
            Some(true),
            None,
            None,
        );
    }
    let acceptance_report = ultra_final_acceptance_report(plan, config)?;
    if !acceptance_report.is_pass() {
        let reason = acceptance_report.primary_reason();
        let target = classify_repair_target(&acceptance_report);
        eval_events::emit(
            config.eval_events_path.as_deref(),
            json!({
                "event": "ultra_final_acceptance_failed",
                "primary_reason": eval_events::body_snippet(&reason),
                "repair_target": target.as_str(),
                "missing_paths": acceptance_report.missing_paths.clone(),
                "profile_failures": acceptance_report.profile_failures.clone(),
            }),
        );
        let fallback_phase = plan.phases.last().cloned().unwrap_or_else(|| UltraPhase {
            id: "final".to_string(),
            prompt: "Final acceptance".to_string(),
        });
        let handoff = save_ultra_phase_recovery_handoff(
            config,
            plan,
            &fallback_phase,
            "final_acceptance_failure",
            &reason,
            &acceptance_report.missing_paths,
            &[target.as_str().to_string()],
        )
        .unwrap_or_default();
        anyhow::bail!("ultra final acceptance failed: {reason}{handoff}");
    }
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "ultra_plan_complete",
            "total_phases": plan.phases.len(),
            "ok": true,
        }),
    );
    Ok(format!(
        "ultra-plan-run complete: {} phases",
        plan.phases.len()
    ))
}

fn ultra_final_acceptance_report(
    plan: &UltraPlan,
    config: &Config,
) -> anyhow::Result<VerificationReport> {
    let external_contract = CompletionContract::load_for_config(config)?;
    let mut required_paths =
        profile_expected_paths(&config.workspace_root, &plan.profile, &plan.goal);
    let mut required_capabilities = inferred_required_capabilities(&plan.profile, &plan.goal);
    let mut required_evidence = Vec::new();
    let mut deferred_commands = Vec::new();
    if let Some(contract) = external_contract.as_ref() {
        merge_unique_strings(&mut required_paths, &contract.required_paths);
        merge_unique_strings(&mut required_capabilities, &contract.required_capabilities);
        merge_unique_strings(&mut required_evidence, &contract.required_evidence);
        deferred_commands.extend(
            contract
                .deferred_verify_requirements
                .iter()
                .map(|requirement| requirement.command.clone()),
        );
    }
    let missing = missing_final_artifacts(&config.workspace_root, &required_paths);
    let acceptance = verify_runtime_acceptance(
        &config.workspace_root,
        &required_paths,
        &external_contract
            .as_ref()
            .map(|contract| contract.verify_commands.clone())
            .unwrap_or_default(),
        &required_capabilities,
        &required_evidence,
        &deferred_commands,
    );
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "ultra_final_acceptance",
            "required_paths": required_paths.clone(),
            "missing_paths": missing.clone(),
            "required_capabilities": required_capabilities.clone(),
            "required_evidence": required_evidence.clone(),
            "runtime_acceptance_passed": acceptance.passed,
            "runtime_acceptance_inconclusive": acceptance.inconclusive,
            "missing_capabilities": acceptance.missing_capabilities.clone(),
            "missing_evidence": acceptance.missing_evidence.clone(),
            "weak_evidence": acceptance.weak_evidence.clone(),
            "inconclusive_reasons": acceptance.inconclusive_reasons.clone(),
            "primary_reason": eval_events::body_snippet(&acceptance.primary_reason),
        }),
    );
    let mut report = VerificationReport::pass();
    for path in missing {
        report.push_missing_path(path);
    }
    if !acceptance.passed {
        report.push_profile_failure(acceptance.primary_reason);
    }
    Ok(report)
}

fn inferred_required_capabilities(profile: &str, goal: &str) -> Vec<String> {
    let lower = goal.to_ascii_lowercase();
    let is_next = matches!(profile, "nextjs" | "next-js" | "next.js");
    let mut capabilities = Vec::new();
    if is_next
        && (lower.contains("game")
            || lower.contains("playable")
            || lower.contains("interactive")
            || lower.contains("player")
            || lower.contains("enemy")
            || lower.contains("score")
            || goal.contains("ゲーム")
            || goal.contains("インベーダー"))
    {
        merge_unique_strings(&mut capabilities, &["stateful_interaction".to_string()]);
        merge_unique_strings(&mut capabilities, &["player_control".to_string()]);
        merge_unique_strings(&mut capabilities, &["adversary_or_challenge".to_string()]);
        merge_unique_strings(&mut capabilities, &["progression_or_score".to_string()]);
        merge_unique_strings(
            &mut capabilities,
            &["failure_or_collision_rule".to_string()],
        );
    } else if is_next
        && (lower.contains("button")
            || lower.contains("form")
            || lower.contains("keyboard")
            || lower.contains("input"))
    {
        merge_unique_strings(&mut capabilities, &["stateful_interaction".to_string()]);
        merge_unique_strings(&mut capabilities, &["user_input_or_action".to_string()]);
        merge_unique_strings(&mut capabilities, &["visible_state_change".to_string()]);
    }
    capabilities
}

fn emit_ultra_phase_event(
    config: &Config,
    event: &str,
    plan: &UltraPlan,
    phase: &UltraPhase,
    index: usize,
    stage: &str,
    ok: Option<bool>,
    reason: Option<&str>,
    step_count: Option<usize>,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": event,
            "phase_id": phase.id,
            "phase_index": index + 1,
            "total_phases": plan.phases.len(),
            "final_phase": index + 1 == plan.phases.len(),
            "stage": stage,
            "ok": ok,
            "reason": reason.map(eval_events::body_snippet).unwrap_or_default(),
            "step_count": step_count,
        }),
    );
}

fn emit_phase_verification_event(
    config: &Config,
    plan: &UltraPlan,
    phase: &UltraPhase,
    index: usize,
    mode: PhaseVerificationMode,
    ok: bool,
    reason: Option<&str>,
) {
    let mode = match mode {
        PhaseVerificationMode::IntermediateInvariant => "intermediate_invariant",
        PhaseVerificationMode::FinalAcceptance => "final_acceptance",
    };
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "phase_verification_result",
            "phase_id": phase.id,
            "phase_index": index + 1,
            "total_phases": plan.phases.len(),
            "phase_verification_mode": mode,
            "ok": ok,
            "reason": reason.map(eval_events::body_snippet).unwrap_or_default(),
        }),
    );
}

fn save_ultra_phase_recovery_handoff(
    config: &Config,
    plan: &UltraPlan,
    phase: &UltraPhase,
    failure_kind: &str,
    reason: &str,
    missing_paths: &[String],
    repair_targets: &[String],
) -> Option<String> {
    let handoff = RecoveryHandoff {
        profile: plan.profile.clone(),
        original_goal: plan.goal.clone(),
        failed_phase: Some(phase.id.clone()),
        failed_step: None,
        failure_kind: failure_kind.to_string(),
        failure_evidence: vec![reason.to_string()],
        missing_paths: missing_paths.to_vec(),
        verify_commands: Vec::new(),
        changed_paths: Vec::new(),
        repair_targets: repair_targets.to_vec(),
    };
    let scope = format!("phase-{}", recovery_scope_token(&phase.id));
    let path = match save_ultra_recovery_prompt(&config.workspace_root, &scope, &handoff) {
        Ok(path) => path,
        Err(err) => {
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "recovery_prompt_save_failed",
                    "recovery_handoff_kind": failure_kind,
                    "phase_id": phase.id,
                    "reason": eval_events::body_snippet(&err.to_string()),
                }),
            );
            return Some(format!("; recovery prompt save failed: {err}"));
        }
    };
    let command = suggested_ultra_recovery_command(&path, &plan.profile);
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "recovery_prompt_saved",
            "recovery_handoff_kind": failure_kind,
            "phase_id": phase.id,
            "recovery_prompt_path": path.display().to_string(),
            "suggested_recovery_command": command,
            "recovery_profile": plan.profile,
            "local_repair_exhausted": true,
        }),
    );
    eval_events::write_run_summary(
        config.eval_events_path.as_deref(),
        &format!(
            "Recovery prompt saved: {}\nSuggested command: {}\nFailure: {}",
            path.display(),
            suggested_ultra_recovery_command(&path, &plan.profile),
            reason
        ),
    );
    Some(format!(
        "; repair prompt saved: {}; suggested command: {}",
        path.display(),
        suggested_ultra_recovery_command(&path, &plan.profile)
    ))
}

fn recovery_scope_token(value: &str) -> String {
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
        "phase".to_string()
    } else {
        token
    }
}

fn emit_ultra_context_initialized(
    config: &Config,
    plan: &UltraPlan,
    context: &UltraRunContext,
    session_message_count: usize,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "ultra_context_initialized",
            "total_phases": plan.phases.len(),
            "shared_execution_session": true,
            "session_message_count": session_message_count,
            "pending_final_artifacts_count": context.pending_final_artifacts.len(),
            "context_truncated": context.truncated,
        }),
    );
}

fn emit_ultra_phase_context_attached(
    config: &Config,
    plan: &UltraPlan,
    phase: &UltraPhase,
    index: usize,
    context: &UltraRunContext,
    session_message_count: usize,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "ultra_phase_context_attached",
            "phase_id": phase.id,
            "phase_index": index + 1,
            "total_phases": plan.phases.len(),
            "shared_execution_session": true,
            "session_message_count": session_message_count,
            "completed_phase_count": context.completed_phases.len(),
            "changed_path_count": context.created_or_changed_paths.len(),
            "pending_final_artifacts_count": context.pending_final_artifacts.len(),
            "unresolved_repair_target_count": context.unresolved_repair_targets.len(),
            "has_previous_context": index > 0
                && (!context.completed_phases.is_empty()
                    || !context.created_or_changed_paths.is_empty()
                    || context.last_failed_phase.is_some()),
            "context_truncated": context.truncated,
        }),
    );
}

fn emit_ultra_phase_context_updated(
    config: &Config,
    plan: &UltraPlan,
    phase: &UltraPhase,
    index: usize,
    context: &UltraRunContext,
    session_message_count: usize,
    partial_outcome_recorded: bool,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "ultra_phase_context_updated",
            "phase_id": phase.id,
            "phase_index": index + 1,
            "total_phases": plan.phases.len(),
            "shared_execution_session": true,
            "session_message_count": session_message_count,
            "completed_phase_count": context.completed_phases.len(),
            "changed_path_count": context.created_or_changed_paths.len(),
            "pending_final_artifacts_count": context.pending_final_artifacts.len(),
            "recent_verify_failure_count": context.last_verify_failures.len(),
            "recent_repair_changed_path_count": context.last_repair_changed_paths.len(),
            "unresolved_repair_target_count": context.unresolved_repair_targets.len(),
            "partial_outcome_recorded": partial_outcome_recorded,
            "context_truncated": context.truncated,
        }),
    );
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

fn emit_ultra_plan_raw_output_shape(
    config: &Config,
    provider: &str,
    model: &str,
    attempt: usize,
    content: &str,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "ultra_plan_raw_output_shape",
            "planner_provider": provider,
            "planner_model": model,
            "attempt": attempt,
            "content_len": content.chars().count(),
            "has_yaml_fence": content.contains("```yaml") || content.contains("```yml"),
            "contains_goal_key": content.contains("goal:"),
            "contains_profile_key": content.contains("profile:"),
            "contains_style_key": content.contains("style:"),
            "contains_intent_key": content.contains("intent:"),
            "contains_phases_key": content.contains("phases:"),
            "preview": eval_events::body_snippet(content),
        }),
    );
}

fn emit_ultra_plan_generation_attempt(
    config: &Config,
    provider: &str,
    model: &str,
    attempt: usize,
    profile: &str,
    style: &str,
    intent: &str,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "ultra_plan_generation_attempt",
            "planner_provider": provider,
            "planner_model": model,
            "repair_attempt": attempt,
            "profile": profile,
            "style": style,
            "intent": intent,
            "degraded": false,
        }),
    );
}

fn emit_ultra_plan_generation_retry(
    config: &Config,
    provider: &str,
    model: &str,
    attempt: usize,
    failure_kind: &str,
    message: &str,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "ultra_plan_generation_retry",
            "planner_provider": provider,
            "planner_model": model,
            "repair_attempt": attempt,
            "planner_error_kind": failure_kind,
            "planner_error_message": eval_events::body_snippet(message),
            "degraded": false,
        }),
    );
}

fn emit_ultra_plan_generation_failed(config: &Config, provider: &str, model: &str, message: &str) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "ultra_plan_generation_failed",
            "planner_provider": provider,
            "planner_model": model,
            "planner_error_kind": "planner_schema_error",
            "planner_error_message": eval_events::body_snippet(message),
            "degraded": false,
        }),
    );
}

fn emit_ultra_plan_generation_succeeded(
    config: &Config,
    provider: &str,
    model: &str,
    attempt: usize,
    phase_count: usize,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "ultra_plan_generation_succeeded",
            "planner_provider": provider,
            "planner_model": model,
            "repair_attempt": attempt,
            "phase_count": phase_count,
            "degraded": false,
        }),
    );
}

fn emit_ultra_plan_generation_tool_call_rejected(
    config: &Config,
    provider: &str,
    model: &str,
    attempt: usize,
    message: &str,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "ultra_plan_generation_tool_call_rejected",
            "planner_provider": provider,
            "planner_model": model,
            "repair_attempt": attempt,
            "planner_error_kind": "planner_schema_error",
            "planner_error_message": eval_events::body_snippet(message),
            "degraded": false,
        }),
    );
}

fn emit_ultra_plan_generation_metadata_normalized(
    config: &Config,
    provider: &str,
    model: &str,
    attempt: usize,
    fields: &[String],
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "ultra_plan_generation_metadata_normalized",
            "planner_provider": provider,
            "planner_model": model,
            "repair_attempt": attempt,
            "fields": fields,
            "degraded": false,
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
    let issue_hints = schema_retry_issue_hints(error)
        .into_iter()
        .map(|hint| format!("- {hint}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Your previous StepPlan output failed schema validation on attempt {attempt}/3: {error}.\n\
Return only one JSON object and no markdown fences.\n\
Detected schema issues:\n{issue_hints}\n\n\
Required JSON shape:\n\
{{\n  \"goal\": \"{goal}\",\n  \"steps\": [\n    {{\n      \"id\": \"kebab-id\",\n      \"kind\": \"implement\",\n      \"expected_result\": \"pass\",\n      \"instruction\": \"Create the required files for the goal.\",\n      \"expected_paths\": [\"relative/path\"],\n      \"verify\": [\"command\"]\n    }}\n  ]\n}}\n\n\
Rules:\n- Include top-level goal and non-empty steps.\n- Step id must be a quoted string, not a number.\n- expected_result must be exactly \"pass\" or \"fail\", not prose.\n- Keep expected_paths workspace-relative.\n- Use deterministic verify commands only.\n\nGoal: {goal}"
    )
}

fn schema_retry_issue_hints(error: &str) -> Vec<&'static str> {
    let lower = error.to_ascii_lowercase();
    let mut hints = Vec::new();
    if lower.contains("missing goal") || lower.contains("top-level goal") {
        hints.push("Add a top-level goal field equal to the original goal.");
    }
    if lower.contains("missing steps")
        || lower.contains("non-empty steps")
        || lower.contains("at least one step")
    {
        hints.push("Add a non-empty steps array.");
    }
    if lower.contains("id") && (lower.contains("number") || lower.contains("string")) {
        hints.push("Use quoted string step ids such as \"setup-project\".");
    }
    if lower.contains("expected_result") || lower.contains("expected result") {
        hints.push("Set expected_result to exactly \"pass\" or \"fail\".");
    }
    if lower.contains("expected_paths") || lower.contains("workspace-relative") {
        hints.push("Use workspace-relative expected_paths and avoid absolute paths.");
    }
    if hints.is_empty() {
        hints.push("Return the canonical StepPlan JSON object with goal and steps.");
    }
    hints
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
        out.push("- Preserve the verification meaning; do not replace a real check with a weak file-existence check.");
        out.push("- Move setup, dependency installation, and dev-server readiness into setup/implement instructions or external postcheck, not verify.");
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

fn ultra_plan_generation_messages(
    goal: &str,
    config: &Config,
) -> Vec<crate::state::ConversationMessage> {
    let intent = detect_intent(goal);
    vec![
        crate::state::ConversationMessage::system(ultra_plan_generation_system_prompt(
            &config.profile,
            &config.style,
            intent,
        )),
        crate::state::ConversationMessage::user(ultra_plan_generation_user_prompt(
            goal,
            &config.profile,
            &config.style,
            intent,
        )),
    ]
}

fn ultra_plan_generation_system_prompt(profile: &str, style: &str, intent: &str) -> String {
    let style_rules = match style {
        "tdd" => {
            "- Style tdd: use phases for inspect, failing test, implementation, focused verification, and cleanup. The failing-test phase should ask /plan-run to create an expected_result:\"fail\" red verification step.\n"
        }
        "test-hardening" => {
            "- Style test-hardening: inspect existing tests, identify uncovered behavior, add focused tests, make only necessary fixes, then run broader verification.\n"
        }
        _ => {
            "- Style default: use ordinary phased delivery unless the user explicitly asks for TDD or test hardening.\n"
        }
    };
    let profile_rules = profile_generation_rules(profile, intent).unwrap_or(
        "- Profile generic: keep phases concrete, local, deterministic, and safe. Separate setup, implementation, and verification responsibilities.\n",
    );
    format!(
        "You are Anvil's ultra planner. You do not execute tools or emit tool calls. Produce a top-level phase plan whose phases will each be executed by /plan-run.\n\
Output YAML only, with this exact shape:\n\
goal: \"...\"\n\
profile: \"{profile}\"\n\
style: \"{style}\"\n\
intent: \"{intent}\"\n\
phases:\n\
  - id: \"kebab-id\"\n\
    prompt: \"focused natural-language /plan-run goal\"\n\
Rules:\n\
- Return 2 to 6 phases for most tasks, never more than 8.\n\
- Each phase prompt must be a focused natural-language task that can be handled by one /plan-run.\n\
- Phase prompts should name the concrete outcome and the verification expectation when practical.\n\
- If the user goal contains a Required final artifacts list, preserve those exact repository-relative paths across phases. Do not rename or relocate them.\n\
- Do not make a phase prompt a shell command or a REPL command.\n\
- Do not include long-running dev servers, network setup, or package installation as a phase unless the user explicitly requires it.\n\
- Stop at a clean final verification or cleanup phase.\n\
{profile_rules}{style_rules}"
    )
}

fn ultra_plan_generation_user_prompt(
    goal: &str,
    profile: &str,
    style: &str,
    intent: &str,
) -> String {
    format!(
        "Create an UltraPlan YAML for this task using profile `{profile}`, style `{style}`, and work intent `{intent}`.\n\
Use the exact YAML shape from the system message and return YAML only.\n\n\
Task:\n{goal}"
    )
}

fn build_ultra_plan_schema_retry_prompt(goal: &str, error: &str, attempt: usize) -> String {
    format!(
        "Your previous UltraPlan output failed schema parsing on attempt {attempt}/{ULTRA_PLAN_GENERATION_ATTEMPTS}: {error}.\n\
Return corrected YAML only, no markdown fences, no prose, and no tool calls.\n\
Required YAML shape:\n\
goal: \"{goal}\"\n\
profile: \"...\"\n\
style: \"...\"\n\
intent: \"...\"\n\
phases:\n\
  - id: \"kebab-id\"\n\
    prompt: \"focused natural-language /plan-run goal\"\n\
Rules:\n\
- Include top-level goal and 2-8 phases.\n\
- Each phase must have id and prompt.\n\
- Phase prompts must be natural-language tasks, not shell commands.\n\n\
Goal: {goal}"
    )
}

fn build_ultra_plan_lint_retry_prompt(
    goal: &str,
    report: &PlanLintReport,
    attempt: usize,
) -> String {
    let errors = report
        .errors
        .iter()
        .map(|err| format!("- [{}] {}", err.category, err.message))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Your previous UltraPlan YAML failed deterministic lint on attempt {attempt}/{ULTRA_PLAN_GENERATION_ATTEMPTS}.\n\
Fix these issues without weakening safety rules:\n{errors}\n\n\
Return corrected YAML only, no markdown fences, no prose, and no tool calls.\n\
Hard constraints:\n\
- Keep 2-8 phases.\n\
- Use unique kebab-case phase ids.\n\
- Phase prompts must be natural-language /plan-run goals, not shell commands or REPL commands.\n\
- Keep concrete outcomes and verification expectations in phase prompts.\n\
- Preserve any Required final artifacts from the user goal.\n\n\
Goal: {goal}"
    )
}

fn build_ultra_plan_tool_call_retry_prompt(goal: &str, attempt: usize) -> String {
    format!(
        "Your previous UltraPlan generation attempted to emit tool calls on attempt {attempt}/{ULTRA_PLAN_GENERATION_ATTEMPTS}.\n\
Do not call tools. Return corrected UltraPlan YAML only.\n\
Use natural-language phase prompts for later /plan-run execution.\n\n\
Goal: {goal}"
    )
}

fn normalize_ultra_plan_metadata(
    plan: &mut UltraPlan,
    goal: &str,
    profile: &str,
    style: &str,
) -> Vec<String> {
    let intent = detect_intent(goal);
    let mut normalized = Vec::new();
    if plan.goal != goal {
        plan.goal = goal.to_string();
        normalized.push("goal".to_string());
    }
    if plan.profile != profile {
        plan.profile = profile.to_string();
        normalized.push("profile".to_string());
    }
    if plan.style != style {
        plan.style = style.to_string();
        normalized.push("style".to_string());
    }
    if plan.intent != intent {
        plan.intent = intent.to_string();
        normalized.push("intent".to_string());
    }
    normalized
}

fn tool_call_names(tool_calls: &[crate::state::ToolCall]) -> String {
    let names = tool_calls
        .iter()
        .map(|call| call.name.as_str())
        .collect::<Vec<_>>()
        .join(",");
    if names.is_empty() {
        "<unknown>".to_string()
    } else {
        names
    }
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

    prompt.push_str("\n\nRequired final capabilities:\n");
    append_bullets_or_none(&mut prompt, &context.final_required_capabilities);

    prompt.push_str("\n\nRequired final evidence:\n");
    append_bullets_or_none(&mut prompt, &context.final_required_evidence);

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
            "has_required_final_capabilities": prompt.contains("Required final capabilities:"),
            "has_required_final_evidence": prompt.contains("Required final evidence:"),
            "required_final_capabilities": context.final_required_capabilities.clone(),
            "required_final_evidence": context.final_required_evidence.clone(),
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

fn ultra_phase_prompt(
    plan: &UltraPlan,
    phase: &UltraPhase,
    config: &Config,
    context: &UltraRunContext,
) -> String {
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
    let prior_context = context.render_prompt_section();
    format!(
        "Original ultra goal: {}\nProfile: {}\nStyle: {}\nIntent: {}\nPhase id: {}\nPhase task: {}\n\nWorkspace snapshot:\n{}\n\n{}\n\nProfile runtime contract:\n- keep work inside the workspace\n- satisfy required profile artifacts before final phase completion\n- use deterministic verification; preferred commands:\n{}\n{}",
        plan.goal,
        plan.profile,
        plan.style,
        plan.intent,
        phase.id,
        phase.prompt,
        workspace_snapshot,
        prior_context,
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
        let invalid = r#"{"goal":"goal","steps":[{"id":"s1","kind":"implement","expected_result":"pass","instruction":"Create app","expected_paths":["package.json"],"verify":["node check.js || node check2.js"]}]}"#;
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
    fn nextjs_profile_strengthening_does_not_reintroduce_duplicate_package_owner() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        let generated = r#"{
          "goal":"Scaffold a Next.js app",
          "steps":[
            {
              "id":"setup-manifests",
              "kind":"setup",
              "expected_result":"pass",
              "instruction":"Create package.json and tsconfig.json",
              "expected_paths":["package.json","tsconfig.json"],
              "verify":[]
            },
            {
              "id":"implement-game-page",
              "kind":"implement",
              "expected_result":"pass",
              "instruction":"Create package.json and the app entrypoint",
              "expected_paths":["package.json","src/app/page.tsx"],
              "verify":[]
            }
          ]
        }"#;
        let mut plan =
            parse_generated_step_plan_json(generated, "Scaffold a Next.js Space Invaders app")
                .unwrap();
        repair_generated_step_plan_contract(&mut plan);
        strengthen_step_plan_for_profile(&mut plan, &cfg);
        repair_generated_step_plan_contract(&mut plan);
        crate::planner::lint::lint_step_plan(&plan).unwrap();
        let package_owners = plan
            .steps
            .iter()
            .filter(|step| {
                step.expected_paths
                    .iter()
                    .any(|path| path == "package.json")
            })
            .count();
        assert_eq!(package_owners, 1);
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
        assert!(prompt.contains("Preserve the verification meaning"));
        assert!(prompt.contains("dependency installation"));
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
    fn schema_retry_prompt_reports_missing_goal() {
        let prompt = build_schema_retry_prompt("Build app", "StepPlan missing goal", 1);
        assert!(prompt.contains("Detected schema issues:"));
        assert!(prompt.contains("Add a top-level goal field"));
        assert!(prompt.contains("\"goal\": \"Build app\""));
        assert!(prompt.contains("Return only one JSON object"));
    }

    #[test]
    fn schema_retry_prompt_reports_invalid_step_id_type() {
        let prompt =
            build_schema_retry_prompt("Build app", "step id must be string, not number", 2);
        assert!(prompt.contains("Use quoted string step ids"));
        assert!(prompt.contains("Step id must be a quoted string"));
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
            final_required_capabilities: vec!["player_control".to_string()],
            final_required_evidence: vec!["interactive_ui_source_evidence".to_string()],
        };
        let prompt = build_step_prompt(&plan, &plan.steps[0], &context);
        assert!(prompt.contains("Overall goal:"));
        assert!(prompt.contains("Build a game"));
        assert!(prompt.contains("Current step id:"));
        assert!(prompt.contains("create-page"));
        assert!(prompt.contains("Verification commands for this step:"));
        assert!(prompt.contains("npm run build"));
        assert!(prompt.contains("Required final capabilities:"));
        assert!(prompt.contains("player_control"));
        assert!(prompt.contains("Required final evidence:"));
        assert!(prompt.contains("interactive_ui_source_evidence"));
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
        let invalid = r#"{"goal":"goal","steps":[{"id":"s1","kind":"implement","instruction":"Create app","expected_paths":["package.json"],"verify":["node check.js || node check2.js"]}]}"#;
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
    fn ultra_plan_prompt_includes_source_parity_rules() {
        let mut cfg = config(PathBuf::from("/tmp/work"));
        cfg.profile = "nextjs".to_string();
        let messages = ultra_plan_generation_messages("Build a Next.js app on port 3011", &cfg);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "system");
        assert!(messages[0].content.contains("You do not execute tools"));
        assert!(messages[0].content.contains("Output YAML only"));
        assert!(messages[0].content.contains("phases:"));
        assert!(messages[0].content.contains("next/react/react-dom"));
        assert!(
            messages[0]
                .content
                .contains("dependency setup before any npm run build")
        );
        assert!(messages[0].content.contains("Tailwind"));
        assert_eq!(messages[1].role, "user");
        assert!(
            messages[1]
                .content
                .contains("Build a Next.js app on port 3011")
        );
    }

    #[test]
    fn ultra_plan_prompt_does_not_bake_in_game_scenario_terms() {
        let mut cfg = config(PathBuf::from("/tmp/work"));
        cfg.profile = "nextjs".to_string();
        let system =
            ultra_plan_generation_messages("Build a Space Invaders game on port 3011", &cfg)
                .remove(0)
                .content;
        for term in ["Space Invaders", "enemy", "bullet", "collision", "score"] {
            assert!(!system.contains(term), "{term}: {system}");
        }
    }

    #[test]
    fn ultra_plan_generation_retries_invalid_output() {
        let dir = tempfile::tempdir().unwrap();
        let mut planner = FakeClient::new(vec![
            AssistantReply::text("not yaml"),
            AssistantReply::text(generated_ultra_plan_yaml("goal")),
        ]);
        let plan =
            generate_ultra_plan(&mut planner, "goal", &config(dir.path().to_path_buf())).unwrap();
        assert_eq!(planner.messages.len(), 2);
        assert_eq!(plan.goal, "goal");
        assert_eq!(plan.phases.len(), 2);
    }

    #[test]
    fn ultra_plan_generation_rejects_tool_calls() {
        let dir = tempfile::tempdir().unwrap();
        let mut planner = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"x","content":"x"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text(generated_ultra_plan_yaml("goal")),
        ]);
        let plan =
            generate_ultra_plan(&mut planner, "goal", &config(dir.path().to_path_buf())).unwrap();
        assert_eq!(planner.messages.len(), 2);
        assert_eq!(plan.phases[0].id, "scaffold");
    }

    #[test]
    fn ultra_plan_generation_normalizes_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.style = "default".to_string();
        cfg.eval_events_path = Some(events.clone());
        let plan = UltraPlan {
            goal: "different goal".to_string(),
            profile: "generic".to_string(),
            style: "tdd".to_string(),
            intent: "fix".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "scaffold".to_string(),
                    prompt: "Create the Next.js package and app entrypoint, then verify the files exist.".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "verify".to_string(),
                    prompt: "Run deterministic Next.js build verification and repair failures.".to_string(),
                },
            ],
        };
        let mut planner = FakeClient::new(vec![AssistantReply::text(render_ultra_plan(&plan))]);
        let generated = generate_ultra_plan(&mut planner, "Build app", &cfg).unwrap();
        assert_eq!(generated.goal, "Build app");
        assert_eq!(generated.profile, "nextjs");
        assert_eq!(generated.style, "default");
        assert_eq!(generated.intent, "create");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("ultra_plan_generation_metadata_normalized"));
    }

    #[test]
    fn invalid_ultra_plan_generation_does_not_save_plan_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut planner = FakeClient::new(vec![
            AssistantReply::text("not yaml"),
            AssistantReply::text("still not yaml"),
            AssistantReply::text("nope"),
        ]);
        let mut execution = FakeClient::new(vec![]);
        let err = generate_and_run_ultra_plan(
            &mut planner,
            &mut execution,
            "goal",
            &config(dir.path().to_path_buf()),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("invalid generated UltraPlan after corrective retries"));
        assert!(!dir.path().join(".anvil/plans").exists());
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
        let degraded = r#"{"goal":"Build a Next.js game app","steps":[{"id":"bad","kind":"implement","expected_result":"pass","instruction":"Create app","expected_paths":["package.json"],"verify":["node check.js || node check2.js"]}]}"#;
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
            &UltraRunContext::new(vec!["src/app/page.tsx".to_string()]),
        );
        assert!(prompt.contains("Original ultra goal: 3011 port app"));
        assert!(prompt.contains("Profile: nextjs"));
        assert!(prompt.contains("Phase id: finish"));
        assert!(prompt.contains("Workspace snapshot:"));
        assert!(prompt.contains("Prior ultra context:"));
        assert!(prompt.contains("Pending final artifacts:"));
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
        let good_package = r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#;
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
    fn ultra_phase_emits_plan_validated_event() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(generated_step_plan_json("phase one")),
            AssistantReply::text(generated_step_plan_json("phase two")),
        ]);
        let mut execution = FakeClient::new(vec![]);
        let plan = UltraPlan {
            goal: "Do two phases".to_string(),
            profile: "generic".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "phase-one".to_string(),
                    prompt: "Phase one".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "phase-two".to_string(),
                    prompt: "Phase two".to_string(),
                },
            ],
        };
        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();
        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("ultra_phase_plan_validated"));
        assert!(event_text.contains("\"stage\":\"lint\""));
        assert!(event_text.contains("\"step_count\":1"));
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
        assert!(err.contains("phase scaffold profile invariant verification failed"));
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
        assert!(err.contains("profile invariant verification failed"));
        assert!(err.contains("content changed"));
    }

    #[test]
    fn plan_run_final_contract_fails_when_required_final_artifact_missing() {
        let dir = tempfile::tempdir().unwrap();
        let plan = StepPlan::single("Update docs\n\nRequired final artifacts:\n- README.md");
        let mut fake = FakeClient::new(vec![]);
        let err = run_step_plan(&mut fake, &plan, &config(dir.path().to_path_buf()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("plan final contract failed"));
        assert!(err.contains("README.md"));
    }

    #[test]
    fn plan_run_final_contract_passes_after_step_artifacts_created() {
        let dir = tempfile::tempdir().unwrap();
        let plan = StepPlan {
            goal: "Create a.txt\n\nRequired final artifacts:\n- a.txt".to_string(),
            steps: vec![PlanStep {
                id: "code".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create a.txt".to_string(),
                expected_paths: vec!["a.txt".to_string()],
                verify: Vec::new(),
            }],
        };
        let mut fake = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: vec![crate::state::ToolCall::new(
                "Write",
                serde_json::json!({"path":"a.txt","content":"ok"}),
            )],
            prompt_tokens: None,
            completion_tokens: None,
        }]);
        let result = run_step_plan(&mut fake, &plan, &config(dir.path().to_path_buf())).unwrap();
        assert_eq!(result, "plan-run complete: 1 steps");
        assert!(dir.path().join("a.txt").is_file());
    }

    #[test]
    fn plan_run_external_completion_contract_checked_at_plan_level() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            r#"{"required_paths":[],"verify_commands":["test -f missing.txt"]}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        let plan = StepPlan::single("Inspect workspace");
        let mut fake = FakeClient::new(vec![]);
        let err = run_step_plan(&mut fake, &plan, &cfg)
            .unwrap_err()
            .to_string();
        assert!(err.contains("plan final contract failed"));
    }

    #[test]
    fn plan_run_external_completion_contract_checks_required_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            r#"{"required_paths":["date-helper.js"],"verify_commands":["node date-helper.js"],"required_capabilities":["implementation","deterministic_test"]}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        let plan = StepPlan {
            goal: "Create date helper".to_string(),
            steps: vec![PlanStep {
                id: "code".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create date-helper.js".to_string(),
                expected_paths: vec!["date-helper.js".to_string()],
                verify: Vec::new(),
            }],
        };
        let mut fake = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: vec![crate::state::ToolCall::new(
                "Write",
                serde_json::json!({"path":"date-helper.js","content":"exports.formatDate = (d) => String(d);"}),
            )],
            prompt_tokens: None,
            completion_tokens: None,
        }]);
        let err = run_step_plan(&mut fake, &plan, &cfg)
            .unwrap_err()
            .to_string();
        assert!(err.contains("plan final contract failed"));
        assert!(err.contains("missing_required_evidence"));
    }

    #[test]
    fn run_plan_file_uses_same_step_runtime_options() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let plan = StepPlan {
            goal: "Inspect workspace\n\nRequired final artifacts:\n- README.md".to_string(),
            steps: vec![PlanStep {
                id: "inspect".to_string(),
                kind: "inspect".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Inspect workspace".to_string(),
                expected_paths: Vec::new(),
                verify: Vec::new(),
            }],
        };
        let path = save_step_plan(dir.path(), &plan).unwrap();
        let mut fake = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Bash",
                    serde_json::json!({"command":"true"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("inspected"),
        ]);
        let err = run_plan_file(&mut fake, &path, &cfg)
            .unwrap_err()
            .to_string();
        assert!(err.contains("plan final contract failed"));
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"session_scope\":\"plan-run-step\""));
        assert!(event_text.contains("\"prompt_extracted_paths_enabled\":false"));
        assert!(event_text.contains("\"completion_contract_verification_enabled\":false"));
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

    fn generated_ultra_plan_yaml(goal: &str) -> String {
        render_ultra_plan(&UltraPlan {
            goal: goal.to_string(),
            profile: "generic".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "scaffold".to_string(),
                    prompt: format!("Create the required project artifacts for {goal}."),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "verify".to_string(),
                    prompt: format!(
                        "Run deterministic verification for {goal} and repair failures."
                    ),
                },
            ],
        })
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
