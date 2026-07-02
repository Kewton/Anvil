use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::eval_events;
use crate::minimal_loop::browser_probe::{
    BrowserReadinessObservation, probe_browser_readiness_with_offline,
};
use crate::minimal_loop::build_verifier::emit_dependency_build_lifecycle;
use crate::minimal_loop::completion::{CompletionContract, evidence_hint_tokens_for_goal};
use crate::minimal_loop::dependency_setup::{self, NodeDependencySetupAuthority};
use crate::minimal_loop::evidence::{
    RuntimeAcceptanceReport, required_evidence_for_capability,
    verify_runtime_acceptance_with_browser_dirs_and_hints,
};
use crate::minimal_loop::loop_run::{
    ContractEnforcement, RunSessionError, RunSessionOptions, RunSessionOutcome, RunSessionStepKind,
    extract_requested_artifact_paths, run_session_with_outcome_with_options,
};
use crate::minimal_loop::reachability::{
    RepairReachability, assess_repair_reachability, reachability_failure_kind,
    reachability_recovery_reason,
};
use crate::minimal_loop::repair_target::{
    RepairFollowThrough, classify_repair_follow_through, classify_repair_target,
};
use crate::minimal_loop::verifier_env;
use crate::planner::intent::detect_intent;
use crate::planner::lint::{
    PlanLintReport, PlanQualityContext, PlanQualityReport, lint_step_plan_report_with_workspace,
    lint_ultra_plan_report, step_plan_quality_report, step_plan_quality_warnings,
};
use crate::planner::profile::{
    PhaseVerificationMode, ProfileSnapshot, profile_auto_repair, profile_before_phase,
    profile_expected_paths, profile_generation_rules, profile_guidance, profile_post_step_repair,
    profile_quality_expectations, profile_repair_prompt, profile_runtime_contract,
    verify_profile_final, verify_profile_invariant,
};
use crate::planner::repair::{
    RecoveryHandoff, RepairContext, build_repair_prompt_with_context, save_recovery_ultra_plan,
    save_repair_report_with_context, save_ultra_recovery_prompt,
    suggested_recovery_ultra_plan_command, suggested_ultra_recovery_command,
    workspace_relative_handoff_path,
};
use crate::planner::step_plan::{
    PlanStep, StepKind, StepPlan, extract_json_object, parse_generated_step_plan_json,
    parse_step_plan, render_step_plan, repair_generated_step_plan_contract,
};
use crate::planner::ultra_plan::{UltraPhase, UltraPlan, parse_ultra_plan, render_ultra_plan};
use crate::planner::verify::{
    VerificationReport, verify_setup_dependency_state_with_setup_observed_with_offline,
    verify_step_with_setup_observed_with_offline,
};
use crate::providers::{ChatClient, model_for};
use crate::state::SessionSnapshot;
use crate::tools::path_guard::resolve_existing;
use crate::tui::status::UiStatus;
use crate::tui::{InteractionUi, NOOP_UI};
use serde_json::{Value, json};

const STEP_TURN_MAX_ITERATIONS: usize = 8;
const STEP_REPAIR_MAX_ITERATIONS: usize = 6;
const STEP_REPAIR_MAX_TURNS: usize = 4;
const STEP_REPAIR_IDENTICAL_NO_CHANGE_LIMIT: usize = 2;
const ULTRA_PLAN_GENERATION_ATTEMPTS: usize = 3;
const FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS: usize = 1;
const NEXTJS_DEV_SERVER_DEFAULT_PORT: u16 = 3011;
const NEXTJS_DEV_SERVER_READY_TIMEOUT: Duration = Duration::from_secs(8);
const NEXTJS_DEV_SERVER_CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const NEXTJS_DEV_SERVER_WAIT_INTERVAL: Duration = Duration::from_millis(250);
const DEV_SERVER_ROUTE: &str = "/";
const DEV_SERVER_LIFECYCLE_STAGES: [&str; 4] = ["start", "wait", "probe", "cleanup"];
const PROFILE_REPAIR_FILE_EXCERPT_MAX_CHARS: usize = 2_400;

#[derive(Debug, Clone)]
struct RecoveryArtifactValidation {
    prompt_exists: bool,
    prompt_parse_ok: bool,
    prompt_parse_error: Option<String>,
    yaml_exists: bool,
    yaml_parse_ok: bool,
    yaml_parse_error: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct ReleaseRecoveryHandoffSummary {
    recovery_handoff_kind: String,
    acceptance_layer: String,
    recovery_prompt_path: String,
    recovery_ultra_plan_path: String,
    suggested_recovery_command: String,
    suggested_recovery_yaml_command: String,
}

impl ReleaseRecoveryHandoffSummary {
    fn has_artifact(&self) -> bool {
        !self.recovery_prompt_path.is_empty() || !self.recovery_ultra_plan_path.is_empty()
    }
}

#[derive(Debug, Clone)]
struct BoundCompletionContract {
    contract: CompletionContract,
    path: String,
    fs_path: Option<PathBuf>,
    generated: bool,
    required: bool,
}

impl RecoveryArtifactValidation {
    fn prompt_command_available(&self) -> bool {
        self.prompt_exists && self.prompt_parse_ok
    }

    fn yaml_command_available(&self) -> bool {
        self.yaml_exists && self.yaml_parse_ok
    }

    fn command_targets_valid(&self) -> bool {
        self.prompt_command_available() && self.yaml_command_available()
    }
}

fn validate_recovery_artifacts(
    prompt_path: &Path,
    recovery_plan_path: Option<&Path>,
) -> RecoveryArtifactValidation {
    let prompt_result = validate_recovery_prompt(prompt_path);
    let (yaml_exists, yaml_parse_ok, yaml_parse_error) = match recovery_plan_path {
        Some(path) => {
            let exists = path.is_file();
            match validate_recovery_yaml(path) {
                Ok(()) => (exists, true, None),
                Err(err) => (exists, false, Some(err)),
            }
        }
        None => (false, false, Some("recovery_yaml_missing".to_string())),
    };
    RecoveryArtifactValidation {
        prompt_exists: prompt_path.is_file(),
        prompt_parse_ok: prompt_result.is_ok(),
        prompt_parse_error: prompt_result.err(),
        yaml_exists,
        yaml_parse_ok,
        yaml_parse_error,
    }
}

fn validate_recovery_prompt(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err("recovery_prompt_missing".to_string());
    }
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("recovery_prompt_unreadable: {}", err))?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("recovery_prompt_empty".to_string());
    }
    if trimmed.contains("Required recovery action:")
        && (trimmed.contains("Failure evidence:") || trimmed.contains("Primary failure:"))
    {
        return Ok(());
    }
    Err("recovery_prompt_missing_recovery_sections".to_string())
}

fn validate_recovery_yaml(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err("recovery_yaml_missing".to_string());
    }
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("recovery_yaml_unreadable: {}", err))?;
    let parsed =
        parse_ultra_plan(&text).map_err(|err| format!("recovery_yaml_parse_failed: {}", err))?;
    let rendered = render_ultra_plan(&parsed);
    let reparsed = parse_ultra_plan(&rendered)
        .map_err(|err| format!("recovery_yaml_roundtrip_parse_failed: {}", err))?;
    if reparsed != parsed {
        return Err("recovery_yaml_roundtrip_mismatch".to_string());
    }
    Ok(())
}

fn recovery_artifact_check_summary(validation: &RecoveryArtifactValidation) -> String {
    format!(
        "Recovery artifact check: prompt_parse_ok={}, yaml_parse_ok={}, command_targets_valid={}",
        validation.prompt_parse_ok,
        validation.yaml_parse_ok,
        validation.command_targets_valid()
    )
}

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
                let verify_before_repair = collect_step_verify_commands(&plan);
                repair_generated_step_plan_contract(&mut plan);
                emit_planner_verify_command_normalized(
                    config,
                    client.label(),
                    model,
                    attempt,
                    &verify_before_repair,
                    &collect_step_verify_commands(&plan),
                );
                strengthen_step_plan_for_profile(&mut plan, config);
                repair_generated_step_plan_contract(&mut plan);
                let lint_report =
                    lint_step_plan_report_with_workspace(&plan, Some(&config.workspace_root));
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
    run_step_plan_with_session_with_ui(
        client,
        &mut session,
        plan,
        config,
        ui,
        true,
        "plan-run",
        None,
    )
    .map(|outcome| outcome.summary)
    .map_err(|err| anyhow::anyhow!("{}", err.message))
}

#[derive(Debug, Clone, Default)]
struct StepPlanRunOutcome {
    summary: String,
    completed_steps: usize,
    total_steps: usize,
    changed_paths: Vec<String>,
    observed_missing_capabilities: Vec<String>,
    observed_missing_evidence: Vec<String>,
    observed_missing_obligations: Vec<String>,
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
    pending_capability_evidence: Vec<String>,
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
        self.merge_observed_contract_debt(outcome);
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

    fn merge_observed_contract_debt(&mut self, outcome: &StepPlanRunOutcome) {
        push_context_items_capped(
            &mut self.pending_capability_evidence,
            &outcome.observed_contract_keys(),
            ULTRA_CONTEXT_MAX_MESSAGES,
            &mut self.truncated,
        );
    }

    fn refresh_pending_capability_evidence(&mut self, report: &RuntimeAcceptanceReport) {
        let still_missing = runtime_missing_signals(report);
        self.pending_capability_evidence
            .retain(|item| still_missing.contains(item));
    }

    fn render_unmet_final_requirements_section(&self) -> String {
        let mut lines = vec!["Unmet final requirements from earlier phases:".to_string()];
        if self.pending_capability_evidence.is_empty() {
            lines.push("- none".to_string());
        } else {
            for item in &self.pending_capability_evidence {
                lines.push(format!("- {item}"));
            }
            lines.push(
                "Close these requirements when they are in scope for this phase.".to_string(),
            );
        }
        lines.join("\n")
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
            && self.pending_capability_evidence.is_empty()
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
            "Pending capability/evidence",
            &self.pending_capability_evidence,
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
        merge_unique_strings(
            &mut self.observed_missing_capabilities,
            &step.observed_missing_capabilities,
        );
        merge_unique_strings(
            &mut self.observed_missing_evidence,
            &step.observed_missing_evidence,
        );
        merge_unique_strings(
            &mut self.observed_missing_obligations,
            &step.observed_missing_obligations,
        );
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

    fn observed_contract_keys(&self) -> Vec<String> {
        let mut out = Vec::new();
        merge_unique_strings(&mut out, &self.observed_missing_capabilities);
        merge_unique_strings(&mut out, &self.observed_missing_evidence);
        merge_unique_strings(&mut out, &self.observed_missing_obligations);
        out
    }
}

#[derive(Debug, Clone, Default)]
struct StepRunOutcome {
    changed_paths: Vec<String>,
    observed_missing_capabilities: Vec<String>,
    observed_missing_evidence: Vec<String>,
    observed_missing_obligations: Vec<String>,
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

#[allow(clippy::result_large_err, clippy::too_many_arguments)]
fn run_step_plan_with_session_with_ui(
    client: &mut dyn ChatClient,
    session: &mut SessionSnapshot,
    plan: &StepPlan,
    config: &Config,
    ui: &dyn InteractionUi,
    verify_final_contract: bool,
    mode: &'static str,
    phase_scope: Option<&str>,
) -> Result<StepPlanRunOutcome, StepPlanRunError> {
    let mut outcome = StepPlanRunOutcome::for_plan(plan);
    let report = lint_step_plan_report_with_workspace(plan, Some(&config.workspace_root));
    if !report.is_pass() {
        emit_planner_error_for_lint(config, "plan-file", &config.planner_model, &report, 0);
        return Err(StepPlanRunError::from_error(
            report.primary_message(),
            outcome,
        ));
    }
    let required_final_artifacts = required_final_artifacts(plan, &config.workspace_root);
    let mut final_required_capabilities =
        inferred_required_capabilities(&config.profile, &plan.goal);
    let final_required_obligations =
        inferred_required_obligations(&config.profile, &plan.goal, &final_required_capabilities);
    let initial_required_evidence =
        inferred_required_evidence(&config.profile, &plan.goal, &final_required_capabilities);
    let bound_contract = bind_completion_contract_for_acceptance(
        config,
        "plan-run",
        &config.profile,
        &plan.goal,
        &required_final_artifacts,
        &final_required_capabilities,
        &initial_required_evidence,
        &final_required_obligations,
    )
    .map_err(|err| StepPlanRunError::from_error(err.to_string(), outcome.clone()))?;
    if let Some(contract) = bound_contract.as_ref().map(|bound| &bound.contract) {
        merge_unique_strings(
            &mut final_required_capabilities,
            &contract.required_capabilities,
        );
    }
    let mut final_required_evidence =
        inferred_required_evidence(&config.profile, &plan.goal, &final_required_capabilities);
    if let Some(contract) = bound_contract.as_ref().map(|bound| &bound.contract) {
        merge_unique_strings(
            &mut final_required_capabilities,
            &contract.required_capabilities,
        );
        merge_unique_strings(&mut final_required_evidence, &contract.required_evidence);
    }
    let contract_enforcement = if verify_final_contract {
        ContractEnforcement::Enforce
    } else {
        ContractEnforcement::Observe
    };
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
            completion_contract_path: bound_contract
                .as_ref()
                .and_then(|bound| bound.fs_path.clone()),
        };
        match run_step(
            client,
            session,
            plan,
            step,
            &prompt_context,
            config,
            ui,
            mode,
            contract_enforcement,
            phase_scope,
        ) {
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
    if verify_final_contract
        && let Err(err) = verify_plan_final_contract(
            plan,
            &required_final_artifacts,
            config,
            bound_contract.as_ref(),
        )
    {
        return Err(StepPlanRunError::from_error(err.to_string(), outcome));
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
    completion_contract_path: Option<PathBuf>,
}

#[allow(clippy::result_large_err, clippy::too_many_arguments)]
fn run_step(
    client: &mut dyn ChatClient,
    session: &mut SessionSnapshot,
    plan: &StepPlan,
    step: &PlanStep,
    prompt_context: &StepPromptContext,
    config: &Config,
    ui: &dyn InteractionUi,
    mode: &'static str,
    contract_enforcement: ContractEnforcement,
    phase_scope: Option<&str>,
) -> Result<StepRunOutcome, StepRunError> {
    let instruction = build_step_prompt(plan, step, prompt_context);
    emit_step_prompt_contract(config, step, prompt_context, &instruction);
    if step.step_kind() == StepKind::Report
        && step.expected_paths.is_empty()
        && step.verify.is_empty()
    {
        return Ok(StepRunOutcome::default());
    }
    let mut step_config = capped_config(config, STEP_TURN_MAX_ITERATIONS);
    if step.step_kind() == StepKind::Implement
        && let Some(path) = prompt_context.completion_contract_path.clone()
    {
        step_config.completion_contract_path = Some(path);
    }
    let setup_authority = step_verify_setup_authority(plan, step);
    let contract_setup_authority = step_contract_setup_authority(plan, step, phase_scope);
    let step_options = step_run_session_options(
        step,
        contract_enforcement,
        phase_scope,
        contract_setup_authority,
    );
    let initial = run_session_with_outcome_with_options(
        client,
        session,
        &instruction,
        &step.expected_paths,
        &step_config,
        ui,
        step_options.clone(),
    )
    .map_err(|err| {
        let message = err.to_string();
        StepRunError {
            outcome: step_run_outcome_from_session_error(&err, "initial_turn_error"),
            message,
        }
    })?;
    let mut outcome = StepRunOutcome {
        changed_paths: initial.changed_paths.clone(),
        observed_missing_capabilities: initial.missing_capabilities.clone(),
        observed_missing_evidence: initial.missing_evidence.clone(),
        observed_missing_obligations: initial.missing_obligations.clone(),
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
    let (mut report, mut build_lifecycles) = verify_step_with_setup_observed_with_offline(
        &config.workspace_root,
        step,
        setup_authority,
        config.offline,
    );
    if should_run_setup_step_dependency_state_lifecycle(
        &config.workspace_root,
        step,
        phase_scope,
        setup_authority,
    ) {
        let (dependency_report, mut dependency_lifecycles) =
            verify_setup_dependency_state_with_setup_observed_with_offline(
                &config.workspace_root,
                setup_authority,
                config.offline,
            );
        merge_verification_report(&mut report, dependency_report);
        build_lifecycles.append(&mut dependency_lifecycles);
    }
    for lifecycle in &build_lifecycles {
        emit_dependency_build_lifecycle(
            config.eval_events_path.as_deref(),
            mode,
            Some(&step.id),
            lifecycle,
        );
    }
    if report.is_pass() {
        return Ok(outcome);
    }
    let first_target = classify_repair_target(&report).as_str().to_string();
    let mut current_reachability =
        assess_repair_reachability(&report, None, setup_authority, config.offline);
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
            "repair_reachable": current_reachability.reachable,
            "reachable": current_reachability.reachable,
            "viable_actions": reachability_action_labels(&current_reachability),
            "blocked_requirements": current_reachability.blocked_requirements.clone(),
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
    let mut terminal_repair_failure_kind: Option<String> = None;
    let mut terminal_blocked_requirements: Vec<String> = Vec::new();
    let mut no_change_repairs = 0usize;
    let mut target_not_followed_repairs = 0usize;
    let mut identical_no_change_repairs = 0usize;
    let mut current_report_signature = verification_report_signature(&current_report);
    let repair_config = capped_config(config, STEP_REPAIR_MAX_ITERATIONS);
    if !current_reachability.reachable {
        terminal_repair_failure_kind =
            Some(reachability_failure_kind(&current_reachability).to_string());
        terminal_blocked_requirements = current_reachability.blocked_requirements.clone();
        context.progress_warning = Some(reachability_recovery_reason(&current_reachability));
        emit_repair_unreachable(
            config,
            mode,
            &step.id,
            classify_repair_target(&current_report).as_str(),
            &current_report.primary_reason(),
            &current_reachability,
        );
    }
    if current_reachability.reachable {
        for attempt in 1..=STEP_REPAIR_MAX_TURNS {
            context.repair_attempt = Some(attempt);
            let repair_prompt =
                build_repair_prompt_with_context(&step.id, &current_report, &context);
            let repair = run_session_with_outcome_with_options(
                client,
                session,
                &repair_prompt,
                &step.expected_paths,
                &repair_config,
                ui,
                step_options.clone(),
            )
            .map_err(|err| {
                let message = err.to_string();
                apply_session_error_observations(&mut outcome, &err, &message);
                outcome.primary_failure = Some(message.clone());
                outcome.stop_reason = Some("repair_turn_error".to_string());
                outcome.repair_attempts = attempt;
                outcome.partial = true;
                StepRunError {
                    message,
                    outcome: outcome.clone(),
                }
            })?;
            outcome.repair_attempts = attempt;
            repair_stop_reason = Some(format!("{:?}", repair.stop_reason));
            let changed_paths_before_repair = context.changed_files.clone();
            let mut repair_turn_changed_paths = repair.changed_paths.clone();
            merge_changed_files(&mut context, &repair.changed_paths);
            merge_unique_strings(&mut outcome.changed_paths, &repair.changed_paths);
            merge_unique_strings(
                &mut outcome.observed_missing_capabilities,
                &repair.missing_capabilities,
            );
            merge_unique_strings(
                &mut outcome.observed_missing_evidence,
                &repair.missing_evidence,
            );
            merge_unique_strings(
                &mut outcome.observed_missing_obligations,
                &repair.missing_obligations,
            );
            merge_unique_strings(&mut outcome.repair_changed_paths, &repair.changed_paths);
            match profile_post_step_repair(&config.workspace_root, &config.profile, &plan.goal) {
                Ok(true) => {
                    let package_path = "package.json".to_string();
                    merge_changed_files(&mut context, std::slice::from_ref(&package_path));
                    merge_unique_strings(
                        &mut outcome.changed_paths,
                        std::slice::from_ref(&package_path),
                    );
                    merge_unique_strings(&mut outcome.repair_changed_paths, &[package_path]);
                    merge_unique_strings(
                        &mut repair_turn_changed_paths,
                        &["package.json".to_string()],
                    );
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
            let (retry, retry_lifecycles) = verify_step_with_setup_observed_with_offline(
                &config.workspace_root,
                step,
                setup_authority,
                config.offline,
            );
            for lifecycle in &retry_lifecycles {
                emit_dependency_build_lifecycle(
                    config.eval_events_path.as_deref(),
                    mode,
                    Some(&step.id),
                    lifecycle,
                );
            }
            let retry_target = classify_repair_target(&retry);
            let retry_reachability =
                assess_repair_reachability(&retry, None, setup_authority, config.offline);
            let previous_target = classify_repair_target(&current_report);
            let repair_follow_through =
                classify_repair_follow_through(previous_target, &repair_turn_changed_paths);
            let repair_target_followed = repair_follow_through.followed();
            match repair_follow_through {
                RepairFollowThrough::NoChange => {
                    no_change_repairs += 1;
                }
                RepairFollowThrough::TargetNotFollowed | RepairFollowThrough::UnrelatedChange => {
                    target_not_followed_repairs += 1;
                    no_change_repairs = 0;
                }
                RepairFollowThrough::TargetMatched => {
                    no_change_repairs = 0;
                    target_not_followed_repairs = 0;
                }
            }
            let repair_failure_kind = repair_follow_through.failure_kind().unwrap_or("");
            let retry_signature = verification_report_signature(&retry);
            let report_signature_unchanged = retry_signature == current_report_signature;
            if report_signature_unchanged && repair_turn_changed_paths.is_empty() {
                identical_no_change_repairs += 1;
            } else {
                identical_no_change_repairs = 0;
            }
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
                    "target_relation": repair_follow_through.as_str(),
                    "repair_follow_through": repair_follow_through.as_str(),
                    "failure_kind": repair_failure_kind,
                    "primary_reason": eval_events::body_snippet(&retry.primary_reason()),
                    "changed_paths": repair_turn_changed_paths.clone(),
                    "changed_paths_before": changed_paths_before_repair,
                    "changed_paths_after": context.changed_files.clone(),
                    "repair_turn_changed_paths": repair_turn_changed_paths.clone(),
                    "no_change_repairs": no_change_repairs,
                    "target_not_followed_repairs": target_not_followed_repairs,
                    "identical_no_change_repairs": identical_no_change_repairs,
                    "report_signature_unchanged": report_signature_unchanged,
                    "allowed_action": previous_target.allowed_action(),
                    "repair_stop_reason": repair_stop_reason.clone().unwrap_or_default(),
                    "dependency_setup_authority": setup_authority.as_str(),
                    "repair_reachable": retry_reachability.reachable,
                    "reachable": retry_reachability.reachable,
                    "viable_actions": reachability_action_labels(&retry_reachability),
                    "blocked_requirements": retry_reachability.blocked_requirements.clone(),
                }),
            );
            if retry.is_pass() {
                outcome.primary_failure = None;
                outcome.stop_reason = repair_stop_reason.clone();
                return Ok(outcome);
            }
            current_reachability = retry_reachability;
            if !current_reachability.reachable {
                terminal_repair_failure_kind =
                    Some(reachability_failure_kind(&current_reachability).to_string());
                terminal_blocked_requirements = current_reachability.blocked_requirements.clone();
                context.progress_warning =
                    Some(reachability_recovery_reason(&current_reachability));
                current_report = retry;
                emit_repair_unreachable(
                    config,
                    mode,
                    &step.id,
                    classify_repair_target(&current_report).as_str(),
                    &current_report.primary_reason(),
                    &current_reachability,
                );
                break;
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
            current_report_signature = retry_signature;
            if identical_no_change_repairs >= STEP_REPAIR_IDENTICAL_NO_CHANGE_LIMIT {
                terminal_repair_failure_kind = Some("verify_repair_progress_unchanged".to_string());
                break;
            }
        }
    }
    let final_failure_kind =
        terminal_repair_failure_kind.unwrap_or_else(|| "bounded_repair_exhausted".to_string());
    context.repair_stop_reason = Some(final_failure_kind.to_string());
    let final_repair_target = classify_repair_target(&current_report);
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "loop_stop",
            "reason": final_failure_kind,
            "step_id": step.id,
            "repair_target": final_repair_target.as_str(),
            "repair_stop_reason": repair_stop_reason.clone().unwrap_or_default(),
            "local_repair_exhausted": true,
        }),
    );
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
    let step_handoff = RecoveryHandoff {
        profile: config.profile.clone(),
        original_goal: context
            .overall_goal
            .clone()
            .unwrap_or_else(|| plan.goal.clone()),
        failed_phase: None,
        failed_step: Some(step.id.clone()),
        failure_kind: final_failure_kind.to_string(),
        failure_evidence: std::iter::once(current_report.primary_reason())
            .chain(reachability_blocked_evidence(
                &terminal_blocked_requirements,
            ))
            .chain(
                current_report
                    .command_failures
                    .iter()
                    .map(|failure| format!("{}: {}", failure.command, failure.reason)),
            )
            .collect(),
        missing_paths: current_report.missing_paths.clone(),
        missing_capabilities: vec![final_repair_target.as_str().to_string()],
        verify_commands: context.verify_commands.clone(),
        changed_paths: context.changed_files.clone(),
        repair_targets: vec![final_repair_target.as_str().to_string()],
    };
    let recovery_plan_path =
        match save_recovery_ultra_plan(&config.workspace_root, &step.id, &step_handoff) {
            Ok(path) => Some(path),
            Err(err) => {
                let repair_report_display = handoff_path(&repair_report_path);
                eval_events::emit(
                    config.eval_events_path.as_deref(),
                    json!({
                        "event": "recovery_ultra_plan_save_failed",
                        "recovery_handoff_kind": final_failure_kind,
                        "step_id": step.id,
                        "recovery_prompt_path": repair_report_display,
                        "reason": eval_events::body_snippet(&err.to_string()),
                        "recovery_yaml_missing": true,
                    }),
                );
                None
            }
        };
    let validation =
        validate_recovery_artifacts(&repair_report_path, recovery_plan_path.as_deref());
    let raw_suggested_command =
        suggested_ultra_recovery_command(&repair_report_path, &config.profile);
    let suggested_command = if validation.prompt_command_available() {
        raw_suggested_command
    } else {
        String::new()
    };
    let suggested_yaml_command = recovery_plan_path
        .as_ref()
        .filter(|_| validation.yaml_command_available())
        .map(|path| suggested_recovery_ultra_plan_command(path));
    let repair_report_display = handoff_path(&repair_report_path);
    let recovery_plan_display = optional_handoff_path(recovery_plan_path.as_ref());
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "recovery_prompt_saved",
            "recovery_handoff_kind": final_failure_kind,
            "step_id": step.id,
            "recovery_prompt_path": &repair_report_display,
            "recovery_ultra_plan_path": &recovery_plan_display,
            "recovery_yaml_missing": recovery_plan_path.is_none(),
            "recovery_prompt_exists": validation.prompt_exists,
            "recovery_prompt_parse_ok": validation.prompt_parse_ok,
            "recovery_prompt_parse_error": validation.prompt_parse_error.as_deref().unwrap_or_default(),
            "recovery_yaml_exists": validation.yaml_exists,
            "recovery_yaml_parse_ok": validation.yaml_parse_ok,
            "recovery_yaml_parse_error": validation.yaml_parse_error.as_deref().unwrap_or_default(),
            "recovery_command_targets_valid": validation.command_targets_valid(),
            "suggested_recovery_command": suggested_command.clone(),
            "suggested_recovery_yaml_command": suggested_yaml_command.clone().unwrap_or_default(),
            "recovery_profile": config.profile,
            "local_repair_exhausted": true,
            "failure_kind": final_failure_kind,
            "status": "incomplete",
        }),
    );
    let yaml_summary = recovery_plan_path
        .as_ref()
        .map(|path| {
            let display = handoff_path(path);
            if validation.yaml_parse_ok {
                format!("Recovery UltraPlan YAML saved: {display}")
            } else {
                format!(
                    "Recovery UltraPlan YAML invalid: {} ({})",
                    display,
                    validation
                        .yaml_parse_error
                        .as_deref()
                        .unwrap_or("recovery_yaml_invalid")
                )
            }
        })
        .unwrap_or_else(|| {
            "Recovery UltraPlan YAML missing: failed to save valid recovery plan".to_string()
        });
    let prompt_command_summary = if validation.prompt_command_available() {
        format!("Suggested command: {suggested_command}")
    } else {
        format!(
            "Suggested command: unavailable because recovery prompt validation failed ({})",
            validation
                .prompt_parse_error
                .as_deref()
                .unwrap_or("recovery_prompt_invalid")
        )
    };
    let yaml_command_summary = suggested_yaml_command
        .as_ref()
        .map(|command| format!("Suggested YAML command: {command}"))
        .unwrap_or_else(|| {
            "Suggested YAML command: unavailable because recovery YAML is missing".to_string()
        });
    let artifact_check_summary = recovery_artifact_check_summary(&validation);
    eval_events::write_run_summary(
        config.eval_events_path.as_deref(),
        &format!(
            "Status: incomplete\n{}\n{}\nRecovery prompt saved: {}\n{}\n{}\nFailure: {}",
            yaml_summary,
            yaml_command_summary,
            repair_report_display,
            prompt_command_summary,
            artifact_check_summary,
            eval_events::body_snippet(&current_report.primary_reason())
        ),
    );
    let yaml_message = recovery_plan_path
        .as_ref()
        .zip(suggested_yaml_command.as_ref())
        .map(|(path, command)| {
            let display = handoff_path(path);
            format!(
                "; incomplete; recovery YAML saved: {}; suggested YAML command: {}",
                display, command
            )
        })
        .unwrap_or_else(|| "; incomplete; recovery YAML missing".to_string());
    let prompt_message = if validation.prompt_command_available() {
        format!("suggested command: {suggested_command}")
    } else {
        "suggested command unavailable because recovery prompt validation failed".to_string()
    };
    let message = format!(
        "step {} failed verification after bounded repair: {}; repair prompt saved: {}; {}; {}; {}",
        step.id,
        current_report.primary_reason(),
        repair_report_display,
        prompt_message,
        yaml_message.trim_start_matches("; "),
        artifact_check_summary
    );
    outcome.primary_failure = Some(current_report.primary_reason());
    outcome.stop_reason = Some(final_failure_kind.to_string());
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

fn step_contract_setup_authority(
    _plan: &StepPlan,
    step: &PlanStep,
    phase_scope: Option<&str>,
) -> NodeDependencySetupAuthority {
    if step.step_kind() != StepKind::Implement {
        return NodeDependencySetupAuthority::None;
    }
    if step_or_phase_is_dependency_setup_purpose(step, phase_scope) {
        NodeDependencySetupAuthority::PlanSetupStep
    } else {
        NodeDependencySetupAuthority::None
    }
}

fn step_or_phase_is_dependency_setup_purpose(step: &PlanStep, phase_scope: Option<&str>) -> bool {
    [
        step.id.as_str(),
        step.kind.as_str(),
        step.instruction.as_str(),
    ]
    .into_iter()
    .chain(phase_scope)
    .any(text_mentions_dependency_setup)
}

fn text_mentions_dependency_setup(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    (lower.contains("setup") || lower.contains("install"))
        && (lower.contains("depend") || lower.contains("workspace") || lower.contains("package"))
}

fn should_run_setup_step_dependency_state_lifecycle(
    root: &Path,
    step: &PlanStep,
    phase_scope: Option<&str>,
    setup_authority: NodeDependencySetupAuthority,
) -> bool {
    step.step_kind() == StepKind::Setup
        && setup_authority == NodeDependencySetupAuthority::PlanSetupStep
        && step_or_phase_is_dependency_setup_purpose(step, phase_scope)
        && dependency_setup::package_json_declares_dependencies(root)
        && !dependency_setup::node_declared_dependencies_ready(root)
}

fn merge_verification_report(report: &mut VerificationReport, extra: VerificationReport) {
    for path in extra.missing_paths {
        report.push_missing_path(path);
    }
    for reason in extra.dependency_missing {
        report.push_dependency_missing(reason);
    }
    for failure in extra.command_failures {
        report.push_command_failure(failure.command, failure.reason);
    }
    for reason in extra.profile_failures {
        report.push_profile_failure(reason);
    }
}

fn step_run_session_options(
    step: &PlanStep,
    contract_enforcement: ContractEnforcement,
    phase_scope: Option<&str>,
    setup_authority: NodeDependencySetupAuthority,
) -> RunSessionOptions {
    RunSessionOptions::plan_step_with_enforcement(
        run_session_step_kind(step),
        contract_enforcement,
        phase_scope.map(str::to_string),
    )
    .with_dependency_setup_authority(setup_authority)
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

#[allow(clippy::too_many_arguments)]
fn bind_completion_contract_for_acceptance(
    config: &Config,
    scope: &str,
    profile: &str,
    goal: &str,
    required_paths: &[String],
    required_capabilities: &[String],
    required_evidence: &[String],
    required_obligations: &[String],
) -> anyhow::Result<Option<BoundCompletionContract>> {
    let required = completion_contract_required(profile, goal, required_capabilities);
    if let Some(contract) = CompletionContract::load_for_config(config)? {
        let mut contract = contract;
        contract.merge_evidence_hint_tokens_from_goal(goal);
        let path = explicit_completion_contract_path(config)
            .map(|path| display_path_for_event(&config.workspace_root, &path))
            .unwrap_or_else(|| "<inline-config>".to_string());
        let fs_path = explicit_completion_contract_path(config);
        let bound = BoundCompletionContract {
            contract,
            path,
            fs_path,
            generated: false,
            required,
        };
        emit_completion_contract_bound(config, scope, &bound);
        return Ok(Some(bound));
    }
    if !required {
        return Ok(None);
    }
    let contract = CompletionContract {
        required_paths: required_paths.to_vec(),
        verify_commands: Vec::new(),
        profile: None,
        goal: Some(goal.to_string()),
        required_capabilities: required_capabilities.to_vec(),
        deterministic_oracles: Vec::new(),
        required_evidence: required_evidence.to_vec(),
        evidence_hint_tokens: evidence_hint_tokens_for_goal(goal),
        required_obligations: required_obligations.to_vec(),
        deferred_verify_requirements: Vec::new(),
        verify_repair_cap: 2,
    }
    .validate(&config.workspace_root)?;
    let path = generated_completion_contract_path(config, scope);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(&contract)?;
    std::fs::write(&path, format!("{text}\n"))?;
    let bound = BoundCompletionContract {
        contract,
        path: display_path_for_event(&config.workspace_root, &path),
        fs_path: Some(path),
        generated: true,
        required,
    };
    emit_completion_contract_bound(config, scope, &bound);
    Ok(Some(bound))
}

fn completion_contract_required(
    profile: &str,
    goal: &str,
    required_capabilities: &[String],
) -> bool {
    let profile = profile.to_ascii_lowercase();
    let goal = goal.to_ascii_lowercase();
    let web_or_app_profile = matches!(
        profile.as_str(),
        "nextjs" | "next-js" | "next.js" | "vite" | "react" | "web"
    ) || profile.contains("next");
    let interactive_goal = [
        "interactive",
        "app",
        "game",
        "playable",
        "browser",
        "canvas",
        "keyboard",
        "player",
        "enemy",
        "collision",
        "ゲーム",
    ]
    .iter()
    .any(|needle| goal.contains(needle));
    let interactive_capability = required_capabilities.iter().any(|capability| {
        matches!(
            capability.as_str(),
            "stateful_interaction"
                | "start_or_restart_flow"
                | "player_control"
                | "adversary_or_challenge"
                | "progression_or_score"
                | "failure_or_collision_rule"
                | "browser_interaction"
                | "playable_ui"
        )
    });
    interactive_capability || (web_or_app_profile && interactive_goal)
}

fn explicit_completion_contract_path(config: &Config) -> Option<PathBuf> {
    config
        .completion_contract_path
        .clone()
        .or_else(|| std::env::var_os("ANVIL_COMPLETION_CONTRACT").map(PathBuf::from))
}

fn generated_completion_contract_path(config: &Config, scope: &str) -> PathBuf {
    let filename = format!(
        "completion-contract-{}.json",
        scope
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
            .collect::<String>()
    );
    if let Some(parent) = config
        .eval_events_path
        .as_ref()
        .and_then(|path| path.parent())
    {
        return parent.join(filename);
    }
    config.workspace_root.join(".anvil").join(filename)
}

fn display_path_for_event(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

fn emit_completion_contract_bound(config: &Config, scope: &str, bound: &BoundCompletionContract) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "completion_contract_bound",
            "session_scope": scope,
            "completion_contract_verification_enabled": true,
            "completion_contract_path_merge_enabled": true,
            "external_contract_checked": true,
            "external_contract_required": bound.required,
            "completion_contract_generated": bound.generated,
            "completion_contract_path": bound.path,
            "required_paths": bound.contract.required_paths.clone(),
            "required_capabilities": bound.contract.required_capabilities.clone(),
            "required_evidence": bound.contract.required_evidence.clone(),
            "evidence_hint_tokens": bound.contract.evidence_hint_tokens.clone(),
            "required_obligations": bound.contract.required_obligations.clone(),
        }),
    );
}

fn verify_plan_final_contract(
    plan: &StepPlan,
    required_final_artifacts: &[String],
    config: &Config,
    bound_contract: Option<&BoundCompletionContract>,
) -> anyhow::Result<()> {
    let mut required_paths = required_final_artifacts.to_vec();
    let mut required_capabilities = inferred_required_capabilities(&config.profile, &plan.goal);
    let mut required_evidence =
        inferred_required_evidence(&config.profile, &plan.goal, &required_capabilities);
    let mut required_obligations =
        inferred_required_obligations(&config.profile, &plan.goal, &required_capabilities);
    let mut evidence_hint_tokens = evidence_hint_tokens_for_goal(&plan.goal);
    let owned_bound_contract;
    let bound_contract = if let Some(bound_contract) = bound_contract {
        Some(bound_contract)
    } else {
        owned_bound_contract = bind_completion_contract_for_acceptance(
            config,
            "plan-run",
            &config.profile,
            &plan.goal,
            &required_paths,
            &required_capabilities,
            &required_evidence,
            &required_obligations,
        )?;
        owned_bound_contract.as_ref()
    };
    let mut verify_commands = Vec::new();
    let mut deferred_commands = Vec::new();
    if let Some(bound) = bound_contract {
        let contract = &bound.contract;
        merge_unique_strings(&mut required_paths, &contract.required_paths);
        merge_unique_strings(&mut required_capabilities, &contract.required_capabilities);
        merge_unique_strings(&mut required_evidence, &contract.required_evidence);
        merge_unique_strings(&mut required_obligations, &contract.required_obligations);
        merge_unique_strings(&mut verify_commands, &contract.verify_commands);
        merge_unique_strings(&mut evidence_hint_tokens, &contract.evidence_hint_tokens);
        deferred_commands.extend(
            contract
                .deferred_verify_requirements
                .iter()
                .map(|requirement| requirement.command.clone()),
        );
    }
    merge_unique_strings(
        &mut required_evidence,
        &inferred_required_evidence(&config.profile, &plan.goal, &required_capabilities),
    );
    let missing_final_artifacts = missing_final_artifacts(&config.workspace_root, &required_paths);
    let external_report = bound_contract.map(|bound| {
        bound
            .contract
            .verify_with_goal(&config.workspace_root, &plan.goal)
    });
    let runtime_acceptance_required = !required_capabilities.is_empty()
        || !required_evidence.is_empty()
        || !required_obligations.is_empty();
    let runtime_acceptance = runtime_acceptance_required.then(|| {
        verify_runtime_acceptance_with_browser_dirs_and_hints(
            &config.workspace_root,
            &required_paths,
            &verify_commands,
            &required_capabilities,
            &required_evidence,
            &required_obligations,
            &deferred_commands,
            &release_evidence_extra_dirs(config),
            &evidence_hint_tokens,
        )
    });
    let release_gate = final_acceptance_release_gate(
        config,
        &config.profile,
        &plan.goal,
        &required_capabilities,
        runtime_acceptance.as_ref(),
    );
    let contract_required =
        completion_contract_required(&config.profile, &plan.goal, &required_capabilities)
            || bound_contract.is_some_and(|bound| bound.required);
    let external_contract_checked = bound_contract.is_some();
    let contract_binding_missing = contract_required && !external_contract_checked;
    let external_ok = !contract_binding_missing
        && external_report
            .as_ref()
            .is_none_or(|report| report.is_pass());
    let runtime_ok = runtime_acceptance
        .as_ref()
        .is_none_or(|report| report.passed);
    let release_gate_failed = release_gate.status == "failed";
    let ok =
        missing_final_artifacts.is_empty() && external_ok && runtime_ok && !release_gate_failed;
    let final_acceptance_status = release_gate_final_acceptance_status(&release_gate);
    let runtime_acceptance_status =
        runtime_acceptance_status(runtime_ok, runtime_acceptance.as_ref());
    let release_quality_completion =
        release_quality_completion_status(&release_gate, final_acceptance_status);
    let next_action = release_gate_next_action(&release_gate, final_acceptance_status);
    let primary_reason = if !missing_final_artifacts.is_empty() {
        format!(
            "missing final artifacts: {}",
            missing_final_artifacts.join(", ")
        )
    } else if contract_binding_missing {
        "completion contract binding required but missing".to_string()
    } else if let Some(report) = runtime_acceptance.as_ref().filter(|report| !report.passed) {
        report.primary_reason.clone()
    } else if let Some(report) = external_report.as_ref().filter(|report| !report.is_pass()) {
        report.primary_reason()
    } else if release_gate_failed {
        format!("release gate failed: {}", release_gate.reasons.join("; "))
    } else {
        "ok".to_string()
    };
    let recovery_handoff = if !ok || release_recovery_needed(&release_gate, final_acceptance_status)
    {
        let acceptance_layer =
            release_recovery_acceptance_layer(&release_gate, final_acceptance_status);
        let failure_kind =
            release_recovery_failure_kind(&release_gate, final_acceptance_status, &primary_reason);
        let scope = format!("release-{}", recovery_scope_token(acceptance_layer));
        save_release_recovery_handoff(
            config,
            &config.profile,
            &plan.goal,
            &scope,
            acceptance_layer,
            &failure_kind,
            release_recovery_failure_evidence(
                &release_gate,
                final_acceptance_status,
                &primary_reason,
                runtime_acceptance.as_ref(),
            ),
            missing_final_artifacts.clone(),
            release_recovery_missing_capabilities(runtime_acceptance.as_ref()),
            release_recovery_repair_targets(&release_gate, runtime_acceptance.as_ref()),
            release_recovery_verify_commands(&config.profile, &release_gate),
        )
    } else {
        None
    };
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "plan_final_contract",
            "required_final_artifacts": required_paths,
            "missing_final_artifacts": missing_final_artifacts,
            "completion_contract_verification_enabled": external_contract_checked,
            "completion_contract_path_merge_enabled": external_contract_checked,
            "completion_contract_path": bound_contract
                .map(|bound| bound.path.clone())
                .unwrap_or_default(),
            "completion_contract_generated": bound_contract
                .map(|bound| bound.generated)
                .unwrap_or(false),
            "external_contract_checked": external_contract_checked,
            "external_contract_required": contract_required,
            "external_contract_ok": external_ok,
            "required_capabilities": required_capabilities,
            "required_evidence": required_evidence,
            "required_obligations": required_obligations,
            "missing_capabilities": runtime_acceptance
                .as_ref()
                .map(|report| report.missing_capabilities.clone())
                .unwrap_or_default(),
            "missing_evidence": runtime_acceptance
                .as_ref()
                .map(|report| report.missing_evidence.clone())
                .unwrap_or_default(),
            "missing_obligations": runtime_acceptance
                .as_ref()
                .map(|report| report.missing_obligations.clone())
                .unwrap_or_default(),
            "weak_evidence": runtime_acceptance
                .as_ref()
                .map(|report| report.weak_evidence.clone())
                .unwrap_or_default(),
            "artifact_obligations": runtime_acceptance
                .as_ref()
                .map(|report| report.artifact_obligations.clone())
                .unwrap_or_default(),
            "capability_evidence_bindings": runtime_acceptance
                .as_ref()
                .map(|report| report.capability_evidence_bindings.clone())
                .unwrap_or_default(),
            "obligation_repair_targets": runtime_acceptance
                .as_ref()
                .map(|report| report.obligation_repair_targets.clone())
                .unwrap_or_default(),
            "inconclusive_reasons": runtime_acceptance
                .as_ref()
                .map(|report| report.inconclusive_reasons.clone())
                .unwrap_or_default(),
            "runtime_acceptance_inconclusive": runtime_acceptance
                .as_ref()
                .map(|report| report.inconclusive)
                .unwrap_or(false),
            "runtime_acceptance_passed": runtime_ok,
            "runtime_acceptance_status": runtime_acceptance_status,
            "final_acceptance_status": final_acceptance_status,
            "release_quality_completion": release_quality_completion,
            "release_gate_status": release_gate.status.clone(),
            "release_gate_reasons": release_gate.reasons.clone(),
            "browser_readiness_status": release_gate.browser_readiness_status.clone(),
            "browser_readiness_evidence_path": release_gate.browser_readiness_evidence_path.clone(),
            "interaction_evidence_status": release_gate.interaction_evidence_status.clone(),
            "interaction_evidence_path": release_gate.interaction_evidence_path.clone(),
            "next_action": next_action,
            "recovery_handoff_kind": recovery_handoff
                .as_ref()
                .map(|handoff| handoff.recovery_handoff_kind.as_str())
                .unwrap_or_default(),
            "acceptance_layer": recovery_handoff
                .as_ref()
                .map(|handoff| handoff.acceptance_layer.as_str())
                .unwrap_or_default(),
            "recovery_prompt_path": recovery_handoff
                .as_ref()
                .map(|handoff| handoff.recovery_prompt_path.as_str())
                .unwrap_or_default(),
            "recovery_ultra_plan_path": recovery_handoff
                .as_ref()
                .map(|handoff| handoff.recovery_ultra_plan_path.as_str())
                .unwrap_or_default(),
            "suggested_recovery_command": recovery_handoff
                .as_ref()
                .map(|handoff| handoff.suggested_recovery_command.as_str())
                .unwrap_or_default(),
            "suggested_recovery_yaml_command": recovery_handoff
                .as_ref()
                .map(|handoff| handoff.suggested_recovery_yaml_command.as_str())
                .unwrap_or_default(),
            "recovery_handoff_saved": recovery_handoff
                .as_ref()
                .is_some_and(ReleaseRecoveryHandoffSummary::has_artifact),
            "handoff_saved_not_success": recovery_handoff.is_some(),
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

fn step_run_outcome_from_session_error(err: &anyhow::Error, stop_reason: &str) -> StepRunOutcome {
    let message = err.to_string();
    let mut outcome = StepRunOutcome {
        primary_failure: Some(message.clone()),
        stop_reason: Some(stop_reason.to_string()),
        partial: true,
        ..StepRunOutcome::default()
    };
    apply_session_error_observations(&mut outcome, err, &message);
    outcome
}

fn apply_session_error_observations(
    outcome: &mut StepRunOutcome,
    err: &anyhow::Error,
    message: &str,
) {
    if let Some(session_error) = err.downcast_ref::<RunSessionError>() {
        merge_unique_strings(
            &mut outcome.observed_missing_capabilities,
            &session_error.context.missing_capabilities,
        );
        merge_unique_strings(
            &mut outcome.observed_missing_evidence,
            &session_error.context.missing_evidence,
        );
        merge_unique_strings(
            &mut outcome.observed_missing_obligations,
            &session_error.context.missing_obligations,
        );
        if let Some(repair_target) = session_error.context.repair_target.as_ref() {
            merge_unique_strings(
                &mut outcome.repair_targets,
                std::slice::from_ref(repair_target),
            );
        }
    }

    let missing_capabilities =
        missing_signal_values_after_prefix(message, "missing_required_capabilities:");
    let missing_evidence =
        missing_signal_values_after_prefix(message, "missing_required_evidence:");
    let missing_obligations =
        missing_signal_values_after_prefix(message, "missing_required_obligations:");
    merge_unique_strings(
        &mut outcome.observed_missing_capabilities,
        &missing_capabilities,
    );
    merge_unique_strings(&mut outcome.observed_missing_evidence, &missing_evidence);
    merge_unique_strings(
        &mut outcome.observed_missing_obligations,
        &missing_obligations,
    );
    ensure_session_error_repair_target(outcome);
}

fn ensure_session_error_repair_target(outcome: &mut StepRunOutcome) {
    if !outcome.repair_targets.is_empty() {
        return;
    }
    if !outcome.observed_missing_evidence.is_empty()
        || !outcome.observed_missing_obligations.is_empty()
    {
        outcome
            .repair_targets
            .push("required_evidence_missing".to_string());
    } else if !outcome.observed_missing_capabilities.is_empty() {
        outcome
            .repair_targets
            .push("capability_missing".to_string());
    }
}

fn runtime_missing_signals(report: &RuntimeAcceptanceReport) -> Vec<String> {
    let mut out = Vec::new();
    merge_unique_strings(&mut out, &report.missing_capabilities);
    merge_unique_strings(&mut out, &report.missing_evidence);
    merge_unique_strings(&mut out, &report.missing_obligations);
    out
}

fn verification_missing_signals(report: &VerificationReport) -> Vec<String> {
    let mut out = Vec::new();
    merge_unique_strings(
        &mut out,
        &missing_signals_from_text(&report.primary_reason()),
    );
    for failure in &report.profile_failures {
        merge_unique_strings(&mut out, &missing_signals_from_text(failure));
    }
    out
}

fn missing_signals_from_text(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    merge_unique_strings(
        &mut out,
        &missing_signal_values_after_prefix(text, "missing_required_capabilities:"),
    );
    merge_unique_strings(
        &mut out,
        &missing_signal_values_after_prefix(text, "missing_required_evidence:"),
    );
    merge_unique_strings(
        &mut out,
        &missing_signal_values_after_prefix(text, "missing_required_obligations:"),
    );
    merge_unique_strings(&mut out, &missing_obligation_targets_from_text(text));
    out
}

fn missing_signal_values_after_prefix(text: &str, prefix: &str) -> Vec<String> {
    let Some((_, rest)) = text.split_once(prefix) else {
        return Vec::new();
    };
    let end = rest
        .find(|ch: char| ch.is_whitespace() || matches!(ch, ';' | ')' | ']'))
        .unwrap_or(rest.len());
    rest[..end]
        .split(',')
        .filter_map(|value| {
            let value = value
                .trim()
                .trim_matches(|ch: char| matches!(ch, '.' | ':'));
            let value = value
                .strip_prefix("required_obligation:")
                .unwrap_or(value)
                .trim();
            (!value.is_empty()).then(|| value.to_string())
        })
        .collect()
}

fn missing_obligation_targets_from_text(text: &str) -> Vec<String> {
    let Some((_, rest)) = text.split_once("missing_required_obligation_target:") else {
        return Vec::new();
    };
    let end = rest
        .find(|ch: char| ch.is_whitespace() || matches!(ch, ';' | ')' | ']'))
        .unwrap_or(rest.len());
    rest[..end]
        .split(',')
        .filter_map(|value| value.split(':').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn handoff_path(path: &Path) -> String {
    workspace_relative_handoff_path(path)
}

fn optional_handoff_path(path: Option<&PathBuf>) -> String {
    path.map(|path| handoff_path(path.as_path()))
        .unwrap_or_default()
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

fn reachability_action_labels(reachability: &RepairReachability) -> Vec<&'static str> {
    reachability
        .viable_actions
        .iter()
        .map(|action| action.as_str())
        .collect()
}

fn reachability_blocked_evidence(blocked_requirements: &[String]) -> Vec<String> {
    blocked_requirements
        .iter()
        .map(|requirement| match requirement.as_str() {
            "dependency_setup_authority_required" => {
                "dependency_setup_authority_required: requires a Setup-authority step running dependency install before verification can pass".to_string()
            }
            "dependency_setup_blocked_offline" => {
                "dependency_setup_blocked_offline: dependency verification requires dependency setup lifecycle, but offline mode blocks install".to_string()
            }
            other => format!("repair_unreachable: {other}"),
        })
        .collect()
}

fn emit_repair_unreachable(
    config: &Config,
    mode: &str,
    step_id: &str,
    repair_target: &str,
    primary_reason: &str,
    reachability: &RepairReachability,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "repair_unreachable",
            "mode": mode,
            "step_id": step_id,
            "reason": reachability_failure_kind(reachability),
            "blocked_requirements": reachability.blocked_requirements.clone(),
            "viable_actions": reachability_action_labels(reachability),
            "repair_target": repair_target,
            "primary_reason": eval_events::body_snippet(primary_reason),
        }),
    );
}

fn verification_report_signature(report: &VerificationReport) -> Vec<String> {
    let mut signature = Vec::new();
    signature.extend(
        report
            .missing_paths
            .iter()
            .map(|path| format!("missing:{path}")),
    );
    signature.extend(
        report
            .dependency_missing
            .iter()
            .map(|reason| format!("dependency:{reason}")),
    );
    signature.extend(report.command_failures.iter().map(|failure| {
        format!(
            "command:{}:{}",
            failure.command,
            normalize_report_reason_for_signature(&failure.reason)
        )
    }));
    signature.extend(
        report
            .profile_failures
            .iter()
            .map(|reason| format!("profile:{reason}")),
    );
    signature.sort();
    signature
}

fn normalize_report_reason_for_signature(reason: &str) -> String {
    let mut normalized = Vec::new();
    let mut parts = reason.split_whitespace();
    while let Some(part) = parts.next() {
        if part == "elapsed_ms:" {
            let _ = parts.next();
            normalized.push("elapsed_ms:<n>".to_string());
        } else {
            normalized.push(part.to_string());
        }
    }
    normalized.join(" ")
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
                    UltraPhaseRecoveryRequest {
                        failure_kind: "phase_scaffold_error",
                        reason: &message,
                        missing_paths: &missing_final_artifacts(
                            &config.workspace_root,
                            &final_expected_paths,
                        ),
                        missing_signals: &[],
                        repair_targets: &["phase_scaffold".to_string()],
                    },
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
            "ultra-plan-run",
            Some(&phase.id),
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
                let missing_signals = err.partial_outcome.observed_contract_keys();
                let handoff = save_ultra_phase_recovery_handoff(
                    config,
                    plan,
                    phase,
                    UltraPhaseRecoveryRequest {
                        failure_kind: "phase_execute_error",
                        reason: &message,
                        missing_paths: &missing_final_artifacts(
                            &config.workspace_root,
                            &final_expected_paths,
                        ),
                        missing_signals: &missing_signals,
                        repair_targets: &err.partial_outcome.repair_targets,
                    },
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
        if !final_phase {
            let acceptance = ultra_contract_runtime_acceptance_report(plan, config)?;
            ultra_context.refresh_pending_capability_evidence(&acceptance);
        }
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
        let mut invariant_report = verify_profile_invariant(
            &config.workspace_root,
            &plan.profile,
            &plan.goal,
            &profile_snapshot,
        );
        if !final_phase && !invariant_report.is_pass() {
            invariant_report = repair_intermediate_profile_invariant(
                execution,
                &mut ultra_session,
                config,
                plan,
                phase,
                index,
                &profile_snapshot,
                &step_plan,
                &mut ultra_context,
                &final_expected_paths,
                ui,
                invariant_report,
            )?;
        }
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
                    UltraPhaseRecoveryRequest {
                        failure_kind: "profile_invariant_failure",
                        reason: &invariant_report.primary_reason(),
                        missing_paths: &invariant_report.missing_paths,
                        missing_signals: &verification_missing_signals(&invariant_report),
                        repair_targets: &["profile_contract".to_string()],
                    },
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
                let auto_repair_target = classify_repair_target(&profile_report);
                eval_events::emit(
                    config.eval_events_path.as_deref(),
                    json!({
                        "event": "deterministic_scaffold_recovery",
                        "lifecycle_stage": "profile_auto_repair",
                        "phase_id": phase.id,
                        "fallback_kind": "nextjs_profile_auto_repair",
                        "repair_target": auto_repair_target.as_str(),
                        "original_failure": eval_events::body_snippet(&profile_report.primary_reason()),
                        "used_for_completion": false,
                        "requires_continuation": true,
                        "summary": "deterministic profile repair materialized recovery scaffold; final success still requires implementation continuation and acceptance verification",
                    }),
                );
                let expected_paths =
                    profile_expected_paths(&config.workspace_root, &plan.profile, &plan.goal);
                let continuation_prompt = profile_auto_repair_continuation_prompt(
                    plan,
                    phase,
                    &profile_report,
                    &ultra_context,
                    &expected_paths,
                );
                let repair_config = capped_config(config, STEP_REPAIR_MAX_ITERATIONS);
                eval_events::emit(
                    config.eval_events_path.as_deref(),
                    json!({
                        "event": "profile_auto_repair_continuation_start",
                        "phase_id": phase.id,
                        "shared_execution_session": true,
                        "bounded_repair": true,
                        "max_iterations": repair_config.max_iterations,
                        "expected_path_count": expected_paths.len(),
                    }),
                );
                let continuation_outcome = run_session_with_outcome_with_options(
                    execution,
                    &mut ultra_session,
                    &continuation_prompt,
                    &expected_paths,
                    &repair_config,
                    ui,
                    RunSessionOptions::plan_step(RunSessionStepKind::Implement),
                )
                .map_err(|err| {
                    let message = err.to_string();
                    let handoff = save_ultra_phase_recovery_handoff(
                        config,
                        plan,
                        phase,
                        UltraPhaseRecoveryRequest {
                            failure_kind: "profile_auto_repair_continuation_failed",
                            reason: &message,
                            missing_paths: &missing_final_artifacts(
                                &config.workspace_root,
                                &final_expected_paths,
                            ),
                            missing_signals: &missing_signals_from_text(&message),
                            repair_targets: &["profile_contract".to_string()],
                        },
                    )
                    .unwrap_or_default();
                    anyhow::anyhow!(
                        "phase {} profile auto repair continuation failed: {message}{handoff}",
                        phase.id
                    )
                })?;
                push_context_items_capped(
                    &mut ultra_context.created_or_changed_paths,
                    &continuation_outcome.changed_paths,
                    ULTRA_CONTEXT_MAX_PATHS,
                    &mut ultra_context.truncated,
                );
                push_context_items_capped(
                    &mut ultra_context.last_repair_changed_paths,
                    &continuation_outcome.changed_paths,
                    ULTRA_CONTEXT_MAX_PATHS,
                    &mut ultra_context.truncated,
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
                eval_events::emit(
                    config.eval_events_path.as_deref(),
                    json!({
                        "event": "profile_auto_repair_continuation_complete",
                        "phase_id": phase.id,
                        "shared_execution_session": true,
                        "changed_path_count": continuation_outcome.changed_paths.len(),
                        "iterations": continuation_outcome.iterations,
                        "tool_calls": continuation_outcome.tool_calls,
                    }),
                );
                let retry = verify_profile_final(&config.workspace_root, &plan.profile, &plan.goal);
                let continuation_followed = classify_repair_follow_through(
                    auto_repair_target,
                    &continuation_outcome.changed_paths,
                )
                .followed();
                if retry.is_pass() && continuation_followed {
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
                if retry.is_pass() {
                    let follow_through = classify_repair_follow_through(
                        auto_repair_target,
                        &continuation_outcome.changed_paths,
                    );
                    eval_events::emit(
                        config.eval_events_path.as_deref(),
                        json!({
                            "event": "profile_auto_repair_continuation_incomplete",
                            "phase_id": phase.id,
                            "shared_execution_session": true,
                            "reason": "deterministic_fallback_without_targeted_implementation_continuation",
                            "repair_target": auto_repair_target.as_str(),
                            "repair_follow_through": follow_through.as_str(),
                            "changed_path_count": continuation_outcome.changed_paths.len(),
                            "used_for_completion": false,
                        }),
                    );
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
                let repair_config = capped_config(config, STEP_REPAIR_MAX_ITERATIONS);
                eval_events::emit(
                    config.eval_events_path.as_deref(),
                    json!({
                        "event": "profile_repair_start",
                        "phase_id": phase.id,
                        "shared_execution_session": true,
                        "bounded_repair": true,
                        "max_iterations": repair_config.max_iterations,
                        "session_message_count": ultra_session.messages.len(),
                    }),
                );
                let repair_outcome = run_profile_repair_with_ultra_session(
                    execution,
                    &mut ultra_session,
                    &repair_prompt,
                    &expected_paths,
                    &repair_config,
                    ui,
                )
                .map_err(|err| {
                    anyhow::anyhow!("phase {} profile repair failed: {err}", phase.id)
                })?;
                push_context_items_capped(
                    &mut ultra_context.created_or_changed_paths,
                    &repair_outcome.changed_paths,
                    ULTRA_CONTEXT_MAX_PATHS,
                    &mut ultra_context.truncated,
                );
                push_context_items_capped(
                    &mut ultra_context.last_repair_changed_paths,
                    &repair_outcome.changed_paths,
                    ULTRA_CONTEXT_MAX_PATHS,
                    &mut ultra_context.truncated,
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
                eval_events::emit(
                    config.eval_events_path.as_deref(),
                    json!({
                        "event": "profile_repair_complete",
                        "phase_id": phase.id,
                        "shared_execution_session": true,
                        "changed_path_count": repair_outcome.changed_paths.len(),
                        "iterations": repair_outcome.iterations,
                        "tool_calls": repair_outcome.tool_calls,
                    }),
                );
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
                        UltraPhaseRecoveryRequest {
                            failure_kind: "profile_repair_failed",
                            reason: &format!("{:?}", retry.status),
                            missing_paths: &retry.missing_paths,
                            missing_signals: &verification_missing_signals(&retry),
                            repair_targets: &["profile_contract".to_string()],
                        },
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
                UltraPhaseRecoveryRequest {
                    failure_kind: "profile_final_failure",
                    reason: &format!("{:?}", profile_report.status),
                    missing_paths: &profile_report.missing_paths,
                    missing_signals: &verification_missing_signals(&profile_report),
                    repair_targets: &["profile_contract".to_string()],
                },
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
    let mut acceptance_report = ultra_final_acceptance_report(plan, config)?;
    if !acceptance_report.is_pass() {
        let initial_reason = acceptance_report.primary_reason();
        let initial_target = classify_repair_target(&acceptance_report);
        eval_events::emit(
            config.eval_events_path.as_deref(),
            json!({
                "event": "ultra_final_acceptance_failed",
                "lifecycle_stage": "final_acceptance",
                "primary_reason": eval_events::body_snippet(&initial_reason),
                "repair_target": initial_target.as_str(),
                "missing_paths": acceptance_report.missing_paths.clone(),
                "profile_failures": acceptance_report.profile_failures.clone(),
                "bounded_repair_available": true,
                "max_attempts": FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS,
            }),
        );
        let fallback_phase = plan.phases.last().cloned().unwrap_or_else(|| UltraPhase {
            id: "final".to_string(),
            prompt: "Final acceptance".to_string(),
        });
        let expected_paths =
            final_acceptance_repair_expected_paths(plan, config, &acceptance_report)?;
        let repair_config = capped_config(config, STEP_REPAIR_MAX_ITERATIONS);
        let repair_prompt = final_acceptance_repair_prompt(
            plan,
            &acceptance_report,
            &ultra_context,
            initial_target.as_str(),
            &expected_paths,
            1,
            FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS,
        );
        eval_events::emit(
            config.eval_events_path.as_deref(),
            json!({
                "event": "final_acceptance_repair_start",
                "lifecycle_stage": "final_acceptance_repair",
                "attempt": 1,
                "max_attempts": FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS,
                "repair_target": initial_target.as_str(),
                "missing_paths": acceptance_report.missing_paths.clone(),
                "profile_failures": acceptance_report.profile_failures.clone(),
                "bounded_repair": true,
                "max_iterations": repair_config.max_iterations,
                "shared_execution_session": true,
                "session_message_count": ultra_session.messages.len(),
            }),
        );
        let repair_outcome = match run_final_acceptance_repair_with_ultra_session(
            execution,
            &mut ultra_session,
            &repair_prompt,
            &expected_paths,
            &repair_config,
            ui,
        ) {
            Ok(outcome) => outcome,
            Err(err) => {
                let err_text = err.to_string();
                eval_events::emit(
                    config.eval_events_path.as_deref(),
                    json!({
                        "event": "final_acceptance_repair_failed",
                        "lifecycle_stage": "final_acceptance_repair",
                        "attempt": 1,
                        "max_attempts": FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS,
                        "repair_target": initial_target.as_str(),
                        "reason": eval_events::body_snippet(&err_text),
                        "bounded_repair_exhausted": true,
                    }),
                );
                let handoff = save_ultra_phase_recovery_handoff(
                    config,
                    plan,
                    &fallback_phase,
                    UltraPhaseRecoveryRequest {
                        failure_kind: "final_acceptance_repair_failed",
                        reason: &err_text,
                        missing_paths: &acceptance_report.missing_paths,
                        missing_signals: &verification_missing_signals(&acceptance_report),
                        repair_targets: &[initial_target.as_str().to_string()],
                    },
                )
                .unwrap_or_default();
                anyhow::bail!("ultra final acceptance repair failed: {err_text}{handoff}");
            }
        };
        push_context_items_capped(
            &mut ultra_context.created_or_changed_paths,
            &repair_outcome.changed_paths,
            ULTRA_CONTEXT_MAX_PATHS,
            &mut ultra_context.truncated,
        );
        push_context_items_capped(
            &mut ultra_context.last_repair_changed_paths,
            &repair_outcome.changed_paths,
            ULTRA_CONTEXT_MAX_PATHS,
            &mut ultra_context.truncated,
        );
        eval_events::emit(
            config.eval_events_path.as_deref(),
            json!({
                "event": "final_acceptance_repair_complete",
                "lifecycle_stage": "final_acceptance_repair",
                "attempt": 1,
                "max_attempts": FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS,
                "repair_target": initial_target.as_str(),
                "changed_path_count": repair_outcome.changed_paths.len(),
                "iterations": repair_outcome.iterations,
                "tool_calls": repair_outcome.tool_calls,
                "shared_execution_session": true,
                "session_message_count": ultra_session.messages.len(),
            }),
        );
        acceptance_report = ultra_final_acceptance_report(plan, config)?;
        if !acceptance_report.is_pass() {
            let reason = acceptance_report.primary_reason();
            let target = classify_repair_target(&acceptance_report);
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "final_acceptance_repair_exhausted",
                    "lifecycle_stage": "final_acceptance_repair",
                    "attempt": FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS,
                    "max_attempts": FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS,
                    "repair_target": target.as_str(),
                    "primary_reason": eval_events::body_snippet(&reason),
                    "missing_paths": acceptance_report.missing_paths.clone(),
                    "profile_failures": acceptance_report.profile_failures.clone(),
                    "bounded_repair_exhausted": true,
                }),
            );
            let handoff = save_ultra_phase_recovery_handoff(
                config,
                plan,
                &fallback_phase,
                UltraPhaseRecoveryRequest {
                    failure_kind: "final_acceptance_repair_exhausted",
                    reason: &reason,
                    missing_paths: &acceptance_report.missing_paths,
                    missing_signals: &verification_missing_signals(&acceptance_report),
                    repair_targets: &[target.as_str().to_string()],
                },
            )
            .unwrap_or_default();
            anyhow::bail!("ultra final acceptance failed after bounded repair: {reason}{handoff}");
        }
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

#[allow(clippy::too_many_arguments)]
fn repair_intermediate_profile_invariant(
    execution: &mut dyn ChatClient,
    ultra_session: &mut SessionSnapshot,
    config: &Config,
    plan: &UltraPlan,
    phase: &UltraPhase,
    index: usize,
    profile_snapshot: &ProfileSnapshot,
    step_plan: &StepPlan,
    ultra_context: &mut UltraRunContext,
    final_expected_paths: &[String],
    ui: &dyn InteractionUi,
    failed_report: VerificationReport,
) -> anyhow::Result<VerificationReport> {
    let mut retry = failed_report.clone();
    let deterministic_error = match profile_auto_repair(
        &config.workspace_root,
        &plan.profile,
        &plan.goal,
        &failed_report,
    ) {
        Ok(changed) => {
            retry = verify_profile_invariant(
                &config.workspace_root,
                &plan.profile,
                &plan.goal,
                profile_snapshot,
            );
            emit_profile_invariant_repair_event(
                config,
                plan,
                phase,
                index,
                "deterministic",
                changed,
                retry.is_pass(),
                &retry.primary_reason(),
            );
            None
        }
        Err(err) => {
            let message = err.to_string();
            emit_profile_invariant_repair_event(
                config,
                plan,
                phase,
                index,
                "deterministic",
                false,
                false,
                &message,
            );
            Some(message)
        }
    };
    if retry.is_pass() {
        return Ok(confirm_phase_build_after_profile_repair(
            config, plan, phase, index, step_plan, retry,
        ));
    }

    let expected_paths = profile_expected_paths(&config.workspace_root, &plan.profile, &plan.goal);
    let repair_prompt = profile_invariant_model_repair_prompt(
        plan,
        phase,
        &retry,
        ultra_context,
        &expected_paths,
        config,
        deterministic_error.as_deref(),
    );
    let repair_config = capped_config(config, STEP_REPAIR_MAX_ITERATIONS);
    match run_profile_repair_with_ultra_session(
        execution,
        ultra_session,
        &repair_prompt,
        &expected_paths,
        &repair_config,
        ui,
    ) {
        Ok(repair_outcome) => {
            push_context_items_capped(
                &mut ultra_context.created_or_changed_paths,
                &repair_outcome.changed_paths,
                ULTRA_CONTEXT_MAX_PATHS,
                &mut ultra_context.truncated,
            );
            push_context_items_capped(
                &mut ultra_context.last_repair_changed_paths,
                &repair_outcome.changed_paths,
                ULTRA_CONTEXT_MAX_PATHS,
                &mut ultra_context.truncated,
            );
            emit_ultra_phase_context_updated(
                config,
                plan,
                phase,
                index,
                ultra_context,
                ultra_session.messages.len(),
                true,
            );
            retry = verify_profile_invariant(
                &config.workspace_root,
                &plan.profile,
                &plan.goal,
                profile_snapshot,
            );
            emit_profile_invariant_repair_event(
                config,
                plan,
                phase,
                index,
                "model",
                !repair_outcome.changed_paths.is_empty(),
                retry.is_pass(),
                &retry.primary_reason(),
            );
            if retry.is_pass() {
                return Ok(confirm_phase_build_after_profile_repair(
                    config, plan, phase, index, step_plan, retry,
                ));
            }
        }
        Err(err) => {
            emit_profile_invariant_repair_event(
                config,
                plan,
                phase,
                index,
                "model",
                false,
                false,
                &err.to_string(),
            );
        }
    }
    ultra_context.update_after_profile_failure(
        phase,
        &retry.primary_reason(),
        missing_final_artifacts(&config.workspace_root, final_expected_paths),
    );
    emit_ultra_phase_context_updated(
        config,
        plan,
        phase,
        index,
        ultra_context,
        ultra_session.messages.len(),
        true,
    );
    Ok(retry)
}

#[allow(clippy::too_many_arguments)]
fn emit_profile_invariant_repair_event(
    config: &Config,
    plan: &UltraPlan,
    phase: &UltraPhase,
    index: usize,
    method: &str,
    changed: bool,
    ok: bool,
    reason: &str,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "profile_invariant_repair",
            "phase_id": phase.id,
            "phase_index": index + 1,
            "total_phases": plan.phases.len(),
            "final_phase": false,
            "method": method,
            "changed": changed,
            "ok": ok,
            "reason": eval_events::body_snippet(reason),
            "bounded_repair": method == "model",
        }),
    );
}

fn confirm_phase_build_after_profile_repair(
    config: &Config,
    plan: &UltraPlan,
    phase: &UltraPhase,
    index: usize,
    step_plan: &StepPlan,
    profile_report: VerificationReport,
) -> VerificationReport {
    let build_commands = phase_build_verify_commands(step_plan);
    if build_commands.is_empty() {
        return profile_report;
    }
    let build_step = PlanStep {
        id: "profile-repair-build".to_string(),
        kind: "verify".to_string(),
        expected_result: "pass".to_string(),
        instruction: "Re-run the phase build verification after profile repair".to_string(),
        expected_paths: Vec::new(),
        verify: build_commands.clone(),
    };
    let (build_report, build_lifecycles) = verify_step_with_setup_observed_with_offline(
        &config.workspace_root,
        &build_step,
        NodeDependencySetupAuthority::None,
        config.offline,
    );
    for lifecycle in &build_lifecycles {
        emit_dependency_build_lifecycle(
            config.eval_events_path.as_deref(),
            "ultra-plan-run",
            Some(&phase.id),
            lifecycle,
        );
    }
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "profile_invariant_repair_build_verify",
            "phase_id": phase.id,
            "phase_index": index + 1,
            "total_phases": plan.phases.len(),
            "commands": build_commands,
            "ok": build_report.is_pass(),
            "reason": eval_events::body_snippet(&build_report.primary_reason()),
        }),
    );
    if build_report.is_pass() {
        profile_report
    } else {
        build_report
    }
}

fn phase_build_verify_commands(plan: &StepPlan) -> Vec<String> {
    let mut commands = Vec::new();
    for command in plan.steps.iter().flat_map(|step| step.verify.iter()) {
        if is_nextjs_build_verify_command_like(command) && !commands.contains(command) {
            commands.push(command.clone());
        }
    }
    commands
}

fn is_nextjs_build_verify_command_like(command: &str) -> bool {
    let lower = command.trim().to_ascii_lowercase();
    lower == "npm run build"
        || lower.starts_with("npm run build ")
        || lower == "pnpm build"
        || lower.starts_with("pnpm build ")
        || lower == "yarn build"
        || lower.starts_with("yarn build ")
        || lower == "next build"
        || lower.starts_with("next build ")
}

fn profile_invariant_model_repair_prompt(
    plan: &UltraPlan,
    phase: &UltraPhase,
    report: &VerificationReport,
    context: &UltraRunContext,
    expected_paths: &[String],
    config: &Config,
    deterministic_error: Option<&str>,
) -> String {
    let exact_reason = report.primary_reason();
    let expected = render_prompt_bullets(expected_paths);
    let file_excerpts = profile_invariant_offending_file_excerpts(
        &config.workspace_root,
        &plan.profile,
        &exact_reason,
    );
    let deterministic_note = deterministic_error
        .map(|err| format!("\nDeterministic repair error:\n{err}\n"))
        .unwrap_or_default();
    format!(
        "Repair this intermediate profile invariant failure in one bounded turn.\n\n\
Original ultra goal:\n{goal}\n\n\
Profile: {profile}\nIntent: {intent}\nPhase id: {phase_id}\nPhase task:\n{phase_task}\n\n\
Exact invariant reason:\n\"{exact_reason}\"\n{deterministic_note}\n\
Offending file contents:\n{file_excerpts}\n\n\
Expected profile artifacts:\n{expected}\n\n\
{prior_context}\n\n\
Bounded repair rules:\n\
- Repair only the quoted invariant reason without weakening scripts, dependencies, tests, or profile checks.\n\
- Prefer the smallest edit to the offending file shown above.\n\
- For Tailwind, package.json must include tailwindcss/postcss/autoprefixer and postcss.config plugins must include BOTH tailwindcss and autoprefixer.\n\
- Stop after one repair pass; do not start a new planning cycle.",
        goal = plan.goal,
        profile = plan.profile,
        intent = plan.intent,
        phase_id = phase.id,
        phase_task = phase.prompt,
        exact_reason = exact_reason,
        deterministic_note = deterministic_note,
        file_excerpts = file_excerpts,
        expected = expected,
        prior_context = context.render_prompt_section(),
    )
}

fn profile_invariant_offending_file_excerpts(root: &Path, profile: &str, reason: &str) -> String {
    let paths = profile_invariant_relevant_paths(root, profile, reason);
    if paths.is_empty() {
        return "- no matching profile files found".to_string();
    }
    paths
        .into_iter()
        .filter_map(|path| {
            let content = std::fs::read_to_string(&path).ok()?;
            let label = path
                .strip_prefix(root)
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| path.display().to_string());
            Some(format!(
                "--- {label} ---\n{}",
                bounded_file_excerpt(&content, PROFILE_REPAIR_FILE_EXCERPT_MAX_CHARS)
            ))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn profile_invariant_relevant_paths(root: &Path, profile: &str, reason: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !matches!(profile, "nextjs" | "next-js" | "next.js") {
        return out;
    }
    let tailwind_failure = reason.contains("tailwind_contract_failure");
    let rels: &[&str] = if tailwind_failure {
        &[
            "package.json",
            "postcss.config.js",
            "postcss.config.cjs",
            "postcss.config.mjs",
            "postcss.config",
            "tailwind.config.js",
            "tailwind.config.cjs",
            "tailwind.config.mjs",
            "tailwind.config.ts",
            "src/app/layout.tsx",
            "src/app/layout.jsx",
            "src/app/layout.ts",
            "src/app/layout.js",
            "app/layout.tsx",
            "app/layout.jsx",
            "app/layout.ts",
            "app/layout.js",
            "src/app/globals.css",
            "src/app/global.css",
            "app/globals.css",
            "app/global.css",
            "src/styles/globals.css",
            "styles/globals.css",
        ]
    } else {
        &[
            "package.json",
            "tsconfig.json",
            "src/app/page.tsx",
            "src/app/layout.tsx",
            "app/page.tsx",
            "app/layout.tsx",
        ]
    };
    for project_root in profile_excerpt_project_roots(root) {
        for rel in rels {
            let path = project_root.join(rel);
            if path.is_file() && !out.contains(&path) {
                out.push(path);
            }
        }
    }
    out
}

fn profile_excerpt_project_roots(root: &Path) -> Vec<PathBuf> {
    let mut roots = vec![root.to_path_buf()];
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join("package.json").is_file() && !roots.contains(&path) {
                roots.push(path);
            }
        }
    }
    roots
}

fn bounded_file_excerpt(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        return content.to_string();
    }
    let mut excerpt = content.chars().take(max_chars).collect::<String>();
    excerpt.push_str("\n[truncated]");
    excerpt
}

fn ultra_final_acceptance_report(
    plan: &UltraPlan,
    config: &Config,
) -> anyhow::Result<VerificationReport> {
    let mut required_paths =
        profile_expected_paths(&config.workspace_root, &plan.profile, &plan.goal);
    let mut required_capabilities = inferred_required_capabilities(&plan.profile, &plan.goal);
    let mut required_obligations =
        inferred_required_obligations(&plan.profile, &plan.goal, &required_capabilities);
    let mut required_evidence =
        inferred_required_evidence(&plan.profile, &plan.goal, &required_capabilities);
    let mut evidence_hint_tokens = evidence_hint_tokens_for_goal(&plan.goal);
    let bound_contract = bind_completion_contract_for_acceptance(
        config,
        "ultra-plan-run",
        &plan.profile,
        &plan.goal,
        &required_paths,
        &required_capabilities,
        &required_evidence,
        &required_obligations,
    )?;
    let mut deferred_commands = Vec::new();
    let mut verify_commands = Vec::new();
    if let Some(contract) = bound_contract.as_ref().map(|bound| &bound.contract) {
        merge_unique_strings(&mut required_paths, &contract.required_paths);
        merge_unique_strings(&mut required_capabilities, &contract.required_capabilities);
        merge_unique_strings(&mut required_evidence, &contract.required_evidence);
        merge_unique_strings(&mut required_obligations, &contract.required_obligations);
        merge_unique_strings(&mut verify_commands, &contract.verify_commands);
        merge_unique_strings(&mut evidence_hint_tokens, &contract.evidence_hint_tokens);
        deferred_commands.extend(
            contract
                .deferred_verify_requirements
                .iter()
                .map(|requirement| requirement.command.clone()),
        );
    }
    merge_unique_strings(
        &mut required_evidence,
        &inferred_required_evidence(&plan.profile, &plan.goal, &required_capabilities),
    );
    let browser_probe = maybe_run_ultra_final_browser_probe(config, plan, &required_capabilities);
    let missing = missing_final_artifacts(&config.workspace_root, &required_paths);
    let acceptance = verify_runtime_acceptance_with_browser_dirs_and_hints(
        &config.workspace_root,
        &required_paths,
        &verify_commands,
        &required_capabilities,
        &required_evidence,
        &required_obligations,
        &deferred_commands,
        &release_evidence_extra_dirs(config),
        &evidence_hint_tokens,
    );
    let external_report = bound_contract.as_ref().map(|bound| {
        bound
            .contract
            .verify_with_goal(&config.workspace_root, &plan.goal)
    });
    let contract_required =
        completion_contract_required(&plan.profile, &plan.goal, &required_capabilities)
            || bound_contract.as_ref().is_some_and(|bound| bound.required);
    let external_contract_checked = bound_contract.is_some();
    let contract_binding_missing = contract_required && !external_contract_checked;
    let external_ok = !contract_binding_missing
        && external_report
            .as_ref()
            .is_none_or(|report| report.is_pass());
    let release_gate = final_acceptance_release_gate(
        config,
        &plan.profile,
        &plan.goal,
        &required_capabilities,
        Some(&acceptance),
    );
    let final_acceptance_status = release_gate_final_acceptance_status(&release_gate);
    let runtime_acceptance_status = runtime_acceptance_status(acceptance.passed, Some(&acceptance));
    let release_quality_completion =
        release_quality_completion_status(&release_gate, final_acceptance_status);
    let next_action = release_gate_next_action(&release_gate, final_acceptance_status);
    let primary_reason = if !missing.is_empty() {
        format!("missing final artifacts: {}", missing.join(", "))
    } else if contract_binding_missing {
        "completion contract binding required but missing".to_string()
    } else if !acceptance.passed {
        acceptance.primary_reason.clone()
    } else if let Some(report) = external_report.as_ref().filter(|report| !report.is_pass()) {
        report.primary_reason()
    } else if matches!(release_gate.status.as_str(), "partial" | "failed") {
        release_gate.reasons.join("; ")
    } else {
        acceptance.primary_reason.clone()
    };
    let recovery_handoff = if acceptance.passed
        && external_ok
        && (matches!(release_gate.status.as_str(), "partial" | "failed")
            || final_acceptance_status == "partial")
    {
        let acceptance_layer =
            release_recovery_acceptance_layer(&release_gate, final_acceptance_status);
        let failure_kind =
            release_recovery_failure_kind(&release_gate, final_acceptance_status, &primary_reason);
        let scope = format!("release-{}", recovery_scope_token(acceptance_layer));
        save_release_recovery_handoff(
            config,
            &plan.profile,
            &plan.goal,
            &scope,
            acceptance_layer,
            &failure_kind,
            release_recovery_failure_evidence(
                &release_gate,
                final_acceptance_status,
                &primary_reason,
                Some(&acceptance),
            ),
            missing.clone(),
            release_recovery_missing_capabilities(Some(&acceptance)),
            release_recovery_repair_targets(&release_gate, Some(&acceptance)),
            release_recovery_verify_commands(&plan.profile, &release_gate),
        )
    } else {
        None
    };
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "ultra_final_acceptance",
            "required_paths": required_paths.clone(),
            "missing_paths": missing.clone(),
            "completion_contract_verification_enabled": external_contract_checked,
            "completion_contract_path_merge_enabled": external_contract_checked,
            "completion_contract_path": bound_contract
                .as_ref()
                .map(|bound| bound.path.clone())
                .unwrap_or_default(),
            "completion_contract_generated": bound_contract
                .as_ref()
                .map(|bound| bound.generated)
                .unwrap_or(false),
            "external_contract_checked": external_contract_checked,
            "external_contract_required": contract_required,
            "external_contract_ok": external_ok,
            "required_capabilities": required_capabilities.clone(),
            "required_evidence": required_evidence.clone(),
            "required_obligations": required_obligations.clone(),
            "runtime_acceptance_passed": acceptance.passed,
            "runtime_acceptance_status": runtime_acceptance_status,
            "runtime_acceptance_inconclusive": acceptance.inconclusive,
            "final_acceptance_status": final_acceptance_status,
            "release_quality_completion": release_quality_completion,
            "missing_capabilities": acceptance.missing_capabilities.clone(),
            "missing_evidence": acceptance.missing_evidence.clone(),
            "missing_obligations": acceptance.missing_obligations.clone(),
            "weak_evidence": acceptance.weak_evidence.clone(),
            "artifact_obligations": acceptance.artifact_obligations.clone(),
            "capability_evidence_bindings": acceptance.capability_evidence_bindings.clone(),
            "obligation_repair_targets": acceptance.obligation_repair_targets.clone(),
            "inconclusive_reasons": acceptance.inconclusive_reasons.clone(),
            "release_gate_status": release_gate.status.clone(),
            "release_gate_reasons": release_gate.reasons.clone(),
            "browser_readiness_status": release_gate.browser_readiness_status.clone(),
            "browser_readiness_evidence_path": release_gate.browser_readiness_evidence_path.clone(),
            "interaction_evidence_status": release_gate.interaction_evidence_status.clone(),
            "interaction_evidence_path": release_gate.interaction_evidence_path.clone(),
            "next_action": next_action,
            "recovery_handoff_kind": recovery_handoff
                .as_ref()
                .map(|handoff| handoff.recovery_handoff_kind.as_str())
                .unwrap_or_default(),
            "acceptance_layer": recovery_handoff
                .as_ref()
                .map(|handoff| handoff.acceptance_layer.as_str())
                .unwrap_or_default(),
            "recovery_prompt_path": recovery_handoff
                .as_ref()
                .map(|handoff| handoff.recovery_prompt_path.as_str())
                .unwrap_or_default(),
            "recovery_ultra_plan_path": recovery_handoff
                .as_ref()
                .map(|handoff| handoff.recovery_ultra_plan_path.as_str())
                .unwrap_or_default(),
            "suggested_recovery_command": recovery_handoff
                .as_ref()
                .map(|handoff| handoff.suggested_recovery_command.as_str())
                .unwrap_or_default(),
            "suggested_recovery_yaml_command": recovery_handoff
                .as_ref()
                .map(|handoff| handoff.suggested_recovery_yaml_command.as_str())
                .unwrap_or_default(),
            "recovery_handoff_saved": recovery_handoff
                .as_ref()
                .is_some_and(ReleaseRecoveryHandoffSummary::has_artifact),
            "handoff_saved_not_success": recovery_handoff.is_some(),
            "primary_reason": eval_events::body_snippet(&primary_reason),
        }),
    );
    let mut report = VerificationReport::pass();
    for path in missing {
        report.push_missing_path(path);
    }
    if !acceptance.passed {
        report.push_profile_failure(acceptance.primary_reason.clone());
        for target in &acceptance.obligation_repair_targets {
            report.push_profile_failure(format!(
                "missing_required_obligation_target:{}:{}",
                target.obligation, target.target_path
            ));
        }
    }
    if contract_binding_missing {
        report.push_profile_failure("completion contract binding required but missing".to_string());
    }
    if let Some(external_report) = external_report.filter(|report| !report.is_pass()) {
        report.push_profile_failure(format!(
            "external contract failed: {}",
            external_report.primary_reason()
        ));
    }
    if release_gate.status == "failed" {
        report.push_profile_failure(format!(
            "release gate failed: {}",
            release_gate.reasons.join("; ")
        ));
    }
    if let Some(observation) = &browser_probe
        && let Some(reason) = observation.failure_reason()
    {
        report.push_profile_failure(format!("browser_readiness_failed:{reason}"));
        if !observation.output_excerpt.trim().is_empty() {
            report.push_profile_failure(format!(
                "browser_probe_output: {}",
                eval_events::body_snippet(&observation.output_excerpt)
            ));
        }
    }
    Ok(report)
}

fn maybe_run_ultra_final_browser_probe(
    config: &Config,
    plan: &UltraPlan,
    required_capabilities: &[String],
) -> Option<BrowserReadinessObservation> {
    if !ultra_browser_probe_required(&plan.profile, &plan.goal, required_capabilities)
        || !ultra_browser_probe_runtime_enabled(config)
    {
        return None;
    }
    let observation = probe_browser_readiness_with_offline(
        &config.workspace_root,
        &plan.profile,
        None,
        Duration::from_secs(30),
        config.offline,
    );
    emit_browser_probe_event(config, &observation);
    Some(observation)
}

fn ultra_browser_probe_required(
    profile: &str,
    goal: &str,
    required_capabilities: &[String],
) -> bool {
    let lower = goal.to_ascii_lowercase();
    matches!(profile, "nextjs" | "next-js" | "next.js")
        && (required_capabilities.iter().any(|capability| {
            matches!(
                capability.as_str(),
                "stateful_interaction"
                    | "player_control"
                    | "user_input_or_action"
                    | "visible_state_change"
                    | "adversary_or_challenge"
                    | "progression_or_score"
                    | "failure_or_collision_rule"
            )
        }) || lower.contains("interactive")
            || lower.contains("game")
            || lower.contains("keyboard")
            || goal.contains("ゲーム"))
}

fn ultra_browser_probe_runtime_enabled(config: &Config) -> bool {
    #[cfg(test)]
    {
        config
            .workspace_root
            .join(".anvil")
            .join("enable-browser-probe-tests")
            .is_file()
    }
    #[cfg(not(test))]
    {
        let _ = config;
        true
    }
}

fn emit_browser_probe_event(config: &Config, observation: &BrowserReadinessObservation) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "browser_probe",
            "profile": observation.profile,
            "status": observation.status,
            "ok": observation.ok,
            "port": observation.port,
            "route": observation.route,
            "command": observation.command,
            "http_status": observation.http_status,
            "failure_kind": observation.failure_kind,
            "elapsed_ms": observation.elapsed_ms,
            "evidence_path": observation.evidence_path.display().to_string(),
            "output_excerpt": eval_events::body_snippet(&observation.output_excerpt),
            "child_spawned": observation.child_spawned,
            "child_reaped": observation.child_reaped,
        }),
    );
}

fn ultra_contract_runtime_acceptance_report(
    plan: &UltraPlan,
    config: &Config,
) -> anyhow::Result<RuntimeAcceptanceReport> {
    let mut required_paths =
        profile_expected_paths(&config.workspace_root, &plan.profile, &plan.goal);
    let mut required_capabilities = inferred_required_capabilities(&plan.profile, &plan.goal);
    let mut required_obligations =
        inferred_required_obligations(&plan.profile, &plan.goal, &required_capabilities);
    let mut required_evidence =
        inferred_required_evidence(&plan.profile, &plan.goal, &required_capabilities);
    let mut evidence_hint_tokens = evidence_hint_tokens_for_goal(&plan.goal);
    let bound_contract = bind_completion_contract_for_acceptance(
        config,
        "ultra-plan-run",
        &plan.profile,
        &plan.goal,
        &required_paths,
        &required_capabilities,
        &required_evidence,
        &required_obligations,
    )?;
    let mut deferred_commands = Vec::new();
    let mut verify_commands = Vec::new();
    if let Some(contract) = bound_contract.as_ref().map(|bound| &bound.contract) {
        merge_unique_strings(&mut required_paths, &contract.required_paths);
        merge_unique_strings(&mut required_capabilities, &contract.required_capabilities);
        merge_unique_strings(&mut required_evidence, &contract.required_evidence);
        merge_unique_strings(&mut required_obligations, &contract.required_obligations);
        merge_unique_strings(&mut verify_commands, &contract.verify_commands);
        merge_unique_strings(&mut evidence_hint_tokens, &contract.evidence_hint_tokens);
        deferred_commands.extend(
            contract
                .deferred_verify_requirements
                .iter()
                .map(|requirement| requirement.command.clone()),
        );
    }
    merge_unique_strings(
        &mut required_evidence,
        &inferred_required_evidence(&plan.profile, &plan.goal, &required_capabilities),
    );
    Ok(verify_runtime_acceptance_with_browser_dirs_and_hints(
        &config.workspace_root,
        &required_paths,
        &verify_commands,
        &required_capabilities,
        &required_evidence,
        &required_obligations,
        &deferred_commands,
        &release_evidence_extra_dirs(config),
        &evidence_hint_tokens,
    ))
}

fn inferred_required_capabilities(profile: &str, goal: &str) -> Vec<String> {
    let lower = goal.to_ascii_lowercase();
    let is_next = matches!(profile, "nextjs" | "next-js" | "next.js");
    let mut capabilities = Vec::new();
    let game_like = lower.contains("game")
        || lower.contains("playable")
        || lower.contains("canvas")
        || lower.contains("player")
        || lower.contains("enemy")
        || lower.contains("enemies")
        || lower.contains("adversary")
        || lower.contains("opponent")
        || lower.contains("obstacle")
        || lower.contains("collision")
        || lower.contains("bullet")
        || lower.contains("lives")
        || lower.contains("game over")
        || goal.contains("ゲーム")
        || goal.contains("シューティング");
    if is_next && game_like {
        merge_unique_strings(&mut capabilities, &["stateful_interaction".to_string()]);
        merge_unique_strings(&mut capabilities, &["start_or_restart_flow".to_string()]);
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
            || lower.contains("input")
            || lower.contains("interactive")
            || lower.contains("score")
            || goal.contains("操作"))
    {
        merge_unique_strings(&mut capabilities, &["stateful_interaction".to_string()]);
        merge_unique_strings(&mut capabilities, &["user_input_or_action".to_string()]);
        merge_unique_strings(&mut capabilities, &["visible_state_change".to_string()]);
    }
    capabilities
}

fn inferred_required_evidence(
    profile: &str,
    goal: &str,
    required_capabilities: &[String],
) -> Vec<String> {
    let mut evidence = Vec::new();
    let lower = goal.to_ascii_lowercase();
    let is_next = matches!(profile, "nextjs" | "next-js" | "next.js");
    let app_like_goal = lower.contains("app")
        || lower.contains("game")
        || lower.contains("interactive")
        || lower.contains("ui")
        || goal.contains("アプリ")
        || goal.contains("ゲーム")
        || !required_capabilities.is_empty();
    if is_next && app_like_goal {
        merge_unique_strings(
            &mut evidence,
            &[
                "nextjs_route_evidence".to_string(),
                "build_command_or_dependency_missing_boundary".to_string(),
            ],
        );
    }
    for capability in required_capabilities {
        merge_unique_strings(&mut evidence, &required_evidence_for_capability(capability));
    }
    evidence
}

fn inferred_required_obligations(
    profile: &str,
    goal: &str,
    required_capabilities: &[String],
) -> Vec<String> {
    let lower = goal.to_ascii_lowercase();
    let is_app_profile = matches!(profile, "nextjs" | "next-js" | "next.js" | "web" | "vite");
    let app_like_goal = lower.contains("app")
        || lower.contains("game")
        || lower.contains("interactive")
        || lower.contains("ui")
        || goal.contains("アプリ")
        || goal.contains("ゲーム");
    if is_app_profile && (app_like_goal || !required_capabilities.is_empty()) {
        return vec!["implementation".to_string()];
    }
    if required_capabilities.iter().any(|capability| {
        matches!(
            capability.as_str(),
            "implementation"
                | "entrypoint"
                | "input_output_contract"
                | "player_control"
                | "stateful_interaction"
                | "user_input_or_action"
                | "visible_state_change"
                | "adversary_or_challenge"
                | "progression_or_score"
                | "failure_or_collision_rule"
        )
    }) {
        return vec!["implementation".to_string()];
    }
    Vec::new()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReleaseGateSummary {
    status: String,
    reasons: Vec<String>,
    browser_readiness_status: String,
    browser_readiness_evidence_path: String,
    interaction_evidence_status: String,
    interaction_evidence_path: String,
}

fn final_acceptance_release_gate(
    config: &Config,
    profile: &str,
    goal: &str,
    required_capabilities: &[String],
    acceptance: Option<&crate::minimal_loop::evidence::RuntimeAcceptanceReport>,
) -> ReleaseGateSummary {
    let lower = goal.to_ascii_lowercase();
    let is_next = matches!(profile, "nextjs" | "next-js" | "next.js");
    let requires_browser = is_next
        && (required_capabilities.iter().any(|capability| {
            matches!(
                capability.as_str(),
                "stateful_interaction"
                    | "player_control"
                    | "user_input_or_action"
                    | "visible_state_change"
                    | "adversary_or_challenge"
                    | "progression_or_score"
                    | "failure_or_collision_rule"
            )
        }) || lower.contains("interactive")
            || lower.contains("game")
            || lower.contains("keyboard")
            || goal.contains("ゲーム"));
    let Some(report) = acceptance else {
        return ReleaseGateSummary {
            status: "not_applicable".to_string(),
            reasons: Vec::new(),
            browser_readiness_status: "not_applicable".to_string(),
            browser_readiness_evidence_path: String::new(),
            interaction_evidence_status: "not_applicable".to_string(),
            interaction_evidence_path: String::new(),
        };
    };
    if !report.passed {
        return ReleaseGateSummary {
            status: "failed".to_string(),
            reasons: vec![report.primary_reason.clone()],
            browser_readiness_status: "not_checked".to_string(),
            browser_readiness_evidence_path: String::new(),
            interaction_evidence_status: "not_checked".to_string(),
            interaction_evidence_path: String::new(),
        };
    }
    if requires_browser {
        return browser_release_gate(config);
    }
    ReleaseGateSummary {
        status: "pass".to_string(),
        reasons: Vec::new(),
        browser_readiness_status: "not_applicable".to_string(),
        browser_readiness_evidence_path: String::new(),
        interaction_evidence_status: "not_applicable".to_string(),
        interaction_evidence_path: String::new(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReleaseEvidenceStatus {
    Passed,
    Failed(String),
    Unavailable(String),
}

impl ReleaseEvidenceStatus {
    fn as_status(&self) -> String {
        match self {
            Self::Passed => "passed".to_string(),
            Self::Failed(reason) => format!("failed:{reason}"),
            Self::Unavailable(reason) => format!("unavailable:{reason}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReleaseEvidence {
    status: ReleaseEvidenceStatus,
    path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReleaseEvidenceKind {
    BrowserReadiness,
    Interaction,
}

#[derive(Debug, Clone)]
struct NextjsDevServerProbeSpec {
    package_manager: String,
    args: Vec<String>,
    command_display: String,
    port: u16,
    route: String,
}

#[derive(Debug, Clone)]
struct HttpProbeResult {
    status: i64,
    body_excerpt: String,
}

fn browser_release_gate(config: &Config) -> ReleaseGateSummary {
    let mut browser = read_release_evidence(
        config,
        &[
            "browser-readiness.json",
            "browser.json",
            "browser-readiness-evidence.json",
        ],
        "browser_readiness_evidence_missing",
        ReleaseEvidenceKind::BrowserReadiness,
    );
    if matches!(
        &browser.status,
        ReleaseEvidenceStatus::Unavailable(reason)
            if reason == "browser_readiness_evidence_missing"
    ) {
        browser = nextjs_dev_route_release_evidence(config);
    }
    let interaction = read_release_evidence(
        config,
        &[
            "interaction-evidence.json",
            "interaction.json",
            "browser-interaction.json",
        ],
        "interaction_evidence_missing",
        ReleaseEvidenceKind::Interaction,
    );
    let browser_status = browser.status.as_status();
    let interaction_status = interaction.status.as_status();
    if let ReleaseEvidenceStatus::Failed(reason) = &browser.status {
        return ReleaseGateSummary {
            status: "failed".to_string(),
            reasons: vec![format!("browser_readiness_failed:{reason}")],
            browser_readiness_status: browser_status,
            browser_readiness_evidence_path: browser.path,
            interaction_evidence_status: interaction_status,
            interaction_evidence_path: interaction.path,
        };
    }
    if let ReleaseEvidenceStatus::Failed(reason) = &interaction.status {
        return ReleaseGateSummary {
            status: "failed".to_string(),
            reasons: vec![format!("browser_interaction_failed:{reason}")],
            browser_readiness_status: browser_status,
            browser_readiness_evidence_path: browser.path,
            interaction_evidence_status: interaction_status,
            interaction_evidence_path: interaction.path,
        };
    }
    if let ReleaseEvidenceStatus::Unavailable(reason) = &browser.status {
        return ReleaseGateSummary {
            status: "partial".to_string(),
            reasons: vec![format!(
                "browser_readiness_or_interaction_evidence_required:{reason}"
            )],
            browser_readiness_status: browser_status,
            browser_readiness_evidence_path: browser.path,
            interaction_evidence_status: interaction_status,
            interaction_evidence_path: interaction.path,
        };
    }
    if let ReleaseEvidenceStatus::Unavailable(reason) = &interaction.status {
        return ReleaseGateSummary {
            status: "partial".to_string(),
            reasons: vec![format!("browser_interaction_evidence_required:{reason}")],
            browser_readiness_status: browser_status,
            browser_readiness_evidence_path: browser.path,
            interaction_evidence_status: interaction_status,
            interaction_evidence_path: interaction.path,
        };
    }
    ReleaseGateSummary {
        status: "pass".to_string(),
        reasons: Vec::new(),
        browser_readiness_status: browser_status,
        browser_readiness_evidence_path: browser.path,
        interaction_evidence_status: interaction_status,
        interaction_evidence_path: interaction.path,
    }
}

fn nextjs_dev_route_release_evidence(config: &Config) -> ReleaseEvidence {
    let path = nextjs_dev_route_evidence_path(config);
    let value = run_nextjs_dev_route_probe(config, &path);
    let status = classify_release_evidence_json(ReleaseEvidenceKind::BrowserReadiness, &value);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(&value) {
        let _ = std::fs::write(&path, format!("{text}\n"));
    }
    ReleaseEvidence {
        status,
        path: path.display().to_string(),
    }
}

fn nextjs_dev_route_evidence_path(config: &Config) -> PathBuf {
    if let Some(events_path) = &config.eval_events_path
        && let Some(run_dir) = events_path.parent()
    {
        return run_dir.join("browser-readiness.json");
    }
    config
        .workspace_root
        .join(".anvil")
        .join("browser-readiness.json")
}

fn run_nextjs_dev_route_probe(config: &Config, evidence_path: &Path) -> Value {
    if !dev_server_probe_runtime_enabled() {
        let failure_kind = if cfg!(test) {
            "browser_unavailable:dev_server_probe_disabled_in_tests"
        } else {
            "browser_unavailable:dev_server_probe_disabled"
        };
        emit_dev_server_unavailable_lifecycle(
            config,
            NEXTJS_DEV_SERVER_DEFAULT_PORT,
            DEV_SERVER_ROUTE,
            "",
            failure_kind,
            evidence_path,
        );
        return dev_server_unavailable_evidence(
            NEXTJS_DEV_SERVER_DEFAULT_PORT,
            DEV_SERVER_ROUTE,
            "",
            failure_kind,
            "",
        );
    }

    let spec = match load_nextjs_dev_server_probe_spec(&config.workspace_root) {
        Ok(spec) => spec,
        Err(failure_kind) => {
            emit_dev_server_unavailable_lifecycle(
                config,
                NEXTJS_DEV_SERVER_DEFAULT_PORT,
                DEV_SERVER_ROUTE,
                "",
                &failure_kind,
                evidence_path,
            );
            return dev_server_unavailable_evidence(
                NEXTJS_DEV_SERVER_DEFAULT_PORT,
                DEV_SERVER_ROUTE,
                "",
                &failure_kind,
                "",
            );
        }
    };

    if localhost_port_accepts_connection(spec.port) {
        let failure_kind = "port_in_use";
        emit_dev_server_unavailable_lifecycle(
            config,
            spec.port,
            &spec.route,
            &spec.command_display,
            failure_kind,
            evidence_path,
        );
        return dev_server_unavailable_evidence(
            spec.port,
            &spec.route,
            &spec.command_display,
            failure_kind,
            "",
        );
    }

    let mut command =
        verifier_env::normalized_command_at_root(&spec.package_manager, &config.workspace_root);
    command
        .args(&spec.args)
        .current_dir(&config.workspace_root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("PORT", spec.port.to_string());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            let failure_kind = dev_server_spawn_failure_kind(&err);
            emit_dev_server_lifecycle_stage(
                config,
                "start",
                false,
                spec.port,
                &spec.route,
                &spec.command_display,
                Some(&failure_kind),
                None,
                evidence_path,
                None,
            );
            emit_dev_server_lifecycle_stage(
                config,
                "wait",
                false,
                spec.port,
                &spec.route,
                &spec.command_display,
                Some(&failure_kind),
                None,
                evidence_path,
                None,
            );
            emit_dev_server_lifecycle_stage(
                config,
                "probe",
                false,
                spec.port,
                &spec.route,
                &spec.command_display,
                Some(&failure_kind),
                None,
                evidence_path,
                None,
            );
            emit_dev_server_lifecycle_stage(
                config,
                "cleanup",
                true,
                spec.port,
                &spec.route,
                &spec.command_display,
                Some(&failure_kind),
                None,
                evidence_path,
                None,
            );
            return dev_server_unavailable_evidence(
                spec.port,
                &spec.route,
                &spec.command_display,
                &failure_kind,
                &err.to_string(),
            );
        }
    };

    let pid = child.id();
    emit_dev_server_lifecycle_stage(
        config,
        "start",
        true,
        spec.port,
        &spec.route,
        &spec.command_display,
        None,
        None,
        evidence_path,
        Some(pid),
    );

    let deadline = Instant::now() + NEXTJS_DEV_SERVER_READY_TIMEOUT;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_status)) => {
                let output = child.wait_with_output().ok();
                let output_excerpt = output
                    .as_ref()
                    .map(output_excerpt)
                    .unwrap_or_else(|| "dev server exited before readiness".to_string());
                let failure_kind = classify_dev_server_startup_failure(&output_excerpt)
                    .unwrap_or_else(|| "browser_unavailable:dev_server_exited".to_string());
                let failure_kind = classify_dev_server_env_conflict(&failure_kind, &output_excerpt);
                let output_excerpt = dev_server_output_excerpt(&failure_kind, &output_excerpt);
                emit_dev_server_lifecycle_stage(
                    config,
                    "wait",
                    false,
                    spec.port,
                    &spec.route,
                    &spec.command_display,
                    Some(&failure_kind),
                    None,
                    evidence_path,
                    Some(pid),
                );
                emit_dev_server_lifecycle_stage(
                    config,
                    "probe",
                    false,
                    spec.port,
                    &spec.route,
                    &spec.command_display,
                    Some(&failure_kind),
                    None,
                    evidence_path,
                    Some(pid),
                );
                emit_dev_server_lifecycle_stage(
                    config,
                    "cleanup",
                    true,
                    spec.port,
                    &spec.route,
                    &spec.command_display,
                    Some(&failure_kind),
                    None,
                    evidence_path,
                    Some(pid),
                );
                return dev_server_unavailable_evidence(
                    spec.port,
                    &spec.route,
                    &spec.command_display,
                    &failure_kind,
                    &output_excerpt,
                );
            }
            Ok(None) => {}
            Err(err) => {
                let failure_kind = "browser_unavailable:dev_server_status_unreadable";
                let cleanup = cleanup_dev_server_child(child);
                let combined = format!("{} {}", err, cleanup.output_excerpt);
                let failure_kind = classify_dev_server_env_conflict(failure_kind, &combined);
                let output_excerpt = dev_server_output_excerpt(&failure_kind, &combined);
                emit_dev_server_lifecycle_stage(
                    config,
                    "wait",
                    false,
                    spec.port,
                    &spec.route,
                    &spec.command_display,
                    Some(&failure_kind),
                    None,
                    evidence_path,
                    Some(pid),
                );
                emit_dev_server_lifecycle_stage(
                    config,
                    "probe",
                    false,
                    spec.port,
                    &spec.route,
                    &spec.command_display,
                    Some(&failure_kind),
                    None,
                    evidence_path,
                    Some(pid),
                );
                emit_dev_server_lifecycle_stage(
                    config,
                    "cleanup",
                    cleanup.ok,
                    spec.port,
                    &spec.route,
                    &spec.command_display,
                    Some(&failure_kind),
                    None,
                    evidence_path,
                    Some(pid),
                );
                return dev_server_unavailable_evidence(
                    spec.port,
                    &spec.route,
                    &spec.command_display,
                    &failure_kind,
                    &output_excerpt,
                );
            }
        }

        match http_get_local_route(spec.port, &spec.route) {
            Ok(response) => {
                emit_dev_server_lifecycle_stage(
                    config,
                    "wait",
                    true,
                    spec.port,
                    &spec.route,
                    &spec.command_display,
                    None,
                    Some(response.status),
                    evidence_path,
                    Some(pid),
                );
                let failure_kind =
                    classify_dev_route_failure_kind(response.status, &response.body_excerpt);
                let probe_ok = failure_kind.is_none();
                let cleanup = cleanup_dev_server_child(child);
                let failure_kind = failure_kind.map(|kind| {
                    let combined = format!("{}\n{}", response.body_excerpt, cleanup.output_excerpt);
                    classify_dev_server_env_conflict(&kind, &combined)
                });
                let body_excerpt = failure_kind
                    .as_deref()
                    .map(|kind| dev_server_output_excerpt(kind, &response.body_excerpt))
                    .unwrap_or_else(|| response.body_excerpt.clone());
                let output_excerpt = failure_kind
                    .as_deref()
                    .map(|kind| dev_server_output_excerpt(kind, &cleanup.output_excerpt))
                    .unwrap_or_else(|| cleanup.output_excerpt.clone());
                emit_dev_server_lifecycle_stage(
                    config,
                    "probe",
                    probe_ok && failure_kind.is_none(),
                    spec.port,
                    &spec.route,
                    &spec.command_display,
                    failure_kind.as_deref(),
                    Some(response.status),
                    evidence_path,
                    Some(pid),
                );
                emit_dev_server_lifecycle_stage(
                    config,
                    "cleanup",
                    cleanup.ok,
                    spec.port,
                    &spec.route,
                    &spec.command_display,
                    failure_kind.as_deref(),
                    Some(response.status),
                    evidence_path,
                    Some(pid),
                );
                if let Some(failure_kind) = failure_kind {
                    return dev_server_failed_evidence(
                        spec.port,
                        &spec.route,
                        &spec.command_display,
                        response.status,
                        &failure_kind,
                        &body_excerpt,
                        &output_excerpt,
                    );
                }
                return dev_server_passed_evidence(
                    spec.port,
                    &spec.route,
                    &spec.command_display,
                    response.status,
                    &response.body_excerpt,
                );
            }
            Err(_) => {
                std::thread::sleep(NEXTJS_DEV_SERVER_WAIT_INTERVAL);
            }
        }
    }

    let failure_kind = "startup_timeout";
    let cleanup = cleanup_dev_server_child(child);
    let failure_kind = classify_dev_server_env_conflict(failure_kind, &cleanup.output_excerpt);
    let output_excerpt = dev_server_output_excerpt(&failure_kind, &cleanup.output_excerpt);
    emit_dev_server_lifecycle_stage(
        config,
        "wait",
        false,
        spec.port,
        &spec.route,
        &spec.command_display,
        Some(&failure_kind),
        None,
        evidence_path,
        Some(pid),
    );
    emit_dev_server_lifecycle_stage(
        config,
        "probe",
        false,
        spec.port,
        &spec.route,
        &spec.command_display,
        Some(&failure_kind),
        None,
        evidence_path,
        Some(pid),
    );
    emit_dev_server_lifecycle_stage(
        config,
        "cleanup",
        cleanup.ok,
        spec.port,
        &spec.route,
        &spec.command_display,
        Some(&failure_kind),
        None,
        evidence_path,
        Some(pid),
    );
    dev_server_unavailable_evidence(
        spec.port,
        &spec.route,
        &spec.command_display,
        &failure_kind,
        &output_excerpt,
    )
}

fn dev_server_probe_runtime_enabled() -> bool {
    if env_flag_is_false("ANVIL_DEV_SERVER_PROBE") {
        return false;
    }
    if cfg!(test) && !env_flag_is_true("ANVIL_TEST_DEV_SERVER_PROBE") {
        return false;
    }
    true
}

fn env_flag_is_false(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
        .unwrap_or(false)
}

fn env_flag_is_true(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn load_nextjs_dev_server_probe_spec(root: &Path) -> Result<NextjsDevServerProbeSpec, String> {
    let manifest_path = root.join("package.json");
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|_| "browser_unavailable:package_json_missing".to_string())?;
    let value = serde_json::from_str::<Value>(&text)
        .map_err(|_| "browser_unavailable:package_json_invalid".to_string())?;
    let script = value
        .get("scripts")
        .and_then(Value::as_object)
        .and_then(|scripts| scripts.get("dev"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|script| !script.is_empty())
        .ok_or_else(|| "browser_unavailable:dev_script_missing".to_string())?;
    if !script_contains_next_dev(script) {
        return Err("browser_unavailable:dev_script_not_next_dev".to_string());
    }
    let port = parse_next_dev_port(script).unwrap_or(NEXTJS_DEV_SERVER_DEFAULT_PORT);
    let (package_manager, args) = package_manager_dev_command(root);
    let command_display = std::iter::once(package_manager.as_str())
        .chain(args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    Ok(NextjsDevServerProbeSpec {
        package_manager,
        args,
        command_display,
        port,
        route: DEV_SERVER_ROUTE.to_string(),
    })
}

fn script_contains_next_dev(script: &str) -> bool {
    let lower = script.to_ascii_lowercase();
    lower.contains("next") && lower.contains("dev")
}

fn parse_next_dev_port(script: &str) -> Option<u16> {
    let tokens = script.split_whitespace().collect::<Vec<_>>();
    for (index, token) in tokens.iter().enumerate() {
        if matches!(*token, "-p" | "--port")
            && let Some(raw) = tokens.get(index + 1)
            && let Ok(port) = raw.parse::<u16>()
        {
            return Some(port);
        }
        if let Some(raw) = token.strip_prefix("-p")
            && !raw.is_empty()
            && let Ok(port) = raw.parse::<u16>()
        {
            return Some(port);
        }
        if let Some(raw) = token.strip_prefix("--port=")
            && let Ok(port) = raw.parse::<u16>()
        {
            return Some(port);
        }
    }
    None
}

fn package_manager_dev_command(root: &Path) -> (String, Vec<String>) {
    if root.join("pnpm-lock.yaml").is_file() {
        return (
            "pnpm".to_string(),
            vec!["run".to_string(), "dev".to_string()],
        );
    }
    if root.join("yarn.lock").is_file() {
        return ("yarn".to_string(), vec!["dev".to_string()]);
    }
    (
        "npm".to_string(),
        vec!["run".to_string(), "dev".to_string()],
    )
}

fn localhost_port_accepts_connection(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(150)).is_ok()
}

fn http_get_local_route(port: u16, route: &str) -> Result<HttpProbeResult, String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, NEXTJS_DEV_SERVER_CONNECT_TIMEOUT)
        .map_err(|err| err.to_string())?;
    let _ = stream.set_read_timeout(Some(NEXTJS_DEV_SERVER_CONNECT_TIMEOUT));
    let _ = stream.set_write_timeout(Some(NEXTJS_DEV_SERVER_CONNECT_TIMEOUT));
    let path = if route.starts_with('/') {
        route.to_string()
    } else {
        format!("/{route}")
    };
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nUser-Agent: anvilminimal-dev-server-probe\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|err| err.to_string())?;
    let mut response = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                response.extend_from_slice(&buffer[..n]);
                if response.len() >= 32_768 {
                    break;
                }
            }
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(err) => return Err(err.to_string()),
        }
    }
    let response_text = String::from_utf8_lossy(&response).to_string();
    let status_line = response_text
        .lines()
        .next()
        .ok_or_else(|| "empty_http_response".to_string())?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| "http_status_missing".to_string())?
        .parse::<i64>()
        .map_err(|_| "http_status_invalid".to_string())?;
    let body = response_text
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or(&response_text);
    Ok(HttpProbeResult {
        status,
        body_excerpt: eval_events::body_snippet(body),
    })
}

fn dev_server_spawn_failure_kind(err: &std::io::Error) -> String {
    match err.kind() {
        std::io::ErrorKind::NotFound => "browser_unavailable:dev_server_command_missing",
        std::io::ErrorKind::PermissionDenied => "browser_unavailable:dev_server_command_denied",
        _ => "browser_unavailable:dev_server_spawn_failed",
    }
    .to_string()
}

fn classify_dev_server_startup_failure(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    if lower.contains("eaddrinuse") || lower.contains("address already in use") {
        return Some("port_in_use".to_string());
    }
    if lower.contains("eacces")
        || lower.contains("permission denied")
        || lower.contains("operation not permitted")
    {
        return Some("bind_denied".to_string());
    }
    if tailwind_dev_pipeline_failure(&lower) {
        return Some("tailwind_dev_pipeline_failure".to_string());
    }
    None
}

fn classify_dev_route_failure_kind(status: i64, body_excerpt: &str) -> Option<String> {
    if status < 400 {
        return None;
    }
    let lower = body_excerpt.to_ascii_lowercase();
    if tailwind_dev_pipeline_failure(&lower) {
        return Some("tailwind_dev_pipeline_failure".to_string());
    }
    Some(format!("http_{status}"))
}

fn classify_dev_server_env_conflict(failure_kind: &str, output: &str) -> String {
    if verifier_env::is_env_node_env_conflict_output(output) {
        verifier_env::ENV_NODE_ENV_CONFLICT_KIND.to_string()
    } else {
        failure_kind.to_string()
    }
}

fn dev_server_output_excerpt(failure_kind: &str, output: &str) -> String {
    if failure_kind == verifier_env::ENV_NODE_ENV_CONFLICT_KIND {
        verifier_env::with_env_node_env_remediation(output)
    } else {
        output.to_string()
    }
}

fn tailwind_dev_pipeline_failure(lower_text: &str) -> bool {
    lower_text.contains("@tailwind")
        && (lower_text.contains("module parse failed")
            || lower_text.contains("unexpected character")
            || lower_text.contains("postcss")
            || lower_text.contains("tailwind"))
}

#[derive(Debug)]
struct DevServerCleanup {
    ok: bool,
    output_excerpt: String,
}

fn cleanup_dev_server_child(mut child: Child) -> DevServerCleanup {
    let _ = child.kill();
    match child.wait_with_output() {
        Ok(output) => DevServerCleanup {
            ok: true,
            output_excerpt: output_excerpt(&output),
        },
        Err(err) => DevServerCleanup {
            ok: false,
            output_excerpt: err.to_string(),
        },
    }
}

fn output_excerpt(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    eval_events::body_snippet(&format!("{stdout}\n{stderr}"))
}

fn emit_dev_server_unavailable_lifecycle(
    config: &Config,
    port: u16,
    route: &str,
    command: &str,
    failure_kind: &str,
    evidence_path: &Path,
) {
    emit_dev_server_lifecycle_stage(
        config,
        "start",
        false,
        port,
        route,
        command,
        Some(failure_kind),
        None,
        evidence_path,
        None,
    );
    emit_dev_server_lifecycle_stage(
        config,
        "wait",
        false,
        port,
        route,
        command,
        Some(failure_kind),
        None,
        evidence_path,
        None,
    );
    emit_dev_server_lifecycle_stage(
        config,
        "probe",
        false,
        port,
        route,
        command,
        Some(failure_kind),
        None,
        evidence_path,
        None,
    );
    emit_dev_server_lifecycle_stage(
        config,
        "cleanup",
        true,
        port,
        route,
        command,
        Some(failure_kind),
        None,
        evidence_path,
        None,
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_dev_server_lifecycle_stage(
    config: &Config,
    stage: &str,
    ok: bool,
    port: u16,
    route: &str,
    command: &str,
    failure_kind: Option<&str>,
    http_status: Option<i64>,
    evidence_path: &Path,
    pid: Option<u32>,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "dev_server_lifecycle",
            "profile": "nextjs",
            "stage": stage,
            "ok": ok,
            "port": port,
            "route": route,
            "command": command,
            "failure_kind": failure_kind.unwrap_or(""),
            "http_status": http_status,
            "pid": pid,
            "evidence_path": evidence_path.display().to_string(),
            "lifecycle_stages": DEV_SERVER_LIFECYCLE_STAGES,
            "probe_environment": dev_server_probe_environment(port),
        }),
    );
}

fn dev_server_probe_environment(port: u16) -> Value {
    json!({
        "NODE_ENV": "",
        "NODE_OPTIONS": "",
        "NEXT_TELEMETRY_DISABLED": "1",
        "PORT": port.to_string(),
        "host_env_contamination": verifier_env::host_env_contamination(),
        "ANVIL_DEV_SERVER_PROBE": std::env::var("ANVIL_DEV_SERVER_PROBE").unwrap_or_default(),
        "ANVIL_TEST_DEV_SERVER_PROBE": std::env::var("ANVIL_TEST_DEV_SERVER_PROBE").unwrap_or_default(),
    })
}

fn dev_server_unavailable_evidence(
    port: u16,
    route: &str,
    command: &str,
    failure_kind: &str,
    output_excerpt: &str,
) -> Value {
    json!({
        "status": "unavailable",
        "browser_failure_kind": failure_kind,
        "failure_kind": failure_kind,
        "dev_server": {
            "profile": "nextjs",
            "port": port,
            "route": route,
            "command": command,
            "failure_kind": failure_kind,
            "output_excerpt": eval_events::body_snippet(output_excerpt),
            "lifecycle_stages": DEV_SERVER_LIFECYCLE_STAGES,
            "probe_environment": dev_server_probe_environment(port),
        }
    })
}

fn dev_server_failed_evidence(
    port: u16,
    route: &str,
    command: &str,
    http_status: i64,
    failure_kind: &str,
    body_excerpt: &str,
    output_excerpt: &str,
) -> Value {
    json!({
        "status": "failed",
        "ok": false,
        "http_status": http_status,
        "route_rendered": false,
        "browser_failure_kind": failure_kind,
        "failure_kind": failure_kind,
        "body_excerpt": eval_events::body_snippet(body_excerpt),
        "dev_server": {
            "profile": "nextjs",
            "port": port,
            "route": route,
            "command": command,
            "failure_kind": failure_kind,
            "output_excerpt": eval_events::body_snippet(output_excerpt),
            "lifecycle_stages": DEV_SERVER_LIFECYCLE_STAGES,
            "probe_environment": dev_server_probe_environment(port),
        }
    })
}

fn dev_server_passed_evidence(
    port: u16,
    route: &str,
    command: &str,
    http_status: i64,
    body_excerpt: &str,
) -> Value {
    json!({
        "status": "ready",
        "ok": true,
        "http_status": http_status,
        "route_rendered": true,
        "dev_server": {
            "profile": "nextjs",
            "port": port,
            "route": route,
            "command": command,
            "body_excerpt": eval_events::body_snippet(body_excerpt),
            "lifecycle_stages": DEV_SERVER_LIFECYCLE_STAGES,
            "probe_environment": dev_server_probe_environment(port),
        }
    })
}

fn read_release_evidence(
    config: &Config,
    names: &[&str],
    missing_reason: &'static str,
    kind: ReleaseEvidenceKind,
) -> ReleaseEvidence {
    for path in release_evidence_candidate_paths(config, names) {
        if !path.is_file() {
            continue;
        }
        let display = path.display().to_string();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return ReleaseEvidence {
                status: ReleaseEvidenceStatus::Failed("evidence_unreadable".to_string()),
                path: display,
            };
        };
        let Ok(json) = serde_json::from_str::<Value>(&text) else {
            return ReleaseEvidence {
                status: ReleaseEvidenceStatus::Failed("evidence_invalid_json".to_string()),
                path: display,
            };
        };
        return ReleaseEvidence {
            status: classify_release_evidence_json(kind, &json),
            path: display,
        };
    }
    ReleaseEvidence {
        status: ReleaseEvidenceStatus::Unavailable(missing_reason.to_string()),
        path: String::new(),
    }
}

fn release_evidence_candidate_paths(config: &Config, names: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(events_path) = &config.eval_events_path
        && let Some(run_dir) = events_path.parent()
    {
        for name in names {
            out.push(run_dir.join(name));
        }
    }
    for name in names {
        out.push(
            config
                .workspace_root
                .join(".anvil")
                .join("evidence")
                .join(name),
        );
        out.push(config.workspace_root.join(".anvil").join(name));
        out.push(config.workspace_root.join(name));
    }
    out
}

fn release_evidence_extra_dirs(config: &Config) -> Vec<PathBuf> {
    config
        .eval_events_path
        .as_ref()
        .and_then(|events_path| events_path.parent())
        .map(|run_dir| vec![run_dir.to_path_buf()])
        .unwrap_or_default()
}

fn classify_release_evidence_json(
    kind: ReleaseEvidenceKind,
    value: &Value,
) -> ReleaseEvidenceStatus {
    let details = value
        .get("browser_details")
        .or_else(|| value.get("details"))
        .filter(|value| value.is_object());
    let text_status = text_field_deep(value, details, &["status"]);
    if let Some(status) = text_status.as_deref()
        && is_release_evidence_unavailable_status(status)
    {
        return ReleaseEvidenceStatus::Unavailable(evidence_unavailable_reason(
            value, details, status,
        ));
    }
    if let Some(status) =
        numeric_field_deep(value, details, &["http_status", "status", "status_code"])
        && status >= 400
    {
        return ReleaseEvidenceStatus::Failed(evidence_http_failure_reason(value, details, status));
    }
    if let Some(success) = bool_field_deep(
        value,
        details,
        &["ok", "success", "browser_success", "interaction_success"],
    ) && !success
    {
        return ReleaseEvidenceStatus::Failed(evidence_failure_reason(value, details));
    }
    if let Some(reason) = explicit_release_evidence_failure(kind, value, details) {
        return ReleaseEvidenceStatus::Failed(reason);
    }
    if let Some(status) = text_status.as_deref()
        && matches!(status, "failed" | "fail" | "error")
    {
        return ReleaseEvidenceStatus::Failed(evidence_failure_reason(value, details));
    }
    if let Some(kind_value) = text_field_deep(
        value,
        details,
        &["browser_failure_kind", "failure_kind", "error_kind"],
    ) && !kind_value.is_empty()
    {
        return ReleaseEvidenceStatus::Failed(kind_value);
    }
    if release_evidence_has_required_detail(kind, value, details) {
        return ReleaseEvidenceStatus::Passed;
    }
    let status_is_pass_like = text_status
        .as_deref()
        .is_some_and(|status| matches!(status, "ok" | "pass" | "passed" | "ready"));
    let success_is_true = bool_field_deep(
        value,
        details,
        &["ok", "success", "browser_success", "interaction_success"],
    ) == Some(true);
    let http_is_ok = numeric_field_deep(value, details, &["http_status", "status", "status_code"])
        .is_some_and(|status| (200..400).contains(&status));
    if success_is_true || status_is_pass_like || http_is_ok {
        return ReleaseEvidenceStatus::Unavailable(
            match kind {
                ReleaseEvidenceKind::BrowserReadiness => "browser_render_evidence_missing",
                ReleaseEvidenceKind::Interaction => "interaction_detail_missing",
            }
            .to_string(),
        );
    }
    ReleaseEvidenceStatus::Unavailable("evidence_inconclusive".to_string())
}

fn explicit_release_evidence_failure(
    kind: ReleaseEvidenceKind,
    value: &Value,
    details: Option<&Value>,
) -> Option<String> {
    match kind {
        ReleaseEvidenceKind::BrowserReadiness => {
            if bool_field_deep(
                value,
                details,
                &["route_rendered", "rendered", "page_loaded", "dom_ready"],
            ) == Some(false)
            {
                return Some("browser_route_not_rendered".to_string());
            }
        }
        ReleaseEvidenceKind::Interaction => {
            if bool_field_deep(value, details, &["canvas_found", "canvas_available"]) == Some(false)
            {
                return Some("canvas_unavailable".to_string());
            }
            if bool_field_deep(
                value,
                details,
                &["interactive_surface", "interaction_surface"],
            ) == Some(false)
            {
                return Some("interactive_surface_missing".to_string());
            }
            if bool_field_deep(
                value,
                details,
                &[
                    "input_event_observed",
                    "keyboard_event_observed",
                    "pointer_event_observed",
                ],
            ) == Some(false)
            {
                return Some("input_event_missing".to_string());
            }
            if bool_field_deep(value, details, &["state_changed", "visible_state_changed"])
                == Some(false)
            {
                return Some("interaction_state_change_missing".to_string());
            }
        }
    }
    None
}

fn release_evidence_has_required_detail(
    kind: ReleaseEvidenceKind,
    value: &Value,
    details: Option<&Value>,
) -> bool {
    match kind {
        ReleaseEvidenceKind::BrowserReadiness => {
            bool_field_deep(
                value,
                details,
                &["route_rendered", "rendered", "page_loaded", "dom_ready"],
            ) == Some(true)
        }
        ReleaseEvidenceKind::Interaction => {
            bool_field_deep(
                value,
                details,
                &[
                    "interaction_performed",
                    "basic_interaction",
                    "interaction_success",
                    "input_event_observed",
                    "keyboard_event_observed",
                    "pointer_event_observed",
                    "state_changed",
                    "visible_state_changed",
                ],
            ) == Some(true)
        }
    }
}

fn evidence_failure_reason(value: &Value, details: Option<&Value>) -> String {
    let text_reason = text_field_deep(
        value,
        details,
        &[
            "browser_failure_kind",
            "failure_kind",
            "error_kind",
            "status",
        ],
    );
    if text_reason
        .as_deref()
        .is_some_and(prefer_release_evidence_failure_kind_over_http)
    {
        return text_reason.unwrap();
    }
    if let Some(status) =
        numeric_field_deep(value, details, &["http_status", "status", "status_code"])
        && status >= 400
    {
        return format!("http_{status}");
    }
    text_reason.unwrap_or_else(|| "browser_check_failed".to_string())
}

fn evidence_http_failure_reason(value: &Value, details: Option<&Value>, status: i64) -> String {
    text_field_deep(
        value,
        details,
        &["browser_failure_kind", "failure_kind", "error_kind"],
    )
    .filter(|reason| prefer_release_evidence_failure_kind_over_http(reason))
    .unwrap_or_else(|| format!("http_{status}"))
}

fn evidence_unavailable_reason(value: &Value, details: Option<&Value>, status: &str) -> String {
    text_field_deep(
        value,
        details,
        &[
            "browser_failure_kind",
            "failure_kind",
            "error_kind",
            "reason",
        ],
    )
    .filter(|reason| !reason.is_empty())
    .unwrap_or_else(|| status.to_string())
}

fn is_release_evidence_unavailable_status(status: &str) -> bool {
    matches!(
        status,
        "not_enabled" | "adapter_not_implemented" | "unavailable" | "skipped"
    ) || status.starts_with("unavailable:")
        || status == "browser_unavailable"
        || status.starts_with("browser_unavailable:")
        || status == "skipped_offline"
        || status == "skipped_unsupported_profile"
}

fn prefer_release_evidence_failure_kind_over_http(reason: &str) -> bool {
    matches!(
        reason,
        "tailwind_dev_pipeline_failure"
            | "css_dev_pipeline_failure"
            | "nextjs_dev_pipeline_failure"
    )
}

fn bool_field_deep(value: &Value, details: Option<&Value>, keys: &[&str]) -> Option<bool> {
    bool_field(value, keys).or_else(|| details.and_then(|details| bool_field(details, keys)))
}

fn bool_field(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_bool))
}

fn numeric_field_deep(value: &Value, details: Option<&Value>, keys: &[&str]) -> Option<i64> {
    numeric_field(value, keys).or_else(|| details.and_then(|details| numeric_field(details, keys)))
}

fn numeric_field(value: &Value, keys: &[&str]) -> Option<i64> {
    for key in keys {
        let Some(raw) = value.get(*key) else {
            continue;
        };
        if let Some(number) = raw.as_i64() {
            return Some(number);
        }
        if let Some(text) = raw.as_str()
            && let Ok(number) = text.parse::<i64>()
        {
            return Some(number);
        }
    }
    None
}

fn text_field_deep(value: &Value, details: Option<&Value>, keys: &[&str]) -> Option<String> {
    text_field(value, keys).or_else(|| details.and_then(|details| text_field(details, keys)))
}

fn text_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(|text| text.trim().to_ascii_lowercase())
}

fn release_gate_final_acceptance_status(release_gate: &ReleaseGateSummary) -> &'static str {
    match release_gate.status.as_str() {
        "pass" | "not_applicable" => "full_success",
        "partial" => "partial",
        "failed" => "incomplete",
        _ => "incomplete",
    }
}

fn runtime_acceptance_status(
    runtime_ok: bool,
    report: Option<&crate::minimal_loop::evidence::RuntimeAcceptanceReport>,
) -> &'static str {
    match report {
        Some(report) if report.inconclusive => "inconclusive",
        Some(_) if runtime_ok => "pass",
        Some(_) => "failed",
        None => "not_checked",
    }
}

fn release_quality_completion_status(
    release_gate: &ReleaseGateSummary,
    final_acceptance_status: &str,
) -> &'static str {
    match release_gate.status.as_str() {
        "pass" | "not_applicable" => "release_ready",
        "partial" => "partial",
        "failed" => "failed",
        _ if final_acceptance_status == "partial" => "partial",
        _ => "failed",
    }
}

fn release_gate_next_action(
    release_gate: &ReleaseGateSummary,
    final_acceptance_status: &str,
) -> &'static str {
    match release_gate.status.as_str() {
        "partial" => "collect_missing_release_evidence_or_continue_release_recovery",
        "failed" => "repair_release_gate_failure",
        _ if final_acceptance_status == "partial" => "collect_missing_final_acceptance_evidence",
        _ => "none",
    }
}

fn release_recovery_needed(
    release_gate: &ReleaseGateSummary,
    final_acceptance_status: &str,
) -> bool {
    matches!(release_gate.status.as_str(), "partial" | "failed")
        || matches!(final_acceptance_status, "partial" | "failed" | "incomplete")
}

fn release_recovery_acceptance_layer(
    release_gate: &ReleaseGateSummary,
    final_acceptance_status: &str,
) -> &'static str {
    match release_gate.status.as_str() {
        "partial" | "failed" => "release_gate",
        _ if final_acceptance_status == "partial" => "final_acceptance_partial",
        _ => "final_acceptance",
    }
}

fn release_recovery_failure_kind(
    release_gate: &ReleaseGateSummary,
    final_acceptance_status: &str,
    primary_reason: &str,
) -> String {
    if release_gate.status == "partial" {
        if release_gate
            .reasons
            .iter()
            .any(|reason| reason.contains("browser_readiness_or_interaction_evidence_required"))
            || release_gate
                .browser_readiness_status
                .starts_with("unavailable:")
            || release_gate
                .browser_readiness_status
                .contains("browser_readiness_evidence_missing")
            || release_gate
                .browser_readiness_status
                .contains("browser_render_evidence_missing")
        {
            return "browser_readiness_missing".to_string();
        }
        if release_gate
            .interaction_evidence_status
            .contains("interaction_evidence_missing")
        {
            return "browser_interaction_evidence_missing".to_string();
        }
        return "release_gate_partial".to_string();
    }
    if release_gate.status == "failed" {
        if release_gate
            .browser_readiness_status
            .contains(verifier_env::ENV_NODE_ENV_CONFLICT_KIND)
            || release_gate
                .reasons
                .iter()
                .any(|reason| reason.contains(verifier_env::ENV_NODE_ENV_CONFLICT_KIND))
        {
            return verifier_env::ENV_NODE_ENV_CONFLICT_KIND.to_string();
        }
        if release_gate
            .browser_readiness_status
            .contains("tailwind_dev_pipeline_failure")
        {
            return "tailwind_dev_pipeline_failure".to_string();
        }
        if release_gate.browser_readiness_status.starts_with("failed:")
            || release_gate
                .reasons
                .iter()
                .any(|reason| reason.contains("browser_readiness_failed"))
        {
            return "browser_readiness_failed".to_string();
        }
        if release_gate
            .interaction_evidence_status
            .starts_with("failed:")
            || release_gate
                .reasons
                .iter()
                .any(|reason| reason.contains("browser_interaction_failed"))
        {
            return "browser_interaction_failed".to_string();
        }
        return "release_gate_failed".to_string();
    }
    if final_acceptance_status == "partial" {
        "final_acceptance_partial".to_string()
    } else if primary_reason == "ok" {
        "final_acceptance_recovery_required".to_string()
    } else {
        "final_acceptance_failed".to_string()
    }
}

fn release_recovery_failure_evidence(
    release_gate: &ReleaseGateSummary,
    final_acceptance_status: &str,
    primary_reason: &str,
    runtime_acceptance: Option<&crate::minimal_loop::evidence::RuntimeAcceptanceReport>,
) -> Vec<String> {
    let mut evidence = Vec::new();
    evidence.push(format!(
        "failed acceptance layer: {}",
        release_recovery_acceptance_layer(release_gate, final_acceptance_status)
    ));
    evidence.push(format!(
        "final acceptance status: {final_acceptance_status}"
    ));
    evidence.push(format!("release gate status: {}", release_gate.status));
    if primary_reason != "ok" {
        evidence.push(format!("primary reason: {primary_reason}"));
    }
    evidence.extend(
        release_gate
            .reasons
            .iter()
            .map(|reason| format!("release gate reason: {reason}")),
    );
    evidence.push(format!(
        "browser readiness: {}",
        release_gate.browser_readiness_status
    ));
    if release_gate
        .browser_readiness_status
        .contains(verifier_env::ENV_NODE_ENV_CONFLICT_KIND)
        || release_gate
            .reasons
            .iter()
            .any(|reason| reason.contains(verifier_env::ENV_NODE_ENV_CONFLICT_KIND))
    {
        evidence.push(format!(
            "host environment remediation: {}",
            verifier_env::ENV_NODE_ENV_REMEDIATION
        ));
    }
    if !release_gate.browser_readiness_evidence_path.is_empty() {
        evidence.push(format!(
            "browser readiness evidence: {}",
            release_gate.browser_readiness_evidence_path
        ));
    }
    evidence.push(format!(
        "interaction evidence: {}",
        release_gate.interaction_evidence_status
    ));
    if !release_gate.interaction_evidence_path.is_empty() {
        evidence.push(format!(
            "interaction evidence path: {}",
            release_gate.interaction_evidence_path
        ));
    }
    if let Some(report) = runtime_acceptance {
        evidence.extend(
            report
                .missing_evidence
                .iter()
                .map(|item| format!("missing runtime evidence: {item}")),
        );
        evidence.extend(
            report
                .missing_obligations
                .iter()
                .map(|item| format!("missing runtime obligation: {item}")),
        );
        evidence.extend(
            report
                .inconclusive_reasons
                .iter()
                .map(|item| format!("runtime acceptance inconclusive: {item}")),
        );
    }
    dedup_strings(evidence)
}

fn release_recovery_missing_capabilities(
    runtime_acceptance: Option<&crate::minimal_loop::evidence::RuntimeAcceptanceReport>,
) -> Vec<String> {
    runtime_acceptance
        .map(|report| report.missing_capabilities.clone())
        .unwrap_or_default()
}

fn release_recovery_repair_targets(
    release_gate: &ReleaseGateSummary,
    runtime_acceptance: Option<&crate::minimal_loop::evidence::RuntimeAcceptanceReport>,
) -> Vec<String> {
    let mut targets = Vec::new();
    let browser_status = release_gate.browser_readiness_status.to_ascii_lowercase();
    let interaction_status = release_gate
        .interaction_evidence_status
        .to_ascii_lowercase();
    if browser_status.contains("tailwind_dev_pipeline_failure")
        || browser_status.contains("css")
        || browser_status.contains("http_500")
    {
        targets.push("framework_config".to_string());
    }
    if browser_status.starts_with("unavailable:")
        || browser_status.contains("evidence_missing")
        || interaction_status.starts_with("unavailable:")
        || interaction_status.contains("evidence_missing")
    {
        targets.push("required_evidence_missing".to_string());
    }
    if browser_status.starts_with("failed:") || interaction_status.starts_with("failed:") {
        targets.push("test_or_evidence".to_string());
    }
    if let Some(report) = runtime_acceptance {
        targets.extend(
            report
                .obligation_repair_targets
                .iter()
                .map(|target| format!("{}:{}", target.obligation, target.target_path)),
        );
    }
    if targets.is_empty() {
        targets.push("release_acceptance".to_string());
    }
    dedup_strings(targets)
}

fn release_recovery_verify_commands(
    profile: &str,
    release_gate: &ReleaseGateSummary,
) -> Vec<String> {
    let mut commands = Vec::new();
    if matches!(profile, "nextjs" | "next-js" | "next.js") {
        commands.push("npm run build".to_string());
        commands.push("start dev server with npm run dev and wait for readiness".to_string());
        commands.push("probe browser route GET / and record HTTP status".to_string());
        commands.push("write browser-readiness.json with route_rendered/http_status".to_string());
        commands
            .push("collect interaction-evidence.json for required browser interaction".to_string());
    } else {
        commands.push("rerun deterministic acceptance checks for the original goal".to_string());
    }
    if release_gate.status == "partial" {
        commands.push("do not claim release_ready until release gate evidence passes".to_string());
    }
    dedup_strings(commands)
}

fn dedup_strings(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .filter(|value| !value.trim().is_empty())
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

#[allow(clippy::too_many_arguments)]
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

struct UltraPhaseRecoveryRequest<'a> {
    failure_kind: &'a str,
    reason: &'a str,
    missing_paths: &'a [String],
    missing_signals: &'a [String],
    repair_targets: &'a [String],
}

fn save_ultra_phase_recovery_handoff(
    config: &Config,
    plan: &UltraPlan,
    phase: &UltraPhase,
    request: UltraPhaseRecoveryRequest<'_>,
) -> Option<String> {
    let handoff = RecoveryHandoff {
        profile: plan.profile.clone(),
        original_goal: plan.goal.clone(),
        failed_phase: Some(phase.id.clone()),
        failed_step: None,
        failure_kind: request.failure_kind.to_string(),
        failure_evidence: vec![request.reason.to_string()],
        missing_paths: request.missing_paths.to_vec(),
        missing_capabilities: request.missing_signals.to_vec(),
        verify_commands: Vec::new(),
        changed_paths: Vec::new(),
        repair_targets: request.repair_targets.to_vec(),
    };
    let failure_kind = request.failure_kind;
    let reason = request.reason;
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
    let recovery_plan = match save_recovery_ultra_plan(&config.workspace_root, &scope, &handoff) {
        Ok(path) => Some(path),
        Err(err) => {
            let prompt_path = handoff_path(&path);
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "recovery_ultra_plan_save_failed",
                    "recovery_handoff_kind": failure_kind,
                    "phase_id": phase.id,
                    "recovery_prompt_path": prompt_path,
                    "reason": eval_events::body_snippet(&err.to_string()),
                    "recovery_yaml_missing": true,
                }),
            );
            None
        }
    };
    let validation = validate_recovery_artifacts(&path, recovery_plan.as_deref());
    let raw_prompt_command = suggested_ultra_recovery_command(&path, &plan.profile);
    let prompt_command = if validation.prompt_command_available() {
        raw_prompt_command
    } else {
        String::new()
    };
    let recovery_plan_command = recovery_plan
        .as_ref()
        .filter(|_| validation.yaml_command_available())
        .map(|path| suggested_recovery_ultra_plan_command(path));
    let prompt_path = handoff_path(&path);
    let recovery_plan_path = optional_handoff_path(recovery_plan.as_ref());
    let (completed_phases, pending_phases) = ultra_phase_status(plan, phase);
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "recovery_prompt_saved",
            "recovery_handoff_kind": failure_kind,
            "phase_id": phase.id,
            "recovery_prompt_path": &prompt_path,
            "recovery_ultra_plan_path": &recovery_plan_path,
            "recovery_yaml_missing": recovery_plan.is_none(),
            "recovery_prompt_exists": validation.prompt_exists,
            "recovery_prompt_parse_ok": validation.prompt_parse_ok,
            "recovery_prompt_parse_error": validation.prompt_parse_error.as_deref().unwrap_or_default(),
            "recovery_yaml_exists": validation.yaml_exists,
            "recovery_yaml_parse_ok": validation.yaml_parse_ok,
            "recovery_yaml_parse_error": validation.yaml_parse_error.as_deref().unwrap_or_default(),
            "recovery_command_targets_valid": validation.command_targets_valid(),
            "suggested_recovery_command": prompt_command.clone(),
            "suggested_recovery_yaml_command": recovery_plan_command.clone().unwrap_or_default(),
            "recovery_profile": plan.profile,
            "local_repair_exhausted": true,
            "status": "incomplete",
        }),
    );
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "ultra_partial_artifact_summary",
            "status": "incomplete",
            "completed_phase_ids": completed_phases.clone(),
            "failed_phase_id": phase.id,
            "pending_phase_ids": pending_phases.clone(),
            "failure_kind": failure_kind,
            "recovery_prompt_path": &prompt_path,
            "recovery_ultra_plan_path": &recovery_plan_path,
            "recovery_yaml_missing": recovery_plan.is_none(),
            "recovery_prompt_exists": validation.prompt_exists,
            "recovery_prompt_parse_ok": validation.prompt_parse_ok,
            "recovery_yaml_exists": validation.yaml_exists,
            "recovery_yaml_parse_ok": validation.yaml_parse_ok,
            "recovery_command_targets_valid": validation.command_targets_valid(),
            "suggested_recovery_command": prompt_command.clone(),
            "suggested_recovery_yaml_command": recovery_plan_command.clone().unwrap_or_default(),
        }),
    );
    let recovery_yaml_summary = recovery_plan
        .as_ref()
        .map(|path| {
            let display = handoff_path(path);
            if validation.yaml_parse_ok {
                format!("Recovery UltraPlan YAML saved: {display}")
            } else {
                format!(
                    "Recovery UltraPlan YAML invalid: {} ({})",
                    display,
                    validation
                        .yaml_parse_error
                        .as_deref()
                        .unwrap_or("recovery_yaml_invalid")
                )
            }
        })
        .unwrap_or_else(|| {
            "Recovery UltraPlan YAML missing: failed to save valid recovery plan".to_string()
        });
    let prompt_command_summary = if validation.prompt_command_available() {
        format!("Suggested command: {prompt_command}")
    } else {
        format!(
            "Suggested command: unavailable because recovery prompt validation failed ({})",
            validation
                .prompt_parse_error
                .as_deref()
                .unwrap_or("recovery_prompt_invalid")
        )
    };
    let recovery_yaml_command_summary = recovery_plan_command
        .as_ref()
        .map(|command| format!("Suggested YAML command: {command}"))
        .unwrap_or_else(|| {
            "Suggested YAML command: unavailable because recovery YAML is missing".to_string()
        });
    let artifact_check_summary = recovery_artifact_check_summary(&validation);
    eval_events::write_run_summary(
        config.eval_events_path.as_deref(),
        &render_ultra_partial_run_summary(UltraPartialRunSummary {
            completed_phases: &completed_phases,
            failed_phase: &phase.id,
            pending_phases: &pending_phases,
            failure_kind,
            reason,
            recovery_prompt_path: &prompt_path,
            recovery_yaml_summary: &recovery_yaml_summary,
            prompt_command_summary: &prompt_command_summary,
            recovery_yaml_command_summary: &recovery_yaml_command_summary,
            recovery_artifact_check: &artifact_check_summary,
        }),
    );
    let recovery_yaml_message = recovery_plan
        .as_ref()
        .zip(recovery_plan_command.as_ref())
        .map(|(path, command)| {
            let display = handoff_path(path);
            format!(
                "; incomplete; recovery YAML saved: {}; suggested YAML command: {}",
                display, command
            )
        })
        .unwrap_or_else(|| "; incomplete; recovery YAML missing".to_string());
    let prompt_message = if validation.prompt_command_available() {
        format!("suggested command: {prompt_command}")
    } else {
        "suggested command unavailable because recovery prompt validation failed".to_string()
    };
    Some(format!(
        "; repair prompt saved: {}; {}; {}; {}",
        &prompt_path,
        prompt_message,
        recovery_yaml_message.trim_start_matches("; "),
        artifact_check_summary
    ))
}

#[allow(clippy::too_many_arguments)]
fn save_release_recovery_handoff(
    config: &Config,
    profile: &str,
    original_goal: &str,
    scope: &str,
    acceptance_layer: &str,
    failure_kind: &str,
    failure_evidence: Vec<String>,
    missing_paths: Vec<String>,
    missing_capabilities: Vec<String>,
    repair_targets: Vec<String>,
    verify_commands: Vec<String>,
) -> Option<ReleaseRecoveryHandoffSummary> {
    let handoff = RecoveryHandoff {
        profile: profile.to_string(),
        original_goal: original_goal.to_string(),
        failed_phase: Some(acceptance_layer.to_string()),
        failed_step: None,
        failure_kind: failure_kind.to_string(),
        failure_evidence,
        missing_paths,
        missing_capabilities,
        verify_commands,
        changed_paths: Vec::new(),
        repair_targets,
    };
    let path = match save_ultra_recovery_prompt(&config.workspace_root, scope, &handoff) {
        Ok(path) => path,
        Err(err) => {
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "recovery_prompt_save_failed",
                    "recovery_handoff_kind": failure_kind,
                    "acceptance_layer": acceptance_layer,
                    "reason": eval_events::body_snippet(&err.to_string()),
                    "status": "incomplete",
                }),
            );
            return None;
        }
    };
    let recovery_plan = match save_recovery_ultra_plan(&config.workspace_root, scope, &handoff) {
        Ok(path) => Some(path),
        Err(err) => {
            let prompt_path = handoff_path(&path);
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "recovery_ultra_plan_save_failed",
                    "recovery_handoff_kind": failure_kind,
                    "acceptance_layer": acceptance_layer,
                    "recovery_prompt_path": prompt_path,
                    "reason": eval_events::body_snippet(&err.to_string()),
                    "recovery_yaml_missing": true,
                    "status": "incomplete",
                }),
            );
            None
        }
    };
    let validation = validate_recovery_artifacts(&path, recovery_plan.as_deref());
    let raw_prompt_command = suggested_ultra_recovery_command(&path, profile);
    let prompt_command = if validation.prompt_command_available() {
        raw_prompt_command
    } else {
        String::new()
    };
    let recovery_plan_command = recovery_plan
        .as_ref()
        .filter(|_| validation.yaml_command_available())
        .map(|path| suggested_recovery_ultra_plan_command(path));
    let prompt_path = handoff_path(&path);
    let recovery_plan_path = optional_handoff_path(recovery_plan.as_ref());
    let summary = ReleaseRecoveryHandoffSummary {
        recovery_handoff_kind: failure_kind.to_string(),
        acceptance_layer: acceptance_layer.to_string(),
        recovery_prompt_path: prompt_path,
        recovery_ultra_plan_path: recovery_plan_path,
        suggested_recovery_command: prompt_command.clone(),
        suggested_recovery_yaml_command: recovery_plan_command.clone().unwrap_or_default(),
    };
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "recovery_prompt_saved",
            "recovery_handoff_kind": failure_kind,
            "acceptance_layer": acceptance_layer,
            "recovery_scope": scope,
            "recovery_prompt_path": &summary.recovery_prompt_path,
            "recovery_ultra_plan_path": &summary.recovery_ultra_plan_path,
            "recovery_yaml_missing": recovery_plan.is_none(),
            "recovery_prompt_exists": validation.prompt_exists,
            "recovery_prompt_parse_ok": validation.prompt_parse_ok,
            "recovery_prompt_parse_error": validation.prompt_parse_error.as_deref().unwrap_or_default(),
            "recovery_yaml_exists": validation.yaml_exists,
            "recovery_yaml_parse_ok": validation.yaml_parse_ok,
            "recovery_yaml_parse_error": validation.yaml_parse_error.as_deref().unwrap_or_default(),
            "recovery_command_targets_valid": validation.command_targets_valid(),
            "suggested_recovery_command": &summary.suggested_recovery_command,
            "suggested_recovery_yaml_command": &summary.suggested_recovery_yaml_command,
            "recovery_profile": profile,
            "release_acceptance_handoff": true,
            "handoff_saved_not_success": true,
            "status": "incomplete",
        }),
    );
    eval_events::append_run_summary(
        config.eval_events_path.as_deref(),
        &render_release_recovery_handoff_summary(&summary, &validation),
    );
    Some(summary)
}

fn render_release_recovery_handoff_summary(
    summary: &ReleaseRecoveryHandoffSummary,
    validation: &RecoveryArtifactValidation,
) -> String {
    format!(
        "Recovery next action:\n\
- Status: incomplete_release_acceptance\n\
- Failed acceptance layer: {}\n\
- Recovery handoff kind: {}\n\
- Recovery prompt saved: {}\n\
- Recovery UltraPlan YAML saved: {}\n\
- Suggested command: {}\n\
- Suggested YAML command: {}\n\
- Recovery artifact check: {}",
        summary.acceptance_layer,
        summary.recovery_handoff_kind,
        missing_if_empty(&summary.recovery_prompt_path),
        missing_if_empty(&summary.recovery_ultra_plan_path),
        missing_if_empty(&summary.suggested_recovery_command),
        missing_if_empty(&summary.suggested_recovery_yaml_command),
        recovery_artifact_check_summary(validation),
    )
}

fn missing_if_empty(value: &str) -> &str {
    if value.is_empty() { "missing" } else { value }
}

struct UltraPartialRunSummary<'a> {
    completed_phases: &'a [String],
    failed_phase: &'a str,
    pending_phases: &'a [String],
    failure_kind: &'a str,
    reason: &'a str,
    recovery_prompt_path: &'a str,
    recovery_yaml_summary: &'a str,
    prompt_command_summary: &'a str,
    recovery_yaml_command_summary: &'a str,
    recovery_artifact_check: &'a str,
}

fn render_ultra_partial_run_summary(summary: UltraPartialRunSummary<'_>) -> String {
    format!(
        "Status: incomplete\n\n\
Completed phases:\n{}\n\n\
Failed phase:\n- {} ({})\n\n\
Pending phases:\n{}\n\n\
Recovery next action:\n- {}\n- Recovery prompt saved: {}\n- {}\n- {}\n- {}\n\n\
Failure:\n{}",
        render_summary_bullets(summary.completed_phases),
        summary.failed_phase,
        summary.failure_kind,
        render_summary_bullets(summary.pending_phases),
        summary.recovery_yaml_summary,
        summary.recovery_prompt_path,
        summary.prompt_command_summary,
        summary.recovery_yaml_command_summary,
        summary.recovery_artifact_check,
        eval_events::body_snippet(summary.reason),
    )
}

fn render_summary_bullets(items: &[String]) -> String {
    if items.is_empty() {
        "- none".to_string()
    } else {
        items
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn ultra_phase_status(plan: &UltraPlan, failed_phase: &UltraPhase) -> (Vec<String>, Vec<String>) {
    let failed_index = plan
        .phases
        .iter()
        .position(|phase| phase.id == failed_phase.id)
        .unwrap_or(0);
    let completed = plan
        .phases
        .iter()
        .take(failed_index)
        .map(|phase| phase.id.clone())
        .collect();
    let pending = plan
        .phases
        .iter()
        .skip(failed_index + 1)
        .map(|phase| phase.id.clone())
        .collect();
    (completed, pending)
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
            "pending_capability_evidence": context.pending_capability_evidence.clone(),
            "pending_capability_evidence_count": context.pending_capability_evidence.len(),
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
            "pending_capability_evidence": context.pending_capability_evidence.clone(),
            "pending_capability_evidence_count": context.pending_capability_evidence.len(),
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
            "pending_capability_evidence": context.pending_capability_evidence.clone(),
            "pending_capability_evidence_count": context.pending_capability_evidence.len(),
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

fn collect_step_verify_commands(plan: &StepPlan) -> Vec<String> {
    plan.steps
        .iter()
        .flat_map(|step| step.verify.iter().cloned())
        .collect()
}

fn emit_planner_verify_command_normalized(
    config: &Config,
    provider: &str,
    model: &str,
    attempt: usize,
    before: &[String],
    after: &[String],
) {
    if before == after {
        return;
    }
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "planner_verify_command_normalized",
            "planner_stage": "verify_policy",
            "planner_provider": provider,
            "planner_model": model,
            "repair_attempt": attempt,
            "before_count": before.len(),
            "after_count": after.len(),
            "contains_safe_shell_split": before.iter().any(|command| command.contains("&&")),
            "normalization_source": "deterministic_verify_policy",
            "original_command_hash": stable_command_list_hash(before),
            "original_command_summary": command_list_summary(before),
            "normalized_command_hash": stable_command_list_hash(after),
            "normalized_commands": after.iter().map(|command| eval_events::body_snippet(command)).collect::<Vec<_>>(),
            "before_preview": eval_events::body_snippet(&before.join(" | ")),
            "after_preview": eval_events::body_snippet(&after.join(" | ")),
        }),
    );
}

fn stable_command_list_hash(commands: &[String]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for command in commands {
        for byte in eval_events::body_snippet(command).bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn command_list_summary(commands: &[String]) -> String {
    eval_events::body_snippet(&commands.join(" | "))
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
    } else if report.has_category("dependency_order") {
        ("dependency_order", "verify_dependency_order_error")
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
    let runtime_contract = profile_runtime_contract(&plan.profile, &plan.intent, &plan.goal);
    let phase_contract_text = format!("{}\n{}", plan.goal, phase.prompt);
    let required_capabilities = inferred_required_capabilities(&plan.profile, &phase_contract_text);
    let required_evidence =
        inferred_required_evidence(&plan.profile, &phase_contract_text, &required_capabilities);
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
    let capability_section = if required_capabilities.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nRequired final capabilities:\n{}",
            required_capabilities
                .iter()
                .map(|capability| format!("- {capability}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    let evidence_section = if required_evidence.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nRequired final evidence:\n{}",
            required_evidence
                .iter()
                .map(|evidence| format!("- {evidence}"))
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
    let unmet_final_requirements = context.render_unmet_final_requirements_section();
    format!(
        "Original ultra goal: {}\nProfile: {}\nStyle: {}\nIntent: {}\nPhase id: {}\nPhase task: {}\n\nWorkspace snapshot:\n{}\n\n{}\n\n{}\n\nProfile runtime contract:\n{}\n\nDeterministic verification preference:\n{}\n{}{}{}",
        plan.goal,
        plan.profile,
        plan.style,
        plan.intent,
        phase.id,
        phase.prompt,
        workspace_snapshot,
        prior_context,
        unmet_final_requirements,
        runtime_contract,
        preferred_verify,
        required,
        capability_section,
        evidence_section
    )
}

fn profile_auto_repair_continuation_prompt(
    plan: &UltraPlan,
    phase: &UltraPhase,
    failed_report: &VerificationReport,
    context: &UltraRunContext,
    expected_paths: &[String],
) -> String {
    let expected = if expected_paths.is_empty() {
        "- none".to_string()
    } else {
        expected_paths
            .iter()
            .map(|path| format!("- {path}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "Continue the current ultra phase after deterministic profile repair.\n\n\
Original ultra goal:\n{goal}\n\n\
Profile: {profile}\nIntent: {intent}\nPhase id: {phase_id}\nPhase task:\n{phase_task}\n\n\
Profile repair result:\n- deterministic profile repair may have materialized framework files\n- original profile failure: {failure}\n\n\
Expected profile artifacts:\n{expected}\n\n\
{prior_context}\n\n\
Continuation rules:\n\
- Treat the materialized profile files as a recovery scaffold only, not as task completion.\n\
- Continue with task-specific implementation details from the original ultra goal and phase task.\n\
- Prefer editing the real entrypoint or implementation files over adding metadata-only files.\n\
- Keep this continuation bounded to one repair turn budget; do not start a new planning cycle.\n\
- Run or preserve deterministic verification where practical, then stop.",
        goal = plan.goal,
        profile = plan.profile,
        intent = plan.intent,
        phase_id = phase.id,
        phase_task = phase.prompt,
        failure = failed_report.primary_reason(),
        expected = expected,
        prior_context = context.render_prompt_section(),
    )
}

fn final_acceptance_repair_expected_paths(
    plan: &UltraPlan,
    config: &Config,
    report: &VerificationReport,
) -> anyhow::Result<Vec<String>> {
    let mut expected = profile_expected_paths(&config.workspace_root, &plan.profile, &plan.goal);
    if let Some(contract) = CompletionContract::load_for_config(config)? {
        merge_unique_strings(&mut expected, &contract.required_paths);
    }
    merge_unique_strings(&mut expected, &report.missing_paths);
    merge_unique_strings(&mut expected, &obligation_repair_target_paths(report));
    Ok(expected)
}

fn obligation_repair_target_paths(report: &VerificationReport) -> Vec<String> {
    report
        .profile_failures
        .iter()
        .filter_map(|failure| {
            failure
                .strip_prefix("missing_required_obligation_target:")
                .and_then(|rest| rest.split_once(':'))
                .map(|(_, path)| path.trim().to_string())
                .filter(|path| !path.is_empty())
        })
        .collect()
}

fn final_acceptance_repair_prompt(
    plan: &UltraPlan,
    report: &VerificationReport,
    context: &UltraRunContext,
    repair_target: &str,
    expected_paths: &[String],
    attempt: usize,
    max_attempts: usize,
) -> String {
    let expected = render_prompt_bullets(expected_paths);
    let missing = render_prompt_bullets(&report.missing_paths);
    let dependencies = render_prompt_bullets(&report.dependency_missing);
    let profile_failures = render_prompt_bullets(&report.profile_failures);
    let command_failures = command_failure_summaries(report);
    let command_failures = render_prompt_bullets(&command_failures);
    format!(
        "Repair the final acceptance failure for the current ultra run.\n\n\
Original ultra goal:\n{goal}\n\n\
Profile: {profile}\nIntent: {intent}\n\n\
Final acceptance failure:\n\
- primary reason: {primary_reason}\n\
- repair target: {repair_target}\n\
- attempt: {attempt}/{max_attempts}\n\n\
Missing paths:\n{missing}\n\n\
Dependency failures:\n{dependencies}\n\n\
Command failures:\n{command_failures}\n\n\
Profile failures:\n{profile_failures}\n\n\
Expected paths to preserve or create:\n{expected}\n\n\
{prior_context}\n\n\
Bounded repair rules:\n\
- This is a bounded final acceptance repair, not a new planning cycle.\n\
- Repair the concrete missing or failed acceptance obligations without weakening verification, package scripts, or profile contracts.\n\
- If a scaffold exists, continue task-specific implementation instead of treating scaffold/build-only output as complete.\n\
- Prefer the smallest necessary file changes, then stop.",
        goal = plan.goal,
        profile = plan.profile,
        intent = plan.intent,
        primary_reason = report.primary_reason(),
        repair_target = repair_target,
        attempt = attempt,
        max_attempts = max_attempts,
        missing = missing,
        dependencies = dependencies,
        command_failures = command_failures,
        profile_failures = profile_failures,
        expected = expected,
        prior_context = context.render_prompt_section(),
    )
}

fn render_prompt_bullets(items: &[String]) -> String {
    if items.is_empty() {
        "- none".to_string()
    } else {
        items
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn run_profile_repair_with_ultra_session(
    execution: &mut dyn ChatClient,
    ultra_session: &mut SessionSnapshot,
    repair_prompt: &str,
    expected_paths: &[String],
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<RunSessionOutcome> {
    run_session_with_outcome_with_options(
        execution,
        ultra_session,
        repair_prompt,
        expected_paths,
        config,
        ui,
        RunSessionOptions::plan_step(RunSessionStepKind::Implement),
    )
}

fn run_final_acceptance_repair_with_ultra_session(
    execution: &mut dyn ChatClient,
    ultra_session: &mut SessionSnapshot,
    repair_prompt: &str,
    expected_paths: &[String],
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<RunSessionOutcome> {
    run_session_with_outcome_with_options(
        execution,
        ultra_session,
        repair_prompt,
        expected_paths,
        config,
        ui,
        RunSessionOptions::plan_step(RunSessionStepKind::Implement),
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
    fn dev_server_marker_with_contamination_is_env_node_env_conflict() {
        let status = run_ignored_runner_harness(
            "planner::runner::tests::dev_server_marker_with_contamination_is_env_node_env_conflict_child",
        );
        assert!(status.success(), "{status}");
    }

    #[test]
    #[ignore]
    fn dev_server_marker_with_contamination_is_env_node_env_conflict_child() {
        let output = "warn - You are using a non-standard \"NODE_ENV\" value.";
        let kind = classify_dev_server_env_conflict("http_500", output);
        assert_eq!(kind, verifier_env::ENV_NODE_ENV_CONFLICT_KIND);
        assert!(
            dev_server_output_excerpt(&kind, output)
                .contains(verifier_env::ENV_NODE_ENV_REMEDIATION)
        );
    }

    #[test]
    fn plan_artifact_saved() {
        let dir = tempfile::tempdir().unwrap();
        let plan = StepPlan::single("goal");
        let path = save_step_plan(dir.path(), &plan).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn implement_contract_setup_authority_requires_setup_purpose_step_or_phase() {
        let implement = PlanStep {
            id: "app".to_string(),
            kind: "implement".to_string(),
            expected_result: "pass".to_string(),
            instruction: "Build the app".to_string(),
            expected_paths: Vec::new(),
            verify: Vec::new(),
        };
        let setup_implement = PlanStep {
            id: "workspace-and-dependencies-setup".to_string(),
            kind: "implement".to_string(),
            expected_result: "pass".to_string(),
            instruction: "Install dependencies".to_string(),
            expected_paths: Vec::new(),
            verify: Vec::new(),
        };
        let without_setup = StepPlan {
            goal: "Build app".to_string(),
            steps: vec![implement.clone()],
        };
        let with_setup_implement = StepPlan {
            goal: "Build app".to_string(),
            steps: vec![setup_implement.clone()],
        };

        assert_eq!(
            step_contract_setup_authority(&without_setup, &implement, None),
            NodeDependencySetupAuthority::None
        );
        assert_eq!(
            step_contract_setup_authority(&with_setup_implement, &setup_implement, None),
            NodeDependencySetupAuthority::PlanSetupStep
        );
        assert_eq!(
            step_contract_setup_authority(
                &without_setup,
                &implement,
                Some("workspace-and-dependencies-setup"),
            ),
            NodeDependencySetupAuthority::PlanSetupStep
        );
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
    fn safe_and_verify_policy_is_normalized_without_corrective_retry() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules")).unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let generated = r#"{"goal":"goal","steps":[{"id":"s1","kind":"implement","expected_result":"pass","instruction":"Create package.json for the app","expected_paths":["package.json"],"verify":["npm test && test -f package.json"]}]}"#;
        let mut planner = FakeClient::new(vec![AssistantReply::text(generated)]);
        let plan = generate_step_plan(&mut planner, "goal", &cfg).unwrap();
        assert_eq!(
            plan.steps[0].verify,
            vec!["npm test".to_string(), "test -f package.json".to_string()]
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("planner_verify_command_normalized"));
        assert!(event_text.contains("\"normalization_source\":\"deterministic_verify_policy\""));
        assert!(event_text.contains("\"original_command_hash\""));
        assert!(
            event_text
                .contains("\"original_command_summary\":\"npm test && test -f package.json\"")
        );
        assert!(
            event_text.contains("\"normalized_commands\":[\"npm test\",\"test -f package.json\"]")
        );
        assert!(!event_text.contains("\"event\":\"planner_error\""));
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
    fn dependency_order_lint_maps_to_specific_planner_failure_kind() {
        let mut report = PlanLintReport::pass();
        report.push(
            "dependency_order",
            "verify command requires dependency setup or package manifest first",
        );
        let (stage, kind) = planner_stage_and_kind_for_lint(&report);
        assert_eq!(stage, "dependency_order");
        assert_eq!(kind, "verify_dependency_order_error");
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
            completion_contract_path: None,
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
        let invalid = r#"{"goal":"goal","steps":[{"id":"s1","kind":"implement","expected_result":"pass","instruction":"Create app","expected_paths":["package.json"],"verify":["node check.js || node check2.js"]}]}"#;
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
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let mut planner = FakeClient::new(vec![
            AssistantReply::text("not yaml"),
            AssistantReply::text("still not yaml"),
            AssistantReply::text("nope"),
        ]);
        let mut execution = FakeClient::new(vec![]);
        let err = generate_and_run_ultra_plan(&mut planner, &mut execution, "goal", &cfg)
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid generated UltraPlan after corrective retries"));
        assert!(!dir.path().join(".anvil/plans").exists());
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"ultra_plan_generation_failed\""));
        assert!(event_text.contains("\"planner_error_kind\":\"planner_schema_error\""));
        assert!(!event_text.contains("\"event\":\"ultra_plan_generation_succeeded\""));
    }

    #[test]
    fn generated_final_verify_uses_existing_workspace_nextjs_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"build":"next build"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page() { return null; }\n",
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let plan_json = r#"{"goal":"Verify the existing Next.js app","steps":[{"id":"final-verify","kind":"verify","expected_result":"pass","instruction":"Run deterministic Next.js build verification","expected_paths":[],"verify":["npm run build"]}]}"#;
        let mut planner = FakeClient::new(vec![AssistantReply::text(plan_json)]);

        let plan = generate_step_plan(&mut planner, "Verify the existing Next.js app", &cfg)
            .expect("workspace entrypoint and manifest should satisfy generation lint");

        assert_eq!(planner.messages.len(), 1);
        assert_eq!(plan.steps[0].id, "final-verify");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(!event_text.contains("\"event\":\"planner_error\""));
    }

    #[test]
    fn step_plan_quality_warning_does_not_change_exit_status() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let plan_json = r#"{"goal":"Build a Next.js game app","steps":[{"id":"s1","kind":"implement","expected_result":"pass","instruction":"Create the app","expected_paths":["package.json","src/app/page.tsx"],"verify":[]}]}"#;
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
        assert!(prompt.contains("Keep next/react/react-dom dependencies"));
        assert!(prompt.contains("Do not treat scaffold-only"));
        assert!(prompt.contains("Deterministic verification preference:"));
        assert!(prompt.contains("Required final artifacts:"));
        assert!(prompt.contains("- package.json"));
        assert!(prompt.contains("- src/app/page.tsx"));
    }

    #[test]
    fn ultra_phase_prompt_derives_interactive_capability_evidence_from_goal_and_phase() {
        let dir = tempfile::tempdir().unwrap();
        let plan = UltraPlan {
            goal: "Create an interactive browser game".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![crate::planner::ultra_plan::UltraPhase {
                id: "gameplay".to_string(),
                prompt:
                    "Implement keyboard controls, score progression, collision rules, and restart state."
                        .to_string(),
            }],
        };
        let prompt = ultra_phase_prompt(
            &plan,
            &plan.phases[0],
            &config(dir.path().to_path_buf()),
            &UltraRunContext::new(vec!["src/app/page.tsx".to_string()]),
        );
        assert!(prompt.contains("Required final capabilities:"));
        assert!(prompt.contains("- stateful_interaction"));
        assert!(prompt.contains("- start_or_restart_flow"));
        assert!(prompt.contains("- player_control"));
        assert!(prompt.contains("Required final evidence:"));
        assert!(prompt.contains("- visible_interactive_surface_evidence"));
        assert!(prompt.contains("- user_input_handler_evidence"));
        assert!(prompt.contains("- stateful_update_evidence"));
        assert!(prompt.contains("- score_or_progression_evidence"));
        assert!(prompt.contains("- failure_or_collision_evidence"));
        assert!(prompt.contains("- restart_or_recoverable_state_evidence"));
    }

    #[test]
    fn ultra_non_final_contract_observation_carries_forward_and_final_phase_can_satisfy() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.completion_contract_path = Some(write_challenge_contract(dir.path()));
        let phase_plan = challenge_implement_step_plan_json();
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(phase_plan.clone()),
            AssistantReply::text(phase_plan),
        ]);
        let static_page =
            "export default function Page(){ return <main><canvas>ready</canvas></main>; }";
        let complete_page = "export default function Page(){ const enemies = ['drone']; return <main>enemy challenge {enemies.length}</main>; }";
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/app/page.tsx","content":static_page}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Edit",
                    serde_json::json!({
                        "path":"src/app/page.tsx",
                        "old": static_page,
                        "new": complete_page,
                    }),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);
        let plan = challenge_ultra_plan();

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"contract_observation_incomplete\""));
        assert!(event_text.contains("\"contract_enforcement\":\"observe\""));
        assert!(event_text.contains("\"phase_scope\":\"phase-one\""));
        assert!(
            event_text
                .contains("\"pending_capability_evidence\":[\"challenge_or_adversary_evidence\"]")
        );
        assert!(event_text.contains("\"event\":\"ultra_plan_complete\""));
        let phase_two_prompt = planner_request_text(&planner, 1);
        assert!(phase_two_prompt.contains("Unmet final requirements from earlier phases:"));
        assert!(phase_two_prompt.contains("- challenge_or_adversary_evidence"));
        assert!(phase_two_prompt.contains("Close these requirements when they are in scope"));
    }

    #[test]
    fn ultra_final_phase_enforces_when_observed_contract_debt_remains() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.completion_contract_path = Some(write_challenge_contract(dir.path()));
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(challenge_implement_step_plan_json()),
            AssistantReply::text(challenge_setup_step_plan_json()),
        ]);
        let static_page =
            "export default function Page(){ return <main><canvas>ready</canvas></main>; }";
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/app/page.tsx","content":static_page}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"phase-two.txt","content":"verified final phase without adversary evidence"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("final acceptance repair could not add evidence"),
        ]);
        let plan = challenge_ultra_plan();

        let err = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg)
            .unwrap_err()
            .to_string();

        assert!(err.contains("challenge_or_adversary_evidence"), "{err}");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"plan_final_contract\""));
        assert!(event_text.contains("\"ok\":false"));
        assert!(event_text.contains("challenge_or_adversary_evidence"));
        assert!(!event_text.contains("\"event\":\"ultra_plan_complete\""));
    }

    #[test]
    fn ultra_final_acceptance_report_enforces_contract_evidence_without_implement_step() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.completion_contract_path = Some(write_challenge_contract(dir.path()));
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page(){ return <main><canvas>ready</canvas></main>; }",
        )
        .unwrap();
        let plan = challenge_ultra_plan();

        let report = ultra_final_acceptance_report(&plan, &cfg).unwrap();

        assert!(!report.is_pass(), "{report:?}");
        assert!(
            report
                .primary_reason()
                .contains("challenge_or_adversary_evidence"),
            "{report:?}"
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"ultra_final_acceptance\""));
        assert!(event_text.contains("\"runtime_acceptance_passed\":false"));
        assert!(event_text.contains("\"missing_evidence\":[\"challenge_or_adversary_evidence\"]"));
    }

    #[test]
    fn ultra_final_acceptance_report_records_browser_probe_failure() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        enable_browser_probe_test_override(dir.path());
        let port = write_browser_probe_mock_command(dir.path(), "500");
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            format!(
                r#"{{"scripts":{{"build":"next build","start":"next start -p {port}"}},"dependencies":{{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"}}}}"#
            ),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            interactive_game_page_source(),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/layout.tsx"),
            "export default function Layout({children}:{children:React.ReactNode}){return <html><body>{children}</body></html>;}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/global.d.ts"),
            "declare module \"*.css\";",
        )
        .unwrap();
        let plan = UltraPlan {
            goal: "Create an interactive browser game".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![crate::planner::ultra_plan::UltraPhase {
                id: "finish".to_string(),
                prompt: "Finish app".to_string(),
            }],
        };

        let report = ultra_final_acceptance_report(&plan, &cfg).unwrap();

        assert!(!report.is_pass(), "{report:?}");
        let primary = report.primary_reason();
        assert!(
            primary.contains("browser_readiness_failed:http_500"),
            "{primary}"
        );
        assert!(
            report
                .profile_failures
                .iter()
                .any(|failure| failure.contains("browser_probe_output")
                    && failure.contains("Module parse failed")),
            "{report:?}"
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"browser_probe\""));
        assert!(event_text.contains("\"status\":\"failed\""));
        assert!(event_text.contains("\"failure_kind\":\"http_500\""));
        assert!(event_text.contains("\"child_reaped\":true"));
        assert!(
            std::fs::read_to_string(dir.path().join(".anvil/evidence/browser-readiness.json"))
                .unwrap()
                .contains("\"http_status\": 500")
        );
    }

    #[test]
    fn non_ultra_plan_run_still_enforces_completion_contract() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.completion_contract_path = Some(write_challenge_contract(dir.path()));
        let plan = StepPlan {
            goal: "Create the page".to_string(),
            steps: vec![PlanStep {
                id: "page".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create src/app/page.tsx".to_string(),
                expected_paths: vec!["src/app/page.tsx".to_string()],
                verify: Vec::new(),
            }],
        };
        let mut fake = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: vec![crate::state::ToolCall::new(
                "Write",
                serde_json::json!({
                    "path":"src/app/page.tsx",
                    "content":"export default function Page(){ return <main><canvas>ready</canvas></main>; }",
                }),
            )],
            prompt_tokens: None,
            completion_tokens: None,
        }]);

        let err = run_step_plan(&mut fake, &plan, &cfg)
            .unwrap_err()
            .to_string();

        assert!(err.contains("completion contract verify failed"), "{err}");
        assert!(err.contains("challenge_or_adversary_evidence"), "{err}");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"contract_enforcement\":\"enforce\""));
        assert!(!event_text.contains("\"event\":\"contract_observation_incomplete\""));
    }

    #[test]
    fn profile_auto_repair_continuation_prompt_treats_scaffold_as_incomplete() {
        let plan = UltraPlan {
            goal: "Create an interactive app".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![crate::planner::ultra_plan::UltraPhase {
                id: "finish".to_string(),
                prompt: "Finish the app".to_string(),
            }],
        };
        let phase = plan.phases[0].clone();
        let mut context = UltraRunContext::new(vec!["src/app/page.tsx".to_string()]);
        context.last_failed_phase = Some("finish".to_string());
        let prompt = profile_auto_repair_continuation_prompt(
            &plan,
            &phase,
            &VerificationReport::missing_path("src/app/page.tsx"),
            &context,
            &["package.json".to_string(), "src/app/page.tsx".to_string()],
        );
        assert!(prompt.contains("recovery scaffold only, not as task completion"));
        assert!(prompt.contains("task-specific implementation details"));
        assert!(prompt.contains("one repair turn budget"));
        assert!(prompt.contains("Pending final artifacts"));
    }

    #[test]
    fn profile_repair_uses_existing_ultra_session_context() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = SessionSnapshot::new();
        session.messages.push(ConversationMessage::user(
            "Prior phase created package.json".to_string(),
        ));
        let mut fake = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"repair.txt","content":"fixed"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("done"),
        ]);
        let outcome = run_profile_repair_with_ultra_session(
            &mut fake,
            &mut session,
            "Create repair.txt as a bounded profile repair.",
            &["repair.txt".to_string()],
            &config(dir.path().to_path_buf()),
            &NOOP_UI,
        )
        .unwrap();
        assert!(dir.path().join("repair.txt").is_file());
        assert!(outcome.changed_paths.contains(&"repair.txt".to_string()));
        let first_request = fake
            .messages
            .first()
            .expect("profile repair request")
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(first_request.contains("Prior phase created package.json"));
        assert!(first_request.contains("Create repair.txt as a bounded profile repair."));
    }

    #[test]
    fn ultra_final_acceptance_failure_runs_bounded_repair() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(generated_nextjs_fixture_plan_json_with_kind(
                "scaffold phase",
                "check_scaffold.py",
                "setup",
            )),
            AssistantReply::text(generated_nextjs_fixture_plan_json_with_kind(
                "finish phase",
                "check_finish.py",
                "setup",
            )),
        ]);
        let package = r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#;
        let static_page =
            "export default function Page(){return <main>Press any key to start</main>;}";
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"package.json","content":package}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/page.tsx","content":static_page}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/layout.tsx","content":"export default function Layout({children}:{children:React.ReactNode}){return <html><body>{children}</body></html>;}"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/global.d.ts","content":"declare module \"*.css\";"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"check_scaffold.py","content":"x = 1\n"}),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"check_finish.py","content":"x = 2\n"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Edit",
                    serde_json::json!({
                        "path":"src/app/page.tsx",
                        "old": static_page,
                        "new": interactive_game_page_source()
                    }),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);
        let plan = UltraPlan {
            goal: "Create an interactive browser game".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "scaffold".to_string(),
                    prompt: "Scaffold Next.js app".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "finish".to_string(),
                    prompt: "Finish the app".to_string(),
                },
            ],
        };
        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();
        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let page = std::fs::read_to_string(dir.path().join("src/app/page.tsx")).unwrap();
        assert!(page.contains("onKeyDown"));
        assert!(page.contains("score"));
        assert!(page.contains("collision"));
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("ultra_final_acceptance_failed"));
        assert!(event_text.contains("final_acceptance_repair_start"));
        assert!(event_text.contains("final_acceptance_repair_complete"));
        assert!(event_text.contains("ultra_plan_complete"));
        assert!(event_text.contains("\"release_gate_status\":\"partial\""));
        assert!(event_text.contains("\"event\":\"recovery_prompt_saved\""));
        assert!(event_text.contains("\"handoff_saved_not_success\":true"));
        assert!(event_text.contains("\"recovery_handoff_saved\":true"));
        let repair_prompt = execution
            .messages
            .iter()
            .map(|messages| {
                messages
                    .iter()
                    .map(|message| message.content.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .find(|prompt| prompt.contains("Repair the final acceptance failure"))
            .expect("final acceptance repair request");
        assert!(repair_prompt.contains("Repair the final acceptance failure"));
        assert!(repair_prompt.contains("attempt: 1/1"));
        assert!(repair_prompt.contains("without weakening verification"));
    }

    #[test]
    fn ultra_final_acceptance_repair_failure_saves_recovery_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(generated_nextjs_fixture_plan_json_with_kind(
                "scaffold phase",
                "check_scaffold.py",
                "setup",
            )),
            AssistantReply::text(generated_nextjs_fixture_plan_json_with_kind(
                "finish phase",
                "check_finish.py",
                "setup",
            )),
        ]);
        let package = r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#;
        let static_page =
            "export default function Page(){return <main>Press any key to start</main>;}";
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"package.json","content":package}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/page.tsx","content":static_page}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/layout.tsx","content":"export default function Layout({children}:{children:React.ReactNode}){return <html><body>{children}</body></html>;}"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/global.d.ts","content":"declare module \"*.css\";"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"check_scaffold.py","content":"x = 1\n"}),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"check_finish.py","content":"x = 2\n"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);
        let plan = UltraPlan {
            goal: "Create an interactive browser game".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "scaffold".to_string(),
                    prompt: "Scaffold Next.js app".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "finish".to_string(),
                    prompt: "Finish the app".to_string(),
                },
            ],
        };
        let err = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg)
            .unwrap_err()
            .to_string();
        assert!(err.contains("ultra final acceptance repair failed"));
        assert!(err.contains("Recovery artifact check"));
        let repairs_dir = dir.path().join(".anvil/repairs");
        assert!(repairs_dir.is_dir());
        assert!(std::fs::read_dir(&repairs_dir).unwrap().next().is_some());
        let recovery_plan = assert_single_recovery_ultra_plan(dir.path());
        assert_eq!(recovery_plan.goal, plan.goal);
        assert_eq!(recovery_plan.profile, "nextjs");
        assert!(recovery_plan.phases.iter().any(|phase| {
            phase
                .prompt
                .contains("Missing capability or artifact signals")
        }));
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("final_acceptance_repair_start"));
        assert!(event_text.contains("final_acceptance_repair_failed"));
        assert!(event_text.contains("recovery_prompt_saved"));
        assert!(event_text.contains("recovery_ultra_plan_path"));
        assert!(event_text.contains("\"recovery_prompt_parse_ok\":true"));
        assert!(event_text.contains("\"recovery_yaml_parse_ok\":true"));
        assert!(event_text.contains("\"recovery_command_targets_valid\":true"));
        assert!(event_text.contains("suggested_recovery_yaml_command"));
        assert!(event_text.contains("suggested_recovery_command"));
        let summary = std::fs::read_to_string(dir.path().join("summary.md")).unwrap();
        assert!(summary.contains("Status: incomplete"));
        assert!(summary.contains("Completed phases:\n- scaffold"));
        assert!(summary.contains("Failed phase:\n- finish"));
        assert!(summary.contains("Pending phases:\n- none"));
        assert!(summary.contains("Recovery next action:"));
        assert!(summary.contains("Recovery UltraPlan YAML saved:"));
        assert!(summary.contains("Suggested YAML command:"));
        assert!(summary.contains("Recovery artifact check:"));
    }

    #[test]
    fn ultra_phase_scaffold_failure_saves_recovery_yaml_and_incomplete_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let mut planner = FakeClient::new(vec![
            AssistantReply::text("not a step plan"),
            AssistantReply::text("still not a step plan"),
            AssistantReply::text("no valid step plan"),
        ]);
        let mut execution = FakeClient::new(Vec::new());
        let plan = UltraPlan {
            goal: "Build an interactive web game".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "web-audio-synth-and-ui".to_string(),
                    prompt: "Add audio, HUD, overlays, and deterministic verification".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "final-verify".to_string(),
                    prompt: "Verify the recovered interactive app".to_string(),
                },
            ],
        };
        let err = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg)
            .unwrap_err()
            .to_string();
        assert!(err.contains("phase scaffold failed"), "{err}");
        assert!(err.contains("incomplete"), "{err}");
        assert!(err.contains("recovery YAML saved"), "{err}");
        assert!(err.contains("Recovery artifact check"), "{err}");
        assert!(
            err.contains("/run-ultra-plan .anvil/plans/recovery-ultra-plan-"),
            "{err}"
        );
        let recovery_plan = assert_single_recovery_ultra_plan(dir.path());
        assert_eq!(recovery_plan.goal, "Build an interactive web game");
        assert_eq!(recovery_plan.profile, "nextjs");
        let rendered = render_ultra_plan(&recovery_plan);
        assert_eq!(parse_ultra_plan(&rendered).unwrap(), recovery_plan);
        assert!(
            recovery_plan
                .phases
                .iter()
                .any(|phase| phase.prompt.contains("web-audio-synth-and-ui"))
        );
        assert!(recovery_plan.phases.iter().any(|phase| {
            phase
                .prompt
                .contains("Missing capability or artifact signals")
        }));
        assert!(
            recovery_plan
                .phases
                .iter()
                .any(|phase| phase.prompt.contains("Verify preference"))
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"planner_error_kind\":\"phase_scaffold_error\""));
        assert!(event_text.contains("\"event\":\"recovery_prompt_saved\""));
        assert!(event_text.contains("\"status\":\"incomplete\""));
        assert!(event_text.contains("\"recovery_yaml_missing\":false"));
        assert!(event_text.contains("\"recovery_prompt_parse_ok\":true"));
        assert!(event_text.contains("\"recovery_yaml_parse_ok\":true"));
        assert!(event_text.contains("\"recovery_command_targets_valid\":true"));
        assert!(event_text.contains("\"recovery_ultra_plan_path\""));
        assert!(event_text.contains("\"suggested_recovery_yaml_command\""));
        let summary = std::fs::read_to_string(dir.path().join("summary.md")).unwrap();
        assert!(summary.contains("Status: incomplete"));
        assert!(summary.contains("Completed phases:\n- none"));
        assert!(summary.contains("Failed phase:\n- web-audio-synth-and-ui"));
        assert!(summary.contains("Pending phases:\n- final-verify"));
        assert!(summary.contains("Recovery next action:"));
        assert!(summary.contains("Recovery UltraPlan YAML saved:"));
        assert!(summary.contains("Suggested YAML command:"));
        assert!(summary.contains("Recovery artifact check:"));
    }

    #[test]
    fn ultra_phase_recovery_handoff_renders_observed_missing_evidence_keys() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let plan = challenge_ultra_plan();
        let phase = &plan.phases[0];

        let message = save_ultra_phase_recovery_handoff(
            &cfg,
            &plan,
            phase,
            UltraPhaseRecoveryRequest {
                failure_kind: "phase_execute_error",
                reason: "missing_required_evidence:challenge_or_adversary_evidence",
                missing_paths: &[],
                missing_signals: &["challenge_or_adversary_evidence".to_string()],
                repair_targets: &["completion_contract".to_string()],
            },
        )
        .expect("recovery handoff should be saved");

        assert!(message.contains("recovery YAML saved"), "{message}");
        let repairs_dir = dir.path().join(".anvil/repairs");
        let repair_text = std::fs::read_dir(&repairs_dir)
            .unwrap()
            .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
            .find(|text| text.contains("Missing capabilities:"))
            .expect("recovery prompt");
        assert!(
            repair_text.contains("- challenge_or_adversary_evidence"),
            "{repair_text}"
        );
        assert!(!repair_text.contains("Missing capabilities:\n- none"));
        assert!(repair_text.contains("Repair targets:\n- completion_contract"));
        let recovery_plan = assert_single_recovery_ultra_plan(dir.path());
        assert!(
            recovery_plan
                .phases
                .iter()
                .any(|phase| { phase.prompt.contains("- challenge_or_adversary_evidence") })
        );
        assert!(recovery_plan.phases.iter().any(|phase| {
            phase
                .prompt
                .contains("Repair targets:\n- completion_contract")
        }));
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"recovery_prompt_path\":\".anvil/repairs/"));
        assert!(event_text.contains("\"recovery_ultra_plan_path\":\".anvil/plans/"));
    }

    #[test]
    fn session_error_text_missing_evidence_populates_step_outcome() {
        let err = anyhow::anyhow!(
            "completion contract verify failed after 1 attempts: \
             missing_required_capabilities:stateful_interaction \
             missing_required_evidence:challenge_or_adversary_evidence"
        );

        let outcome = step_run_outcome_from_session_error(&err, "initial_turn_error");

        assert_eq!(
            outcome.observed_missing_capabilities,
            vec!["stateful_interaction".to_string()]
        );
        assert_eq!(
            outcome.observed_missing_evidence,
            vec!["challenge_or_adversary_evidence".to_string()]
        );
        assert_eq!(
            outcome.repair_targets,
            vec!["required_evidence_missing".to_string()]
        );
        assert_eq!(outcome.stop_reason.as_deref(), Some("initial_turn_error"));
        assert!(outcome.partial);
    }

    #[test]
    fn ultra_phase_recovery_handoff_uses_contract_error_observations() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.completion_contract_path = Some(write_challenge_contract(dir.path()));
        let plan = challenge_ultra_plan();
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(challenge_setup_step_plan_json()),
            AssistantReply::text(challenge_implement_step_plan_json()),
        ]);
        let static_page =
            "export default function Page(){ return <main><canvas>ready</canvas></main>; }";
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"phase-two.txt","content":"phase one complete"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/app/page.tsx","content":static_page}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);

        let err = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg)
            .unwrap_err()
            .to_string();

        assert!(err.contains("challenge_or_adversary_evidence"), "{err}");
        let repairs_dir = dir.path().join(".anvil/repairs");
        let repair_text = std::fs::read_dir(&repairs_dir)
            .unwrap()
            .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
            .find(|text| text.contains("- phase: phase-two"))
            .expect("phase recovery prompt");
        assert!(
            repair_text.contains("- challenge_or_adversary_evidence"),
            "{repair_text}"
        );
        assert!(!repair_text.contains("Missing capabilities:\n- none"));
        assert!(!repair_text.contains("Repair targets:\n- none"));
        assert!(repair_text.contains("Repair targets:\n- "), "{repair_text}");
        let recovery_plan = std::fs::read_dir(dir.path().join(".anvil/plans"))
            .unwrap()
            .filter_map(|entry| {
                let path = entry.unwrap().path();
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with("recovery-ultra-plan-") && name.ends_with(".yaml")
                    })
                    .then(|| parse_ultra_plan(&std::fs::read_to_string(path).unwrap()).unwrap())
            })
            .find(|plan| {
                plan.phases.iter().any(|phase| {
                    phase.prompt.contains("- challenge_or_adversary_evidence")
                        && phase.prompt.contains("Repair targets:\n- ")
                        && !phase.prompt.contains("Repair targets:\n- none")
                })
            })
            .expect("phase recovery ultra plan");
        assert!(
            recovery_plan
                .phases
                .iter()
                .any(|phase| phase.prompt.contains("- challenge_or_adversary_evidence"))
        );
        assert!(recovery_plan.phases.iter().any(|phase| {
            phase.prompt.contains("Repair targets:\n- ")
                && !phase.prompt.contains("Repair targets:\n- none")
        }));
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"completion_verify\""));
        assert!(event_text.contains("\"event\":\"ultra_phase_failed\""));
    }

    #[test]
    fn ultra_phase_failure_stop_reason_and_summary_use_relative_recovery_paths() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let mut planner = FakeClient::new(vec![
            AssistantReply::text("not a step plan"),
            AssistantReply::text("still not a step plan"),
            AssistantReply::text("no valid step plan"),
        ]);
        let mut execution = FakeClient::new(Vec::new());
        let plan = challenge_ultra_plan();

        let err = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg)
            .unwrap_err()
            .to_string();
        let summary = std::fs::read_to_string(dir.path().join("summary.md")).unwrap();
        let event_text = std::fs::read_to_string(events).unwrap();
        let root = dir.path().display().to_string();

        for text in [&err, &summary, &event_text] {
            assert!(
                !text.contains(&format!("{root}{root}")),
                "workspace root duplicated in:\n{text}"
            );
            assert!(
                text.contains(".anvil/plans/recovery-ultra-plan-"),
                "missing relative recovery yaml path in:\n{text}"
            );
        }
        assert!(
            !err.contains(&format!("{root}/.anvil/plans/recovery-ultra-plan-")),
            "{err}"
        );
        assert!(
            !summary.contains(&format!("{root}/.anvil/plans/recovery-ultra-plan-")),
            "{summary}"
        );
        assert!(
            event_text.contains("\"recovery_ultra_plan_path\":\".anvil/plans/"),
            "{event_text}"
        );
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
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Edit",
                    serde_json::json!({
                        "path":"src/app/page.tsx",
                        "old":"Space Invaders",
                        "new":"Space Invaders with keyboard controls, score, waves, and collisions"
                    }),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"package.json","content":good_package}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("continuation complete"),
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
        let prompts = execution
            .messages
            .iter()
            .map(|messages| {
                messages
                    .iter()
                    .map(|message| message.content.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .collect::<Vec<_>>();
        assert!(
            prompts
                .iter()
                .any(|prompt| prompt.contains("recovery scaffold only, not as task completion")),
            "{prompts:#?}"
        );
        assert!(
            execution.messages.len() >= 3,
            "expected initial phase, follow-up phase, and repair prompts: {prompts:#?}"
        );
    }

    #[test]
    fn deterministic_profile_fallback_requires_targeted_continuation_before_success() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let scaffold_plan = generated_nextjs_fixture_plan_json_with_kind(
            "Scaffold interactive app",
            "check_scaffold.py",
            "setup",
        );
        let finish_plan = generated_nextjs_fixture_plan_json_with_kind(
            "Create interactive app",
            "check_finish.py",
            "setup",
        );
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(scaffold_plan),
            AssistantReply::text(finish_plan.clone()),
            AssistantReply::text(finish_plan),
        ]);
        let package = r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#;
        let bad_package =
            r#"{"dependencies":{},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#;
        let mut first_phase_calls =
            nextjs_interactive_app_tool_calls(interactive_game_page_source());
        first_phase_calls.push(crate::state::ToolCall::new(
            "Write",
            serde_json::json!({"path":"package.json","content":package}),
        ));
        first_phase_calls.push(crate::state::ToolCall::new(
            "Write",
            serde_json::json!({"path":"check_scaffold.py","content":"x = 1\n"}),
        ));
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: first_phase_calls,
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"package.json","content":bad_package}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"check_finish.py","content":"x = 2\n"}),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"README.md","content":"# Recovery note\nThe scaffold exists but implementation still needs task-specific gameplay."}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"package.json","content":package}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);
        let plan = UltraPlan {
            goal: "Create an interactive browser game".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "scaffold".to_string(),
                    prompt: "Scaffold the app".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "finish".to_string(),
                    prompt: "Finish the interactive app".to_string(),
                },
            ],
        };
        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();
        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"deterministic_scaffold_recovery\""));
        assert!(event_text.contains("\"used_for_completion\":false"));
        assert!(event_text.contains("\"event\":\"profile_auto_repair_continuation_incomplete\""));
        assert!(event_text.contains("\"repair_follow_through\":\"unrelated_change\""));
        assert!(event_text.contains("\"event\":\"profile_repair_complete\""));
        assert!(event_text.contains("\"event\":\"ultra_plan_complete\""));
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
    fn ultra_plan_non_final_tailwind_invariant_repair_completes_and_runs_next_phase() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let scaffold_plan = generated_nextjs_fixture_plan_json_with_kind(
            "scaffold phase",
            "check_scaffold.py",
            "setup",
        );
        let finish_plan = generated_nextjs_fixture_plan_json_with_kind(
            "finish phase",
            "check_finish.py",
            "setup",
        );
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(scaffold_plan),
            AssistantReply::text(finish_plan),
        ]);
        let package = r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0","tailwindcss":"^3.4.19","postcss":"^8.5.15","autoprefixer":"^10.4.20"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#;
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"package.json","content":package}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/page.tsx","content":"export default function Page(){return <main className=\"min-h-screen\">App</main>;}"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/layout.tsx","content":"import './globals.css';\nexport default function Layout({children}:{children:React.ReactNode}){return <html><body>{children}</body></html>;}"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/global.d.ts","content":"declare module \"*.css\";"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/globals.css","content":"@tailwind base;\n@tailwind components;\n@tailwind utilities;\n"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"tailwind.config.js","content":"module.exports = { content: ['./src/pages/**/*.{ts,tsx}', './src/components/**/*.{ts,tsx}', './src/app/**/*.{ts,tsx}'], theme: { extend: {} }, plugins: [] };\n"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"postcss.config.js","content":"module.exports = { plugins: { tailwindcss: {} } };\n"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"check_scaffold.py","content":"x = 1\n"}),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"check_finish.py","content":"x = 1\n"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
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

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let postcss = std::fs::read_to_string(dir.path().join("postcss.config.js")).unwrap();
        assert!(postcss.contains("tailwindcss"));
        assert!(postcss.contains("autoprefixer"));
        assert!(dir.path().join("check_finish.py").is_file());
        assert!(!dir.path().join(".anvil/repairs").exists());
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"profile_invariant_repair\""));
        assert!(event_text.contains("\"method\":\"deterministic\""));
        assert!(event_text.contains("\"ok\":true"));
        assert!(event_text.contains("\"event\":\"ultra_plan_complete\""));
    }

    #[test]
    fn ultra_plan_non_final_profile_repair_exhaustion_still_saves_handoff() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let step_json = generated_nextjs_artifact_plan_json("Scaffold project");
        let mut planner = FakeClient::new(
            (0..3)
                .map(|_| AssistantReply::text(step_json.clone()))
                .collect(),
        );
        let package = r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#;
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"package.json","content":package}),
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
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"next.config.js","content":"/** @type {import('next').NextConfig} */\nconst nextConfig = {};\n\nmodule.exports = nextConfig;\n"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/globals.css","content":"body { margin: 0; }\n"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"tsconfig.json","content":"{\"compilerOptions\":{\"rootDir\":\"src\"}}\n"}),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("model repair made no changes"),
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
        let err = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("phase scaffold profile invariant verification failed"),
            "{err}"
        );
        assert!(err.contains("tsconfig.rootDir"), "{err}");
        assert!(dir.path().join(".anvil/repairs").is_dir());
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"profile_invariant_repair\""));
        assert!(event_text.contains("\"method\":\"deterministic\""));
        assert!(event_text.contains("\"method\":\"model\""));
        assert!(event_text.contains("\"recovery_handoff_kind\":\"profile_invariant_failure\""));
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
    fn plan_run_nextjs_game_setup_only_fails_inferred_obligation() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let plan = StepPlan {
            goal: "Create an interactive browser game\n\nRequired final artifacts:\n- package.json"
                .to_string(),
            steps: vec![PlanStep {
                id: "setup".to_string(),
                kind: "setup".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create package.json".to_string(),
                expected_paths: vec!["package.json".to_string()],
                verify: Vec::new(),
            }],
        };
        let mut fake = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: vec![crate::state::ToolCall::new(
                "Write",
                serde_json::json!({"path":"package.json","content":"{\"scripts\":{\"build\":\"next build\"}}"}),
            )],
            prompt_tokens: None,
            completion_tokens: None,
        }]);
        let err = run_step_plan(&mut fake, &plan, &cfg)
            .unwrap_err()
            .to_string();
        assert!(err.contains("plan final contract failed"), "{err}");
        assert!(err.contains("missing_required_evidence"), "{err}");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"required_obligations\":[\"implementation\"]"));
        assert!(event_text.contains("\"missing_obligations\":[\"implementation\"]"));
        assert!(event_text.contains("\"obligation_repair_targets\""));
        assert!(event_text.contains("\"target_path\":\"src/app/page.tsx\""));
    }

    #[test]
    fn plan_run_nextjs_game_scaffold_only_fails_inferred_capabilities() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let plan = StepPlan {
            goal: "Create an interactive browser game\n\nRequired final artifacts:\n- src/app/page.tsx"
                .to_string(),
            steps: vec![PlanStep {
                id: "page".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create src/app/page.tsx".to_string(),
                expected_paths: vec!["src/app/page.tsx".to_string()],
                verify: Vec::new(),
            }],
        };
        let mut fake = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/app/page.tsx","content":"export default function Page(){ return <main>Press any key to start</main>; }"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("done"),
        ]);
        let err = run_step_plan(&mut fake, &plan, &cfg)
            .unwrap_err()
            .to_string();
        assert!(err.contains("completion contract verify"), "{err}");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"runtime_acceptance_inconclusive\":false"));
        assert!(event_text.contains("\"missing_evidence\""));
        assert!(event_text.contains("\"capability_evidence_bindings\""));
        assert!(event_text.contains("\"role\":\"scaffold\""));
        assert!(event_text.contains("\"artifact_paths\":[]"));
        assert!(event_text.contains("\"event\":\"step_obligation_scope\""));
        assert!(event_text.contains("\"step_kind\":\"implement\""));
        assert!(event_text.contains("\"completion_contract_path_merge_enabled\":true"));
        assert!(event_text.contains("\"completion_contract_verification_enabled\":true"));
        assert!(event_text.contains("\"contract_paths_merged\":true"));
        assert!(event_text.contains("\"event\":\"completion_contract_bound\""));
        assert!(event_text.contains("\"session_scope\":\"plan-run\""));
        assert!(event_text.contains("\"completion_contract_verification_enabled\":true"));
        assert!(event_text.contains("\"external_contract_checked\":true"));
        assert!(event_text.contains("\"completion_contract_generated\":true"));
        assert!(event_text.contains("\"step_prompt_contract\""));
        assert!(event_text.contains("\"has_required_final_evidence\":true"));
        assert!(event_text.contains("\"visible_interactive_surface_evidence\""));
        assert!(event_text.contains("\"user_input_handler_evidence\""));
        assert!(event_text.contains("\"restart_or_recoverable_state_evidence\""));
    }

    #[test]
    fn plan_run_nextjs_game_docs_only_fails_inferred_capabilities() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let plan = StepPlan {
            goal: "Create an interactive browser game\n\nRequired final artifacts:\n- README.md"
                .to_string(),
            steps: vec![PlanStep {
                id: "docs".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create README.md".to_string(),
                expected_paths: vec!["README.md".to_string()],
                verify: Vec::new(),
            }],
        };
        let mut fake = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"README.md","content":"# Game\nUse arrow keys."}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("done"),
        ]);
        let err = run_step_plan(&mut fake, &plan, &cfg)
            .unwrap_err()
            .to_string();
        assert!(err.contains("completion contract verify"), "{err}");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"step_obligation_scope\""));
        assert!(event_text.contains("\"step_kind\":\"implement\""));
        assert!(event_text.contains("\"completion_contract_path_merge_enabled\":true"));
        assert!(event_text.contains("\"completion_contract_verification_enabled\":true"));
        assert!(event_text.contains("\"missing_evidence\""));
        assert!(event_text.contains("\"role\":\"acceptance_evidence\""));
    }

    #[test]
    fn plan_run_nextjs_interactive_app_records_partial_release_gate() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let plan = StepPlan {
            goal: "Create an interactive browser game\n\nRequired final artifacts:\n- package.json\n- src/app/page.tsx\n- src/app/layout.tsx\n- src/app/global.d.ts".to_string(),
            steps: vec![PlanStep {
                id: "app".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create the interactive Next.js game app".to_string(),
                expected_paths: vec![
                    "package.json".to_string(),
                    "src/app/page.tsx".to_string(),
                    "src/app/layout.tsx".to_string(),
                    "src/app/global.d.ts".to_string(),
                ],
                verify: Vec::new(),
            }],
        };
        let page = r#""use client";
import { useEffect, useState } from "react";
export default function Page(){
  const [score,setScore] = useState(0);
  const [gameState,setGameState] = useState("ready");
  useEffect(() => {
    const onKeyDown = () => {
      setGameState("playing");
      setScore((value) => value + 1);
    };
    const frame = requestAnimationFrame(() => {
      const collision = true;
      if (collision) setGameState("gameover");
    });
    window.addEventListener("keydown", onKeyDown);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, []);
  return <main><button onClick={() => setGameState("playing")}>Start</button><button onClick={() => { setGameState("ready"); setScore(0); }}>Restart</button><canvas /><p>enemy bullet collision score {score} {gameState}</p></main>;
}
"#;
        let mut fake = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: vec![
                crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"package.json","content":"{\"scripts\":{\"build\":\"next build\"},\"dependencies\":{\"next\":\"^14.2.0\",\"react\":\"^18.3.0\",\"react-dom\":\"^18.3.0\"}}"}),
                ),
                crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/app/page.tsx","content":page}),
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
        }]);
        let result = run_step_plan(&mut fake, &plan, &cfg).unwrap();
        assert_eq!(result, "plan-run complete: 1 steps");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"ok\":true"));
        assert!(event_text.contains("\"nextjs_route_evidence\""));
        assert!(event_text.contains("\"build_command_or_dependency_missing_boundary\""));
        assert!(event_text.contains("\"release_gate_status\":\"partial\""));
        assert!(event_text.contains("\"final_acceptance_status\":\"partial\""));
        assert!(
            event_text.contains("browser_readiness_or_interaction_evidence_required"),
            "{event_text}"
        );
        let readiness_text = std::fs::read_to_string(dir.path().join("browser-readiness.json"))
            .expect("generated browser readiness evidence");
        assert!(readiness_text.contains("\"lifecycle_stages\""));
        assert!(readiness_text.contains("\"probe_environment\""));
        assert!(readiness_text.contains("\"NODE_ENV\""));
        assert!(event_text.contains("\"event\":\"recovery_prompt_saved\""));
        assert!(event_text.contains("\"recovery_handoff_kind\":\"browser_readiness_missing\""));
        assert!(event_text.contains("\"acceptance_layer\":\"release_gate\""));
        assert!(event_text.contains("\"suggested_recovery_yaml_command\""));
        assert!(event_text.contains("\"handoff_saved_not_success\":true"));
        let recovery_plan = assert_single_recovery_ultra_plan(dir.path());
        assert_eq!(recovery_plan.goal, plan.goal);
        let recovery_text = render_ultra_plan(&recovery_plan);
        assert!(recovery_text.contains("Failed acceptance layer or phase"));
        assert!(recovery_text.contains("browser_readiness_missing"));
        assert!(recovery_text.contains("Preferred verify/browser check"));
    }

    #[test]
    fn plan_run_nextjs_browser_http_500_fails_final_contract() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        std::fs::write(
            dir.path().join("browser-readiness.json"),
            r#"{"ok":false,"http_status":500,"failure_kind":"browser_http_500"}"#,
        )
        .unwrap();
        let plan = StepPlan {
            goal: "Create an interactive browser game\n\nRequired final artifacts:\n- package.json\n- src/app/page.tsx\n- src/app/layout.tsx\n- src/app/global.d.ts".to_string(),
            steps: vec![PlanStep {
                id: "app".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create the interactive Next.js game app".to_string(),
                expected_paths: vec![
                    "package.json".to_string(),
                    "src/app/page.tsx".to_string(),
                    "src/app/layout.tsx".to_string(),
                    "src/app/global.d.ts".to_string(),
                ],
                verify: Vec::new(),
            }],
        };
        let page = interactive_game_page_source();
        let mut fake = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: nextjs_interactive_app_tool_calls(page),
            prompt_tokens: None,
            completion_tokens: None,
        }]);
        let err = run_step_plan(&mut fake, &plan, &cfg)
            .unwrap_err()
            .to_string();
        assert!(err.contains("plan final contract failed"), "{err}");
        assert!(err.contains("release gate failed"), "{err}");
        assert!(err.contains("browser_readiness_failed:http_500"), "{err}");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"release_gate_status\":\"failed\""));
        assert!(event_text.contains("\"browser_readiness_status\":\"failed:http_500\""));
        assert!(event_text.contains("\"event\":\"recovery_prompt_saved\""));
        assert!(event_text.contains("\"recovery_handoff_kind\":\"browser_readiness_failed\""));
        assert!(event_text.contains("\"acceptance_layer\":\"release_gate\""));
        assert!(event_text.contains("\"recovery_handoff_saved\":true"));
        let recovery_plan = assert_single_recovery_ultra_plan(dir.path());
        assert_eq!(recovery_plan.goal, plan.goal);
        let recovery_text = render_ultra_plan(&recovery_plan);
        assert!(recovery_text.contains("release gate reason"));
        assert!(recovery_text.contains("browser readiness"));
        assert!(recovery_text.contains("Preferred verify/browser check"));
    }

    #[test]
    fn plan_run_nextjs_tailwind_dev_route_failure_keeps_failure_kind() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        std::fs::write(
            dir.path().join("browser-readiness.json"),
            r#"{"ok":false,"http_status":500,"browser_failure_kind":"tailwind_dev_pipeline_failure","body_excerpt":"Module parse failed: Unexpected character '@' (1:0)\n> @tailwind base;"}"#,
        )
        .unwrap();
        let plan = StepPlan {
            goal: "Create an interactive browser game\n\nRequired final artifacts:\n- package.json\n- src/app/page.tsx\n- src/app/layout.tsx\n- src/app/global.d.ts".to_string(),
            steps: vec![PlanStep {
                id: "app".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create the interactive Next.js game app".to_string(),
                expected_paths: vec![
                    "package.json".to_string(),
                    "src/app/page.tsx".to_string(),
                    "src/app/layout.tsx".to_string(),
                    "src/app/global.d.ts".to_string(),
                ],
                verify: Vec::new(),
            }],
        };
        let page = interactive_game_page_source();
        let mut fake = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: nextjs_interactive_app_tool_calls(page),
            prompt_tokens: None,
            completion_tokens: None,
        }]);
        let err = run_step_plan(&mut fake, &plan, &cfg)
            .unwrap_err()
            .to_string();
        assert!(err.contains("release gate failed"), "{err}");
        assert!(
            err.contains("browser_readiness_failed:tailwind_dev_pipeline_failure"),
            "{err}"
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"release_gate_status\":\"failed\""));
        assert!(
            event_text
                .contains("\"browser_readiness_status\":\"failed:tailwind_dev_pipeline_failure\""),
            "{event_text}"
        );
    }

    #[test]
    fn nextjs_dev_route_probe_disabled_records_lifecycle_stages() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let evidence_path = nextjs_dev_route_evidence_path(&cfg);
        let evidence = run_nextjs_dev_route_probe(&cfg, &evidence_path);
        assert_eq!(
            evidence.get("status").and_then(Value::as_str),
            Some("unavailable")
        );
        assert_eq!(
            classify_release_evidence_json(ReleaseEvidenceKind::BrowserReadiness, &evidence),
            ReleaseEvidenceStatus::Unavailable(
                "browser_unavailable:dev_server_probe_disabled_in_tests".to_string()
            )
        );
        let dev_server = evidence
            .get("dev_server")
            .and_then(Value::as_object)
            .expect("dev server evidence object");
        let stages = dev_server
            .get("lifecycle_stages")
            .and_then(Value::as_array)
            .expect("lifecycle stages");
        assert_eq!(
            stages.iter().filter_map(Value::as_str).collect::<Vec<_>>(),
            vec!["start", "wait", "probe", "cleanup"]
        );
        let environment = dev_server
            .get("probe_environment")
            .and_then(Value::as_object)
            .expect("probe environment");
        assert_eq!(
            environment.get("PORT").and_then(Value::as_str),
            Some("3011")
        );
        assert!(environment.contains_key("NODE_ENV"));
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"dev_server_lifecycle\""));
        assert!(event_text.contains("\"stage\":\"start\""));
        assert!(event_text.contains("\"stage\":\"wait\""));
        assert!(event_text.contains("\"stage\":\"probe\""));
        assert!(event_text.contains("\"stage\":\"cleanup\""));
        assert!(event_text.contains("\"probe_environment\""));
        assert!(
            event_text.contains("browser_unavailable:dev_server_probe_disabled_in_tests"),
            "{event_text}"
        );
    }

    #[test]
    fn plan_run_nextjs_browser_ready_without_interaction_is_partial() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        std::fs::write(
            dir.path().join("browser-readiness.json"),
            r#"{"ok":true,"http_status":200,"route_rendered":true}"#,
        )
        .unwrap();
        let plan = StepPlan {
            goal: "Create an interactive browser game\n\nRequired final artifacts:\n- package.json\n- src/app/page.tsx\n- src/app/layout.tsx\n- src/app/global.d.ts".to_string(),
            steps: vec![PlanStep {
                id: "app".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create the interactive Next.js game app".to_string(),
                expected_paths: vec![
                    "package.json".to_string(),
                    "src/app/page.tsx".to_string(),
                    "src/app/layout.tsx".to_string(),
                    "src/app/global.d.ts".to_string(),
                ],
                verify: Vec::new(),
            }],
        };
        let page = interactive_game_page_source();
        let mut fake = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: nextjs_interactive_app_tool_calls(page),
            prompt_tokens: None,
            completion_tokens: None,
        }]);
        let result = run_step_plan(&mut fake, &plan, &cfg).unwrap();
        assert_eq!(result, "plan-run complete: 1 steps");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"release_gate_status\":\"partial\""));
        assert!(event_text.contains("\"browser_readiness_status\":\"passed\""));
        assert!(
            event_text.contains(
                "\"interaction_evidence_status\":\"unavailable:interaction_evidence_missing\""
            ),
            "{event_text}"
        );
    }

    #[test]
    fn plan_run_nextjs_browser_and_interaction_evidence_passes_release_gate() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        std::fs::write(
            dir.path().join("browser-readiness.json"),
            r#"{"ok":true,"http_status":200,"route_rendered":true}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("interaction-evidence.json"),
            r#"{"ok":true,"interaction_performed":true,"input_event_observed":true,"state_changed":true,"canvas_found":true}"#,
        )
        .unwrap();
        let plan = StepPlan {
            goal: "Create an interactive browser game\n\nRequired final artifacts:\n- package.json\n- src/app/page.tsx\n- src/app/layout.tsx\n- src/app/global.d.ts".to_string(),
            steps: vec![PlanStep {
                id: "app".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create the interactive Next.js game app".to_string(),
                expected_paths: vec![
                    "package.json".to_string(),
                    "src/app/page.tsx".to_string(),
                    "src/app/layout.tsx".to_string(),
                    "src/app/global.d.ts".to_string(),
                ],
                verify: Vec::new(),
            }],
        };
        let page = interactive_game_page_source();
        let mut fake = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: nextjs_interactive_app_tool_calls(page),
            prompt_tokens: None,
            completion_tokens: None,
        }]);
        let result = run_step_plan(&mut fake, &plan, &cfg).unwrap();
        assert_eq!(result, "plan-run complete: 1 steps");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"release_gate_status\":\"pass\""));
        assert!(event_text.contains("\"final_acceptance_status\":\"full_success\""));
        assert!(event_text.contains("\"browser_readiness_status\":\"passed\""));
        assert!(event_text.contains("\"interaction_evidence_status\":\"passed\""));
    }

    #[test]
    fn plan_run_nextjs_browser_ok_without_render_detail_is_partial() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        std::fs::write(
            dir.path().join("browser-readiness.json"),
            r#"{"ok":true,"http_status":200}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("interaction-evidence.json"),
            r#"{"ok":true,"interaction_performed":true,"state_changed":true,"canvas_found":true}"#,
        )
        .unwrap();
        let plan = StepPlan {
            goal: "Create an interactive browser game\n\nRequired final artifacts:\n- package.json\n- src/app/page.tsx\n- src/app/layout.tsx\n- src/app/global.d.ts".to_string(),
            steps: vec![PlanStep {
                id: "app".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create the interactive Next.js game app".to_string(),
                expected_paths: vec![
                    "package.json".to_string(),
                    "src/app/page.tsx".to_string(),
                    "src/app/layout.tsx".to_string(),
                    "src/app/global.d.ts".to_string(),
                ],
                verify: Vec::new(),
            }],
        };
        let page = interactive_game_page_source();
        let mut fake = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: nextjs_interactive_app_tool_calls(page),
            prompt_tokens: None,
            completion_tokens: None,
        }]);
        let result = run_step_plan(&mut fake, &plan, &cfg).unwrap();
        assert_eq!(result, "plan-run complete: 1 steps");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"release_gate_status\":\"partial\""));
        assert!(event_text.contains("\"final_acceptance_status\":\"partial\""));
        assert!(
            event_text.contains(
                "\"browser_readiness_status\":\"unavailable:browser_render_evidence_missing\""
            ),
            "{event_text}"
        );
    }

    #[test]
    fn plan_run_nextjs_canvas_unavailable_fails_release_gate() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        std::fs::write(
            dir.path().join("browser-readiness.json"),
            r#"{"ok":true,"http_status":200,"route_rendered":true}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("interaction-evidence.json"),
            r#"{"ok":true,"interaction_performed":true,"state_changed":true,"canvas_found":false}"#,
        )
        .unwrap();
        let plan = StepPlan {
            goal: "Create an interactive browser game\n\nRequired final artifacts:\n- package.json\n- src/app/page.tsx\n- src/app/layout.tsx\n- src/app/global.d.ts".to_string(),
            steps: vec![PlanStep {
                id: "app".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create the interactive Next.js game app".to_string(),
                expected_paths: vec![
                    "package.json".to_string(),
                    "src/app/page.tsx".to_string(),
                    "src/app/layout.tsx".to_string(),
                    "src/app/global.d.ts".to_string(),
                ],
                verify: Vec::new(),
            }],
        };
        let page = interactive_game_page_source();
        let mut fake = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: nextjs_interactive_app_tool_calls(page),
            prompt_tokens: None,
            completion_tokens: None,
        }]);
        let err = run_step_plan(&mut fake, &plan, &cfg)
            .unwrap_err()
            .to_string();
        assert!(err.contains("release gate failed"), "{err}");
        assert!(
            err.contains("browser_interaction_failed:canvas_unavailable"),
            "{err}"
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"release_gate_status\":\"failed\""));
        assert!(event_text.contains("\"final_acceptance_status\":\"incomplete\""));
        assert!(
            event_text.contains("\"interaction_evidence_status\":\"failed:canvas_unavailable\"")
        );
    }

    fn nextjs_interactive_app_tool_calls(page: &str) -> Vec<crate::state::ToolCall> {
        vec![
            crate::state::ToolCall::new(
                "Write",
                serde_json::json!({"path":"package.json","content":"{\"scripts\":{\"build\":\"next build\"},\"dependencies\":{\"next\":\"^14.2.0\",\"react\":\"^18.3.0\",\"react-dom\":\"^18.3.0\"}}"}),
            ),
            crate::state::ToolCall::new(
                "Write",
                serde_json::json!({"path":"src/app/page.tsx","content":page}),
            ),
            crate::state::ToolCall::new(
                "Write",
                serde_json::json!({"path":"src/app/layout.tsx","content":"export default function Layout({children}:{children:React.ReactNode}){return <html><body>{children}</body></html>;}"}),
            ),
            crate::state::ToolCall::new(
                "Write",
                serde_json::json!({"path":"src/app/global.d.ts","content":"declare module \"*.css\";"}),
            ),
        ]
    }

    #[test]
    fn plan_run_emits_dependency_build_lifecycle_event() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page(){return <main/>;}",
        )
        .unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let package = r#"{"scripts":{"build":"next build"},"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"}}"#;
        let plan = StepPlan {
            goal: "Create a Next.js app".to_string(),
            steps: vec![PlanStep {
                id: "app".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create package.json and src/app/page.tsx then verify build"
                    .to_string(),
                expected_paths: vec!["package.json".to_string(), "src/app/page.tsx".to_string()],
                verify: vec!["npm run build".to_string()],
            }],
        };
        let mut fake = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: vec![
                crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"package.json","content":package}),
                ),
                crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/app/page.tsx","content":"export default function Page(){return <main/>;}"}),
                ),
            ],
            prompt_tokens: None,
            completion_tokens: None,
        }]);
        let err = run_step_plan(&mut fake, &plan, &cfg)
            .unwrap_err()
            .to_string();
        assert!(!err.is_empty());
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"dependency_build_lifecycle\""));
        assert!(event_text.contains("\"mode\":\"plan-run\""));
        assert!(event_text.contains("setup_blocked"));
        assert!(event_text.contains("verification_dependency_missing"));
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
        let events = dir.path().join("events.jsonl");
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            r#"{"required_paths":["date-helper.js"],"verify_commands":["node date-helper.js"],"required_capabilities":["implementation","deterministic_test"]}"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.completion_contract_path = Some(contract);
        cfg.eval_events_path = Some(events.clone());
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
        let mut fake = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"date-helper.js","content":"exports.formatDate = (d) => String(d);"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("done"),
        ]);
        let err = run_step_plan(&mut fake, &plan, &cfg)
            .unwrap_err()
            .to_string();
        assert!(err.contains("completion contract verify"), "{err}");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"step_obligation_scope\""));
        assert!(event_text.contains("\"step_kind\":\"implement\""));
        assert!(event_text.contains("\"completion_contract_path_merge_enabled\":true"));
        assert!(event_text.contains("\"completion_contract_verification_enabled\":true"));
        assert!(event_text.contains("\"event\":\"completion_verify\""));
        assert!(event_text.contains("\"missing_evidence\""));
        assert!(event_text.contains("\"test_artifact\""));
        assert!(event_text.contains("\"bound_verify_command\""));
    }

    #[test]
    fn ultra_final_acceptance_binds_generated_completion_contract() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"build":"next build"},"dependencies":{"next":"latest","react":"latest","react-dom":"latest"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
import { useEffect, useState } from "react";
export default function Page(){
  const [score, setScore] = useState(0);
  const [gameState, setGameState] = useState("ready");
  const enemies = [{ x: 10, y: 20 }];
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") {
        setGameState("playing");
        setScore((value) => value + 1);
      }
    };
    const frame = requestAnimationFrame(() => {
      const collision = enemies.some((enemy) => enemy.x > 0);
      if (collision) setGameState("gameover");
    });
    window.addEventListener("keydown", onKeyDown);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, []);
  return <main><button onClick={() => setGameState("playing")}>Start</button><button onClick={() => { setGameState("ready"); setScore(0); }}>Restart</button><canvas /><p>score {score} enemy collision {gameState}</p></main>;
}
"#,
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let plan = UltraPlan {
            goal: "Build an interactive browser game".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![UltraPhase {
                id: "final".to_string(),
                prompt: "Final acceptance".to_string(),
            }],
        };
        let _report = ultra_final_acceptance_report(&plan, &cfg).unwrap();
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"completion_contract_bound\""));
        assert!(event_text.contains("\"session_scope\":\"ultra-plan-run\""));
        assert!(event_text.contains("\"event\":\"ultra_final_acceptance\""));
        assert!(event_text.contains("\"completion_contract_verification_enabled\":true"));
        assert!(event_text.contains("\"external_contract_checked\":true"));
        assert!(event_text.contains("\"completion_contract_generated\":true"));
        assert!(
            dir.path()
                .join("completion-contract-ultra-plan-run.json")
                .is_file()
        );
    }

    #[test]
    fn step_repair_missing_entrypoint_followthrough_creates_expected_artifact() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let plan = StepPlan {
            goal: "Create app entrypoint\n\nRequired final artifacts:\n- src/app/page.tsx"
                .to_string(),
            steps: vec![PlanStep {
                id: "entrypoint".to_string(),
                kind: "verify".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Verify and repair src/app/page.tsx if missing".to_string(),
                expected_paths: Vec::new(),
                verify: vec!["test -f src/app/page.tsx".to_string()],
            }],
        };
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
            AssistantReply::text("initial incomplete"),
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/app/page.tsx","content":"export default function Page(){return <main>ok</main>; }"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("repair done"),
        ]);
        let result = run_step_plan(&mut fake, &plan, &cfg).unwrap();
        assert_eq!(result, "plan-run complete: 1 steps");
        assert!(dir.path().join("src/app/page.tsx").is_file());
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"step_verify_repair\""));
        assert!(event_text.contains("\"previous_repair_target\":\"missing_entrypoint\""));
        assert!(event_text.contains("\"repair_follow_through\":\"target_matched\""));
        assert!(event_text.contains("\"repair_target_followed\":true"));
        assert!(event_text.contains("\"changed_paths_before\""));
        assert!(event_text.contains("\"changed_paths_after\""));
        assert!(event_text.contains("\"repair_turn_changed_paths\":[\"src/app/page.tsx\"]"));
        assert!(event_text.contains("\"allowed_action\":\"create_missing_entrypoint_artifact\""));
    }

    #[test]
    fn step_repair_no_change_stops_on_progress_unchanged_and_handoff_saved() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let plan = StepPlan {
            goal: "Create app entrypoint\n\nRequired final artifacts:\n- src/app/page.tsx"
                .to_string(),
            steps: vec![PlanStep {
                id: "entrypoint".to_string(),
                kind: "verify".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Verify and repair src/app/page.tsx if missing".to_string(),
                expected_paths: Vec::new(),
                verify: vec!["test -f src/app/page.tsx".to_string()],
            }],
        };
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
            AssistantReply::text("initial incomplete"),
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Bash",
                    serde_json::json!({"command":"true"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("still incomplete"),
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Bash",
                    serde_json::json!({"command":"true"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("still incomplete again"),
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Bash",
                    serde_json::json!({"command":"true"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("still incomplete third"),
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Bash",
                    serde_json::json!({"command":"true"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("still incomplete fourth"),
        ]);
        let err = run_step_plan(&mut fake, &plan, &cfg)
            .unwrap_err()
            .to_string();
        assert!(err.contains("repair prompt saved"), "{err}");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"repair_follow_through\":\"no_change\""));
        assert!(event_text.contains("\"reason\":\"verify_repair_progress_unchanged\""));
        assert!(event_text.contains("\"event\":\"recovery_prompt_saved\""));
        assert!(event_text.contains("\"failure_kind\":\"verify_repair_progress_unchanged\""));
        assert!(event_text.contains("\"recovery_ultra_plan_path\""));
        assert!(event_text.contains("\"suggested_recovery_yaml_command\""));
        let recovery_plan = assert_single_recovery_ultra_plan(dir.path());
        assert!(recovery_plan.goal.contains("Create app entrypoint"));
        assert!(
            recovery_plan
                .phases
                .iter()
                .any(|phase| phase.prompt.contains("verify_repair_progress_unchanged"))
        );
        let repair_dir = dir.path().join(".anvil/repairs");
        assert!(repair_dir.is_dir());
        assert!(std::fs::read_dir(repair_dir).unwrap().any(|entry| {
            entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|ext| ext == "md")
        }));
    }

    #[test]
    fn saved_recovery_ultra_plan_can_drive_fixture_recovery_success() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let handoff = RecoveryHandoff {
            profile: "generic".to_string(),
            original_goal: "Create app entrypoint".to_string(),
            failed_phase: Some("minimal-loop".to_string()),
            failed_step: Some("completion-verify".to_string()),
            failure_kind: "verify_repair_no_change".to_string(),
            failure_evidence: vec!["src/app/page.tsx is missing".to_string()],
            missing_paths: vec!["src/app/page.tsx".to_string()],
            missing_capabilities: Vec::new(),
            verify_commands: vec!["test -f src/app/page.tsx".to_string()],
            changed_paths: Vec::new(),
            repair_targets: vec!["missing_entrypoint".to_string()],
        };
        let recovery_path =
            save_recovery_ultra_plan(dir.path(), "fixture-recovery", &handoff).unwrap();
        let recovery_plan =
            parse_ultra_plan(&std::fs::read_to_string(&recovery_path).unwrap()).unwrap();
        assert_eq!(
            parse_ultra_plan(&render_ultra_plan(&recovery_plan)).unwrap(),
            recovery_plan
        );
        let inspect_plan =
            serde_json::to_string(&StepPlan::single("Inspect current state")).unwrap();
        let repair_plan = serde_json::to_string(&StepPlan {
            goal: "Repair missing entrypoint".to_string(),
            steps: vec![PlanStep {
                id: "repair-entrypoint".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create src/app/page.tsx".to_string(),
                expected_paths: vec!["src/app/page.tsx".to_string()],
                verify: Vec::new(),
            }],
        })
        .unwrap();
        let verify_plan = serde_json::to_string(&StepPlan {
            goal: "Verify recovered entrypoint".to_string(),
            steps: vec![PlanStep {
                id: "verify-entrypoint".to_string(),
                kind: "verify".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Verify src/app/page.tsx exists".to_string(),
                expected_paths: Vec::new(),
                verify: vec!["test -f src/app/page.tsx".to_string()],
            }],
        })
        .unwrap();
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(inspect_plan),
            AssistantReply::text(repair_plan),
            AssistantReply::text(verify_plan.clone()),
            AssistantReply::text(verify_plan.clone()),
            AssistantReply::text(verify_plan),
        ]);
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/app/page.tsx","content":"export default function Page(){return <main>recovered</main>;}\n"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Bash",
                    serde_json::json!({"command":"true"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("verified"),
        ]);
        let result = run_ultra_plan(&mut planner, &mut execution, &recovery_plan, &cfg).unwrap();
        assert_eq!(result, "ultra-plan-run complete: 3 phases");
        assert!(dir.path().join("src/app/page.tsx").is_file());
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"ultra_plan_complete\""));
    }

    #[test]
    fn dependency_repair_without_setup_authority_saves_unreachable_handoff_without_repair_turn() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let plan = StepPlan {
            goal: "Verify dependency resolution".to_string(),
            steps: vec![PlanStep {
                id: "dependency-probe".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create a deterministic dependency probe script".to_string(),
                expected_paths: vec!["missing-module.sh".to_string()],
                verify: vec!["sh missing-module.sh".to_string()],
            }],
        };
        let mut fake = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: vec![crate::state::ToolCall::new(
                "Write",
                serde_json::json!({
                    "path":"missing-module.sh",
                    "content":"echo \"Cannot find module 'next/package.json'\" >&2\nexit 1\n"
                }),
            )],
            prompt_tokens: None,
            completion_tokens: None,
        }]);

        let err = run_step_plan(&mut fake, &plan, &cfg)
            .unwrap_err()
            .to_string();

        assert!(err.contains("dependency_setup_authority_required"), "{err}");
        assert_eq!(fake.messages.len(), 1);
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"repair_unreachable\""));
        assert!(event_text.contains("\"reason\":\"dependency_setup_authority_required\""));
        assert!(
            event_text.contains("\"repair_attempts\":0")
                || !event_text.contains("step_verify_repair")
        );
        let repair_dir = dir.path().join(".anvil/repairs");
        let prompt = std::fs::read_dir(repair_dir)
            .unwrap()
            .flatten()
            .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
            .find(|text| {
                text.contains("requires a Setup-authority step running dependency install")
            })
            .unwrap_or_default();
        assert!(
            prompt.contains("requires a Setup-authority step running dependency install"),
            "{prompt}"
        );
    }

    #[test]
    fn step_repair_target_not_followed_streak_continues_while_report_improves() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let plan = StepPlan {
            goal: "Create app entrypoint and supporting files\n\nRequired final artifacts:\n- src/app/page.tsx\n- src/app/widget.tsx\n- src/app/sidebar.tsx"
                .to_string(),
            steps: vec![PlanStep {
                id: "entrypoint".to_string(),
                kind: "verify".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Verify and repair the app entrypoint if missing".to_string(),
                expected_paths: Vec::new(),
                verify: vec![
                    "test -f src/app/page.tsx".to_string(),
                    "test -f src/app/widget.tsx".to_string(),
                    "test -f src/app/sidebar.tsx".to_string(),
                ],
            }],
        };
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
            AssistantReply::text("initial incomplete"),
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/app/widget.tsx","content":"export function Widget(){return null;}"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("repair one done"),
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/app/sidebar.tsx","content":"export function Sidebar(){return null;}"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("repair two done"),
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/app/page.tsx","content":"export default function Page(){return <main>ok</main>;}"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("repair three done"),
        ]);
        let result = run_step_plan(&mut fake, &plan, &cfg).unwrap();
        assert_eq!(result, "plan-run complete: 1 steps");
        assert!(dir.path().join("src/app/page.tsx").exists());
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"repair_follow_through\":\"target_not_followed\""));
        assert!(event_text.contains("\"target_not_followed_repairs\":2"));
        assert!(event_text.contains("\"failure_kind\":\"repair_target_not_followed\""));
        assert!(!event_text.contains("\"reason\":\"repair_target_not_followed\""));
    }

    #[test]
    fn step_repair_unrelated_change_is_telemetry_and_handoff_saved() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let plan = StepPlan {
            goal: "Create app entrypoint\n\nRequired final artifacts:\n- src/app/page.tsx"
                .to_string(),
            steps: vec![PlanStep {
                id: "entrypoint".to_string(),
                kind: "verify".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Verify and repair src/app/page.tsx if missing".to_string(),
                expected_paths: Vec::new(),
                verify: vec!["test -f src/app/page.tsx".to_string()],
            }],
        };
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
            AssistantReply::text("initial incomplete"),
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"README.md","content":"not the app entrypoint"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("repair one done"),
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"docs/notes.md","content":"still not the app entrypoint"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("repair two done"),
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"docs/notes-2.md","content":"still unrelated"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("repair three done"),
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"docs/notes-3.md","content":"still unrelated"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("repair four done"),
        ]);
        let err = run_step_plan(&mut fake, &plan, &cfg)
            .unwrap_err()
            .to_string();
        assert!(err.contains("repair prompt saved"), "{err}");
        assert!(!dir.path().join("src/app/page.tsx").exists());
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"repair_follow_through\":\"unrelated_change\""));
        assert!(event_text.contains("\"reason\":\"bounded_repair_exhausted\""));
        assert!(event_text.contains("\"event\":\"recovery_prompt_saved\""));
        assert!(event_text.contains("\"failure_kind\":\"repair_unrelated_change\""));
        assert!(event_text.contains("\"failure_kind\":\"bounded_repair_exhausted\""));
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

    fn assert_single_recovery_ultra_plan(root: &Path) -> UltraPlan {
        let plans_dir = root.join(".anvil/plans");
        assert!(
            plans_dir.is_dir(),
            "missing plans dir: {}",
            plans_dir.display()
        );
        let mut paths = std::fs::read_dir(&plans_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with("recovery-ultra-plan-") && name.ends_with(".yaml")
                    })
            })
            .collect::<Vec<_>>();
        paths.sort();
        assert_eq!(paths.len(), 1, "recovery plan paths: {paths:#?}");
        parse_ultra_plan(&std::fs::read_to_string(&paths[0]).unwrap()).unwrap()
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

    fn challenge_ultra_plan() -> UltraPlan {
        UltraPlan {
            goal: "Create a browser challenge screen".to_string(),
            profile: "generic".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "phase-one".to_string(),
                    prompt: "Create the first page artifact.".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "phase-two".to_string(),
                    prompt: "Close remaining final requirements.".to_string(),
                },
            ],
        }
    }

    fn challenge_implement_step_plan_json() -> String {
        serde_json::to_string(&StepPlan {
            goal: "Create src/app/page.tsx".to_string(),
            steps: vec![PlanStep {
                id: "page".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create or update src/app/page.tsx".to_string(),
                expected_paths: vec!["src/app/page.tsx".to_string()],
                verify: Vec::new(),
            }],
        })
        .unwrap()
    }

    fn challenge_setup_step_plan_json() -> String {
        serde_json::to_string(&StepPlan {
            goal: "Record phase two setup completion".to_string(),
            steps: vec![PlanStep {
                id: "phase-two-marker".to_string(),
                kind: "setup".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create phase-two.txt".to_string(),
                expected_paths: vec!["phase-two.txt".to_string()],
                verify: Vec::new(),
            }],
        })
        .unwrap()
    }

    fn write_challenge_contract(root: &Path) -> PathBuf {
        let path = root.join("challenge-contract.json");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&serde_json::json!({
                "required_paths": ["src/app/page.tsx"],
                "required_evidence": ["challenge_or_adversary_evidence"],
                "verify_repair_cap": 1
            }))
            .unwrap(),
        )
        .unwrap();
        path
    }

    fn planner_request_text(client: &FakeClient, index: usize) -> String {
        client.messages[index]
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n")
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

    fn generated_nextjs_fixture_plan_json_with_kind(
        goal: &str,
        check_path: &str,
        kind: &str,
    ) -> String {
        let mut expected_paths = vec![check_path.to_string()];
        if check_path.contains("scaffold") {
            expected_paths = vec![
                "package.json".to_string(),
                "src/app/page.tsx".to_string(),
                "src/app/layout.tsx".to_string(),
                "src/app/global.d.ts".to_string(),
                check_path.to_string(),
            ];
        }
        let verify = if kind == "setup" {
            Vec::new()
        } else {
            vec![format!("python3 -m py_compile {check_path}")]
        };
        serde_json::to_string(&StepPlan {
            goal: goal.to_string(),
            steps: vec![PlanStep {
                id: "create-and-check-artifacts".to_string(),
                kind: kind.to_string(),
                expected_result: "pass".to_string(),
                instruction: format!(
                    "Create the declared artifacts including {check_path} and keep the Next.js files coherent"
                ),
                expected_paths,
                verify,
            }],
        })
        .unwrap()
    }

    fn interactive_game_page_source() -> &'static str {
        r#""use client";
import { useEffect, useState } from "react";
export default function Page(){
  const [score, setScore] = useState(0);
  const [gameState, setGameState] = useState("ready");
  const enemies = [{ x: 10, y: 20 }];
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") {
        setGameState("playing");
        setScore((value) => value + 1);
      }
    };
    const frame = requestAnimationFrame(() => {
      const collision = enemies.some((enemy) => enemy.x > 0);
      if (collision) setGameState("gameover");
    });
    window.addEventListener("keydown", onKeyDown);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, []);
  return <main><button onClick={() => setGameState("playing")}>Start</button><button onClick={() => { setGameState("ready"); setScore(0); }}>Restart</button><canvas /><p>score {score} enemy collision {gameState}</p></main>;
}
"#
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

    fn enable_browser_probe_test_override(root: &Path) {
        std::fs::create_dir_all(root.join(".anvil")).unwrap();
        std::fs::write(root.join(".anvil/enable-browser-probe-tests"), "1").unwrap();
    }

    fn write_browser_probe_mock_command(root: &Path, status: &str) -> u16 {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let dir = root.join(".anvil/evidence");
        std::fs::create_dir_all(&dir).unwrap();
        let exe = std::env::current_exe().unwrap();
        let command = serde_json::json!({
            "program": exe.display().to_string(),
            "args": [
                "--ignored",
                "--exact",
                "minimal_loop::browser_probe::tests::browser_probe_mock_server_child",
                "--nocapture"
            ],
            "env": {
                "ANVIL_BROWSER_PROBE_MOCK_CHILD": "1",
                "ANVIL_BROWSER_PROBE_MOCK_PORT": port.to_string(),
                "ANVIL_BROWSER_PROBE_MOCK_STATUS": status,
                "ANVIL_BROWSER_PROBE_MOCK_DELAY_MS": "0"
            },
            "port": port,
            "require_build": false,
            "display": "mock browser probe child"
        });
        std::fs::write(
            dir.join("browser-probe-command.json"),
            serde_json::to_string_pretty(&command).unwrap(),
        )
        .unwrap();
        port
    }

    fn run_ignored_runner_harness(test_name: &str) -> std::process::ExitStatus {
        let exe = std::env::current_exe().unwrap();
        std::process::Command::new(exe)
            .args(["--ignored", "--exact", test_name, "--nocapture"])
            .env("NODE_ENV", "production")
            .status()
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
