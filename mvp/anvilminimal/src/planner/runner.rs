use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use crate::bounded_process;
use crate::config::Config;
use crate::eval_events;
use crate::minimal_loop::behavior_evidence::{self, EvidenceArbitrationReport};
use crate::minimal_loop::browser_probe::{
    BrowserReadinessObservation, html_surface_markers_json,
    probe_browser_readiness_with_offline_and_interaction_options,
};
use crate::minimal_loop::build_verifier::{
    BuildVerifierLifecycleObservation, BuildVerifierObservation, BuildVerifierRequirement,
    BuildVerifierStatus, CompileError, emit_dependency_build_lifecycle,
};
use crate::minimal_loop::completion::{
    CompileRepairPromptProtection, CompletionContract, compile_error_repair_guidance,
    compile_repair_prompt_section_with_root, evidence_hint_tokens_for_goal,
};
use crate::minimal_loop::dependency_setup::{
    self, NodeDependencySetupAuthority, NodeDependencySetupRequirement, NodeDependencySetupStatus,
};
use crate::minimal_loop::evidence::{
    RuntimeAcceptanceReport, comment_stripped_source_corpus, required_evidence_for_capability,
    verify_runtime_acceptance_with_browser_dirs_and_hints,
};
use crate::minimal_loop::import_scan::{
    MissingImport, UnattachedRefDiagnostic, format_missing_import_findings,
    missing_import_target_rel, route_bound_closure, route_bound_unattached_ref_diagnostics,
    scan_relative_imports,
};
use crate::minimal_loop::interaction_probe::{
    self, BrowserInteractionProbeOptions, InteractionProbeOutcome,
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
    RepairFollowThrough, RepairTarget, classify_repair_follow_through, classify_repair_target,
};
use crate::minimal_loop::verifier_env;
use crate::planner::intent::detect_intent;
use crate::planner::lint::{
    PlanLintReport, PlanQualityContext, PlanQualityReport, lint_step_plan_report_with_workspace,
    lint_ultra_plan_report, step_plan_quality_report, step_plan_quality_warnings,
};
use crate::planner::profile::{
    GENERIC_INTERACTIVE_CONTRACT_CAPABILITY, PhaseVerificationMode, ProfileBehaviorProbeReport,
    ProfileInferenceSource, ProfileSnapshot, canonical_profile_name, domain_profile, infer_profile,
    profile_auto_repair, profile_before_phase, profile_expected_paths, profile_generation_rules,
    profile_guidance, profile_post_step_repair, profile_quality_expectations,
    profile_runtime_contract, profile_setup_scaffold_paths, verify_profile_final,
    verify_profile_invariant,
};
use crate::planner::repair::{
    RecoveryHandoff, RepairContext, build_repair_prompt_with_context, save_recovery_ultra_plan,
    save_repair_report_with_context, save_ultra_recovery_prompt,
    suggested_recovery_ultra_plan_command, suggested_ultra_recovery_command,
    workspace_relative_handoff_path,
};
use crate::planner::sanitizer::{SanitizerReport, sanitize_step_plan_against_policy};
use crate::planner::signals;
use crate::planner::step_plan::{
    PlanStep, StepKind, StepPlan, extract_json_object, parse_generated_step_plan_json,
    parse_step_plan, render_step_plan, repair_generated_step_plan_contract,
};
use crate::planner::ultra_plan::{UltraPhase, UltraPlan, parse_ultra_plan, render_ultra_plan};
use crate::planner::verify::{
    VerificationReport, verify_setup_dependency_state_with_setup_observed_with_offline,
    verify_step_with_profile_setup_observed_with_offline,
    verify_step_with_profile_setup_observed_with_offline_and_events,
};
use crate::provider_call::{self, ProviderCallScope};
use crate::providers::{AssistantReply, ChatClient, model_for};
use crate::state::SessionSnapshot;
use crate::tools::path_guard::resolve_existing;
use crate::tui::status::UiStatus;
use crate::tui::{InteractionUi, NOOP_UI};
use serde_json::{Value, json};

const STEP_TURN_MAX_ITERATIONS: usize = 8;
const STEP_REPAIR_MAX_ITERATIONS: usize = 6;
const STEP_REPAIR_MAX_TURNS: usize = 4;
const STEP_REPAIR_IDENTICAL_NO_CHANGE_LIMIT: usize = 2;
const PLANNER_PROVIDER_REQUEST_ATTEMPTS: usize = 2;
const PLANNER_PROVIDER_REQUEST_RETRY_DELAY: Duration = Duration::from_millis(80);
const ULTRA_PLAN_GENERATION_ATTEMPTS: usize = 3;
const FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS: usize = 2;
const FINAL_ACCEPTANCE_COMPILE_NO_SNAPSHOT_EXTRA_ATTEMPTS: usize = 1;
const FINAL_ACCEPTANCE_REPAIR_WALL_CLOCK_CAP: Duration = Duration::from_secs(240);
const NEXTJS_DEV_SERVER_DEFAULT_PORT: u16 = 3011;
const NEXTJS_DEV_SERVER_READY_TIMEOUT: Duration = Duration::from_secs(8);
const NEXTJS_DEV_SERVER_CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const NEXTJS_DEV_SERVER_WAIT_INTERVAL: Duration = Duration::from_millis(250);
const DEV_SERVER_CLEANUP_TERM_TIMEOUT: Duration = Duration::from_secs(5);
const DEV_SERVER_CLEANUP_KILL_TIMEOUT: Duration = Duration::from_secs(1);
const DEV_SERVER_LOG_EXCERPT_BYTES: usize = 24_000;
const DEV_SERVER_ROUTE: &str = "/";
const DEV_SERVER_LIFECYCLE_STAGES: [&str; 4] = ["start", "wait", "probe", "cleanup"];
const PROFILE_REPAIR_FILE_EXCERPT_MAX_CHARS: usize = 2_400;
const INTERACTION_STATE_CHANGE_REPAIR_REQUIREMENT: &str = "keyboard or pointer input must visibly change game state (player position, projectiles, score/health, or state transitions); wire input handlers into the render/update loop.";
const INTERACTION_START_REPAIR_REQUIREMENT: &str = "primary/start controls must transition the visible app state before input is evaluated; wire the start action into state and render updates.";
const PERSISTENCE_RELOAD_REPAIR_REQUIREMENT: &str = "load persisted state on mount (e.g. read localStorage in initialization) and write on mutation";
const TEXT_ECHO_REPAIR_REQUIREMENT: &str = "token never rendered; render the input's content reactively (no manual rebuild) - the typed text must appear in the preview/list";
const TEXT_ECHO_AFTER_RELOAD_REPAIR_REQUIREMENT: &str =
    "preview renders only after reload - make it reactive to input";
const APP_BEHAVIOR_PROBE_FAILURE_KINDS: [&str; 15] = [
    "canvas_blank",
    "interaction_state_change_missing",
    "input_state_change_missing_after_start",
    "input_state_change_not_evaluated_after_start",
    "persistence_after_reload_reset",
    "primary_start_transition_missing",
    "start_transition_missing",
    "surface_missing",
    "text_entry_missing",
    "text_input_state_change_missing",
    "token_echo_after_reload_only",
    "token_echo_missing",
    "surface_visible_missing",
    "interactive_surface_missing",
    "canvas_unavailable",
];

const GENERIC_INTERACTIVE_EVIDENCE_KEYS: [&str; 3] = [
    "user_input_handler_evidence",
    "stateful_update_evidence",
    "visible_interactive_surface_evidence",
];

thread_local! {
    static FINAL_ACCEPTANCE_CYCLE_INDEX: Cell<usize> = const { Cell::new(0) };
}

fn current_final_acceptance_cycle_index() -> usize {
    FINAL_ACCEPTANCE_CYCLE_INDEX.with(Cell::get)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EffectiveRequestedPort {
    port: u16,
    telemetry: String,
}

fn effective_requested_port(
    profile: &str,
    goal: &str,
    plan_text: Option<&str>,
) -> Option<EffectiveRequestedPort> {
    if let Some(requested) = signals::requested_port(goal, plan_text) {
        return Some(EffectiveRequestedPort {
            port: requested.port,
            telemetry: format!("{} ({})", requested.port, requested.source.as_str()),
        });
    }
    (canonical_profile_name(profile) == "nextjs").then(|| EffectiveRequestedPort {
        port: crate::planner::profiles::nextjs::DEFAULT_REQUESTED_PORT,
        telemetry: format!(
            "{} (default)",
            crate::planner::profiles::nextjs::DEFAULT_REQUESTED_PORT
        ),
    })
}

fn with_final_acceptance_cycle<T>(cycle_index: usize, f: impl FnOnce() -> T) -> T {
    FINAL_ACCEPTANCE_CYCLE_INDEX.with(|cell| {
        let previous = cell.replace(cycle_index);
        let result = f();
        cell.set(previous);
        result
    })
}

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

#[derive(Debug, Clone, Default)]
struct FinalAcceptanceCycleDelta {
    cycle_index: usize,
    resolved_keys: Vec<String>,
    remaining_keys: Vec<String>,
    changed_paths: Vec<String>,
    route_bound_changed_paths: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PlanAdherenceReport {
    present: Vec<String>,
    missing: Vec<String>,
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

fn planner_chat_with_request_retry(
    client: &mut dyn ChatClient,
    config: &Config,
    scope: ProviderCallScope,
    model: &str,
    messages: &[crate::state::ConversationMessage],
    ui: &dyn InteractionUi,
) -> anyhow::Result<AssistantReply> {
    let mut last_error = None;
    for request_attempt in 1..=PLANNER_PROVIDER_REQUEST_ATTEMPTS {
        let result = {
            let _guard = ui.before_model_call(&format!("planner {} {model}", client.label()));
            provider_call::chat_with_cancel(
                client,
                config,
                provider_call::ProviderChatRequest {
                    scope,
                    model,
                    messages,
                    tools: &[],
                    native_tools_enabled: false,
                },
                || ui.interrupted(),
            )
            .result
        };
        match result {
            Ok(reply) => return Ok(reply),
            Err(err) => {
                last_error = Some(err.to_string());
                if last_error
                    .as_deref()
                    .is_some_and(provider_call::is_aborted_by_user)
                {
                    break;
                }
                if request_attempt < PLANNER_PROVIDER_REQUEST_ATTEMPTS {
                    std::thread::sleep(PLANNER_PROVIDER_REQUEST_RETRY_DELAY);
                }
            }
        }
    }
    let last_error = last_error.unwrap_or_else(|| "unknown provider error".to_string());
    if provider_call::is_aborted_by_user(&last_error) {
        anyhow::bail!("{last_error}");
    }
    if provider_call::is_scoped_timeout(scope, &last_error) {
        anyhow::bail!("{last_error}");
    }
    anyhow::bail!(
        "provider request failed after {} attempts: {}",
        PLANNER_PROVIDER_REQUEST_ATTEMPTS,
        last_error
    )
}

pub fn generate_step_plan_with_ui(
    client: &mut dyn ChatClient,
    goal: &str,
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<StepPlan> {
    generate_step_plan_with_ui_for_phase(client, goal, config, ui, None)
}

fn generate_step_plan_with_ui_for_phase(
    client: &mut dyn ChatClient,
    goal: &str,
    config: &Config,
    ui: &dyn InteractionUi,
    phase_label: Option<&str>,
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
        let reply = planner_chat_with_request_retry(
            client,
            config,
            ProviderCallScope::PlannerStep,
            model,
            &messages,
            ui,
        )?;
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
                let sanitizer_report =
                    sanitize_step_plan_against_policy(&mut plan, Some(&config.workspace_root));
                let plan_was_sanitized = !sanitizer_report.is_empty();
                emit_planner_plan_sanitized(
                    config,
                    client.label(),
                    model,
                    attempt,
                    &sanitizer_report,
                );
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
                    if quality_report.has_retryable_quality() && !plan_was_sanitized {
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
                    emit_step_plan_presentation(phase_label, &plan, Some(&sanitizer_report));
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
                        emit_step_plan_presentation(phase_label, &plan, None);
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
        emit_step_plan_presentation(phase_label, &plan, None);
        return Ok(plan);
    }
    if let Some(plan) = fallback_step_plan_for_setup_phase(goal, config) {
        emit_planner_fallback_plan(
            config,
            client.label(),
            model,
            goal,
            &plan,
            last_error.as_deref().unwrap_or("unknown parse error"),
        );
        emit_step_plan_presentation(phase_label, &plan, None);
        return Ok(plan);
    }
    anyhow::bail!(
        "invalid StepPlan after corrective retries: {}",
        last_error.unwrap_or_else(|| "unknown parse error".to_string())
    )
}

fn emit_step_plan_presentation(
    phase_label: Option<&str>,
    plan: &StepPlan,
    sanitizer_report: Option<&SanitizerReport>,
) {
    let phase = phase_label.unwrap_or(&plan.goal);
    crate::tui::presentation::emit_step_plan_block(phase, plan, sanitizer_report);
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
    compile_rollbacks: Vec<CompileRollbackOutcome>,
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
    carry_forward_guidance: Vec<String>,
    truncated: bool,
}

#[derive(Debug, Clone)]
struct ProfilePromotionState {
    eligible: bool,
    promoted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProfilePromotion {
    id: String,
    at_phase: usize,
    phase_id: String,
    requested_port: Option<String>,
    contract_origin: String,
    delta_capabilities: Vec<String>,
    delta_requirements: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ContractRequirements {
    capabilities: Vec<String>,
    evidence: Vec<String>,
    obligations: Vec<String>,
}

const ULTRA_CONTEXT_MAX_PHASES: usize = 12;
const ULTRA_CONTEXT_MAX_PATHS: usize = 24;
const ULTRA_CONTEXT_MAX_MESSAGES: usize = 10;
const ULTRA_PROMPT_GUIDANCE_MAX_LINES: usize = 8;

impl ProfilePromotionState {
    fn for_run(plan: &UltraPlan, config: &Config) -> Self {
        Self {
            eligible: canonical_profile_name(&plan.profile) == "generic"
                && !config.profile_explicit,
            promoted: false,
        }
    }

    fn can_promote(&self, plan: &UltraPlan) -> bool {
        self.eligible && !self.promoted && canonical_profile_name(&plan.profile) == "generic"
    }
}

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
        for rollback in &outcome.compile_rollbacks {
            push_context_items_capped(
                &mut self.carry_forward_guidance,
                &rollback.carry_forward_guidance,
                ULTRA_CONTEXT_MAX_MESSAGES,
                &mut self.truncated,
            );
        }
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
        push_context_items_capped(
            &mut self.pending_capability_evidence,
            &report.diagnostics,
            ULTRA_CONTEXT_MAX_MESSAGES,
            &mut self.truncated,
        );
    }

    fn render_unmet_final_requirements_section(&self) -> String {
        if self.pending_capability_evidence.is_empty() {
            return "Unmet final requirements from earlier phases:\n- none".to_string();
        }
        render_bounded_prompt_section(
            "Unmet final requirements from earlier phases:",
            &self.pending_capability_evidence,
            Some("Close these requirements when they are in scope for this phase."),
            ULTRA_PROMPT_GUIDANCE_MAX_LINES,
        )
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
        for rollback in &outcome.compile_rollbacks {
            push_context_items_capped(
                &mut self.carry_forward_guidance,
                &rollback.carry_forward_guidance,
                ULTRA_CONTEXT_MAX_MESSAGES,
                &mut self.truncated,
            );
        }
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
            && self.carry_forward_guidance.is_empty()
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
        append_context_list(
            &mut lines,
            "Carry-forward guidance",
            &self.carry_forward_guidance,
        );
        if self.truncated {
            lines.push("- Context was truncated to bounded path/failure summaries".to_string());
        }
        lines.join("\n")
    }
}

fn try_promote_profile_at_phase_boundary(
    config: &mut Config,
    plan: &mut UltraPlan,
    context: &mut UltraRunContext,
    final_expected_paths: &mut Vec<String>,
    promotion_state: &mut ProfilePromotionState,
    phase: &UltraPhase,
    index: usize,
) -> anyhow::Result<Option<ProfilePromotion>> {
    if !promotion_state.can_promote(plan) {
        return Ok(None);
    }
    let Some(inference) = infer_profile(None, &config.workspace_root) else {
        return Ok(None);
    };
    if inference.source != ProfileInferenceSource::Workspace {
        return Ok(None);
    }
    let promoted = canonical_profile_name(inference.profile);
    if promoted == "generic" || promoted == canonical_profile_name(&plan.profile) {
        return Ok(None);
    }

    let generic_paths = profile_expected_paths(&config.workspace_root, "generic", &plan.goal);
    let mut generic_requirements = inferred_contract_requirements("generic", &plan.goal);
    let pre_promotion_contract = bind_completion_contract_for_acceptance(
        config,
        "ultra-plan-run-pre-promotion",
        "generic",
        &plan.goal,
        &generic_paths,
        &generic_requirements.capabilities,
        &generic_requirements.evidence,
        &generic_requirements.obligations,
    )?;
    if let Some(contract) = pre_promotion_contract.as_ref().map(|bound| &bound.contract) {
        merge_unique_strings(
            &mut generic_requirements.capabilities,
            &contract.required_capabilities,
        );
        merge_unique_strings(
            &mut generic_requirements.evidence,
            &contract.required_evidence,
        );
        merge_unique_strings(
            &mut generic_requirements.obligations,
            &contract.required_obligations,
        );
    }

    let mut promoted_requirements = inferred_contract_requirements(&promoted, &plan.goal);
    carry_pre_promotion_contract_requirements(
        &promoted,
        &plan.goal,
        &generic_requirements,
        &mut promoted_requirements,
    );
    merge_unique_strings(
        &mut promoted_requirements.evidence,
        &inferred_required_evidence(&promoted, &plan.goal, &promoted_requirements.capabilities),
    );
    let promoted_paths = profile_expected_paths(&config.workspace_root, &promoted, &plan.goal);
    let bound_contract = bind_completion_contract_for_acceptance(
        config,
        "ultra-plan-run",
        &promoted,
        &plan.goal,
        &promoted_paths,
        &promoted_requirements.capabilities,
        &promoted_requirements.evidence,
        &promoted_requirements.obligations,
    )?;
    if let Some(contract) = bound_contract.as_ref().map(|bound| &bound.contract) {
        merge_unique_strings(
            &mut promoted_requirements.capabilities,
            &contract.required_capabilities,
        );
        merge_unique_strings(
            &mut promoted_requirements.evidence,
            &contract.required_evidence,
        );
        merge_unique_strings(
            &mut promoted_requirements.obligations,
            &contract.required_obligations,
        );
    }
    merge_unique_strings(
        &mut promoted_requirements.evidence,
        &inferred_required_evidence(&promoted, &plan.goal, &promoted_requirements.capabilities),
    );

    let mapped_generic_capabilities = mapped_pre_promotion_capabilities_for_profile(
        &promoted,
        &generic_requirements.capabilities,
    );
    debug_assert!(
        mapped_generic_capabilities
            .iter()
            .all(|capability| promoted_requirements.capabilities.contains(capability)),
        "profile promotion dropped a pre-promotion capability"
    );
    debug_assert!(
        generic_requirements
            .evidence
            .iter()
            .all(|evidence| promoted_requirements.evidence.contains(evidence)),
        "profile promotion dropped pre-promotion evidence"
    );
    debug_assert!(
        generic_requirements
            .obligations
            .iter()
            .all(|obligation| promoted_requirements.obligations.contains(obligation)),
        "profile promotion dropped pre-promotion obligations"
    );

    let delta_capabilities = ordered_string_difference(
        &promoted_requirements.capabilities,
        &generic_requirements.capabilities,
    );
    let mut generic_requirement_keys = generic_requirements.capabilities.clone();
    merge_unique_strings(
        &mut generic_requirement_keys,
        &generic_requirements.evidence,
    );
    merge_unique_strings(
        &mut generic_requirement_keys,
        &generic_requirements.obligations,
    );
    let mut promoted_requirement_keys = promoted_requirements.capabilities.clone();
    merge_unique_strings(
        &mut promoted_requirement_keys,
        &promoted_requirements.evidence,
    );
    merge_unique_strings(
        &mut promoted_requirement_keys,
        &promoted_requirements.obligations,
    );
    let delta_requirements =
        ordered_string_difference(&promoted_requirement_keys, &generic_requirement_keys);
    push_context_items_capped(
        &mut context.pending_capability_evidence,
        &delta_requirements,
        ULTRA_CONTEXT_MAX_MESSAGES,
        &mut context.truncated,
    );

    plan.profile = promoted.clone();
    config.profile = promoted.clone();
    config.profile_inference = Some(inference);
    *final_expected_paths =
        profile_expected_paths(&config.workspace_root, &plan.profile, &plan.goal);
    context.pending_final_artifacts =
        missing_final_artifacts(&config.workspace_root, final_expected_paths);
    promotion_state.promoted = true;

    let promotion = ProfilePromotion {
        id: promoted,
        at_phase: index + 1,
        phase_id: phase.id.clone(),
        requested_port: effective_requested_port(
            &plan.profile,
            &plan.goal,
            Some(&ultra_plan_phase_signal_text(plan)),
        )
        .map(|requested| requested.telemetry),
        contract_origin: "promoted_union".to_string(),
        delta_capabilities,
        delta_requirements,
    };
    emit_profile_reinferred(config, &promotion);
    Ok(Some(promotion))
}

fn ordered_string_difference(values: &[String], baseline: &[String]) -> Vec<String> {
    let baseline = baseline.iter().map(String::as_str).collect::<BTreeSet<_>>();
    values
        .iter()
        .filter(|value| !baseline.contains(value.as_str()))
        .cloned()
        .collect()
}

fn inferred_contract_requirements(profile: &str, goal: &str) -> ContractRequirements {
    let capabilities = inferred_required_capabilities(profile, goal);
    ContractRequirements {
        evidence: inferred_required_evidence(profile, goal, &capabilities),
        obligations: inferred_required_obligations(profile, goal, &capabilities),
        capabilities,
    }
}

fn carry_pre_promotion_contract_requirements(
    promoted_profile: &str,
    goal: &str,
    pre_promotion: &ContractRequirements,
    promoted: &mut ContractRequirements,
) {
    merge_unique_strings(
        &mut promoted.capabilities,
        &mapped_pre_promotion_capabilities_for_profile(
            promoted_profile,
            &pre_promotion.capabilities,
        ),
    );
    if pre_promotion
        .capabilities
        .iter()
        .any(|capability| capability == GENERIC_INTERACTIVE_CONTRACT_CAPABILITY)
        && signals::contains_app_intent_token(goal)
    {
        merge_unique_strings(
            &mut promoted.capabilities,
            &interactive_app_capabilities_for_profile(promoted_profile),
        );
    }
    merge_unique_strings(&mut promoted.evidence, &pre_promotion.evidence);
    merge_unique_strings(&mut promoted.obligations, &pre_promotion.obligations);
}

fn mapped_pre_promotion_capabilities_for_profile(
    promoted_profile: &str,
    capabilities: &[String],
) -> Vec<String> {
    let mut mapped = Vec::new();
    for capability in capabilities {
        if capability == GENERIC_INTERACTIVE_CONTRACT_CAPABILITY {
            let equivalents = interactive_app_capabilities_for_profile(promoted_profile);
            if equivalents.is_empty() {
                merge_unique_strings(&mut mapped, std::slice::from_ref(capability));
            } else {
                merge_unique_strings(&mut mapped, &equivalents);
            }
        } else {
            merge_unique_strings(&mut mapped, std::slice::from_ref(capability));
        }
    }
    mapped
}

fn interactive_app_capabilities_for_profile(profile: &str) -> Vec<String> {
    match canonical_profile_name(profile).as_str() {
        "nextjs" | "react" | "vite" | "web" => vec![
            "stateful_interaction".to_string(),
            "user_input_or_action".to_string(),
            "visible_state_change".to_string(),
        ],
        _ => Vec::new(),
    }
}

fn emit_profile_reinferred(config: &Config, promotion: &ProfilePromotion) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "profile_reinferred",
            "id": promotion.id,
            "profile": promotion.id,
            "at_phase": promotion.at_phase,
            "phase_id": promotion.phase_id,
            "from": "workspace",
            "from_profile": "generic",
            "to_profile": promotion.id,
            "requested_port": promotion.requested_port.clone(),
            "contract_origin": promotion.contract_origin,
            "delta_capabilities": promotion.delta_capabilities.clone(),
            "delta_requirements": promotion.delta_requirements.clone(),
        }),
    );
    let line = format!(
        "Profile promoted: generic -> {} (workspace evidence, phase {})",
        promotion.id, promotion.at_phase
    );
    eprintln!("{line}");
    eval_events::write_run_summary(config.eval_events_path.as_deref(), &line);
}

fn contract_origin_for_acceptance(config: &Config) -> &'static str {
    if config
        .eval_events_path
        .as_deref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .is_some_and(|text| {
            text.lines()
                .any(|line| line.contains(r#""event":"profile_reinferred""#))
        })
    {
        "promoted_union"
    } else {
        "initial"
    }
}

fn carry_recorded_promotion_contract_requirements(
    config: &Config,
    effective_profile: &str,
    goal: &str,
    requirements: &mut ContractRequirements,
) {
    if contract_origin_for_acceptance(config) != "promoted_union"
        || canonical_profile_name(effective_profile) == "generic"
    {
        return;
    }
    let generic_requirements = inferred_contract_requirements("generic", goal);
    carry_pre_promotion_contract_requirements(
        effective_profile,
        goal,
        &generic_requirements,
        requirements,
    );
    merge_unique_strings(
        &mut requirements.evidence,
        &inferred_required_evidence(effective_profile, goal, &requirements.capabilities),
    );
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
        self.compile_rollbacks
            .extend(step.compile_rollbacks.iter().cloned());
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
    compile_rollbacks: Vec<CompileRollbackOutcome>,
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

#[derive(Debug, Clone, Default)]
struct UltraRunSetupAuthorityState {
    reasons: Vec<String>,
}

impl UltraRunSetupAuthorityState {
    fn authority(&self) -> NodeDependencySetupAuthority {
        if self.reasons.is_empty() {
            NodeDependencySetupAuthority::None
        } else {
            NodeDependencySetupAuthority::PlanSetupStep
        }
    }

    fn grant(&mut self, reason: &str) {
        if !self.reasons.iter().any(|existing| existing == reason) {
            self.reasons.push(reason.to_string());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DependencyReconciliationTrigger {
    Promotion,
    ManifestRepair,
}

impl DependencyReconciliationTrigger {
    fn as_str(self) -> &'static str {
        match self {
            Self::Promotion => "promotion",
            Self::ManifestRepair => "manifest_repair",
        }
    }
}

fn reconcile_run_dependency_setup(
    config: &Config,
    profile: &str,
    trigger: DependencyReconciliationTrigger,
    setup_authority: &UltraRunSetupAuthorityState,
) -> anyhow::Result<Option<Vec<String>>> {
    let authority = setup_authority.authority();
    let Some(requirement) =
        dependency_reconciliation_requirement(&config.workspace_root, profile, trigger, authority)
    else {
        return Ok(None);
    };
    let setup = dependency_setup::run_node_dependency_setup_with_program_and_offline(
        &config.workspace_root,
        &requirement,
        Path::new("npm"),
        config.offline,
    );
    let lifecycle = dependency_reconciliation_lifecycle(&requirement, setup.clone());
    emit_dependency_build_lifecycle(
        config.eval_events_path.as_deref(),
        "ultra-plan-run",
        Some(trigger.as_str()),
        &lifecycle,
    );
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "dependency_setup_reconciliation",
            "trigger": trigger.as_str(),
            "profile": canonical_profile_name(profile),
            "authority": authority.as_str(),
            "setup_kind": setup.setup_kind.as_str(),
            "status": setup.status.as_str(),
            "attempted": setup.attempted,
            "command": setup.command.clone(),
            "added": setup.changed_paths.clone(),
            "primary_reason": eval_events::body_snippet(&setup.primary_reason),
            "duration_ms": setup.duration_ms,
            "timeout_ms": setup.timeout_ms,
            "classification": if setup.status == NodeDependencySetupStatus::TimedOut {
                "dependency_setup_timeout"
            } else {
                ""
            },
            "offline": config.offline,
        }),
    );
    match setup.status {
        NodeDependencySetupStatus::Passed | NodeDependencySetupStatus::NotRequired => {
            Ok(Some(setup.changed_paths))
        }
        NodeDependencySetupStatus::Blocked
            if setup.primary_reason == "dependency_setup_blocked_offline" =>
        {
            anyhow::bail!("dependency_setup_blocked_offline")
        }
        NodeDependencySetupStatus::Blocked => {
            anyhow::bail!("dependency_setup_blocked: {}", setup.primary_reason)
        }
        NodeDependencySetupStatus::Failed | NodeDependencySetupStatus::TimedOut => {
            anyhow::bail!(
                "dependency_setup_lifecycle_failed: {}",
                setup.primary_reason
            )
        }
        NodeDependencySetupStatus::Attempted => {
            anyhow::bail!("dependency_setup_lifecycle_failed: dependency setup did not finish")
        }
    }
}

fn dependency_reconciliation_requirement(
    root: &Path,
    profile: &str,
    trigger: DependencyReconciliationTrigger,
    authority: NodeDependencySetupAuthority,
) -> Option<NodeDependencySetupRequirement> {
    let reason = format!("{} dependency reconciliation", trigger.as_str());
    match canonical_profile_name(profile).as_str() {
        "nextjs"
            if dependency_setup::package_json_declares_dependencies(root)
                && !dependency_setup::next_build_dependencies_ready(root) =>
        {
            Some(dependency_setup::requirement_for_next_build(
                root,
                Some("nextjs"),
                &reason,
                authority,
            ))
        }
        "python-cli" | "python_cli"
            if dependency_setup::python_cli_declares_dependencies(root)
                && !dependency_setup::python_cli_dependencies_ready(root) =>
        {
            Some(dependency_setup::requirement_for_python_cli_dependencies(
                root,
                Some("python-cli"),
                &reason,
                authority,
            ))
        }
        _ if dependency_setup::package_json_declares_dependencies(root)
            && !dependency_setup::node_declared_dependencies_ready(root) =>
        {
            Some(
                dependency_setup::requirement_for_node_declared_dependencies(
                    root,
                    Some(&canonical_profile_name(profile)),
                    &reason,
                    authority,
                ),
            )
        }
        _ => None,
    }
}

fn dependency_reconciliation_lifecycle(
    requirement: &NodeDependencySetupRequirement,
    setup: dependency_setup::NodeDependencySetupObservation,
) -> BuildVerifierLifecycleObservation {
    let before = BuildVerifierObservation {
        command: "dependency reconciliation".to_string(),
        profile: requirement.profile.clone(),
        authority: requirement.setup_authority.as_str().to_string(),
        required_for_completion: true,
        requires_dependency_setup: true,
        dependency_ready: false,
        attempted: false,
        status: BuildVerifierStatus::DependencyMissing,
        primary_reason: requirement.reason.clone(),
        output_snippet: String::new(),
        compile_errors: Vec::new(),
        foreign_toolchain: None,
    };
    let after_status = match setup.status {
        NodeDependencySetupStatus::Passed | NodeDependencySetupStatus::NotRequired => {
            BuildVerifierStatus::Passed
        }
        NodeDependencySetupStatus::Blocked | NodeDependencySetupStatus::TimedOut => {
            BuildVerifierStatus::Blocked
        }
        NodeDependencySetupStatus::Failed | NodeDependencySetupStatus::Attempted => {
            BuildVerifierStatus::Failed
        }
    };
    let after = (after_status == BuildVerifierStatus::Passed).then(|| BuildVerifierObservation {
        command: "dependency reconciliation".to_string(),
        profile: requirement.profile.clone(),
        authority: requirement.setup_authority.as_str().to_string(),
        required_for_completion: true,
        requires_dependency_setup: true,
        dependency_ready: true,
        attempted: false,
        status: BuildVerifierStatus::Passed,
        primary_reason: "dependency setup reconciliation passed".to_string(),
        output_snippet: String::new(),
        compile_errors: Vec::new(),
        foreign_toolchain: None,
    });
    let final_reason = after
        .as_ref()
        .map(|observation| observation.primary_reason.clone())
        .unwrap_or_else(|| setup.primary_reason.clone());
    BuildVerifierLifecycleObservation {
        requirement: BuildVerifierRequirement {
            command: "dependency reconciliation".to_string(),
            profile: requirement.profile.clone(),
            reason: requirement.reason.clone(),
            authority: requirement.setup_authority.as_str().to_string(),
            status: "required".to_string(),
            requires_dependency_setup: true,
            required_for_completion: true,
        },
        before_setup: before,
        setup: Some(setup),
        after_setup: after,
        final_status: after_status,
        final_reason,
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
    overall_goal_override: Option<&str>,
) -> Result<StepPlanRunOutcome, StepPlanRunError> {
    run_step_plan_with_session_with_ui_and_run_authority(
        client,
        session,
        plan,
        config,
        ui,
        verify_final_contract,
        mode,
        phase_scope,
        overall_goal_override,
        None,
    )
}

#[allow(clippy::result_large_err, clippy::too_many_arguments)]
fn run_step_plan_with_session_with_ui_and_run_authority(
    client: &mut dyn ChatClient,
    session: &mut SessionSnapshot,
    plan: &StepPlan,
    config: &Config,
    ui: &dyn InteractionUi,
    verify_final_contract: bool,
    mode: &'static str,
    phase_scope: Option<&str>,
    overall_goal_override: Option<&str>,
    mut run_setup_authority: Option<&mut UltraRunSetupAuthorityState>,
) -> Result<StepPlanRunOutcome, StepPlanRunError> {
    let mut outcome = StepPlanRunOutcome::for_plan(plan);
    let overall_goal = overall_goal_override.unwrap_or(&plan.goal);
    let report = lint_step_plan_report_with_workspace(plan, Some(&config.workspace_root));
    if !report.is_pass() {
        emit_planner_error_for_lint(config, "plan-file", &config.planner_model, &report, 0);
        if !lint_report_is_runtime_repairable_verifier_command(&report) {
            return Err(StepPlanRunError::from_error(
                report.primary_message(),
                outcome,
            ));
        }
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
            overall_goal: overall_goal.to_string(),
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
            run_setup_authority.as_deref_mut(),
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
        if let Some(state) = run_setup_authority.as_deref_mut()
            && step_carries_setup_authority(plan, step, phase_scope)
        {
            state.grant("phase_setup_step");
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

fn lint_report_is_runtime_repairable_verifier_command(report: &PlanLintReport) -> bool {
    !report.errors.is_empty()
        && report.errors.iter().all(|error| {
            error.category == "verify_policy"
                && (error
                    .message
                    .contains("grep pattern begins with '-' but command lacks `--` or `-e`")
                    || error.message.contains(
                        "grep package.json script assertion replaced with JSON parser check",
                    ))
        })
}

#[derive(Debug, Clone, Default)]
struct StepPromptContext {
    overall_goal: String,
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
    mut run_setup_authority: Option<&mut UltraRunSetupAuthorityState>,
) -> Result<StepRunOutcome, StepRunError> {
    let mut runtime_step = step.clone();
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
    let run_authority = run_setup_authority
        .as_deref()
        .map(UltraRunSetupAuthorityState::authority)
        .unwrap_or(NodeDependencySetupAuthority::None);
    let setup_authority = step_verify_setup_authority(plan, step, run_authority);
    let contract_setup_authority =
        step_contract_setup_authority(plan, step, phase_scope, run_authority);
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
    let overall_goal = prompt_context.overall_goal.as_str();
    match profile_post_step_repair(&config.workspace_root, &config.profile, overall_goal) {
        Ok(true) => {
            if let Some(state) = run_setup_authority.as_deref_mut() {
                state.grant("manifest_repair");
                if let Err(err) = reconcile_run_dependency_setup(
                    config,
                    &config.profile,
                    DependencyReconciliationTrigger::ManifestRepair,
                    state,
                ) {
                    let message = err.to_string();
                    outcome.primary_failure = Some(message.clone());
                    outcome.stop_reason =
                        Some("dependency_setup_reconciliation_failed".to_string());
                    outcome.partial = true;
                    return Err(StepRunError { message, outcome });
                }
            }
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
    let (mut report, mut build_lifecycles) =
        verify_step_with_profile_setup_observed_with_offline_and_events(
            &config.workspace_root,
            &runtime_step,
            Some(&config.profile),
            setup_authority,
            config.offline,
            config.eval_events_path.as_deref(),
        );
    apply_runtime_command_normalizations(&mut runtime_step, &report);
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
        if production_build_lifecycle_passed(&build_lifecycles) {
            snapshot_last_known_good_sources(
                config,
                mode,
                Some(&step.id),
                &config.profile,
                overall_goal,
                &step.expected_paths,
            );
        }
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
        overall_goal: Some(overall_goal.to_string()),
        required_final_artifacts: prompt_context.required_final_artifacts.clone(),
        step_instruction: Some(step.instruction.clone()),
        expected_paths: step.expected_paths.clone(),
        verify_commands: runtime_step.verify.clone(),
        expected_result: Some(step_expected_result(step).to_string()),
        max_repair_turns: Some(STEP_REPAIR_MAX_TURNS),
        missing_paths: report.missing_paths.clone(),
        changed_files: initial.changed_paths.clone(),
        initial_stop_reason: Some(format!("{:?}", initial.stop_reason)),
        workspace_root: Some(config.workspace_root.clone()),
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
            context.compile_reanchored_retry =
                no_change_repairs > 0 && !current_report.compile_errors.is_empty();
            context.compile_narrow_no_snapshot_retry = false;
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
            match profile_post_step_repair(&config.workspace_root, &config.profile, overall_goal) {
                Ok(true) => {
                    if let Some(state) = run_setup_authority.as_deref_mut() {
                        state.grant("manifest_repair");
                        if let Err(err) = reconcile_run_dependency_setup(
                            config,
                            &config.profile,
                            DependencyReconciliationTrigger::ManifestRepair,
                            state,
                        ) {
                            let message = err.to_string();
                            outcome.primary_failure = Some(message.clone());
                            outcome.stop_reason =
                                Some("dependency_setup_reconciliation_failed".to_string());
                            outcome.partial = true;
                            return Err(StepRunError { message, outcome });
                        }
                    }
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
            let (retry, retry_lifecycles) =
                verify_step_with_profile_setup_observed_with_offline_and_events(
                    &config.workspace_root,
                    &runtime_step,
                    Some(&config.profile),
                    setup_authority,
                    config.offline,
                    config.eval_events_path.as_deref(),
                );
            apply_runtime_command_normalizations(&mut runtime_step, &retry);
            context.verify_commands = runtime_step.verify.clone();
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
                if production_build_lifecycle_passed(&retry_lifecycles) {
                    snapshot_last_known_good_sources(
                        config,
                        mode,
                        Some(&step.id),
                        &config.profile,
                        &plan.goal,
                        &step.expected_paths,
                    );
                }
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
    if !current_report.compile_errors.is_empty() {
        match try_compile_rollback_after_repair_exhaustion(
            config,
            &config.profile,
            overall_goal,
            phase_scope.unwrap_or(&step.id),
            &step.instruction,
            &current_report,
            &final_failure_kind,
        ) {
            Ok(Some(rollback)) => {
                outcome.primary_failure = None;
                outcome.stop_reason = Some("compile_rollback_applied".to_string());
                outcome.partial = true;
                outcome.compile_rollbacks.push(rollback);
                return Ok(outcome);
            }
            Ok(None) => {}
            Err(err) => {
                let message = err.to_string();
                outcome.primary_failure = Some(message.clone());
                outcome.stop_reason = Some("compile_rollback_error".to_string());
                outcome.partial = true;
                return Err(StepRunError { message, outcome });
            }
        }
    }
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
            .chain(
                current_report
                    .verifier_command_false_negatives
                    .iter()
                    .map(|failure| {
                        format!(
                            "deterministic_verify_command_bug: {}: {}",
                            failure.command, failure.reason
                        )
                    }),
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
    let prompt_message = if validation.prompt_command_available() {
        format!("suggested command: {suggested_command}")
    } else {
        "suggested command unavailable because recovery prompt validation failed".to_string()
    };
    let yaml_message = suggested_yaml_command
        .as_ref()
        .map(|command| format!("suggested YAML command: {command}"))
        .unwrap_or_else(|| {
            "suggested YAML command unavailable because recovery YAML is missing".to_string()
        });
    let message = eval_events::render_stop_reason(&eval_events::StopReasonParts {
        free_text: format!(
            "step {} failed verification after bounded repair: {}; failure_kind={}; incomplete; {}",
            step.id,
            current_report.primary_reason(),
            final_failure_kind,
            artifact_check_summary
        ),
        paths: vec![
            format!("repair prompt saved: {repair_report_display}"),
            yaml_summary,
        ],
        commands: vec![prompt_message, yaml_message],
    });
    outcome.primary_failure = Some(current_report.primary_reason());
    outcome.stop_reason = Some(final_failure_kind.to_string());
    outcome.partial = true;
    Err(StepRunError { message, outcome })
}

fn step_verify_setup_authority(
    plan: &StepPlan,
    step: &PlanStep,
    fallback: NodeDependencySetupAuthority,
) -> NodeDependencySetupAuthority {
    if step.step_kind() == StepKind::Setup {
        return NodeDependencySetupAuthority::PlanSetupStep;
    }
    if step.step_kind() != StepKind::Verify {
        return fallback;
    }
    let prior_setup_exists = plan
        .steps
        .iter()
        .take_while(|candidate| candidate.id != step.id)
        .any(|candidate| candidate.step_kind() == StepKind::Setup);
    if prior_setup_exists {
        NodeDependencySetupAuthority::PlanSetupStep
    } else {
        fallback
    }
}

fn step_contract_setup_authority(
    _plan: &StepPlan,
    step: &PlanStep,
    phase_scope: Option<&str>,
    fallback: NodeDependencySetupAuthority,
) -> NodeDependencySetupAuthority {
    if step.step_kind() != StepKind::Implement {
        return NodeDependencySetupAuthority::None;
    }
    if step_or_phase_is_dependency_setup_purpose(step, phase_scope) {
        NodeDependencySetupAuthority::PlanSetupStep
    } else {
        fallback
    }
}

fn step_carries_setup_authority(
    plan: &StepPlan,
    step: &PlanStep,
    phase_scope: Option<&str>,
) -> bool {
    step_verify_setup_authority(plan, step, NodeDependencySetupAuthority::None).allows_setup()
        || step_contract_setup_authority(
            plan,
            step,
            phase_scope,
            NodeDependencySetupAuthority::None,
        )
        .allows_setup()
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
    signals::contains_setup_token(text)
        && (lower.contains("depend")
            || lower.contains("workspace")
            || lower.contains("package")
            || lower.contains("npm")
            || text.contains("依存"))
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
    for failure in extra.verifier_command_false_negatives {
        report.push_verifier_command_false_negative(failure.command, failure.reason);
    }
    for normalization in extra.runtime_command_normalizations {
        report.runtime_command_normalizations.push(normalization);
    }
    for error in extra.compile_errors {
        if !report.compile_errors.contains(&error) {
            report.compile_errors.push(error);
        }
    }
    for reason in extra.profile_failures {
        report.push_profile_failure(reason);
    }
    report.refresh_status();
}

fn apply_runtime_command_normalizations(step: &mut PlanStep, report: &VerificationReport) {
    for normalization in &report.runtime_command_normalizations {
        for command in &mut step.verify {
            if command == &normalization.original {
                *command = normalization.repaired.clone();
            }
        }
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
        emit_completion_contract_bound(config, scope, profile, goal, &bound);
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
    emit_completion_contract_bound(config, scope, profile, goal, &bound);
    Ok(Some(bound))
}

fn completion_contract_required(
    profile: &str,
    goal: &str,
    required_capabilities: &[String],
) -> bool {
    if domain_profile(profile).completion_contract_required(goal, required_capabilities) {
        return true;
    }
    let profile = canonical_profile_name(profile);
    let web_or_app_profile = matches!(profile.as_str(), "vite" | "react" | "web");
    let interactive_goal = signals::contains_app_like_token(goal)
        || signals::contains_browser_probe_token(goal)
        || signals::contains_canvas_token(goal);
    let interactive_capability = required_capabilities.iter().any(|capability| {
        matches!(
            capability.as_str(),
            "stateful_interaction"
                | "start_or_restart_flow"
                | "player_control"
                | "adversary_or_challenge"
                | "progression_or_score"
                | "failure_or_collision_rule"
                | "persistence"
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

fn emit_completion_contract_bound(
    config: &Config,
    scope: &str,
    profile: &str,
    goal: &str,
    bound: &BoundCompletionContract,
) {
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
    if canonical_profile_name(profile) == "generic"
        && bound
            .contract
            .required_capabilities
            .iter()
            .any(|capability| capability == GENERIC_INTERACTIVE_CONTRACT_CAPABILITY)
        && let Some(matched_intent_token) = signals::matched_app_intent_token(goal)
    {
        eval_events::emit(
            config.eval_events_path.as_deref(),
            json!({
                "event": "generic_contract_bound",
                "session_scope": scope,
                "matched_intent_token": matched_intent_token,
                "inferred_keys": GENERIC_INTERACTIVE_EVIDENCE_KEYS,
                "required_capabilities": bound.contract.required_capabilities.clone(),
                "required_evidence": bound.contract.required_evidence.clone(),
                "required_obligations": bound.contract.required_obligations.clone(),
                "completion_contract_path": bound.path,
            }),
        );
    }
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
    let mut runtime_acceptance = runtime_acceptance_required.then(|| {
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
    let evidence_arbitration = runtime_acceptance.as_mut().map(|report| {
        final_acceptance_evidence_arbitration(
            config,
            report,
            &required_capabilities,
            &required_evidence,
            &required_obligations,
        )
    });
    let release_gate = final_acceptance_release_gate(
        config,
        &config.profile,
        &plan.goal,
        &required_capabilities,
        runtime_acceptance.as_ref(),
        false,
    );
    let contract_required =
        completion_contract_required(&config.profile, &plan.goal, &required_capabilities)
            || bound_contract.is_some_and(|bound| bound.required);
    let external_contract_checked = bound_contract.is_some();
    let contract_binding_missing = contract_required && !external_contract_checked;
    let external_ok = !contract_binding_missing
        && external_contract_ok_after_runtime_arbitration(
            external_report.as_ref(),
            runtime_acceptance.as_ref(),
        );
    let runtime_ok = runtime_acceptance
        .as_ref()
        .is_none_or(|report| report.passed);
    let release_gate_failed = release_gate.status == "failed";
    let ok =
        missing_final_artifacts.is_empty() && external_ok && runtime_ok && !release_gate_failed;
    let final_acceptance_status = release_gate_final_acceptance_status(&release_gate);
    let runtime_acceptance_status =
        runtime_acceptance_status(runtime_ok, runtime_acceptance.as_ref());
    let (assurance_level, assurance_reason) =
        assurance_for_completion(&config.profile, &required_capabilities);
    let release_quality_completion =
        release_quality_completion_status(&release_gate, final_acceptance_status);
    let next_action = release_gate_next_action(&release_gate, final_acceptance_status);
    let state_dimensions_changed =
        interaction_state_dimensions_changed_from_path(&release_gate.interaction_evidence_path);
    let action_hooks = interaction_action_hooks_from_path(&release_gate.interaction_evidence_path);
    let text_telemetry =
        interaction_text_telemetry_from_path(&release_gate.interaction_evidence_path);
    let requested_port = effective_requested_port(&config.profile, &plan.goal, None);
    let primary_reason = if !missing_final_artifacts.is_empty() {
        format!(
            "missing final artifacts: {}",
            missing_final_artifacts.join(", ")
        )
    } else if contract_binding_missing {
        "completion contract binding required but missing".to_string()
    } else if let Some(report) = runtime_acceptance.as_ref().filter(|report| !report.passed) {
        report.primary_reason.clone()
    } else if let Some(report) = external_report.as_ref().filter(|report| {
        !external_contract_ok_after_runtime_arbitration(Some(*report), runtime_acceptance.as_ref())
    }) {
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
            "profile": config.profile,
            "requested_port": requested_port.as_ref().map(|requested| requested.telemetry.clone()),
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
            "runtime_acceptance_diagnostics": runtime_acceptance
                .as_ref()
                .map(|report| report.diagnostics.clone())
                .unwrap_or_default(),
            "unverified_evidence": runtime_acceptance
                .as_ref()
                .map(|report| report.unverified_evidence.clone())
                .unwrap_or_default(),
            "evidence_tiers": runtime_acceptance
                .as_ref()
                .map(|report| report.evidence_tiers.clone())
                .unwrap_or_default(),
            "evidence_arbitration": evidence_arbitration
                .as_ref()
                .map(|report| report.records.clone())
                .unwrap_or_default(),
            "evidence_arbitration_summary": evidence_arbitration
                .as_ref()
                .map(|report| report.summary.clone())
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
            "assurance_level": assurance_level,
            "assurance_reason": assurance_reason,
            "release_quality_completion": release_quality_completion,
            "release_gate_status": release_gate.status.clone(),
            "release_gate_reasons": release_gate.reasons.clone(),
            "browser_readiness_status": release_gate.browser_readiness_status.clone(),
            "browser_readiness_evidence_path": release_gate.browser_readiness_evidence_path.clone(),
            "interaction_evidence_status": release_gate.interaction_evidence_status.clone(),
            "interaction_evidence_path": release_gate.interaction_evidence_path.clone(),
            "state_dimensions_changed": state_dimensions_changed,
            "action_hooks": action_hooks,
            "text_entry": text_telemetry.text_entry,
            "text_entry_target": text_telemetry.text_entry_target,
            "typed_token": text_telemetry.typed_token,
            "token_echoed": text_telemetry.token_echoed,
            "echo_latency_ms": text_telemetry.echo_latency_ms,
            "token_echoed_after_reload": text_telemetry.token_echoed_after_reload,
            "token_echo_after_reload_latency_ms": text_telemetry.token_echo_after_reload_latency_ms,
            "text_input_state_change": text_telemetry.text_input_state_change,
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

#[derive(Debug, Clone)]
struct ProfileInvariantFailureEvidence {
    report: VerificationReport,
    missing_paths: Vec<String>,
    failure_evidence: Vec<String>,
}

fn fresh_profile_invariant_failure_evidence(
    config: &Config,
    plan: &UltraPlan,
    snapshot: &ProfileSnapshot,
    required_paths: &[String],
) -> ProfileInvariantFailureEvidence {
    let report =
        verify_profile_invariant(&config.workspace_root, &plan.profile, &plan.goal, snapshot);
    let mut required = required_paths.to_vec();
    merge_unique_strings(
        &mut required,
        &profile_setup_scaffold_paths(&config.workspace_root, &plan.profile),
    );
    let mut missing_paths = missing_final_artifacts(&config.workspace_root, &required);
    merge_unique_strings(&mut missing_paths, &report.missing_paths);

    let missing_imports = profile_missing_relative_imports(&config.workspace_root, &plan.profile);
    let import_findings = format_missing_import_findings(&config.workspace_root, &missing_imports);
    merge_unique_strings(
        &mut missing_paths,
        &missing_import_target_paths(&config.workspace_root, &missing_imports),
    );
    if missing_paths.is_empty() && !missing_imports.is_empty() {
        merge_unique_strings(&mut missing_paths, &import_findings);
    }

    let mut failure_evidence = vec![report.primary_reason()];
    if !import_findings.is_empty() {
        failure_evidence.push(format!(
            "Missing relative imports:\n{}",
            render_prompt_bullets(&import_findings)
        ));
    }

    ProfileInvariantFailureEvidence {
        report,
        missing_paths,
        failure_evidence,
    }
}

fn profile_missing_relative_imports(root: &Path, profile: &str) -> Vec<MissingImport> {
    let paths = domain_profile(profile).source_paths(root);
    if paths.is_empty() {
        return Vec::new();
    }
    scan_relative_imports(root, &paths).unwrap_or_default()
}

fn missing_import_target_paths(root: &Path, missing: &[MissingImport]) -> Vec<String> {
    missing
        .iter()
        .filter_map(|missing| missing_import_target_rel(root, missing))
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
    merge_unique_strings(&mut out, &report.diagnostics);
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

fn resolved_missing_signals(before: &[String], after: &[String]) -> Vec<String> {
    let after = after.iter().collect::<BTreeSet<_>>();
    before
        .iter()
        .filter(|key| !after.contains(*key))
        .cloned()
        .collect()
}

fn emit_final_acceptance_cycle_delta(
    config: &Config,
    delta: &FinalAcceptanceCycleDelta,
    passed: bool,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "final_acceptance_cycle_complete",
            "cycle_index": delta.cycle_index,
            "ok": passed,
            "resolved_keys": delta.resolved_keys.clone(),
            "remaining_keys": delta.remaining_keys.clone(),
            "changed_paths": delta.changed_paths.clone(),
            "route_bound_changed_paths": delta.route_bound_changed_paths.clone(),
            "route_bound_source_changed": !delta.route_bound_changed_paths.is_empty(),
        }),
    );
}

fn append_final_acceptance_cycle_summary(config: &Config, deltas: &[FinalAcceptanceCycleDelta]) {
    if deltas.is_empty() {
        return;
    }
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "final_acceptance_cycle_summary",
            "cycles": deltas
                .iter()
                .map(|delta| {
                    json!({
                        "cycle_index": delta.cycle_index,
                        "resolved_keys": delta.resolved_keys.clone(),
                        "remaining_keys": delta.remaining_keys.clone(),
                        "changed_paths": delta.changed_paths.clone(),
                        "route_bound_changed_paths": delta.route_bound_changed_paths.clone(),
                    })
                })
                .collect::<Vec<_>>(),
        }),
    );
    let mut lines = vec!["Final acceptance repair cycles:".to_string()];
    for delta in deltas {
        lines.push(format!(
            "- cycle {}: resolved={} remaining={} changed={} route_bound_changed={}",
            delta.cycle_index,
            missing_if_empty(&delta.resolved_keys.join(", ")),
            missing_if_empty(&delta.remaining_keys.join(", ")),
            missing_if_empty(&delta.changed_paths.join(", ")),
            missing_if_empty(&delta.route_bound_changed_paths.join(", "))
        ));
    }
    eval_events::append_run_summary(config.eval_events_path.as_deref(), &lines.join("\n"));
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
    let mut summaries = report
        .command_failures
        .iter()
        .map(|failure| {
            format!(
                "{}: {}",
                failure.command,
                eval_events::body_snippet(&failure.reason)
            )
        })
        .collect::<Vec<_>>();
    summaries.extend(
        report
            .verifier_command_false_negatives
            .iter()
            .map(|failure| {
                format!(
                    "deterministic_verify_command_bug: {}: {}",
                    failure.command,
                    eval_events::body_snippet(&failure.reason)
                )
            }),
    );
    summaries
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
            "deterministic_verify_command_bug" => {
                "deterministic_verify_command_bug: the verify command is malformed; the artifact may already satisfy the requirement".to_string()
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
            .verifier_command_false_negatives
            .iter()
            .map(|failure| {
                format!(
                    "verifier_command:{}:{}",
                    failure.command,
                    normalize_report_reason_for_signature(&failure.reason)
                )
            }),
    );
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

fn render_bounded_prompt_section(
    header: &str,
    items: &[String],
    footer: Option<&str>,
    max_lines: usize,
) -> String {
    let footer_lines = usize::from(footer.is_some());
    let available_item_lines = max_lines.saturating_sub(1 + footer_lines).max(1);
    let mut lines = vec![header.to_string()];
    if items.len() > available_item_lines {
        let shown = available_item_lines.saturating_sub(1);
        for item in items.iter().take(shown) {
            lines.push(format!("- {item}"));
        }
        lines.push(format!(
            "- … and {} more",
            items.len().saturating_sub(shown)
        ));
    } else {
        for item in items {
            lines.push(format!("- {item}"));
        }
    }
    if let Some(footer) = footer {
        lines.push(format!("- {footer}"));
    }
    lines.join("\n")
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
        let reply = planner_chat_with_request_retry(
            client,
            config,
            ProviderCallScope::PlannerUltra,
            model,
            &messages,
            ui,
        )?;
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
                    crate::tui::presentation::emit_ultra_plan_card(
                        &plan,
                        &crate::tui::presentation::PlanProgress::default(),
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
    let mut active_plan = plan.clone();
    let mut active_config = config.clone();
    let plan = &mut active_plan;
    let config = &mut active_config;
    let report = lint_ultra_plan_report(plan);
    if !report.is_pass() {
        emit_planner_error_for_lint(config, "ultra-plan-file", &config.planner_model, &report, 0);
        anyhow::bail!("{}", report.primary_message());
    }
    let mut final_expected_paths =
        profile_expected_paths(&config.workspace_root, &plan.profile, &plan.goal);
    let mut ultra_context = UltraRunContext::new(missing_final_artifacts(
        &config.workspace_root,
        &final_expected_paths,
    ));
    let mut ultra_session = SessionSnapshot::new();
    let mut promotion_state = ProfilePromotionState::for_run(plan, config);
    let mut setup_authority_state = UltraRunSetupAuthorityState::default();
    emit_ultra_context_initialized(config, plan, &ultra_context, ultra_session.messages.len());
    let phases = plan.phases.clone();
    for (index, phase) in phases.iter().enumerate() {
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
        let step_plan = generate_step_plan_with_ui_for_phase(
            planner,
            &phase_prompt,
            config,
            ui,
            Some(&phase.id),
        )
        .map_err(|err| {
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
            );
            anyhow::anyhow!(
                "{}",
                render_failure_stop_reason(format!("phase scaffold failed: {message}"), handoff,)
            )
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
        let step_outcome = match run_step_plan_with_session_with_ui_and_run_authority(
            execution,
            &mut ultra_session,
            &step_plan,
            config,
            ui,
            false,
            "ultra-plan-run",
            Some(&phase.id),
            Some(&plan.goal),
            Some(&mut setup_authority_state),
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
                );
                return Err(anyhow::anyhow!(
                    "{}",
                    render_failure_stop_reason(
                        format!("phase {} failed: {message}", phase.id),
                        handoff
                    )
                ));
            }
        };
        ultra_context.update_after_phase(
            phase,
            &step_outcome,
            missing_final_artifacts(&config.workspace_root, &final_expected_paths),
        );
        for rollback in &step_outcome.compile_rollbacks {
            emit_compile_rollback_context_carried(config, rollback);
        }
        let acceptance = ultra_contract_runtime_acceptance_report(plan, config)?;
        ultra_context.refresh_pending_capability_evidence(&acceptance);
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
                &mut setup_authority_state,
            )?;
        }
        if !final_phase
            && !invariant_report.is_pass()
            && !invariant_report.compile_errors.is_empty()
            && let Some(rollback) = try_compile_rollback_after_repair_exhaustion(
                config,
                &plan.profile,
                &plan.goal,
                &phase.id,
                &phase.prompt,
                &invariant_report,
                "profile_invariant_repair_exhausted",
            )?
        {
            push_context_items_capped(
                &mut ultra_context.carry_forward_guidance,
                &rollback.carry_forward_guidance,
                ULTRA_CONTEXT_MAX_MESSAGES,
                &mut ultra_context.truncated,
            );
            emit_compile_rollback_context_carried(config, &rollback);
            invariant_report = verify_profile_invariant(
                &config.workspace_root,
                &plan.profile,
                &plan.goal,
                &profile_snapshot,
            );
        }
        if !invariant_report.is_pass() {
            let fresh_evidence = fresh_profile_invariant_failure_evidence(
                config,
                plan,
                &profile_snapshot,
                &final_expected_paths,
            );
            invariant_report = fresh_evidence.report.clone();
            let invariant_reason = invariant_report.primary_reason();
            let missing_paths = fresh_evidence.missing_paths.clone();
            let failure_evidence = fresh_evidence.failure_evidence.clone();
            if !final_phase {
                ultra_context.update_after_profile_failure(
                    phase,
                    &invariant_reason,
                    missing_paths.clone(),
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
            }
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
                Some(&invariant_reason),
                None,
            );
            emit_phase_verification_event(
                config,
                plan,
                phase,
                index,
                PhaseVerificationMode::IntermediateInvariant,
                false,
                Some(&invariant_reason),
            );
            if !final_phase {
                let handoff = save_ultra_phase_recovery_handoff_with_evidence(
                    config,
                    plan,
                    phase,
                    UltraPhaseRecoveryRequest {
                        failure_kind: "profile_invariant_failure",
                        reason: &invariant_reason,
                        missing_paths: &missing_paths,
                        missing_signals: &verification_missing_signals(&invariant_report),
                        repair_targets: &["profile_contract".to_string()],
                    },
                    &failure_evidence,
                );
                return Err(anyhow::anyhow!(
                    "{}",
                    render_failure_stop_reason(
                        format!(
                            "phase {} profile invariant verification failed: {}",
                            phase.id, invariant_reason
                        ),
                        handoff,
                    )
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
            if try_promote_profile_at_phase_boundary(
                config,
                plan,
                &mut ultra_context,
                &mut final_expected_paths,
                &mut promotion_state,
                phase,
                index,
            )?
            .is_some()
            {
                setup_authority_state.grant("profile_promotion");
                reconcile_run_dependency_setup(
                    config,
                    &plan.profile,
                    DependencyReconciliationTrigger::Promotion,
                    &setup_authority_state,
                )?;
            }
            continue;
        }
        let final_invariant_reason =
            (!invariant_report.is_pass()).then(|| invariant_report.primary_reason());
        emit_phase_verification_event(
            config,
            plan,
            phase,
            index,
            PhaseVerificationMode::FinalAcceptance,
            invariant_report.is_pass(),
            final_invariant_reason.as_deref(),
        );
        emit_ultra_phase_event(
            config,
            "ultra_phase_profile_check",
            plan,
            phase,
            index,
            "profile_observed",
            Some(invariant_report.is_pass()),
            final_invariant_reason.as_deref(),
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
        if try_promote_profile_at_phase_boundary(
            config,
            plan,
            &mut ultra_context,
            &mut final_expected_paths,
            &mut promotion_state,
            phase,
            index,
        )?
        .is_some()
        {
            setup_authority_state.grant("profile_promotion");
            reconcile_run_dependency_setup(
                config,
                &plan.profile,
                DependencyReconciliationTrigger::Promotion,
                &setup_authority_state,
            )?;
        }
    }
    let mut final_acceptance_cycle_deltas = Vec::new();
    let mut acceptance_report = ultra_final_acceptance_report_with_cycle(plan, config, 0)?;
    if !acceptance_report.is_pass() {
        let initial_reason = acceptance_report.primary_reason();
        let initial_target = classify_repair_target(&acceptance_report);
        eval_events::emit(
            config.eval_events_path.as_deref(),
            json!({
                "event": "ultra_final_acceptance_failed",
                "cycle_index": 0,
                "lifecycle_stage": "final_acceptance",
                "primary_reason": eval_events::body_snippet(&initial_reason),
                "repair_target": initial_target.as_str(),
                "missing_paths": acceptance_report.missing_paths.clone(),
                "compile_errors": acceptance_report.compile_errors.clone(),
                "profile_failures": acceptance_report.profile_failures.clone(),
                "bounded_repair_available": true,
                "max_attempts": FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS,
            }),
        );
        let fallback_phase = plan.phases.last().cloned().unwrap_or_else(|| UltraPhase {
            id: "final".to_string(),
            prompt: "Final acceptance".to_string(),
        });
        let repair_config = capped_config(config, STEP_REPAIR_MAX_ITERATIONS);
        let repair_started = Instant::now();
        let mut attempts_run = 0;
        let mut exhausted_reason = "bounded_repair_exhausted".to_string();
        let mut compile_no_source_change_count = 0usize;
        let mut compile_no_snapshot_narrow_retry_used = false;
        let mut max_repair_attempts = FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS;
        for attempt in 1..=FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS
            + FINAL_ACCEPTANCE_COMPILE_NO_SNAPSHOT_EXTRA_ATTEMPTS
        {
            if attempt > max_repair_attempts {
                break;
            }
            if repair_started.elapsed() > FINAL_ACCEPTANCE_REPAIR_WALL_CLOCK_CAP {
                exhausted_reason = "bounded_repair_wall_clock_cap".to_string();
                break;
            }
            attempts_run = attempt;
            let repair_target = classify_repair_target(&acceptance_report);
            let compile_reanchored_retry =
                compile_no_source_change_count > 0 && !acceptance_report.compile_errors.is_empty();
            let compile_narrow_no_snapshot_retry = compile_no_snapshot_narrow_retry_used
                && compile_no_source_change_count >= FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS
                && !acceptance_report.compile_errors.is_empty();
            let expected_paths =
                final_acceptance_repair_expected_paths(plan, config, &acceptance_report)?;
            let before_missing_keys = verification_missing_signals(&acceptance_report);
            let before_route_bound_paths =
                route_bound_source_paths(&config.workspace_root, &plan.profile);
            let before_source_snapshot = final_acceptance_source_snapshot(
                &config.workspace_root,
                &plan.profile,
                &plan.goal,
                &expected_paths,
                &[],
            );
            let repair_prompt = final_acceptance_repair_prompt(
                &config.workspace_root,
                plan,
                &acceptance_report,
                &ultra_context,
                repair_target.as_str(),
                &expected_paths,
                &plan_adherence_report(plan, &config.workspace_root).missing,
                (attempt, FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS),
                compile_reanchored_retry,
                compile_narrow_no_snapshot_retry,
            );
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "final_acceptance_repair_start",
                    "cycle_index": attempt,
                    "lifecycle_stage": "final_acceptance_repair",
                    "attempt": attempt,
                    "max_attempts": FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS,
                    "repair_target": repair_target.as_str(),
                    "missing_paths": acceptance_report.missing_paths.clone(),
                    "compile_errors": acceptance_report.compile_errors.clone(),
                    "profile_failures": acceptance_report.profile_failures.clone(),
                    "selected_evidence_keys": before_missing_keys.clone(),
                    "selected_interaction_failure": final_acceptance_app_behavior_failure_kind(&acceptance_report)
                        .unwrap_or_default(),
                    "bounded_repair": true,
                    "max_iterations": repair_config.max_iterations,
                    "shared_execution_session": true,
                    "compile_reanchored_retry": compile_reanchored_retry,
                    "compile_narrow_no_snapshot_retry": compile_narrow_no_snapshot_retry,
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
                    if !acceptance_report.compile_errors.is_empty()
                        && err_text.contains("missing tool call for action prompt")
                    {
                        compile_no_source_change_count += 1;
                        exhausted_reason = "compile_repair_no_source_change".to_string();
                        eval_events::emit(
                            config.eval_events_path.as_deref(),
                            json!({
                                "event": "final_acceptance_repair_no_source_change",
                                "cycle_index": attempt,
                                "lifecycle_stage": "final_acceptance_repair",
                                "attempt": attempt,
                                "max_attempts": FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS,
                                "failure_kind": "compile_repair_no_source_change",
                                "repair_target": repair_target.as_str(),
                                "compile_errors": acceptance_report.compile_errors.clone(),
                                "repair_error": eval_events::body_snippet(&err_text),
                                "reanchored_retry": compile_no_source_change_count == 1,
                                "narrow_no_snapshot_retry": compile_narrow_no_snapshot_retry,
                                "proceed_to_rollback": compile_no_source_change_count >= 2,
                            }),
                        );
                        if compile_no_source_change_count >= 2 {
                            if should_run_compile_no_snapshot_narrow_retry(
                                config,
                                &acceptance_report,
                                compile_no_snapshot_narrow_retry_used,
                            ) {
                                compile_no_snapshot_narrow_retry_used = true;
                                max_repair_attempts = FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS
                                    + FINAL_ACCEPTANCE_COMPILE_NO_SNAPSHOT_EXTRA_ATTEMPTS;
                                exhausted_reason =
                                    "compile_repair_no_snapshot_narrow_retry".to_string();
                                emit_compile_no_snapshot_narrow_retry(
                                    config,
                                    attempt,
                                    &acceptance_report,
                                    "repair_error",
                                );
                                continue;
                            }
                            break;
                        }
                        continue;
                    }
                    eval_events::emit(
                        config.eval_events_path.as_deref(),
                        json!({
                            "event": "final_acceptance_repair_failed",
                            "cycle_index": attempt,
                            "lifecycle_stage": "final_acceptance_repair",
                            "attempt": attempt,
                            "max_attempts": FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS,
                            "repair_target": repair_target.as_str(),
                            "reason": eval_events::body_snippet(&err_text),
                            "bounded_repair_exhausted": true,
                        }),
                    );
                    let repair_targets =
                        final_acceptance_recovery_repair_targets(&acceptance_report, repair_target);
                    let missing_signals = verification_missing_signals(&acceptance_report);
                    let failure_evidence =
                        final_acceptance_recovery_failure_evidence(&acceptance_report, &err_text);
                    let handoff = save_ultra_phase_recovery_handoff_with_evidence(
                        config,
                        plan,
                        &fallback_phase,
                        UltraPhaseRecoveryRequest {
                            failure_kind: "final_acceptance_repair_failed",
                            reason: &err_text,
                            missing_paths: &acceptance_report.missing_paths,
                            missing_signals: &missing_signals,
                            repair_targets: &repair_targets,
                        },
                        &failure_evidence,
                    );
                    anyhow::bail!(
                        "{}",
                        render_failure_stop_reason(
                            format!("ultra final acceptance repair failed: {err_text}"),
                            handoff,
                        )
                    );
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
            let after_route_bound_paths =
                route_bound_source_paths(&config.workspace_root, &plan.profile);
            let after_source_snapshot = final_acceptance_source_snapshot(
                &config.workspace_root,
                &plan.profile,
                &plan.goal,
                &expected_paths,
                &repair_outcome.changed_paths,
            );
            let actual_changed_paths =
                changed_snapshot_paths(&before_source_snapshot, &after_source_snapshot);
            let route_bound_changed_paths = route_bound_changed_paths(
                &before_source_snapshot,
                &after_source_snapshot,
                &before_route_bound_paths,
                &after_route_bound_paths,
            );
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "final_acceptance_repair_complete",
                    "cycle_index": attempt,
                    "lifecycle_stage": "final_acceptance_repair",
                    "attempt": attempt,
                    "max_attempts": FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS,
                    "repair_target": repair_target.as_str(),
                    "changed_path_count": actual_changed_paths.len(),
                    "reported_changed_path_count": repair_outcome.changed_paths.len(),
                    "changed_paths": actual_changed_paths.clone(),
                    "reported_changed_paths": repair_outcome.changed_paths.clone(),
                    "route_bound_changed_paths": route_bound_changed_paths.clone(),
                    "route_bound_source_changed": !route_bound_changed_paths.is_empty(),
                    "iterations": repair_outcome.iterations,
                    "tool_calls": repair_outcome.tool_calls,
                    "shared_execution_session": true,
                    "session_message_count": ultra_session.messages.len(),
                }),
            );
            if !acceptance_report.compile_errors.is_empty() && actual_changed_paths.is_empty() {
                compile_no_source_change_count += 1;
                exhausted_reason = "compile_repair_no_source_change".to_string();
                eval_events::emit(
                    config.eval_events_path.as_deref(),
                    json!({
                        "event": "final_acceptance_repair_no_source_change",
                        "cycle_index": attempt,
                        "lifecycle_stage": "final_acceptance_repair",
                        "attempt": attempt,
                        "max_attempts": FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS,
                        "failure_kind": "compile_repair_no_source_change",
                        "repair_target": repair_target.as_str(),
                        "compile_errors": acceptance_report.compile_errors.clone(),
                        "reanchored_retry": compile_no_source_change_count == 1,
                        "narrow_no_snapshot_retry": compile_narrow_no_snapshot_retry,
                        "proceed_to_rollback": compile_no_source_change_count >= 2,
                    }),
                );
                if compile_no_source_change_count >= 2 {
                    if should_run_compile_no_snapshot_narrow_retry(
                        config,
                        &acceptance_report,
                        compile_no_snapshot_narrow_retry_used,
                    ) {
                        compile_no_snapshot_narrow_retry_used = true;
                        max_repair_attempts = FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS
                            + FINAL_ACCEPTANCE_COMPILE_NO_SNAPSHOT_EXTRA_ATTEMPTS;
                        exhausted_reason = "compile_repair_no_snapshot_narrow_retry".to_string();
                        emit_compile_no_snapshot_narrow_retry(
                            config,
                            attempt,
                            &acceptance_report,
                            "no_source_change",
                        );
                        continue;
                    }
                    break;
                }
                continue;
            }
            compile_no_source_change_count = 0;
            if actual_changed_paths.is_empty() {
                exhausted_reason = "final_acceptance_repair_no_source_change".to_string();
                eval_events::emit(
                    config.eval_events_path.as_deref(),
                    json!({
                        "event": "final_acceptance_repair_no_source_change",
                        "cycle_index": attempt,
                        "lifecycle_stage": "final_acceptance_repair",
                        "attempt": attempt,
                        "max_attempts": FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS,
                        "failure_kind": "final_acceptance_repair_no_source_change",
                        "repair_target": repair_target.as_str(),
                        "reported_changed_paths": repair_outcome.changed_paths.clone(),
                        "route_bound_changed_paths": route_bound_changed_paths.clone(),
                        "reprobe_skipped": true,
                    }),
                );
                break;
            }
            clear_final_acceptance_browser_probe_evidence(config);
            acceptance_report = ultra_final_acceptance_report_with_cycle(plan, config, attempt)?;
            let remaining_keys = verification_missing_signals(&acceptance_report);
            let delta = FinalAcceptanceCycleDelta {
                cycle_index: attempt,
                resolved_keys: resolved_missing_signals(&before_missing_keys, &remaining_keys),
                remaining_keys,
                changed_paths: actual_changed_paths,
                route_bound_changed_paths,
            };
            emit_final_acceptance_cycle_delta(config, &delta, acceptance_report.is_pass());
            final_acceptance_cycle_deltas.push(delta);
            if acceptance_report.is_pass() {
                break;
            }
        }
        if !acceptance_report.is_pass()
            && !acceptance_report.compile_errors.is_empty()
            && let Some(rollback) = try_compile_rollback_after_repair_exhaustion(
                config,
                &plan.profile,
                &plan.goal,
                &fallback_phase.id,
                &fallback_phase.prompt,
                &acceptance_report,
                &exhausted_reason,
            )?
        {
            push_context_items_capped(
                &mut ultra_context.carry_forward_guidance,
                &rollback.carry_forward_guidance,
                ULTRA_CONTEXT_MAX_MESSAGES,
                &mut ultra_context.truncated,
            );
            emit_compile_rollback_context_carried(config, &rollback);
            exhausted_reason = "compile_rollback_applied".to_string();
            acceptance_report = ultra_final_acceptance_report_with_cycle(
                plan,
                config,
                final_acceptance_cycle_deltas
                    .last()
                    .map(|delta| delta.cycle_index)
                    .unwrap_or(0),
            )?;
        }
        if !acceptance_report.is_pass() {
            let target = classify_repair_target(&acceptance_report);
            let behavior_failure = final_acceptance_app_behavior_failure_kind(&acceptance_report);
            let report_reason = acceptance_report.primary_reason();
            let reason = behavior_failure
                .clone()
                .unwrap_or_else(|| report_reason.clone());
            let failure_kind = if !acceptance_report.compile_errors.is_empty() {
                "implementation_compile_error".to_string()
            } else {
                behavior_failure
                    .clone()
                    .unwrap_or_else(|| "final_acceptance_repair_exhausted".to_string())
            };
            let handoff_reason =
                final_acceptance_recovery_reason(&acceptance_report, &reason, &exhausted_reason);
            let repair_targets =
                final_acceptance_recovery_repair_targets(&acceptance_report, target);
            let missing_signals = verification_missing_signals(&acceptance_report);
            let failure_evidence =
                final_acceptance_recovery_failure_evidence(&acceptance_report, &handoff_reason);
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "final_acceptance_repair_exhausted",
                    "cycle_index": attempts_run,
                    "lifecycle_stage": "final_acceptance_repair",
                    "attempt": attempts_run,
                    "max_attempts": FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS,
                    "repair_target": target.as_str(),
                    "primary_reason": eval_events::body_snippet(&reason),
                    "failure_kind": failure_kind.clone(),
                    "missing_paths": acceptance_report.missing_paths.clone(),
                    "compile_errors": acceptance_report.compile_errors.clone(),
                    "profile_failures": acceptance_report.profile_failures.clone(),
                    "bounded_repair_exhausted": true,
                    "exhausted_reason": exhausted_reason.clone(),
                }),
            );
            append_final_acceptance_cycle_summary(config, &final_acceptance_cycle_deltas);
            let handoff = save_ultra_phase_recovery_handoff_with_evidence(
                config,
                plan,
                &fallback_phase,
                UltraPhaseRecoveryRequest {
                    failure_kind: &failure_kind,
                    reason: &handoff_reason,
                    missing_paths: &acceptance_report.missing_paths,
                    missing_signals: &missing_signals,
                    repair_targets: &repair_targets,
                },
                &failure_evidence,
            );
            anyhow::bail!(
                "{}",
                render_failure_stop_reason(
                    format!("ultra final acceptance failed after bounded repair: {reason}"),
                    handoff,
                )
            );
        }
    }
    append_final_acceptance_cycle_summary(config, &final_acceptance_cycle_deltas);
    let terminal_capabilities = inferred_required_capabilities(&plan.profile, &plan.goal);
    let (assurance_level, assurance_reason) =
        assurance_for_completion(&plan.profile, &terminal_capabilities);
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "ultra_plan_complete",
            "total_phases": plan.phases.len(),
            "profile": plan.profile,
            "assurance_level": assurance_level,
            "assurance_reason": assurance_reason,
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
    setup_authority_state: &mut UltraRunSetupAuthorityState,
) -> anyhow::Result<VerificationReport> {
    let mut retry = failed_report.clone();
    let deterministic_error = match profile_auto_repair(
        &config.workspace_root,
        &plan.profile,
        &plan.goal,
        &failed_report,
    ) {
        Ok(changed) => {
            if changed {
                setup_authority_state.grant("manifest_repair");
                reconcile_run_dependency_setup(
                    config,
                    &plan.profile,
                    DependencyReconciliationTrigger::ManifestRepair,
                    setup_authority_state,
                )?;
            }
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
    let fresh_evidence = fresh_profile_invariant_failure_evidence(
        config,
        plan,
        profile_snapshot,
        final_expected_paths,
    );
    retry = fresh_evidence.report.clone();
    if retry.is_pass() {
        return Ok(confirm_phase_build_after_profile_repair(
            config, plan, phase, index, step_plan, retry,
        ));
    }
    ultra_context.update_after_profile_failure(
        phase,
        &retry.primary_reason(),
        fresh_evidence.missing_paths,
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
    let (build_report, build_lifecycles) = verify_step_with_profile_setup_observed_with_offline(
        &config.workspace_root,
        &build_step,
        Some(&plan.profile),
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
        let expected_paths = step_plan
            .steps
            .iter()
            .flat_map(|step| step.expected_paths.iter().cloned())
            .collect::<Vec<_>>();
        snapshot_last_known_good_sources(
            config,
            "profile_invariant_repair",
            Some(&phase.id),
            &plan.profile,
            &plan.goal,
            &expected_paths,
        );
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

fn production_build_lifecycle_passed(lifecycles: &[BuildVerifierLifecycleObservation]) -> bool {
    lifecycles
        .iter()
        .any(|lifecycle| lifecycle.final_status == BuildVerifierStatus::Passed)
}

fn snapshot_last_known_good_sources(
    config: &Config,
    origin_kind: &str,
    origin_id: Option<&str>,
    profile: &str,
    goal: &str,
    extra_paths: &[String],
) {
    let Some(latest_dir) = snapshot_latest_dir(config) else {
        return;
    };
    let paths = snapshot_source_candidates(&config.workspace_root, profile, goal, extra_paths);
    let mut saved_paths = Vec::new();
    let mut snapshot_origins = Vec::new();
    let mut failures = Vec::new();
    for rel in paths {
        let Some(source) = readable_workspace_source_path(&config.workspace_root, &rel) else {
            continue;
        };
        let destination = latest_dir.join(&rel);
        if let Some(parent) = destination.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            failures.push(format!("{rel}: create snapshot dir failed: {err}"));
            continue;
        }
        match std::fs::copy(&source, &destination) {
            Ok(_) => {
                saved_paths.push(rel.clone());
                snapshot_origins.push(workspace_relative_handoff_path(&destination));
            }
            Err(err) => failures.push(format!("{rel}: snapshot copy failed: {err}")),
        }
    }
    if !saved_paths.is_empty() || !failures.is_empty() {
        eval_events::emit(
            config.eval_events_path.as_deref(),
            json!({
                "event": "compile_snapshot_saved",
                "origin_kind": origin_kind,
                "origin_id": origin_id.unwrap_or_default(),
                "snapshot_paths": saved_paths,
                "snapshot_origins": snapshot_origins,
                "snapshot_failures": failures,
            }),
        );
    }
}

fn snapshot_source_candidates(
    root: &Path,
    profile: &str,
    goal: &str,
    extra_paths: &[String],
) -> Vec<String> {
    let mut paths = Vec::new();
    merge_source_candidates(&mut paths, extra_paths.iter().map(String::as_str));
    let profile_paths = profile_expected_paths(root, profile, goal);
    merge_source_candidates(&mut paths, profile_paths.iter().map(String::as_str));
    merge_source_candidates(
        &mut paths,
        [
            "src/app/page.tsx",
            "src/app/page.jsx",
            "src/app/page.ts",
            "src/app/page.js",
            "src/app/layout.tsx",
            "src/app/layout.jsx",
            "app/page.tsx",
            "app/page.jsx",
            "app/page.ts",
            "app/page.js",
            "pages/index.tsx",
            "pages/index.jsx",
            "pages/index.ts",
            "pages/index.js",
        ],
    );
    for dir in ["src/app", "app", "pages", "src/components", "components"] {
        collect_source_files_under(root, dir, &mut paths, 0);
    }
    paths.truncate(128);
    paths
}

fn final_acceptance_source_snapshot(
    root: &Path,
    profile: &str,
    goal: &str,
    expected_paths: &[String],
    extra_paths: &[String],
) -> BTreeMap<String, Option<Vec<u8>>> {
    let mut paths = snapshot_source_candidates(root, profile, goal, expected_paths);
    merge_final_acceptance_snapshot_candidates(
        &mut paths,
        expected_paths.iter().map(String::as_str),
    );
    merge_final_acceptance_snapshot_candidates(&mut paths, extra_paths.iter().map(String::as_str));
    merge_final_acceptance_snapshot_candidates(
        &mut paths,
        [
            "package.json",
            "tsconfig.json",
            "next.config.js",
            "next.config.mjs",
            "next.config.ts",
            "postcss.config.js",
            "postcss.config.mjs",
            "tailwind.config.js",
            "tailwind.config.ts",
            "vite.config.js",
            "vite.config.ts",
            "Cargo.toml",
            "pyproject.toml",
        ],
    );
    for path in route_bound_source_paths(root, profile) {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    snapshot_paths(root, &paths)
}

fn route_bound_source_paths(root: &Path, profile: &str) -> Vec<String> {
    route_bound_closure(root, profile)
        .into_iter()
        .filter_map(|path| {
            path.to_str()
                .and_then(safe_source_rel_path)
                .map(|path| path.replace('\\', "/"))
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn snapshot_paths(root: &Path, paths: &[String]) -> BTreeMap<String, Option<Vec<u8>>> {
    let mut snapshot = BTreeMap::new();
    for path in paths {
        let Some(rel) = safe_final_acceptance_snapshot_rel_path(path) else {
            continue;
        };
        snapshot.insert(rel.clone(), std::fs::read(root.join(rel)).ok());
    }
    snapshot
}

fn changed_snapshot_paths(
    before: &BTreeMap<String, Option<Vec<u8>>>,
    after: &BTreeMap<String, Option<Vec<u8>>>,
) -> Vec<String> {
    before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| before.get(path) != after.get(path))
        .collect()
}

fn route_bound_changed_paths(
    before: &BTreeMap<String, Option<Vec<u8>>>,
    after: &BTreeMap<String, Option<Vec<u8>>>,
    before_route_bound: &[String],
    after_route_bound: &[String],
) -> Vec<String> {
    let route_bound = before_route_bound
        .iter()
        .chain(after_route_bound.iter())
        .cloned()
        .collect::<BTreeSet<_>>();
    changed_snapshot_paths(before, after)
        .into_iter()
        .filter(|path| route_bound.contains(path))
        .collect()
}

fn merge_source_candidates<'a>(
    out: &mut Vec<String>,
    candidates: impl IntoIterator<Item = &'a str>,
) {
    for candidate in candidates {
        if let Some(rel) = safe_source_rel_path(candidate)
            && !out.contains(&rel)
        {
            out.push(rel);
        }
    }
}

fn merge_final_acceptance_snapshot_candidates<'a>(
    out: &mut Vec<String>,
    candidates: impl IntoIterator<Item = &'a str>,
) {
    for candidate in candidates {
        if let Some(rel) = safe_final_acceptance_snapshot_rel_path(candidate)
            && !out.contains(&rel)
        {
            out.push(rel);
        }
    }
}

fn collect_source_files_under(root: &Path, rel_dir: &str, out: &mut Vec<String>, depth: usize) {
    if depth > 4 || out.len() >= 128 {
        return;
    }
    let Some(rel_dir) = safe_dir_rel_path(rel_dir) else {
        return;
    };
    let dir = root.join(&rel_dir);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "node_modules" || name == ".anvil" || name.starts_with('.') {
            continue;
        }
        let rel = format!("{rel_dir}/{name}").replace('\\', "/");
        if path.is_dir() {
            collect_source_files_under(root, &rel, out, depth + 1);
        } else if let Some(rel) = safe_source_rel_path(&rel)
            && !out.contains(&rel)
        {
            out.push(rel);
        }
        if out.len() >= 128 {
            break;
        }
    }
}

fn snapshot_latest_dir(config: &Config) -> Option<PathBuf> {
    config
        .eval_events_path
        .as_ref()
        .and_then(|path| path.parent())
        .map(|run_dir| run_dir.join("snapshots").join("latest"))
}

fn readable_workspace_source_path(root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = safe_source_rel_path(rel)?;
    let path = root.join(rel);
    if !path.is_file() {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let canonical = path.canonicalize().ok()?;
    canonical.starts_with(root).then_some(path)
}

fn writable_workspace_source_path(root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = safe_source_rel_path(rel)?;
    let path = root.join(rel);
    let parent = path.parent()?;
    let root = root.canonicalize().ok()?;
    let parent = parent.canonicalize().ok()?;
    parent.starts_with(root).then_some(path)
}

fn safe_dir_rel_path(raw: &str) -> Option<String> {
    let rel = raw.trim().trim_start_matches("./").replace('\\', "/");
    if rel.is_empty() || rel.starts_with('/') || rel.contains('\0') {
        return None;
    }
    let path = Path::new(&rel);
    if !path
        .components()
        .all(|component| matches!(component, std::path::Component::Normal(_)))
    {
        return None;
    }
    let lower = rel.to_ascii_lowercase();
    if lower == ".anvil"
        || lower.starts_with(".anvil/")
        || lower.contains("/.anvil/")
        || lower == "node_modules"
        || lower.starts_with("node_modules/")
        || lower.contains("/node_modules/")
    {
        return None;
    }
    Some(rel)
}

fn safe_source_rel_path(raw: &str) -> Option<String> {
    let rel = safe_dir_rel_path(raw)?;
    let lower = rel.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock" | "bun.lockb" | "cargo.lock"
    ) {
        return None;
    }
    let path = Path::new(&rel);
    let ext = path.extension().and_then(|ext| ext.to_str())?;
    matches!(ext, "tsx" | "ts" | "jsx" | "js" | "mjs" | "cjs" | "css").then_some(rel)
}

fn safe_final_acceptance_snapshot_rel_path(raw: &str) -> Option<String> {
    if let Some(rel) = safe_source_rel_path(raw) {
        return Some(rel);
    }
    let rel = safe_dir_rel_path(raw)?;
    let lower = rel.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "package.json"
            | "tsconfig.json"
            | "next.config.js"
            | "next.config.mjs"
            | "next.config.ts"
            | "postcss.config.js"
            | "postcss.config.mjs"
            | "tailwind.config.js"
            | "tailwind.config.ts"
            | "vite.config.js"
            | "vite.config.ts"
            | "cargo.toml"
            | "pyproject.toml"
    )
    .then_some(rel)
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
    let missing_imports = profile_missing_relative_imports(&config.workspace_root, &plan.profile);
    let import_findings = format_missing_import_findings(&config.workspace_root, &missing_imports);
    let import_scan_section = if import_findings.is_empty() {
        "Current missing relative imports:\n- none".to_string()
    } else {
        format!(
            "Current missing relative imports:\n{}",
            render_prompt_bullets(&import_findings)
        )
    };
    let deterministic_note = deterministic_error
        .map(|err| format!("\nDeterministic repair error:\n{err}\n"))
        .unwrap_or_default();
    format!(
        "Repair this intermediate profile invariant failure in one bounded turn.\n\n\
Original ultra goal:\n{goal}\n\n\
Profile: {profile}\nIntent: {intent}\nPhase id: {phase_id}\nPhase task:\n{phase_task}\n\n\
Exact invariant reason:\n\"{exact_reason}\"\n{deterministic_note}\n\
Offending file contents:\n{file_excerpts}\n\n\
{import_scan_section}\n\n\
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
        import_scan_section = import_scan_section,
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

#[cfg(test)]
fn ultra_final_acceptance_report(
    plan: &UltraPlan,
    config: &Config,
) -> anyhow::Result<VerificationReport> {
    ultra_final_acceptance_report_with_cycle(plan, config, 0)
}

fn ultra_final_acceptance_report_with_cycle(
    plan: &UltraPlan,
    config: &Config,
    cycle_index: usize,
) -> anyhow::Result<VerificationReport> {
    with_final_acceptance_cycle(cycle_index, || {
        ultra_final_acceptance_report_inner(plan, config, cycle_index)
    })
}

fn ultra_final_acceptance_report_inner(
    plan: &UltraPlan,
    config: &Config,
    cycle_index: usize,
) -> anyhow::Result<VerificationReport> {
    let effective_profile = canonical_profile_name(&plan.profile);
    let mut required_paths =
        profile_expected_paths(&config.workspace_root, &effective_profile, &plan.goal);
    let mut required_capabilities = inferred_required_capabilities(&effective_profile, &plan.goal);
    let mut required_obligations =
        inferred_required_obligations(&effective_profile, &plan.goal, &required_capabilities);
    let mut required_evidence =
        inferred_required_evidence(&effective_profile, &plan.goal, &required_capabilities);
    let mut requirements = ContractRequirements {
        capabilities: required_capabilities,
        evidence: required_evidence,
        obligations: required_obligations,
    };
    carry_recorded_promotion_contract_requirements(
        config,
        &effective_profile,
        &plan.goal,
        &mut requirements,
    );
    required_capabilities = requirements.capabilities;
    required_evidence = requirements.evidence;
    required_obligations = requirements.obligations;
    let mut evidence_hint_tokens = evidence_hint_tokens_for_goal(&plan.goal);
    let bound_contract = bind_completion_contract_for_acceptance(
        config,
        "ultra-plan-run",
        &effective_profile,
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
        &inferred_required_evidence(&effective_profile, &plan.goal, &required_capabilities),
    );
    let missing = missing_final_artifacts(&config.workspace_root, &required_paths);
    let mut acceptance = verify_runtime_acceptance_with_browser_dirs_and_hints(
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
    let profile_invariant_report = verify_profile_invariant(
        &config.workspace_root,
        &effective_profile,
        &plan.goal,
        &ProfileSnapshot::None,
    );
    let profile_report = if !profile_invariant_report.is_pass() {
        profile_invariant_report
    } else {
        verify_profile_final(&config.workspace_root, &effective_profile, &plan.goal)
    };
    let external_report = bound_contract.as_ref().map(|bound| {
        bound
            .contract
            .verify_with_goal(&config.workspace_root, &plan.goal)
    });
    let contract_required =
        completion_contract_required(&effective_profile, &plan.goal, &required_capabilities)
            || bound_contract.as_ref().is_some_and(|bound| bound.required);
    let external_contract_checked = bound_contract.is_some();
    let contract_binding_missing = contract_required && !external_contract_checked;
    let external_ok = !contract_binding_missing
        && external_contract_ok_after_runtime_arbitration(
            external_report.as_ref(),
            Some(&acceptance),
        );
    let production_build_failed = report_has_production_build_failure(&profile_report)
        || external_report
            .as_ref()
            .is_some_and(report_has_production_build_failure);
    let browser_probe = if production_build_failed {
        None
    } else {
        run_ultra_final_browser_checks_before_arbitration(
            config,
            plan,
            &effective_profile,
            &required_capabilities,
            &required_evidence,
        )
    };
    let evidence_arbitration = final_acceptance_evidence_arbitration(
        config,
        &mut acceptance,
        &required_capabilities,
        &required_evidence,
        &required_obligations,
    );
    let mut release_gate = if production_build_failed {
        production_build_failed_release_gate()
    } else {
        let signal_text = ultra_plan_signal_text(plan);
        final_acceptance_release_gate(
            config,
            &effective_profile,
            &signal_text,
            &required_capabilities,
            Some(&acceptance),
            true,
        )
    };
    let signal_text = ultra_plan_signal_text(plan);
    let gate_telemetry = acceptance_gate_telemetry(
        &effective_profile,
        &signal_text,
        &required_capabilities,
        &required_evidence,
        &release_gate,
    );
    if acceptance.passed
        && external_ok
        && let Some(reason) = acceptance_gates_disconnected_reason(&gate_telemetry, &release_gate)
    {
        release_gate.status = "failed".to_string();
        release_gate.reasons = dedup_strings(
            release_gate
                .reasons
                .iter()
                .cloned()
                .chain(std::iter::once(reason))
                .collect(),
        );
    }
    let profile_behavior_probe = run_profile_behavior_probe(
        config,
        &effective_profile,
        &plan.goal,
        &required_capabilities,
        &profile_report,
    );
    let profile_behavior_failed = profile_behavior_probe.status == "failed";
    if profile_behavior_failed {
        mark_release_gate_profile_behavior_failed(&mut release_gate, &profile_behavior_probe);
    }
    let final_acceptance_status = release_gate_final_acceptance_status(&release_gate);
    let runtime_acceptance_status = if profile_behavior_failed {
        "failed"
    } else {
        runtime_acceptance_status(acceptance.passed, Some(&acceptance))
    };
    let (assurance_level, assurance_reason) = earned_assurance_for_completion(
        &effective_profile,
        &required_capabilities,
        !contract_required || (external_contract_checked && !contract_binding_missing),
        final_acceptance_status,
        &release_gate,
        &gate_telemetry,
    );
    let release_quality_completion =
        release_quality_completion_status(&release_gate, final_acceptance_status);
    let next_action = release_gate_next_action(&release_gate, final_acceptance_status);
    let state_dimensions_changed =
        interaction_state_dimensions_changed_from_path(&release_gate.interaction_evidence_path);
    let action_hooks = interaction_action_hooks_from_path(&release_gate.interaction_evidence_path);
    let text_telemetry =
        interaction_text_telemetry_from_path(&release_gate.interaction_evidence_path);
    let plan_adherence = plan_adherence_report(plan, &config.workspace_root);
    let phase_signal_text = ultra_plan_phase_signal_text(plan);
    let requested_port =
        effective_requested_port(&effective_profile, &plan.goal, Some(&phase_signal_text));
    let mut compile_errors = profile_report.compile_errors.clone();
    if let Some(report) = external_report.as_ref() {
        for error in &report.compile_errors {
            if !compile_errors.contains(error) {
                compile_errors.push(error.clone());
            }
        }
    }
    let primary_reason = if !missing.is_empty() {
        format!("missing final artifacts: {}", missing.join(", "))
    } else if contract_binding_missing {
        "completion contract binding required but missing".to_string()
    } else if !profile_report.is_pass() {
        profile_report.primary_reason()
    } else if let Some(report) = external_report.as_ref().filter(|report| {
        !external_contract_ok_after_runtime_arbitration(Some(*report), Some(&acceptance))
    }) {
        report.primary_reason()
    } else if profile_behavior_failed {
        profile_behavior_probe.reasons.join("; ")
    } else if !acceptance.passed {
        acceptance.primary_reason.clone()
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
            &effective_profile,
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
            release_recovery_verify_commands(&effective_profile, &release_gate),
        )
    } else {
        None
    };
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "ultra_final_acceptance",
            "cycle_index": cycle_index,
            "profile": effective_profile.clone(),
            "effective_profile": effective_profile.clone(),
            "contract_origin": contract_origin_for_acceptance(config),
            "profile_inferred": config
                .profile_inference
                .map(|inference| inference.profile)
                .unwrap_or(""),
            "profile_inference_source": config
                .profile_inference
                .map(|inference| inference.source.as_str())
                .unwrap_or(""),
            "requested_port": requested_port.as_ref().map(|requested| requested.telemetry.clone()),
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
            "runtime_acceptance_passed": acceptance.passed && !profile_behavior_failed,
            "runtime_acceptance_status": runtime_acceptance_status,
            "runtime_acceptance_inconclusive": acceptance.inconclusive,
            "compile_errors": compile_errors.clone(),
            "compile_error_failure_kind": if compile_errors.is_empty() { "" } else { "implementation_compile_error" },
            "final_acceptance_status": final_acceptance_status,
            "assurance_level": assurance_level,
            "assurance_reason": assurance_reason,
            "release_quality_completion": release_quality_completion,
            "missing_capabilities": acceptance.missing_capabilities.clone(),
            "missing_evidence": acceptance.missing_evidence.clone(),
            "missing_obligations": acceptance.missing_obligations.clone(),
            "weak_evidence": acceptance.weak_evidence.clone(),
            "runtime_acceptance_diagnostics": acceptance.diagnostics.clone(),
            "unverified_evidence": acceptance.unverified_evidence.clone(),
            "evidence_tiers": acceptance.evidence_tiers.clone(),
            "evidence_arbitration": evidence_arbitration.records.clone(),
            "evidence_arbitration_summary": evidence_arbitration.summary.clone(),
            "artifact_obligations": acceptance.artifact_obligations.clone(),
            "capability_evidence_bindings": acceptance.capability_evidence_bindings.clone(),
            "obligation_repair_targets": acceptance.obligation_repair_targets.clone(),
            "inconclusive_reasons": acceptance.inconclusive_reasons.clone(),
            "release_gate_status": release_gate.status.clone(),
            "release_gate_reasons": release_gate.reasons.clone(),
            "profile_behavior_probe_status": profile_behavior_probe.status,
            "profile_behavior_probe_reasons": profile_behavior_probe.reasons.clone(),
            "profile_behavior_probe_evidence_path": profile_behavior_probe.evidence_path.clone().unwrap_or_default(),
            "browser_readiness_applicable": gate_telemetry.browser_readiness_applicable,
            "browser_readiness_execution_status": gate_telemetry.browser_readiness_execution_status.clone(),
            "browser_readiness_status": release_gate.browser_readiness_status.clone(),
            "browser_readiness_evidence_path": release_gate.browser_readiness_evidence_path.clone(),
            "interaction_evidence_applicable": gate_telemetry.interaction_evidence_applicable,
            "interaction_evidence_execution_status": gate_telemetry.interaction_evidence_execution_status.clone(),
            "interaction_evidence_status": release_gate.interaction_evidence_status.clone(),
            "interaction_evidence_path": release_gate.interaction_evidence_path.clone(),
            "state_dimensions_changed": state_dimensions_changed,
            "action_hooks": action_hooks,
            "text_entry": text_telemetry.text_entry,
            "text_entry_target": text_telemetry.text_entry_target,
            "typed_token": text_telemetry.typed_token,
            "token_echoed": text_telemetry.token_echoed,
            "echo_latency_ms": text_telemetry.echo_latency_ms,
            "token_echoed_after_reload": text_telemetry.token_echoed_after_reload,
            "token_echo_after_reload_latency_ms": text_telemetry.token_echo_after_reload_latency_ms,
            "text_input_state_change": text_telemetry.text_input_state_change,
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
            "plan_adherence_present": plan_adherence.present.clone(),
            "plan_adherence_missing": plan_adherence.missing.clone(),
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
    merge_verification_report(&mut report, profile_report);
    if !acceptance.passed {
        report.push_profile_failure(acceptance.primary_reason.clone());
        for guidance in runtime_acceptance_repair_guidance(&acceptance) {
            report.push_profile_failure(format!("repair guidance: {guidance}"));
        }
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
    if profile_behavior_failed {
        for reason in &profile_behavior_probe.reasons {
            report.push_profile_failure(reason.clone());
        }
        if let Some(path) = &profile_behavior_probe.evidence_path {
            report.push_profile_failure(format!("profile behavior evidence: {path}"));
        }
    }
    if let Some(external_report) = external_report.filter(|report| {
        !external_contract_ok_after_runtime_arbitration(Some(report), Some(&acceptance))
    }) {
        let reason = external_report.primary_reason();
        merge_verification_report(&mut report, external_report);
        report.push_profile_failure(format!("external contract failed: {}", reason));
    }
    let append_release_observations = !report.is_pass() || release_gate.status == "failed";
    if release_gate.status == "failed" {
        report.push_profile_failure(format!(
            "release gate failed: {}",
            release_gate.reasons.join("; ")
        ));
    }
    if append_release_observations {
        append_release_gate_observation_failures(&mut report, &release_gate);
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

fn plan_adherence_report(plan: &UltraPlan, root: &Path) -> PlanAdherenceReport {
    let tokens = ultra_plan_requested_feature_tokens(plan);
    if tokens.is_empty() {
        return PlanAdherenceReport::default();
    }
    let corpus = comment_stripped_source_corpus(root);
    let corpus_lower = corpus.to_ascii_lowercase();
    let mut report = PlanAdherenceReport::default();
    for token in tokens {
        let present = if token.is_ascii() {
            corpus_lower.contains(&token)
        } else {
            corpus.contains(&token)
        };
        if present {
            report.present.push(token);
        } else {
            report.missing.push(token);
        }
    }
    report
}

fn ultra_plan_requested_feature_tokens(plan: &UltraPlan) -> Vec<String> {
    let mut tokens = BTreeSet::new();
    for phase in &plan.phases {
        collect_plan_feature_tokens(&phase.prompt, &mut tokens);
    }
    tokens.into_iter().collect()
}

fn collect_plan_feature_tokens(text: &str, tokens: &mut BTreeSet<String>) {
    let mut ascii = String::new();
    let mut katakana = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            flush_katakana_token(&mut katakana, tokens);
            ascii.push(ch.to_ascii_lowercase());
        } else if is_katakana(ch) {
            flush_ascii_token(&mut ascii, tokens);
            katakana.push(ch);
        } else {
            flush_ascii_token(&mut ascii, tokens);
            flush_katakana_token(&mut katakana, tokens);
        }
    }
    flush_ascii_token(&mut ascii, tokens);
    flush_katakana_token(&mut katakana, tokens);
}

fn flush_ascii_token(token: &mut String, tokens: &mut BTreeSet<String>) {
    if token.len() >= 3
        && !token.chars().all(|ch| ch.is_ascii_digit())
        && !plan_adherence_stopword(token)
    {
        tokens.insert(token.clone());
    }
    token.clear();
}

fn flush_katakana_token(token: &mut String, tokens: &mut BTreeSet<String>) {
    if token.chars().count() >= 2 {
        tokens.insert(token.clone());
    }
    token.clear();
}

fn is_katakana(ch: char) -> bool {
    matches!(ch, '\u{30A0}'..='\u{30FF}' | '\u{31F0}'..='\u{31FF}')
}

fn plan_adherence_stopword(token: &str) -> bool {
    signals::plan_adherence_stopword(token)
}

fn ultra_plan_phase_signal_text(plan: &UltraPlan) -> String {
    plan.phases
        .iter()
        .flat_map(|phase| [phase.id.as_str(), phase.prompt.as_str()])
        .collect::<Vec<_>>()
        .join("\n")
}

fn ultra_plan_signal_text(plan: &UltraPlan) -> String {
    let phase_text = ultra_plan_phase_signal_text(plan);
    if phase_text.is_empty() {
        plan.goal.clone()
    } else {
        format!("{}\n{}", plan.goal, phase_text)
    }
}

fn maybe_run_ultra_final_browser_probe(
    config: &Config,
    plan: &UltraPlan,
    effective_profile: &str,
    required_capabilities: &[String],
    interaction_options: BrowserInteractionProbeOptions,
) -> Option<BrowserReadinessObservation> {
    let phase_text = ultra_plan_phase_signal_text(plan);
    let signal_text = ultra_plan_signal_text(plan);
    if !ultra_browser_probe_required(effective_profile, &signal_text, required_capabilities)
        || !ultra_browser_probe_runtime_enabled(config)
    {
        return None;
    }
    let requested_port = effective_requested_port(effective_profile, &plan.goal, Some(&phase_text));
    let observation = probe_browser_readiness_with_offline_and_interaction_options(
        &config.workspace_root,
        effective_profile,
        requested_port.as_ref().map(|requested| requested.port),
        Duration::from_secs(30),
        config.offline,
        interaction_options,
    );
    emit_browser_probe_event(
        config,
        &observation,
        requested_port
            .as_ref()
            .map(|requested| requested.telemetry.clone()),
    );
    Some(observation)
}

fn run_ultra_final_browser_checks_before_arbitration(
    config: &Config,
    plan: &UltraPlan,
    effective_profile: &str,
    required_capabilities: &[String],
    required_evidence: &[String],
) -> Option<BrowserReadinessObservation> {
    let phase_text = ultra_plan_phase_signal_text(plan);
    let signal_text = ultra_plan_signal_text(plan);
    if !ultra_browser_probe_required(effective_profile, &signal_text, required_capabilities) {
        return None;
    }
    let requested_port = effective_requested_port(effective_profile, &plan.goal, Some(&phase_text));
    let interaction_options =
        browser_interaction_probe_options(required_capabilities, required_evidence);
    let browser_probe = maybe_run_ultra_final_browser_probe(
        config,
        plan,
        effective_profile,
        required_capabilities,
        interaction_options,
    );
    let _ = browser_release_gate_with_options(
        config,
        requires_canvas_surface(&signal_text, required_capabilities),
        interaction_options,
        requested_port.as_ref().map(|requested| requested.port),
    );
    browser_probe
}

fn browser_interaction_probe_options(
    required_capabilities: &[String],
    required_evidence: &[String],
) -> BrowserInteractionProbeOptions {
    let text_entry_required = required_capabilities
        .iter()
        .chain(required_evidence.iter())
        .any(|value| {
            let lower = value.to_ascii_lowercase();
            lower.contains("text")
                || lower.contains("editor")
                || lower.contains("note")
                || lower.contains("todo")
                || lower.contains("content")
                || lower.contains("preview")
                || lower.contains("render")
                || lower == "input_output_contract"
                || lower == "requested_content"
                || lower == "requested_content_evidence"
                || lower == "live_preview"
                || lower == "live_preview_evidence"
        });
    let token_echo_required = required_capabilities
        .iter()
        .chain(required_evidence.iter())
        .any(|value| {
            let lower = value.to_ascii_lowercase();
            lower.contains("preview")
                || lower.contains("render")
                || lower.contains("content")
                || lower == "requested_content"
                || lower == "requested_content_evidence"
                || lower == "live_preview"
                || lower == "live_preview_evidence"
        });
    BrowserInteractionProbeOptions {
        persistence_required: required_capabilities
            .iter()
            .any(|capability| capability == "persistence")
            || required_evidence
                .iter()
                .any(|evidence| evidence == "persistence_evidence"),
        text_entry_required,
        token_echo_required,
    }
}

fn report_has_production_build_failure(report: &VerificationReport) -> bool {
    !report.compile_errors.is_empty()
        || report.dependency_missing.iter().any(|reason| {
            let lower = reason.to_ascii_lowercase();
            lower.contains("next.js build")
                || lower.contains("next build")
                || lower.contains("npm run build")
                || lower.contains("dependency_setup_missing")
        })
        || report.command_failures.iter().any(|failure| {
            let lower = format!("{} {}", failure.command, failure.reason).to_ascii_lowercase();
            lower.contains("npm run build")
                || lower.contains("next build")
                || lower.contains("build_verify_failed")
                || lower.contains("implementation_compile_error")
        })
}

fn ultra_browser_probe_required(
    profile: &str,
    signal_text: &str,
    required_capabilities: &[String],
) -> bool {
    canonical_profile_name(profile) == "nextjs"
        && (required_capabilities.iter().any(|capability| {
            matches!(
                capability.as_str(),
                "stateful_interaction"
                    | "player_control"
                    | "user_input_or_action"
                    | "visible_state_change"
                    | "persistence"
                    | "adversary_or_challenge"
                    | "progression_or_score"
                    | "failure_or_collision_rule"
            )
        }) || signals::contains_browser_probe_token(signal_text))
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

fn emit_browser_probe_event(
    config: &Config,
    observation: &BrowserReadinessObservation,
    requested_port: Option<String>,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "browser_probe",
            "cycle_index": current_final_acceptance_cycle_index(),
            "profile": observation.profile,
            "status": observation.status,
            "ok": observation.ok,
            "requested_port": requested_port,
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
            "ssr_has_canvas": observation.has_canvas,
            "ssr_interactive_control_count": observation.interactive_control_count,
            "has_canvas": observation.has_canvas,
            "interactive_control_count": observation.interactive_control_count,
            "title_text_excerpt": observation.title_text_excerpt,
        }),
    );
}

fn emit_browser_interaction_probe_event(config: &Config, outcome: &InteractionProbeOutcome) {
    let source_diagnostics = interaction_source_diagnostics(config);
    let source_diagnostic_labels = unattached_ref_diagnostic_labels(&source_diagnostics);
    match outcome {
        InteractionProbeOutcome::Unavailable(reason) => {
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "browser_interaction_probe",
                    "cycle_index": current_final_acceptance_cycle_index(),
                    "status": "unavailable",
                    "ok": false,
                    "failure_kind": reason,
                    "evidence_path": "",
                    "source_diagnostics": &source_diagnostic_labels,
                    "unattached_ref_diagnostics": &source_diagnostics,
                    "playwright_resolution_location": "",
                    "playwright_version": "",
                }),
            );
        }
        InteractionProbeOutcome::Observation(observation) => {
            annotate_interaction_evidence_with_source_diagnostics(
                &observation.evidence_path,
                &source_diagnostics,
            );
            let workspace_evidence =
                interaction_probe::browser_interaction_evidence_path(&config.workspace_root);
            if workspace_evidence != observation.evidence_path {
                annotate_interaction_evidence_with_source_diagnostics(
                    &workspace_evidence,
                    &source_diagnostics,
                );
            }
            let resolution = observation.playwright_resolution.as_ref();
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "browser_interaction_probe",
                    "cycle_index": current_final_acceptance_cycle_index(),
                    "status": observation.status,
                    "ok": observation.ok,
                    "failure_kind": observation.failure_kind,
                    "failure_category": if observation.failure_kind.starts_with("probe_dependency_missing")
                        || observation.failure_kind.starts_with("probe_infrastructure_failed")
                    {
                        "infrastructure"
                    } else if observation.failure_kind.is_empty() {
                        ""
                    } else {
                        "app"
                    },
                    "stage": observation.stage,
                    "error": observation.error,
                    "stderr_excerpt": observation.stderr_excerpt,
                    "server_http_status": observation.server_http_status,
                    "server_http_error": observation.server_http_error,
                    "navigation_failure_kind": observation.navigation_failure_kind,
                    "cold_start_ms": observation.cold_start_ms,
                    "measured_navigation_ms": observation.measured_navigation_ms,
                    "has_canvas": observation.has_canvas,
                    "interactive_control_count": observation.interactive_control_count,
                    "steps": observation.steps,
                    "probe_mode": observation.probe_mode.as_str(),
                    "contract_hook_status": observation.contract_hook_status.as_str(),
                    "candidate_table": &observation.candidate_table,
                    "input_dispatches": &observation.input_dispatches,
                    "canvas_snapshots": &observation.canvas_snapshots,
                    "canvas_blank_before_start": observation.canvas_blank_before_start,
                    "canvas_blank_after_start": observation.canvas_blank_after_start,
                    "canvas_blank_after_inputs": observation.canvas_blank_after_inputs,
                    "source_diagnostics": &source_diagnostic_labels,
                    "unattached_ref_diagnostics": &source_diagnostics,
                    "state_dimensions_changed": &observation.state_dimensions_changed,
                    "persistence_after_reload": observation.persistence_after_reload.as_str(),
                    "persistence_after_reload_reason": observation.persistence_after_reload_reason.as_str(),
                    "persistence_changed_dimensions": &observation.persistence_changed_dimensions,
                    "action_hooks": &observation.action_hooks,
                    "text_entry": observation.text_entry.as_str(),
                    "text_entry_target": observation.text_entry_target.as_str(),
                    "typed_token": observation.typed_token.as_str(),
                    "token_echoed": observation.token_echoed,
                    "echo_latency_ms": observation.echo_latency_ms,
                    "token_echoed_after_reload": observation.token_echoed_after_reload,
                    "token_echo_after_reload_latency_ms": observation.token_echo_after_reload_latency_ms,
                    "text_input_state_change": observation.text_input_state_change,
                    "input_state_evaluated_after_start": observation.input_state_evaluated_after_start,
                    "primary_start_transition": observation.primary_transition_observed,
                    "informational_failure_kinds": &observation.informational_failure_kinds,
                    "duration_ms": observation.duration_ms,
                    "evidence_path": observation.evidence_path.display().to_string(),
                    "script_path": observation.script_path.display().to_string(),
                    "output_excerpt": eval_events::body_snippet(&observation.output_excerpt),
                    "child_spawned": observation.child_spawned,
                    "child_reaped": observation.child_reaped,
                    "playwright_resolution_location": resolution
                        .map(|resolution| resolution.location.as_str())
                        .unwrap_or(""),
                    "playwright_module_path": resolution
                        .map(|resolution| resolution.module_path.as_str())
                        .unwrap_or(""),
                    "playwright_node_path": resolution
                        .and_then(|resolution| resolution.node_path.as_deref())
                        .unwrap_or(""),
                    "playwright_version": resolution
                        .map(|resolution| resolution.version.as_str())
                        .unwrap_or(""),
                }),
            );
        }
    }
}

fn interaction_source_diagnostics(config: &Config) -> Vec<UnattachedRefDiagnostic> {
    let profile = config
        .profile_inference
        .map(|inference| inference.profile.to_string())
        .unwrap_or_else(|| config.profile.clone());
    route_bound_unattached_ref_diagnostics(&config.workspace_root, &profile)
}

fn unattached_ref_diagnostic_labels(diagnostics: &[UnattachedRefDiagnostic]) -> Vec<String> {
    diagnostics
        .iter()
        .map(|diagnostic| diagnostic.diagnostic.clone())
        .collect()
}

fn annotate_interaction_evidence_with_source_diagnostics(
    path: &Path,
    diagnostics: &[UnattachedRefDiagnostic],
) {
    if diagnostics.is_empty() || !path.is_file() {
        return;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(mut value) = serde_json::from_str::<Value>(&text) else {
        return;
    };
    if !value.is_object() {
        return;
    }
    value["source_diagnostics"] = json!(unattached_ref_diagnostic_labels(diagnostics));
    value["unattached_ref_diagnostics"] = json!(diagnostics);
    if let Ok(text) = serde_json::to_string_pretty(&value) {
        let _ = std::fs::write(path, format!("{text}\n"));
    }
}

fn ultra_contract_runtime_acceptance_report(
    plan: &UltraPlan,
    config: &Config,
) -> anyhow::Result<RuntimeAcceptanceReport> {
    let effective_profile = canonical_profile_name(&plan.profile);
    let mut required_paths =
        profile_expected_paths(&config.workspace_root, &effective_profile, &plan.goal);
    let mut required_capabilities = inferred_required_capabilities(&effective_profile, &plan.goal);
    let mut required_obligations =
        inferred_required_obligations(&effective_profile, &plan.goal, &required_capabilities);
    let mut required_evidence =
        inferred_required_evidence(&effective_profile, &plan.goal, &required_capabilities);
    let mut requirements = ContractRequirements {
        capabilities: required_capabilities,
        evidence: required_evidence,
        obligations: required_obligations,
    };
    carry_recorded_promotion_contract_requirements(
        config,
        &effective_profile,
        &plan.goal,
        &mut requirements,
    );
    required_capabilities = requirements.capabilities;
    required_evidence = requirements.evidence;
    required_obligations = requirements.obligations;
    let mut evidence_hint_tokens = evidence_hint_tokens_for_goal(&plan.goal);
    let bound_contract = bind_completion_contract_for_acceptance(
        config,
        "ultra-plan-run",
        &effective_profile,
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
        &inferred_required_evidence(&effective_profile, &plan.goal, &required_capabilities),
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
    domain_profile(profile).infer_required_capabilities(goal)
}

fn inferred_required_evidence(
    profile: &str,
    goal: &str,
    required_capabilities: &[String],
) -> Vec<String> {
    let mut evidence = domain_profile(profile).infer_required_evidence(goal, required_capabilities);
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
    let profile_obligations =
        domain_profile(profile).infer_required_obligations(goal, required_capabilities);
    if !profile_obligations.is_empty() {
        return profile_obligations;
    }
    let is_app_profile = matches!(canonical_profile_name(profile).as_str(), "web" | "vite");
    let app_like_goal = signals::contains_app_like_token(goal);
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
                | "persistence"
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct AcceptanceGateTelemetry {
    browser_readiness_applicable: bool,
    browser_readiness_execution_status: String,
    interaction_evidence_applicable: bool,
    interaction_evidence_execution_status: String,
}

fn run_profile_behavior_probe(
    config: &Config,
    profile: &str,
    goal: &str,
    required_capabilities: &[String],
    profile_report: &VerificationReport,
) -> ProfileBehaviorProbeReport {
    if !profile_report.is_pass() {
        return ProfileBehaviorProbeReport::pass();
    }
    match domain_profile(profile).behavior_probe(
        &config.workspace_root,
        goal,
        required_capabilities,
        config.offline,
    ) {
        Ok(report) => {
            emit_profile_behavior_probe_event(config, profile, &report);
            report
        }
        Err(err) => {
            let report = ProfileBehaviorProbeReport {
                status: "failed",
                reasons: vec![format!("profile_behavior_probe_error: {err}")],
                evidence_path: None,
            };
            emit_profile_behavior_probe_event(config, profile, &report);
            report
        }
    }
}

fn emit_profile_behavior_probe_event(
    config: &Config,
    profile: &str,
    report: &ProfileBehaviorProbeReport,
) {
    if report.status == "pass" && report.reasons.is_empty() && report.evidence_path.is_none() {
        return;
    }
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "profile_behavior_probe",
            "cycle_index": current_final_acceptance_cycle_index(),
            "profile": profile,
            "status": report.status,
            "ok": report.status == "pass",
            "reasons": report.reasons.clone(),
            "evidence_path": report.evidence_path.clone().unwrap_or_default(),
        }),
    );
}

fn production_build_failed_release_gate() -> ReleaseGateSummary {
    ReleaseGateSummary {
        status: "not_applicable".to_string(),
        reasons: vec!["production_build_failed_before_browser_probe".to_string()],
        browser_readiness_status: "not_applicable".to_string(),
        browser_readiness_evidence_path: String::new(),
        interaction_evidence_status: "not_applicable".to_string(),
        interaction_evidence_path: String::new(),
    }
}

fn final_acceptance_release_gate(
    config: &Config,
    profile: &str,
    goal: &str,
    required_capabilities: &[String],
    acceptance: Option<&crate::minimal_loop::evidence::RuntimeAcceptanceReport>,
    check_browser_on_runtime_failure: bool,
) -> ReleaseGateSummary {
    let effective_profile = canonical_profile_name(profile);
    let is_next = effective_profile == "nextjs";
    let acceptance_required_evidence = acceptance
        .map(|report| report.evidence_tiers.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let interaction_options =
        browser_interaction_probe_options(required_capabilities, &acceptance_required_evidence);
    let requested_port = effective_requested_port(&effective_profile, goal, None);
    let requires_browser = is_next
        && (required_capabilities.iter().any(|capability| {
            matches!(
                capability.as_str(),
                "stateful_interaction"
                    | "player_control"
                    | "user_input_or_action"
                    | "visible_state_change"
                    | "persistence"
                    | "adversary_or_challenge"
                    | "progression_or_score"
                    | "failure_or_collision_rule"
            )
        }) || signals::contains_browser_probe_token(goal));
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
        if requires_browser
            && check_browser_on_runtime_failure
            && runtime_acceptance_has_buildable_nextjs_boundary(report)
        {
            let mut gate = browser_release_gate_with_options(
                config,
                requires_canvas_surface(goal, required_capabilities),
                interaction_options,
                requested_port.as_ref().map(|requested| requested.port),
            );
            let mut reasons = vec![report.primary_reason.clone()];
            reasons.extend(std::mem::take(&mut gate.reasons));
            gate.status = "failed".to_string();
            gate.reasons = dedup_strings(reasons);
            return gate;
        }
        return ReleaseGateSummary {
            status: "failed".to_string(),
            reasons: vec![report.primary_reason.clone()],
            browser_readiness_status: "not_checked".to_string(),
            browser_readiness_evidence_path: String::new(),
            interaction_evidence_status: "not_checked".to_string(),
            interaction_evidence_path: String::new(),
        };
    }
    if !report.unverified_evidence.is_empty() {
        let mut gate = if requires_browser {
            browser_release_gate_with_options(
                config,
                requires_canvas_surface(goal, required_capabilities),
                interaction_options,
                requested_port.as_ref().map(|requested| requested.port),
            )
        } else {
            ReleaseGateSummary {
                status: "partial".to_string(),
                reasons: Vec::new(),
                browser_readiness_status: "not_applicable".to_string(),
                browser_readiness_evidence_path: String::new(),
                interaction_evidence_status: "not_applicable".to_string(),
                interaction_evidence_path: String::new(),
            }
        };
        let mut reasons = runtime_acceptance_unverified_release_reasons(
            report,
            interaction_probe_performed_for_run(config),
        );
        reasons.extend(std::mem::take(&mut gate.reasons));
        gate.status = "partial".to_string();
        gate.reasons = dedup_strings(reasons);
        return gate;
    }
    if requires_browser {
        return browser_release_gate_with_options(
            config,
            requires_canvas_surface(goal, required_capabilities),
            interaction_options,
            requested_port.as_ref().map(|requested| requested.port),
        );
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

fn acceptance_gate_telemetry(
    profile: &str,
    signal_text: &str,
    required_capabilities: &[String],
    required_evidence: &[String],
    release_gate: &ReleaseGateSummary,
) -> AcceptanceGateTelemetry {
    let browser_applicable =
        ultra_browser_probe_required(profile, signal_text, required_capabilities);
    let interaction_applicable =
        browser_applicable && interaction_gate_required(required_capabilities, required_evidence);
    AcceptanceGateTelemetry {
        browser_readiness_applicable: browser_applicable,
        browser_readiness_execution_status: gate_execution_status(
            &release_gate.browser_readiness_status,
        ),
        interaction_evidence_applicable: interaction_applicable,
        interaction_evidence_execution_status: gate_execution_status(
            &release_gate.interaction_evidence_status,
        ),
    }
}

fn interaction_gate_required(
    required_capabilities: &[String],
    required_evidence: &[String],
) -> bool {
    required_capabilities
        .iter()
        .chain(required_evidence.iter())
        .any(|requirement| {
            matches!(
                requirement.as_str(),
                "stateful_interaction"
                    | "player_control"
                    | "user_input_or_action"
                    | "visible_state_change"
                    | "persistence"
                    | "adversary_or_challenge"
                    | "progression_or_score"
                    | "failure_or_collision_rule"
                    | "browser_interaction"
                    | "playable_ui"
                    | "interactive_ui_source_evidence"
                    | "visible_interactive_surface_evidence"
                    | "user_input_handler_evidence"
                    | "stateful_update_evidence"
                    | "non_static_screen_evidence"
                    | "persistence_evidence"
                    | "challenge_or_adversary_evidence"
                    | "score_or_progression_evidence"
                    | "failure_or_collision_evidence"
            )
        })
}

fn gate_execution_status(status: &str) -> String {
    if gate_status_disconnected(status) {
        "disconnected".to_string()
    } else if status == "passed" {
        "performed".to_string()
    } else if status.starts_with("failed") {
        "performed_failed".to_string()
    } else if status.starts_with("unavailable") {
        "unavailable".to_string()
    } else {
        status.to_string()
    }
}

fn gate_status_disconnected(status: &str) -> bool {
    let status = status.trim();
    status.is_empty()
        || matches!(status, "not_applicable" | "not_checked" | "skipped")
        || status.starts_with("skipped:")
}

fn acceptance_gates_disconnected_reason(
    telemetry: &AcceptanceGateTelemetry,
    release_gate: &ReleaseGateSummary,
) -> Option<String> {
    let mut disconnected = Vec::new();
    if telemetry.browser_readiness_applicable
        && gate_status_disconnected(&release_gate.browser_readiness_status)
    {
        disconnected.push(format!(
            "browser_readiness_status={}",
            release_gate.browser_readiness_status
        ));
    }
    if telemetry.interaction_evidence_applicable
        && gate_status_disconnected(&release_gate.interaction_evidence_status)
    {
        disconnected.push(format!(
            "interaction_evidence_status={}",
            release_gate.interaction_evidence_status
        ));
    }
    (!disconnected.is_empty())
        .then(|| format!("acceptance_gates_disconnected:{}", disconnected.join(",")))
}

fn mark_release_gate_profile_behavior_failed(
    release_gate: &mut ReleaseGateSummary,
    profile_behavior_probe: &ProfileBehaviorProbeReport,
) {
    let mut reasons = release_gate.reasons.clone();
    if profile_behavior_probe.reasons.is_empty() {
        reasons.push("profile_behavior_probe_failed".to_string());
    } else {
        reasons.extend(
            profile_behavior_probe
                .reasons
                .iter()
                .map(|reason| format!("profile_behavior_probe_failed:{reason}")),
        );
    }
    if let Some(path) = &profile_behavior_probe.evidence_path {
        reasons.push(format!("profile_behavior_probe_evidence:{path}"));
    }
    release_gate.status = "failed".to_string();
    release_gate.reasons = dedup_strings(reasons);
}

fn runtime_acceptance_unverified_release_reasons(
    report: &crate::minimal_loop::evidence::RuntimeAcceptanceReport,
    probe_performed_for_run: bool,
) -> Vec<String> {
    let mut reasons = Vec::new();
    let mut saw_probe_unavailable = false;
    for evidence in &report.unverified_evidence {
        if let Some(reason) = evidence
            .split_once(":unverified:")
            .map(|(_, reason)| reason.trim())
            .filter(|reason| !reason.is_empty())
        {
            if reason == "probe_unavailable" {
                if probe_performed_for_run {
                    continue;
                }
                saw_probe_unavailable = true;
            } else {
                reasons.push(format!("interaction_unverified:{reason}"));
            }
        }
        if probe_performed_for_run && evidence.contains(":unverified:probe_unavailable") {
            continue;
        }
        reasons.push(format!("unverified_probe_required:{evidence}"));
    }
    if saw_probe_unavailable {
        reasons.insert(0, "interaction_unverified:probe_unavailable".to_string());
        reasons.push(
            crate::minimal_loop::interaction_probe::INTERACTION_PROBE_SETUP_REMEDIATION.to_string(),
        );
    }
    reasons
}

fn interaction_probe_performed_for_run(config: &Config) -> bool {
    release_evidence_candidate_paths(
        config,
        &[
            "browser-interaction.json",
            "interaction-evidence.json",
            "interaction.json",
        ],
    )
    .into_iter()
    .filter_map(|path| std::fs::read_to_string(path).ok())
    .filter_map(|text| serde_json::from_str::<Value>(&text).ok())
    .any(|value| {
        let details = value
            .get("browser_details")
            .or_else(|| value.get("details"))
            .filter(|value| value.is_object());
        bool_field_deep(&value, details, &["interaction_performed"]) == Some(true)
    })
}

fn runtime_acceptance_has_buildable_nextjs_boundary(
    report: &crate::minimal_loop::evidence::RuntimeAcceptanceReport,
) -> bool {
    !report.missing_evidence.iter().any(|item| {
        matches!(
            item.as_str(),
            "implementation_artifact"
                | "nextjs_route_evidence"
                | "build_command_or_dependency_missing_boundary"
        )
    }) && !report
        .missing_obligations
        .iter()
        .any(|item| item == "implementation")
}

fn requires_canvas_surface(signal_text: &str, required_capabilities: &[String]) -> bool {
    signals::contains_canvas_token(signal_text)
        && required_capabilities.iter().any(|capability| {
            matches!(
                capability.as_str(),
                "browser_interaction"
                    | "playable_ui"
                    | "stateful_interaction"
                    | "player_control"
                    | "user_input_or_action"
                    | "visible_state_change"
                    | "adversary_or_challenge"
                    | "progression_or_score"
                    | "failure_or_collision_rule"
            )
        })
}

fn append_release_gate_observation_failures(
    report: &mut VerificationReport,
    release_gate: &ReleaseGateSummary,
) {
    if release_gate.browser_readiness_status != "not_checked"
        && release_gate.browser_readiness_status != "not_applicable"
    {
        report.push_profile_failure(format!(
            "browser readiness status: {}",
            release_gate.browser_readiness_status
        ));
    }
    if !release_gate.browser_readiness_evidence_path.is_empty() {
        report.push_profile_failure(format!(
            "browser readiness evidence: {}",
            release_gate.browser_readiness_evidence_path
        ));
    }
    if release_gate.interaction_evidence_status != "not_checked"
        && release_gate.interaction_evidence_status != "not_applicable"
    {
        report.push_profile_failure(format!(
            "interaction evidence status: {}",
            release_gate.interaction_evidence_status
        ));
    }
    if !release_gate.interaction_evidence_path.is_empty() {
        report.push_profile_failure(format!(
            "interaction evidence path: {}",
            release_gate.interaction_evidence_path
        ));
    }
}

fn external_contract_ok_after_runtime_arbitration(
    report: Option<&VerificationReport>,
    acceptance: Option<&RuntimeAcceptanceReport>,
) -> bool {
    report.is_none_or(|report| {
        report.is_pass()
            || external_contract_report_covered_by_runtime_arbitration(report, acceptance)
    })
}

fn external_contract_report_covered_by_runtime_arbitration(
    report: &VerificationReport,
    acceptance: Option<&RuntimeAcceptanceReport>,
) -> bool {
    let Some(acceptance) = acceptance.filter(|acceptance| acceptance.passed) else {
        return false;
    };
    report.missing_paths.is_empty()
        && report.command_failures.is_empty()
        && report.verifier_command_false_negatives.is_empty()
        && report.dependency_missing.is_empty()
        && report
            .profile_failures
            .iter()
            .all(|failure| external_profile_failure_covered_by_runtime(failure, acceptance))
}

fn external_profile_failure_covered_by_runtime(
    failure: &str,
    acceptance: &RuntimeAcceptanceReport,
) -> bool {
    if let Some(evidence) = failure.strip_prefix("missing_required_evidence:") {
        if evidence.starts_with("required_obligation:") {
            return false;
        }
        return evidence
            .split(',')
            .map(str::trim)
            .filter(|evidence| !evidence.is_empty())
            .all(|evidence| runtime_acceptance_satisfied_evidence(acceptance, evidence));
    }
    if let Some(weak_evidence) = failure.strip_prefix("weak_verification_evidence:") {
        return weak_evidence
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .all(|item| weak_source_evidence_covered_by_runtime(acceptance, item));
    }
    false
}

fn runtime_acceptance_satisfied_evidence(
    acceptance: &RuntimeAcceptanceReport,
    evidence: &str,
) -> bool {
    !acceptance
        .missing_evidence
        .iter()
        .any(|missing| missing == evidence)
        && acceptance
            .evidence_tiers
            .get(evidence)
            .is_some_and(|tier| tier != "absent" && tier != "weak")
}

fn weak_source_evidence_covered_by_runtime(
    acceptance: &RuntimeAcceptanceReport,
    weak_evidence: &str,
) -> bool {
    let Some(rest) = weak_evidence.strip_prefix("weak_source_evidence:") else {
        return false;
    };
    let Some((evidence, _reason)) = rest.split_once(':') else {
        return false;
    };
    runtime_acceptance_satisfied_evidence(acceptance, evidence.trim())
}

fn runtime_acceptance_repair_guidance(
    acceptance: &crate::minimal_loop::evidence::RuntimeAcceptanceReport,
) -> Vec<String> {
    let mut guidance = Vec::new();
    for evidence in &acceptance.missing_evidence {
        match evidence.as_str() {
            "restart_or_recoverable_state_evidence" => guidance.push(
                "provide a reachable terminal/failure state and a restart control (data-anvil-action=\"restart\") that resets observable state"
                    .to_string(),
            ),
            "persistence_evidence" => {
                guidance.push(PERSISTENCE_RELOAD_REPAIR_REQUIREMENT.to_string())
            }
            "live_preview_evidence" | "requested_content_evidence" => {
                guidance.push(TEXT_ECHO_REPAIR_REQUIREMENT.to_string())
            }
            "challenge_or_adversary_evidence" => guidance.push(
                "wire a reachable challenge/adversary entity into state evolution, not only a static label"
                    .to_string(),
            ),
            "failure_or_collision_evidence" => guidance.push(
                "wire a collision/failure conditional that transitions to a reachable failure state"
                    .to_string(),
            ),
            "score_or_progression_evidence" => guidance.push(
                "wire score/progression updates to meaningful state transitions, not only an isolated counter"
                    .to_string(),
            ),
            "stateful_update_evidence" => guidance.push(
                "mutate application state over time or in response to input"
                    .to_string(),
            ),
            "user_input_handler_evidence" => guidance.push(
                "wire keyboard, pointer, click, touch, or form handlers to gameplay state changes"
                    .to_string(),
            ),
            _ => {}
        }
    }
    for weak in &acceptance.weak_evidence {
        if let Some(reason) = weak.split(':').next_back()
            && !reason.trim().is_empty()
        {
            guidance.push(reason.trim().to_string());
        }
    }
    for diagnostic in &acceptance.diagnostics {
        if let Some(path) = diagnostic.strip_prefix("route_unbound_capability_artifact:") {
            for evidence in &acceptance.missing_evidence {
                guidance.push(format!(
                    "For missing evidence {evidence}, {path} contains capability code but is not route-bound; import it from the route page, or consolidate into page.tsx and delete the dead component"
                ));
            }
        }
    }
    dedup_strings(guidance)
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

#[cfg(test)]
fn browser_release_gate(config: &Config) -> ReleaseGateSummary {
    browser_release_gate_with_expectations(config, false)
}

#[cfg(test)]
fn browser_release_gate_with_expectations(
    config: &Config,
    canvas_surface_expected: bool,
) -> ReleaseGateSummary {
    browser_release_gate_with_options(
        config,
        canvas_surface_expected,
        BrowserInteractionProbeOptions::default(),
        None,
    )
}

fn browser_release_gate_with_options(
    config: &Config,
    canvas_surface_expected: bool,
    interaction_options: BrowserInteractionProbeOptions,
    requested_port: Option<u16>,
) -> ReleaseGateSummary {
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
        browser = nextjs_dev_route_release_evidence(config, interaction_options, requested_port);
    }
    let interaction = read_release_evidence(
        config,
        &[
            "browser-interaction.json",
            "interaction-evidence.json",
            "interaction.json",
        ],
        "interaction_evidence_missing",
        ReleaseEvidenceKind::Interaction,
    );
    let browser_status = browser.status.as_status();
    let interaction_status = interaction.status.as_status();
    let canvas_surface_missing =
        release_gate_canvas_surface_missing(canvas_surface_expected, &browser, &interaction);
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
        if interaction_probe_infrastructure_failure_reason(reason) {
            let mut reasons = vec![
                reason.clone(),
                format!("app interaction untested (probe infrastructure failure: {reason})"),
            ];
            if let Some(remediation) = interaction_probe_failure_remediation(&interaction.path) {
                reasons.push(remediation);
            } else if reason == "probe_dependency_missing:browser_binaries_missing" {
                reasons.push(
                    crate::minimal_loop::interaction_probe::INTERACTION_PROBE_SETUP_REMEDIATION
                        .to_string(),
                );
            }
            return ReleaseGateSummary {
                status: "failed".to_string(),
                reasons: dedup_strings(reasons),
                browser_readiness_status: browser_status,
                browser_readiness_evidence_path: browser.path,
                interaction_evidence_status: interaction_status,
                interaction_evidence_path: interaction.path,
            };
        }
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
        let mut reasons = Vec::new();
        if canvas_surface_missing {
            reasons.push(
                "browser_readiness_or_interaction_evidence_required:rendered_without_expected_surface"
                    .to_string(),
            );
        }
        if interaction_probe_unavailable_reason_value(reason) {
            if interaction_probe_performed_for_run(config) {
                reasons.push(
                    "browser_interaction_evidence_required:interaction_detail_missing".to_string(),
                );
                return ReleaseGateSummary {
                    status: "partial".to_string(),
                    reasons: dedup_strings(reasons),
                    browser_readiness_status: browser_status,
                    browser_readiness_evidence_path: browser.path,
                    interaction_evidence_status: interaction_status,
                    interaction_evidence_path: interaction.path,
                };
            }
            reasons.extend([
                "interaction_unverified:probe_unavailable".to_string(),
                crate::minimal_loop::interaction_probe::INTERACTION_PROBE_SETUP_REMEDIATION
                    .to_string(),
            ]);
            return ReleaseGateSummary {
                status: "partial".to_string(),
                reasons: dedup_strings(reasons),
                browser_readiness_status: browser_status,
                browser_readiness_evidence_path: browser.path,
                interaction_evidence_status: interaction_status,
                interaction_evidence_path: interaction.path,
            };
        }
        reasons.push(format!("browser_interaction_evidence_required:{reason}"));
        return ReleaseGateSummary {
            status: "partial".to_string(),
            reasons: dedup_strings(reasons),
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

fn nextjs_dev_route_release_evidence(
    config: &Config,
    interaction_options: BrowserInteractionProbeOptions,
    requested_port: Option<u16>,
) -> ReleaseEvidence {
    let path = nextjs_dev_route_evidence_path(config);
    let value = run_nextjs_dev_route_probe_with_interaction_options(
        config,
        &path,
        interaction_options,
        requested_port,
    );
    let status = classify_release_evidence_json(ReleaseEvidenceKind::BrowserReadiness, &value);
    write_release_evidence_json(&path, &value);
    ReleaseEvidence {
        status,
        path: path.display().to_string(),
    }
}

fn write_release_evidence_json(path: &Path, value: &Value) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(value) {
        let _ = std::fs::write(path, format!("{text}\n"));
    }
}

fn interaction_probe_infrastructure_failure_reason(reason: &str) -> bool {
    reason.starts_with("probe_dependency_missing")
        || reason.starts_with("probe_infrastructure_failed")
}

fn release_gate_has_interaction_probe_infrastructure_failure(
    release_gate: &ReleaseGateSummary,
) -> bool {
    release_gate
        .interaction_evidence_status
        .strip_prefix("failed:")
        .is_some_and(interaction_probe_infrastructure_failure_reason)
        || release_gate
            .reasons
            .iter()
            .any(|reason| interaction_probe_infrastructure_failure_reason(reason))
}

fn interaction_probe_failure_remediation(path: &str) -> Option<String> {
    let value = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())?;
    let details = value
        .get("browser_details")
        .or_else(|| value.get("details"))
        .filter(|value| value.is_object());
    text_field_deep(&value, details, &["remediation"]).filter(|remediation| !remediation.is_empty())
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

#[cfg(test)]
fn run_nextjs_dev_route_probe(config: &Config, evidence_path: &Path) -> Value {
    run_nextjs_dev_route_probe_with_interaction_options(
        config,
        evidence_path,
        BrowserInteractionProbeOptions::default(),
        None,
    )
}

fn run_nextjs_dev_route_probe_with_interaction_options(
    config: &Config,
    evidence_path: &Path,
    interaction_options: BrowserInteractionProbeOptions,
    requested_port: Option<u16>,
) -> Value {
    run_nextjs_dev_route_probe_with_runtime(
        config,
        evidence_path,
        dev_server_probe_runtime_enabled(config),
        cleanup_dev_server_child,
        interaction_options,
        requested_port,
    )
}

type DevServerCleanupFn = fn(Child, &DevServerLogPaths) -> DevServerCleanup;

fn run_nextjs_dev_route_probe_with_runtime(
    config: &Config,
    evidence_path: &Path,
    runtime_enabled: bool,
    cleanup_fn: DevServerCleanupFn,
    interaction_options: BrowserInteractionProbeOptions,
    requested_port: Option<u16>,
) -> Value {
    if !runtime_enabled {
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

    let spec = match load_nextjs_dev_server_probe_spec(&config.workspace_root, requested_port) {
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
        let output_excerpt = dev_server_output_excerpt_for_port(failure_kind, "", spec.port);
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
            &output_excerpt,
        );
    }

    let (logs, stdout_log, stderr_log) = match open_dev_server_log_files(evidence_path) {
        Ok(logs) => logs,
        Err(err) => {
            let failure_kind = "browser_unavailable:dev_server_log_open_failed";
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
                &err.to_string(),
            );
        }
    };

    let mut command =
        verifier_env::normalized_command_at_root(&spec.package_manager, &config.workspace_root);
    command
        .args(&spec.args)
        .current_dir(&config.workspace_root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_log))
        .stderr(Stdio::from(stderr_log))
        .env("PORT", spec.port.to_string());
    let mut child = match bounded_process::spawn_child(&mut command) {
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
                let output_excerpt = dev_server_logs_excerpt(&logs)
                    .unwrap_or_else(|| "dev server exited before readiness".to_string());
                let failure_kind = classify_dev_server_startup_failure(&output_excerpt)
                    .unwrap_or_else(|| "browser_unavailable:dev_server_exited".to_string());
                let failure_kind = classify_dev_server_env_conflict(&failure_kind, &output_excerpt);
                let output_excerpt =
                    dev_server_output_excerpt_for_port(&failure_kind, &output_excerpt, spec.port);
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
                let evidence = dev_server_unavailable_evidence(
                    spec.port,
                    &spec.route,
                    &spec.command_display,
                    &failure_kind,
                    &output_excerpt,
                );
                write_release_evidence_json(evidence_path, &evidence);
                let cleanup = cleanup_fn(child, &logs);
                emit_dev_server_cleanup_lifecycle_stage(
                    config,
                    cleanup.ok,
                    spec.port,
                    &spec.route,
                    &spec.command_display,
                    Some(&failure_kind),
                    None,
                    evidence_path,
                    Some(pid),
                    &cleanup,
                );
                return evidence;
            }
            Ok(None) => {}
            Err(err) => {
                let failure_kind = "browser_unavailable:dev_server_status_unreadable";
                let log_excerpt = dev_server_logs_excerpt(&logs).unwrap_or_default();
                let combined = format!("{} {}", err, log_excerpt);
                let failure_kind = classify_dev_server_env_conflict(failure_kind, &combined);
                let output_excerpt =
                    dev_server_output_excerpt_for_port(&failure_kind, &combined, spec.port);
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
                let evidence = dev_server_unavailable_evidence(
                    spec.port,
                    &spec.route,
                    &spec.command_display,
                    &failure_kind,
                    &output_excerpt,
                );
                write_release_evidence_json(evidence_path, &evidence);
                let cleanup = cleanup_fn(child, &logs);
                emit_dev_server_cleanup_lifecycle_stage(
                    config,
                    cleanup.ok,
                    spec.port,
                    &spec.route,
                    &spec.command_display,
                    Some(&failure_kind),
                    None,
                    evidence_path,
                    Some(pid),
                    &cleanup,
                );
                return evidence;
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
                let log_excerpt = dev_server_logs_excerpt(&logs).unwrap_or_default();
                let failure_kind = failure_kind.map(|kind| {
                    let combined = format!("{}\n{}", response.body_excerpt, log_excerpt);
                    classify_dev_server_env_conflict(&kind, &combined)
                });
                let body_excerpt = failure_kind
                    .as_deref()
                    .map(|kind| {
                        dev_server_output_excerpt_for_port(kind, &response.body_excerpt, spec.port)
                    })
                    .unwrap_or_else(|| response.body_excerpt.clone());
                let output_excerpt = failure_kind
                    .as_deref()
                    .map(|kind| dev_server_output_excerpt_for_port(kind, &log_excerpt, spec.port))
                    .unwrap_or_else(|| log_excerpt.clone());
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
                let evidence = if let Some(failure_kind) = failure_kind.as_deref() {
                    dev_server_failed_evidence(
                        spec.port,
                        &spec.route,
                        &spec.command_display,
                        response.status,
                        failure_kind,
                        &body_excerpt,
                        &output_excerpt,
                    )
                } else {
                    dev_server_passed_evidence(
                        spec.port,
                        &spec.route,
                        &spec.command_display,
                        response.status,
                        &response.body_excerpt,
                    )
                };
                write_release_evidence_json(evidence_path, &evidence);
                if failure_kind.is_none() {
                    let interaction_path = evidence_path.with_file_name("browser-interaction.json");
                    let run_dir = evidence_path.parent().unwrap_or(&config.workspace_root);
                    let interaction =
                        interaction_probe::probe_browser_interaction_against_running_server_with_options(
                            &config.workspace_root,
                            spec.port,
                            run_dir,
                            &interaction_path,
                            Duration::from_secs(120),
                            interaction_options,
                        );
                    emit_browser_interaction_probe_event(config, &interaction);
                }
                let cleanup = cleanup_fn(child, &logs);
                emit_dev_server_cleanup_lifecycle_stage(
                    config,
                    cleanup.ok,
                    spec.port,
                    &spec.route,
                    &spec.command_display,
                    failure_kind.as_deref(),
                    Some(response.status),
                    evidence_path,
                    Some(pid),
                    &cleanup,
                );
                return evidence;
            }
            Err(_) => {
                std::thread::sleep(NEXTJS_DEV_SERVER_WAIT_INTERVAL);
            }
        }
    }

    let failure_kind = "startup_timeout";
    let log_excerpt = dev_server_logs_excerpt(&logs).unwrap_or_default();
    let failure_kind = classify_dev_server_env_conflict(failure_kind, &log_excerpt);
    let output_excerpt = dev_server_output_excerpt_for_port(&failure_kind, &log_excerpt, spec.port);
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
    let evidence = dev_server_unavailable_evidence(
        spec.port,
        &spec.route,
        &spec.command_display,
        &failure_kind,
        &output_excerpt,
    );
    write_release_evidence_json(evidence_path, &evidence);
    let cleanup = cleanup_fn(child, &logs);
    emit_dev_server_cleanup_lifecycle_stage(
        config,
        cleanup.ok,
        spec.port,
        &spec.route,
        &spec.command_display,
        Some(&failure_kind),
        None,
        evidence_path,
        Some(pid),
        &cleanup,
    );
    evidence
}

fn dev_server_probe_runtime_enabled(config: &Config) -> bool {
    if env_flag_is_false("ANVIL_DEV_SERVER_PROBE") {
        return false;
    }
    if cfg!(test)
        && !env_flag_is_true("ANVIL_TEST_DEV_SERVER_PROBE")
        && !config
            .workspace_root
            .join(".anvil")
            .join("enable-dev-server-probe-tests")
            .is_file()
    {
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

fn load_nextjs_dev_server_probe_spec(
    root: &Path,
    requested_port: Option<u16>,
) -> Result<NextjsDevServerProbeSpec, String> {
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
    let port = requested_port.unwrap_or(NEXTJS_DEV_SERVER_DEFAULT_PORT);
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

#[cfg(test)]
fn dev_server_output_excerpt(failure_kind: &str, output: &str) -> String {
    dev_server_output_excerpt_for_port(failure_kind, output, NEXTJS_DEV_SERVER_DEFAULT_PORT)
}

fn dev_server_output_excerpt_for_port(failure_kind: &str, output: &str, port: u16) -> String {
    if failure_kind == verifier_env::ENV_NODE_ENV_CONFLICT_KIND {
        verifier_env::with_env_node_env_remediation(output)
    } else if failure_kind == "port_in_use" {
        port_in_use_remediation(output, port)
    } else {
        output.to_string()
    }
}

fn port_in_use_remediation(output: &str, port: u16) -> String {
    let remediation = format!(
        "Port {port} is already accepting connections. This may be a leftover dev server from a previous run. Inspect it with `lsof -nP -iTCP:{port} -sTCP:LISTEN` and stop the stale process before retrying."
    );
    if output.trim().is_empty() {
        remediation
    } else {
        format!("{output}\n{remediation}")
    }
}

fn tailwind_dev_pipeline_failure(lower_text: &str) -> bool {
    lower_text.contains("@tailwind")
        && (lower_text.contains("module parse failed")
            || lower_text.contains("unexpected character")
            || lower_text.contains("postcss")
            || lower_text.contains("tailwind"))
}

#[derive(Debug, Clone)]
struct DevServerLogPaths {
    stdout: PathBuf,
    stderr: PathBuf,
}

fn open_dev_server_log_files(
    evidence_path: &Path,
) -> std::io::Result<(DevServerLogPaths, std::fs::File, std::fs::File)> {
    let dir = evidence_path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let paths = DevServerLogPaths {
        stdout: dir.join("dev-server.out"),
        stderr: dir.join("dev-server.err"),
    };
    let stdout = std::fs::File::create(&paths.stdout)?;
    let stderr = std::fs::File::create(&paths.stderr)?;
    Ok((paths, stdout, stderr))
}

fn dev_server_logs_excerpt(paths: &DevServerLogPaths) -> Option<String> {
    let stdout = read_dev_server_log_excerpt(&paths.stdout).unwrap_or_default();
    let stderr = read_dev_server_log_excerpt(&paths.stderr).unwrap_or_default();
    let combined = format!("{stdout}\n{stderr}");
    let excerpt = eval_events::body_snippet(combined.trim());
    if excerpt.trim().is_empty() {
        None
    } else {
        Some(excerpt)
    }
}

fn read_dev_server_log_excerpt(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    let start = bytes.len().saturating_sub(DEV_SERVER_LOG_EXCERPT_BYTES);
    Ok(String::from_utf8_lossy(&bytes[start..]).to_string())
}

#[derive(Debug)]
struct DevServerCleanup {
    ok: bool,
    failure_kind: Option<String>,
    output_excerpt: String,
}

fn cleanup_dev_server_child(mut child: Child, logs: &DevServerLogPaths) -> DevServerCleanup {
    #[cfg(unix)]
    {
        cleanup_dev_server_child_unix(&mut child, logs)
    }
    #[cfg(not(unix))]
    {
        cleanup_dev_server_child_non_unix(&mut child, logs)
    }
}

#[cfg(unix)]
fn cleanup_dev_server_child_unix(child: &mut Child, logs: &DevServerLogPaths) -> DevServerCleanup {
    let pid = child.id();
    let mut notes = Vec::new();
    if let Err(err) = signal_dev_server_process_group(pid, libc::SIGTERM) {
        notes.push(format!("SIGTERM process group failed: {err}"));
    }
    match wait_for_dev_server_process_group_exit(
        child,
        pid,
        Instant::now() + DEV_SERVER_CLEANUP_TERM_TIMEOUT,
    ) {
        Ok(true) => {
            return DevServerCleanup {
                ok: true,
                failure_kind: None,
                output_excerpt: dev_server_logs_excerpt(logs).unwrap_or_default(),
            };
        }
        Ok(false) => {}
        Err(err) => notes.push(format!("wait after SIGTERM failed: {err}")),
    }

    if let Err(err) = signal_dev_server_process_group(pid, libc::SIGKILL) {
        notes.push(format!("SIGKILL process group failed: {err}"));
    }
    match wait_for_dev_server_process_group_exit(
        child,
        pid,
        Instant::now() + DEV_SERVER_CLEANUP_KILL_TIMEOUT,
    ) {
        Ok(true) => DevServerCleanup {
            ok: true,
            failure_kind: None,
            output_excerpt: dev_server_logs_excerpt(logs).unwrap_or_default(),
        },
        Ok(false) => DevServerCleanup {
            ok: false,
            failure_kind: Some("dev_server_cleanup_timeout".to_string()),
            output_excerpt: cleanup_timeout_excerpt(logs, &notes),
        },
        Err(err) => {
            notes.push(format!("wait after SIGKILL failed: {err}"));
            DevServerCleanup {
                ok: false,
                failure_kind: Some("dev_server_cleanup_timeout".to_string()),
                output_excerpt: cleanup_timeout_excerpt(logs, &notes),
            }
        }
    }
}

#[cfg(unix)]
fn signal_dev_server_process_group(pid: u32, signal: libc::c_int) -> std::io::Result<()> {
    let pid =
        i32::try_from(pid).map_err(|_| std::io::Error::other("child pid does not fit pid_t"))?;
    // SAFETY: `kill` is called with a process-group id derived from the child
    // pid returned by `std::process::Child` and a libc signal constant.
    let rc = unsafe { libc::kill(-pid, signal) };
    if rc == 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(err)
    }
}

#[cfg(unix)]
fn wait_for_dev_server_process_group_exit(
    child: &mut Child,
    pid: u32,
    deadline: Instant,
) -> std::io::Result<bool> {
    let mut child_exited = false;
    loop {
        if !child_exited && child.try_wait()?.is_some() {
            let _ = child.wait();
            child_exited = true;
        }
        if child_exited && !dev_server_process_group_exists(pid) {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(unix)]
fn dev_server_process_group_exists(pgid: u32) -> bool {
    let Ok(pgid) = i32::try_from(pgid) else {
        return false;
    };
    // SAFETY: signal 0 performs existence/permission checking only, using
    // a process-group id derived from a child process spawned by this probe.
    let rc = unsafe { libc::kill(-pgid, 0) };
    if rc == 0 {
        return true;
    }
    let err = std::io::Error::last_os_error();
    err.raw_os_error() != Some(libc::ESRCH)
}

#[cfg(not(unix))]
fn cleanup_dev_server_child_non_unix(
    child: &mut Child,
    logs: &DevServerLogPaths,
) -> DevServerCleanup {
    let _ = child.kill();
    match wait_for_dev_server_child_exit(child, Instant::now() + DEV_SERVER_CLEANUP_TERM_TIMEOUT) {
        Ok(true) => DevServerCleanup {
            ok: true,
            failure_kind: None,
            output_excerpt: dev_server_logs_excerpt(logs).unwrap_or_default(),
        },
        Ok(false) | Err(_) => DevServerCleanup {
            ok: false,
            failure_kind: Some("dev_server_cleanup_timeout".to_string()),
            output_excerpt: cleanup_timeout_excerpt(logs, &[]),
        },
    }
}

#[cfg(not(unix))]
fn wait_for_dev_server_child_exit(child: &mut Child, deadline: Instant) -> std::io::Result<bool> {
    loop {
        if child.try_wait()?.is_some() {
            let _ = child.wait();
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn cleanup_timeout_excerpt(logs: &DevServerLogPaths, notes: &[String]) -> String {
    let mut parts = notes.to_vec();
    if let Some(log_excerpt) = dev_server_logs_excerpt(logs) {
        parts.push(log_excerpt);
    }
    if parts.is_empty() {
        "dev server cleanup timed out".to_string()
    } else {
        eval_events::body_snippet(&parts.join("\n"))
    }
}

fn cleanup_stage_failure_kind<'a>(
    original_failure_kind: Option<&'a str>,
    cleanup: &'a DevServerCleanup,
) -> Option<&'a str> {
    if !cleanup.ok {
        cleanup.failure_kind.as_deref().or(original_failure_kind)
    } else {
        original_failure_kind
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_dev_server_cleanup_lifecycle_stage(
    config: &Config,
    ok: bool,
    port: u16,
    route: &str,
    command: &str,
    failure_kind: Option<&str>,
    http_status: Option<i64>,
    evidence_path: &Path,
    pid: Option<u32>,
    cleanup: &DevServerCleanup,
) {
    let stage_failure_kind = cleanup_stage_failure_kind(failure_kind, cleanup);
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "dev_server_lifecycle",
            "cycle_index": current_final_acceptance_cycle_index(),
            "profile": "nextjs",
            "stage": "cleanup",
            "ok": ok,
            "port": port,
            "route": route,
            "command": command,
            "failure_kind": stage_failure_kind.unwrap_or(""),
            "http_status": http_status,
            "pid": pid,
            "evidence_path": evidence_path.display().to_string(),
            "output_excerpt": eval_events::body_snippet(&cleanup.output_excerpt),
            "lifecycle_stages": DEV_SERVER_LIFECYCLE_STAGES,
            "probe_environment": dev_server_probe_environment(port),
        }),
    );
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
            "cycle_index": current_final_acceptance_cycle_index(),
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
    let mut value = json!({
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
    });
    add_surface_markers_to_evidence(&mut value, body_excerpt);
    value
}

fn dev_server_passed_evidence(
    port: u16,
    route: &str,
    command: &str,
    http_status: i64,
    body_excerpt: &str,
) -> Value {
    let mut value = json!({
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
    });
    add_surface_markers_to_evidence(&mut value, body_excerpt);
    value
}

fn add_surface_markers_to_evidence(value: &mut Value, body_excerpt: &str) {
    let markers = html_surface_markers_json(body_excerpt);
    for key in [
        "ssr_has_canvas",
        "ssr_interactive_control_count",
        "has_canvas",
        "interactive_control_count",
        "title_text_excerpt",
        "surface_marker_authority",
        "route_rendered_quality",
    ] {
        value[key] = markers.get(key).cloned().unwrap_or(Value::Null);
    }
}

fn release_evidence_canvas_marker_is_false(path: &str) -> bool {
    let value = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    value.as_ref().is_some_and(|value| {
        let details = value
            .get("browser_details")
            .or_else(|| value.get("details"))
            .filter(|value| value.is_object());
        bool_field_deep(value, details, &["ssr_has_canvas", "has_canvas"]) == Some(false)
    })
}

fn release_gate_canvas_surface_missing(
    expected: bool,
    browser: &ReleaseEvidence,
    interaction: &ReleaseEvidence,
) -> bool {
    if !expected {
        return false;
    }
    if release_interaction_surface_authoritative(interaction) {
        return release_interaction_canvas_marker(&interaction.path) == Some(false);
    }
    matches!(browser.status, ReleaseEvidenceStatus::Passed)
        && release_evidence_canvas_marker_is_false(&browser.path)
}

fn release_interaction_surface_authoritative(interaction: &ReleaseEvidence) -> bool {
    if interaction.path.is_empty() {
        return false;
    }
    match &interaction.status {
        ReleaseEvidenceStatus::Passed => true,
        ReleaseEvidenceStatus::Failed(reason) => {
            !interaction_probe_infrastructure_failure_reason(reason)
        }
        ReleaseEvidenceStatus::Unavailable(_) => false,
    }
}

fn release_interaction_canvas_marker(path: &str) -> Option<bool> {
    let value = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())?;
    let details = value
        .get("browser_details")
        .or_else(|| value.get("details"))
        .filter(|value| value.is_object());
    if let Some(has_canvas) = bool_field_deep(
        &value,
        details,
        &[
            "post_js_has_canvas",
            "has_canvas",
            "canvas_found",
            "canvas_available",
        ],
    ) {
        return Some(has_canvas);
    }
    numeric_field_deep(&value, details, &["post_js_canvas_count", "canvas_count"])
        .map(|count| count > 0)
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
    if kind == ReleaseEvidenceKind::Interaction
        && let Some(reason) = interaction_probe_unavailable_reason(&config.workspace_root)
    {
        return ReleaseEvidence {
            status: ReleaseEvidenceStatus::Unavailable(reason),
            path: String::new(),
        };
    }
    ReleaseEvidence {
        status: ReleaseEvidenceStatus::Unavailable(missing_reason.to_string()),
        path: String::new(),
    }
}

fn interaction_probe_unavailable_reason(root: &Path) -> Option<String> {
    interaction_probe::playwright_availability(root)
        .unavailable_reason()
        .map(str::to_string)
}

fn interaction_probe_unavailable_reason_value(reason: &str) -> bool {
    matches!(reason, "playwright_not_installed" | "probe_unavailable")
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

fn clear_final_acceptance_browser_probe_evidence(config: &Config) {
    for path in release_evidence_candidate_paths(
        config,
        &[
            "browser-readiness.json",
            "browser.json",
            "browser-readiness-evidence.json",
            "browser-interaction.json",
        ],
    ) {
        let _ = std::fs::remove_file(path);
    }
}

fn final_acceptance_evidence_arbitration(
    config: &Config,
    report: &mut RuntimeAcceptanceReport,
    required_capabilities: &[String],
    required_evidence: &[String],
    required_obligations: &[String],
) -> EvidenceArbitrationReport {
    behavior_evidence::arbitrate_final_acceptance(
        report,
        &config.workspace_root,
        &release_evidence_extra_dirs(config),
        required_capabilities,
        required_evidence,
        required_obligations,
    )
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
            let transition_observed =
                bool_field_deep(value, details, &["start_transition", "transition_observed"])
                    == Some(true)
                    || string_array_field_contains_deep(
                        value,
                        details,
                        "steps",
                        "start_transition",
                    )
                    || string_array_field_contains_deep(
                        value,
                        details,
                        "steps",
                        "recovery_transition",
                    );
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
                if !transition_observed {
                    return Some("start_transition_missing".to_string());
                }
                if bool_field_deep(value, details, &["input_state_evaluated_after_start"])
                    == Some(false)
                {
                    return Some("input_state_change_not_evaluated_after_start".to_string());
                }
                return Some("input_state_change_missing_after_start".to_string());
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
            let transition_observed =
                bool_field_deep(value, details, &["start_transition", "transition_observed"])
                    == Some(true)
                    || string_array_field_contains_deep(
                        value,
                        details,
                        "steps",
                        "start_transition",
                    )
                    || string_array_field_contains_deep(
                        value,
                        details,
                        "steps",
                        "recovery_transition",
                    );
            let input_state_changed = bool_field_deep(
                value,
                details,
                &[
                    "input_state_change",
                    "state_changed",
                    "visible_state_changed",
                ],
            ) == Some(true)
                || string_array_field_contains_deep(value, details, "steps", "input_state_change");
            transition_observed && input_state_changed
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

fn string_array_field_contains_deep(
    value: &Value,
    details: Option<&Value>,
    key: &str,
    needle: &str,
) -> bool {
    string_array_field_contains(value, key, needle)
        || details.is_some_and(|details| string_array_field_contains(details, key, needle))
}

fn string_array_field_contains(value: &Value, key: &str, needle: &str) -> bool {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|item| item == needle)
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
        Some(report) if !report.unverified_evidence.is_empty() => "partial",
        Some(_) if runtime_ok => "pass",
        Some(_) => "failed",
        None => "not_checked",
    }
}

fn assurance_for_completion(
    profile: &str,
    required_capabilities: &[String],
) -> (&'static str, &'static str) {
    if canonical_profile_name(profile) == "generic" {
        if required_capabilities
            .iter()
            .any(|capability| capability == GENERIC_INTERACTIVE_CONTRACT_CAPABILITY)
        {
            ("static", eval_events::GENERIC_STATIC_ASSURANCE_REASON)
        } else {
            ("reduced", eval_events::GENERIC_REDUCED_ASSURANCE_REASON)
        }
    } else {
        ("full", "")
    }
}

fn earned_assurance_for_completion(
    profile: &str,
    required_capabilities: &[String],
    contract_bound: bool,
    final_acceptance_status: &str,
    release_gate: &ReleaseGateSummary,
    gate_telemetry: &AcceptanceGateTelemetry,
) -> (String, String) {
    let (base_level, base_reason) = assurance_for_completion(profile, required_capabilities);
    if base_level != "full" {
        return (base_level.to_string(), base_reason.to_string());
    }
    if final_acceptance_status == "partial" || release_gate.status == "partial" {
        return (
            "partial".to_string(),
            release_gate
                .reasons
                .first()
                .cloned()
                .unwrap_or_else(|| "acceptance_partial".to_string()),
        );
    }
    if final_acceptance_status != "full_success" || release_gate.status == "failed" {
        return (
            "reduced".to_string(),
            release_gate
                .reasons
                .first()
                .cloned()
                .unwrap_or_else(|| "acceptance_not_full_success".to_string()),
        );
    }
    if canonical_profile_name(profile).is_empty() {
        return (
            "partial".to_string(),
            "effective_profile_unknown".to_string(),
        );
    }
    if !contract_bound {
        return (
            "partial".to_string(),
            "completion_contract_not_bound".to_string(),
        );
    }
    if gate_telemetry.browser_readiness_applicable
        && gate_telemetry.browser_readiness_execution_status != "performed"
    {
        return (
            "partial".to_string(),
            format!(
                "browser_readiness_not_performed:{}",
                gate_telemetry.browser_readiness_execution_status
            ),
        );
    }
    if gate_telemetry.interaction_evidence_applicable
        && gate_telemetry.interaction_evidence_execution_status != "performed"
    {
        return (
            "partial".to_string(),
            format!(
                "interaction_evidence_not_performed:{}",
                gate_telemetry.interaction_evidence_execution_status
            ),
        );
    }
    ("full".to_string(), String::new())
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
            .any(|reason| reason.contains("interaction_unverified:probe_unavailable"))
        {
            return "interaction_unverified_probe_unavailable".to_string();
        }
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

fn final_acceptance_app_behavior_failure_kind(report: &VerificationReport) -> Option<String> {
    let primary_reason = report.primary_reason();
    report
        .profile_failures
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(primary_reason.as_str()))
        .find_map(app_behavior_probe_failure_kind)
}

fn app_behavior_probe_failure_kind(reason: &str) -> Option<String> {
    let lower = reason.to_ascii_lowercase();
    APP_BEHAVIOR_PROBE_FAILURE_KINDS
        .iter()
        .find(|kind| lower.contains(**kind))
        .map(|kind| format!("browser_interaction_failed:{kind}"))
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
        evidence.extend(interaction_probe_failure_evidence_lines(
            &release_gate.interaction_evidence_path,
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
            runtime_acceptance_repair_guidance(report)
                .into_iter()
                .map(|item| format!("runtime repair guidance: {item}")),
        );
        evidence.extend(
            report
                .unverified_evidence
                .iter()
                .map(|item| format!("unverified runtime evidence: {item}")),
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

fn interaction_probe_failure_evidence_lines(path: &str) -> Vec<String> {
    let Some(value) = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
    else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    if let Some(cold_start_ms) = raw_u64_field_deep(&value, "cold_start_ms")
        && cold_start_ms > 10_000
    {
        let seconds = (cold_start_ms + 500) / 1000;
        lines.push(format!(
            "Note: first page load took {seconds}s (cold start; excluded from assertions)"
        ));
    }
    if let Some(mode) = raw_text_field_deep(&value, &["probe_mode"]).filter(|mode| !mode.is_empty())
    {
        lines.push(format!("interaction probe mode: {mode}"));
    }
    if let Some(status) =
        raw_text_field_deep(&value, &["contract_hook_status"]).filter(|status| !status.is_empty())
    {
        lines.push(format!("interaction contract hook status: {status}"));
    }
    if let Some(restart_present) = raw_contract_hook_bool(&value, "restart_present") {
        lines.push(format!(
            "interaction restart hook present: {restart_present}"
        ));
    }
    let inputs = raw_string_array_field_deep(&value, "input_dispatches");
    if !inputs.is_empty() {
        lines.push(format!(
            "interaction redispatched inputs: {}",
            inputs.join(", ")
        ));
    }
    let state_dimensions = raw_string_array_field_deep(&value, "state_dimensions_changed");
    if !state_dimensions.is_empty() {
        lines.push(format!(
            "interaction state dimensions changed: {}",
            state_dimensions.join(", ")
        ));
    }
    let info = raw_string_array_field_deep(&value, "informational_failure_kinds");
    if !info.is_empty() {
        lines.push(format!(
            "interaction informational findings: {}",
            info.join(", ")
        ));
    }
    lines.extend(
        interaction_candidate_prompt_lines(&value)
            .into_iter()
            .map(|line| format!("interaction candidate table: {line}")),
    );
    lines
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
    let interaction_probe_unavailable = release_gate
        .reasons
        .iter()
        .any(|reason| reason.contains("interaction_unverified:probe_unavailable"));
    let interaction_probe_infrastructure =
        release_gate_has_interaction_probe_infrastructure_failure(release_gate);
    if browser_status.contains("tailwind_dev_pipeline_failure")
        || browser_status.contains("css")
        || browser_status.contains("http_500")
    {
        targets.push("framework_config".to_string());
    }
    if browser_status.starts_with("unavailable:")
        || browser_status.contains("evidence_missing")
        || (!interaction_probe_unavailable
            && (interaction_status.starts_with("unavailable:")
                || interaction_status.contains("evidence_missing")))
    {
        targets.push("required_evidence_missing".to_string());
    }
    if browser_status.starts_with("failed:") {
        targets.push("test_or_evidence".to_string());
    }
    if interaction_status.starts_with("failed:") && !interaction_probe_infrastructure {
        targets.extend(interaction_repair_targets_for_reason(&interaction_status));
    }
    if let Some(report) = runtime_acceptance {
        targets.extend(
            report
                .missing_evidence
                .iter()
                .filter(|evidence| behavior_depth_evidence_key(evidence))
                .map(|evidence| format!("implementation:{evidence}")),
        );
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
        if release_gate
            .reasons
            .iter()
            .any(|reason| reason.contains("interaction_unverified:probe_unavailable"))
        {
            commands.push(
                crate::minimal_loop::interaction_probe::INTERACTION_PROBE_SETUP_REMEDIATION
                    .to_string(),
            );
        } else if release_gate_has_interaction_probe_infrastructure_failure(release_gate) {
            commands.push(
                "fix the interaction probe infrastructure before rerunning release checks"
                    .to_string(),
            );
            if release_gate
                .reasons
                .iter()
                .any(|reason| reason.contains("probe_dependency_missing:browser_binaries_missing"))
            {
                commands.push(
                    crate::minimal_loop::interaction_probe::INTERACTION_PROBE_SETUP_REMEDIATION
                        .to_string(),
                );
            }
        } else {
            commands
                .push("run the interaction probe and record browser-interaction.json".to_string());
        }
    } else {
        commands.push("rerun deterministic acceptance checks for the original goal".to_string());
    }
    if release_gate.status == "partial" {
        commands.push("do not claim release_ready until release gate evidence passes".to_string());
    }
    dedup_strings(commands)
}

fn interaction_repair_targets_for_reason(reason: &str) -> Vec<String> {
    let lower = reason.to_ascii_lowercase();
    if lower.contains("input_state_change_missing_after_start")
        || lower.contains("input_state_change_not_evaluated_after_start")
        || lower.contains("interaction_state_change_missing")
        || lower.contains("text_input_state_change_missing")
    {
        vec!["input_state_render_wiring".to_string()]
    } else if lower.contains("token_echo_after_reload_only") || lower.contains("token_echo_missing")
    {
        vec!["live_preview_render_wiring".to_string()]
    } else if lower.contains("text_entry_missing") {
        vec!["text_input_wiring".to_string()]
    } else if lower.contains("persistence_after_reload_reset") {
        vec!["persistence_state_wiring".to_string()]
    } else if lower.contains("start_transition_missing")
        || lower.contains("primary_start_transition_missing")
    {
        vec!["start_control_wiring".to_string()]
    } else {
        vec!["capability_implementation".to_string()]
    }
}

fn behavior_depth_evidence_key(evidence: &str) -> bool {
    matches!(
        evidence,
        "challenge_or_adversary_evidence"
            | "failure_or_collision_evidence"
            | "score_or_progression_evidence"
            | "restart_or_recoverable_state_evidence"
            | "persistence_evidence"
            | "live_preview_evidence"
    )
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
) -> Option<eval_events::StopReasonParts> {
    save_ultra_phase_recovery_handoff_with_evidence(config, plan, phase, request, &[])
}

fn save_ultra_phase_recovery_handoff_with_evidence(
    config: &Config,
    plan: &UltraPlan,
    phase: &UltraPhase,
    request: UltraPhaseRecoveryRequest<'_>,
    failure_evidence: &[String],
) -> Option<eval_events::StopReasonParts> {
    let handoff = RecoveryHandoff {
        profile: plan.profile.clone(),
        original_goal: plan.goal.clone(),
        failed_phase: Some(phase.id.clone()),
        failed_step: None,
        failure_kind: request.failure_kind.to_string(),
        failure_evidence: if failure_evidence.is_empty() {
            vec![request.reason.to_string()]
        } else {
            failure_evidence.to_vec()
        },
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
            return Some(eval_events::StopReasonParts::free_text(format!(
                "recovery prompt save failed: {err}"
            )));
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
            browser_evidence_missing_note: browser_evidence_missing_before_final_acceptance_note(
                config,
            ),
        }),
    );
    let prompt_message = if validation.prompt_command_available() {
        format!("suggested command: {prompt_command}")
    } else {
        "suggested command unavailable because recovery prompt validation failed".to_string()
    };
    let recovery_yaml_command_message = recovery_plan_command
        .as_ref()
        .map(|command| format!("suggested YAML command: {command}"))
        .unwrap_or_else(|| {
            "suggested YAML command unavailable because recovery YAML is missing".to_string()
        });
    Some(eval_events::StopReasonParts {
        free_text: format!("incomplete; {artifact_check_summary}"),
        paths: vec![
            format!("repair prompt saved: {prompt_path}"),
            recovery_yaml_summary,
        ],
        commands: vec![prompt_message, recovery_yaml_command_message],
    })
}

fn render_failure_stop_reason(
    free_text: impl Into<String>,
    handoff: Option<eval_events::StopReasonParts>,
) -> String {
    let free_text = free_text.into();
    let mut parts = handoff.unwrap_or_default();
    parts.free_text = if parts.free_text.trim().is_empty() {
        free_text
    } else {
        format!("{free_text}; {}", parts.free_text)
    };
    eval_events::render_stop_reason(&parts)
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
    browser_evidence_missing_note: Option<&'a str>,
}

const BROWSER_EVIDENCE_MISSING_BEFORE_FINAL_ACCEPTANCE: &str = "Browser evidence missing: run failed before final acceptance (interaction probe installed but not exercised).";

fn render_ultra_partial_run_summary(summary: UltraPartialRunSummary<'_>) -> String {
    let browser_evidence_missing = summary
        .browser_evidence_missing_note
        .map(|note| format!("\n\n{note}"))
        .unwrap_or_default();
    format!(
        "Status: incomplete\n\n\
Completed phases:\n{}\n\n\
Failed phase:\n- {} ({})\n\n\
Pending phases:\n{}\n\n\
Recovery next action:\n- {}\n- Recovery prompt saved: {}\n- {}\n- {}\n- {}\n\n\
Failure:\n{}{}",
        render_summary_bullets(summary.completed_phases),
        summary.failed_phase,
        summary.failure_kind,
        render_summary_bullets(summary.pending_phases),
        summary.recovery_yaml_summary,
        summary.recovery_prompt_path,
        summary.prompt_command_summary,
        summary.recovery_yaml_command_summary,
        summary.recovery_artifact_check,
        eval_events::render_stop_reason_text(summary.reason),
        browser_evidence_missing,
    )
}

fn browser_evidence_missing_before_final_acceptance_note(config: &Config) -> Option<&'static str> {
    if interaction_probe_performed_for_run(config) {
        return None;
    }
    match interaction_probe::playwright_availability(&config.workspace_root) {
        interaction_probe::ProbeAvailability::Available(_) => {
            Some(BROWSER_EVIDENCE_MISSING_BEFORE_FINAL_ACCEPTANCE)
        }
        interaction_probe::ProbeAvailability::Unavailable(_) => None,
    }
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

fn emit_planner_plan_sanitized(
    config: &Config,
    provider: &str,
    model: &str,
    attempt: usize,
    report: &SanitizerReport,
) {
    if report.is_empty() {
        return;
    }
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "planner_plan_sanitized",
            "planner_stage": "sanitize",
            "planner_provider": provider,
            "planner_model": model,
            "repair_attempt": attempt,
            "actions": report.goal_truncations.iter().map(|record| json!({
                "kind": &record.kind,
                "original_len": record.original_len,
                "new_len": record.new_len,
            })).chain(report.shell_control_splits.iter().map(|record| json!({
                "kind": &record.kind,
                "step_id": &record.step_id,
                "original_command": eval_events::body_snippet(&record.original_command),
                "fragments": record.fragments.iter().map(|fragment| eval_events::body_snippet(fragment)).collect::<Vec<_>>(),
                "dropped_fallback": record.dropped_fallback.as_deref().map(eval_events::body_snippet),
            }))).chain(report.setup_verify_relocations.iter().map(|record| json!({
                "kind": "setup_verify_relocated",
                "from_step_id": &record.from_step_id,
                "to_step_id": &record.to_step_id,
                "command": eval_events::body_snippet(&record.command),
            }))).chain(report.instruction_truncations.iter().map(|record| json!({
                "kind": &record.kind,
                "step_id": &record.step_id,
                "original_len": record.original_len,
                "new_len": record.new_len,
            }))).collect::<Vec<_>>(),
            "goal_truncations": report.goal_truncations.iter().map(|record| json!({
                "kind": &record.kind,
                "original_len": record.original_len,
                "new_len": record.new_len,
            })).collect::<Vec<_>>(),
            "normalized_commands": report.normalized_commands.iter().map(|record| json!({
                "kind": &record.kind,
                "step_id": &record.step_id,
                "original_command": eval_events::body_snippet(&record.original_command),
                "normalized_command": eval_events::body_snippet(&record.normalized_command),
                "reason": eval_events::body_snippet(&record.reason),
            })).collect::<Vec<_>>(),
            "shell_control_splits": report.shell_control_splits.iter().map(|record| json!({
                "kind": &record.kind,
                "step_id": &record.step_id,
                "original_command": eval_events::body_snippet(&record.original_command),
                "fragments": record.fragments.iter().map(|fragment| eval_events::body_snippet(fragment)).collect::<Vec<_>>(),
                "dropped_fallback": record.dropped_fallback.as_deref().map(eval_events::body_snippet),
            })).collect::<Vec<_>>(),
            "removed_commands": report.removed_commands.iter().map(|record| json!({
                "step_id": &record.step_id,
                "command": eval_events::body_snippet(&record.command),
                "reason": eval_events::body_snippet(&record.reason),
            })).collect::<Vec<_>>(),
            "substituted_commands": report.substituted_commands.iter().map(|record| json!({
                "step_id": &record.step_id,
                "removed_command": eval_events::body_snippet(&record.removed_command),
                "substituted_command": eval_events::body_snippet(&record.substituted_command),
            })).collect::<Vec<_>>(),
            "moved_commands": report.moved_commands.iter().map(|record| json!({
                "from_step_id": &record.from_step_id,
                "to_step_id": &record.to_step_id,
                "command": eval_events::body_snippet(&record.command),
                "reason": eval_events::body_snippet(&record.reason),
            })).collect::<Vec<_>>(),
            "setup_verify_relocations": report.setup_verify_relocations.iter().map(|record| json!({
                "from_step_id": &record.from_step_id,
                "to_step_id": &record.to_step_id,
                "command": eval_events::body_snippet(&record.command),
                "reason": eval_events::body_snippet(&record.reason),
            })).collect::<Vec<_>>(),
            "dropped_commands": report.dropped_commands.iter().map(|record| json!({
                "step_id": &record.step_id,
                "command": eval_events::body_snippet(&record.command),
                "reason": eval_events::body_snippet(&record.reason),
            })).collect::<Vec<_>>(),
            "retyped_steps": report.retyped_steps.iter().map(|record| json!({
                "step_id": &record.step_id,
                "from_kind": &record.from_kind,
                "to_kind": &record.to_kind,
                "reason": eval_events::body_snippet(&record.reason),
            })).collect::<Vec<_>>(),
            "instruction_truncations": report.instruction_truncations.iter().map(|record| json!({
                "kind": &record.kind,
                "step_id": &record.step_id,
                "original_len": record.original_len,
                "new_len": record.new_len,
            })).collect::<Vec<_>>(),
            "instruction_notes": report.instruction_notes.iter().map(|record| json!({
                "step_id": &record.step_id,
                "note": eval_events::body_snippet(&record.note),
            })).collect::<Vec<_>>(),
        }),
    );
}

fn emit_planner_fallback_plan(
    config: &Config,
    provider: &str,
    model: &str,
    goal: &str,
    plan: &StepPlan,
    reason: &str,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "planner_fallback_plan",
            "planner_stage": "fallback",
            "planner_provider": provider,
            "planner_model": model,
            "planner_error_message": eval_events::body_snippet(reason),
            "profile": config.profile,
            "goal": eval_events::body_snippet(goal),
            "step_count": plan.steps.len(),
            "expected_paths": plan.steps.iter().flat_map(|step| step.expected_paths.iter()).cloned().collect::<Vec<_>>(),
            "verify": collect_step_verify_commands(plan),
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

fn fallback_step_plan_for_setup_phase(goal: &str, config: &Config) -> Option<StepPlan> {
    if !known_profile_for_setup_fallback(&config.profile) || !looks_like_setup_phase_goal(goal) {
        return None;
    }
    let expected_paths = profile_setup_scaffold_paths(&config.workspace_root, &config.profile);
    if expected_paths.is_empty() {
        return None;
    }
    let verify = expected_paths
        .iter()
        .map(|path| format!("test -f {path}"))
        .filter(|command| crate::planner::verify::validate_verify_command(command).is_ok())
        .collect::<Vec<_>>();
    let plan = StepPlan {
        goal: goal.to_string(),
        steps: vec![PlanStep {
            id: "fallback-setup".to_string(),
            kind: "setup".to_string(),
            expected_result: "pass".to_string(),
            instruction: fallback_setup_instruction(&config.profile, goal, &expected_paths),
            expected_paths,
            verify,
        }],
    };
    let lint_report = lint_step_plan_report_with_workspace(&plan, Some(&config.workspace_root));
    lint_report.is_pass().then_some(plan)
}

fn fallback_setup_instruction(profile: &str, goal: &str, expected_paths: &[String]) -> String {
    format!(
        "Create one coherent {profile} App Router scaffold for this setup phase: {goal}. \
         Required files: {paths}. \
         Coherence requirements: src/app/globals.css contains @tailwind base, @tailwind components, and @tailwind utilities; \
         src/app/layout.tsx imports ./globals.css; \
         create exactly one Tailwind config file, preferring tailwind.config.ts and never creating multiple tailwind.config.* files; \
         package.json scripts.dev and scripts.start run next dev and next start on the goal's port when one is mentioned.",
        goal = compact_single_line(goal),
        paths = expected_paths.join(", "),
    )
}

fn known_profile_for_setup_fallback(profile: &str) -> bool {
    matches!(profile, "nextjs" | "next-js" | "next.js")
}

fn looks_like_setup_phase_goal(goal: &str) -> bool {
    let phase_text = phase_id_and_task_text(goal).unwrap_or_else(|| goal.to_string());
    let lower = phase_text.to_ascii_lowercase();
    signals::contains_setup_token(&phase_text)
        || lower.contains("set up")
        || lower.contains("scaffold")
        || lower.contains("init")
        || lower.contains("initialize")
        || lower.contains("initialise")
}

fn phase_id_and_task_text(goal: &str) -> Option<String> {
    let mut out = Vec::new();
    for line in goal.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("Phase id:") || trimmed.starts_with("Phase task:") {
            out.push(trimmed.to_string());
        }
    }
    (!out.is_empty()).then(|| out.join("\n"))
}

fn compact_single_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
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
    if is_only_goal_length_lint(report) {
        return "shorten goal to one sentence; keep steps unchanged".to_string();
    }
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

fn is_only_goal_length_lint(report: &PlanLintReport) -> bool {
    report.errors.len() == 1
        && report.errors[0].category == "contract"
        && report.errors[0].message == "StepPlan goal is too long"
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
- Phase descriptions must not request dev-server startup or page-load/browser-route verification outside the final phase.\n\
- Browser readiness and page-route acceptance are verified by the runtime at final acceptance.\n\
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
    if is_ultra_phase_step_goal(goal) {
        prompt.push_str("\nUltra phase hard constraints:\n");
        prompt.push_str("- StepPlan.goal must be ONE short sentence naming the phase outcome; never copy the phase context, unmet-requirements, or adherence lists into goal -- details belong in step instructions.\n");
        prompt.push_str("- Do not put dev-server startup, page-load probes, curl localhost, or dependency installation in verify.\n");
        prompt.push_str("- Browser readiness is verified by the runtime at final acceptance.\n");
        prompt.push_str("- GOOD verify examples:\n");
        prompt.push_str("  - test -f package.json\n");
        prompt.push_str("  - node -p \"require('./package.json').scripts.dev\"\n");
        prompt.push_str("- BAD verify examples:\n");
        prompt.push_str("  - npm install\n");
        prompt.push_str("  - npm run dev\n");
        prompt.push_str("  - curl http://localhost:3011\n");
    }
    prompt
}

fn is_ultra_phase_step_goal(goal: &str) -> bool {
    goal.contains("Original ultra goal:")
        && goal.contains("Phase id:")
        && goal.contains("Phase task:")
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

fn build_step_prompt(_plan: &StepPlan, step: &PlanStep, context: &StepPromptContext) -> String {
    let mut prompt = String::new();
    prompt.push_str("Execute exactly one StepPlan step.\n\n");
    prompt.push_str("Overall goal:\n");
    prompt.push_str(&context.overall_goal);
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
- For Next.js/App Router work, keep a single route-bound implementation; do not leave capability components unimported.\n\
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
    let generation_rules =
        profile_generation_rules(&plan.profile, &plan.intent).unwrap_or("- none\n");
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
    let plan_adherence = plan_adherence_report(plan, &config.workspace_root);
    let requested_features = render_requested_features_not_detected_line(&plan_adherence.missing);
    let route_bound_constraint = if matches!(
        plan.profile.as_str(),
        "nextjs" | "next-js" | "next.js"
    ) {
        "\nRoute-bound implementation constraint:\n- Keep a single route-bound implementation; do not leave capability components unimported.\n"
    } else {
        ""
    };
    format!(
        "Original ultra goal: {}\nProfile: {}\nStyle: {}\nIntent: {}\nPhase id: {}\nPhase task: {}\n\nWorkspace snapshot:\n{}\n\n{}\n\n{}\n\n{}\n\nProfile generation rules:\n{}\nProfile runtime contract:\n{}\n{}Deterministic verification preference:\n{}\n{}{}{}",
        plan.goal,
        plan.profile,
        plan.style,
        plan.intent,
        phase.id,
        phase.prompt,
        workspace_snapshot,
        prior_context,
        unmet_final_requirements,
        requested_features,
        generation_rules,
        runtime_contract,
        route_bound_constraint,
        preferred_verify,
        required,
        capability_section,
        evidence_section
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
    merge_unique_strings(&mut expected, &compile_error_paths(&report.compile_errors));
    merge_unique_strings(&mut expected, &report.missing_paths);
    merge_unique_strings(&mut expected, &obligation_repair_target_paths(report));
    Ok(expected)
}

fn compile_error_paths(errors: &[CompileError]) -> Vec<String> {
    errors
        .iter()
        .map(|error| error.path.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[derive(Debug, Clone, Default)]
struct CompileRollbackOutcome {
    paths: Vec<String>,
    snapshot_origins: Vec<String>,
    carry_forward_guidance: Vec<String>,
}

fn try_compile_rollback_after_repair_exhaustion(
    config: &Config,
    profile: &str,
    goal: &str,
    phase_id: &str,
    phase_prompt: &str,
    report: &VerificationReport,
    exhausted_reason: &str,
) -> anyhow::Result<Option<CompileRollbackOutcome>> {
    let failing_paths = compile_error_paths(&report.compile_errors)
        .into_iter()
        .filter_map(|path| safe_source_rel_path(&path))
        .collect::<Vec<_>>();
    if failing_paths.is_empty() {
        return Ok(None);
    }
    let Some(latest_dir) = snapshot_latest_dir(config) else {
        emit_compile_rollback_skipped(
            config,
            &failing_paths,
            exhausted_reason,
            "snapshot_store_missing",
        );
        return Ok(None);
    };
    let mut snapshot_paths = Vec::new();
    for rel in &failing_paths {
        let snapshot = latest_dir.join(rel);
        if !snapshot.is_file() {
            emit_compile_rollback_skipped(
                config,
                &failing_paths,
                exhausted_reason,
                &format!("snapshot_missing:{rel}"),
            );
            return Ok(None);
        }
        snapshot_paths.push(snapshot);
    }
    let mut restored_paths = Vec::new();
    let mut origins = Vec::new();
    for (rel, snapshot) in failing_paths.iter().zip(snapshot_paths.iter()) {
        let Some(destination) = writable_workspace_source_path(&config.workspace_root, rel) else {
            emit_compile_rollback_skipped(
                config,
                &failing_paths,
                exhausted_reason,
                &format!("restore_path_rejected:{rel}"),
            );
            return Ok(None);
        };
        let content = std::fs::read(snapshot)?;
        std::fs::write(&destination, content)?;
        restored_paths.push(rel.clone());
        origins.push(workspace_relative_handoff_path(snapshot));
    }
    let rebuild_report = verify_profile_final(&config.workspace_root, profile, goal);
    if report_has_production_build_failure(&rebuild_report) {
        eval_events::emit(
            config.eval_events_path.as_deref(),
            json!({
                "event": "compile_rollback_failed",
                "phase_id": phase_id,
                "paths": restored_paths,
                "snapshot_origins": origins,
                "exhausted_reason": exhausted_reason,
                "rebuild_reason": eval_events::body_snippet(&rebuild_report.primary_reason()),
            }),
        );
        return Ok(None);
    }
    let phase_goal = phase_goal_one_liner(phase_prompt);
    let carry_forward_guidance = restored_paths
        .iter()
        .map(|path| {
            format!(
                "phase {} changes to {} were rolled back; re-apply: {}",
                phase_id, path, phase_goal
            )
        })
        .collect::<Vec<_>>();
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "compile_rollback_applied",
            "phase_id": phase_id,
            "paths": restored_paths.clone(),
            "snapshot_origins": origins.clone(),
            "exhausted_reason": exhausted_reason,
            "carry_forward_guidance": carry_forward_guidance.clone(),
        }),
    );
    eval_events::append_run_summary(
        config.eval_events_path.as_deref(),
        &format!(
            "Compile rollback applied:\n- paths: {}\n- snapshot origin: {}\n- carry-forward: {}",
            missing_if_empty(&restored_paths.join(", ")),
            missing_if_empty(&origins.join(", ")),
            missing_if_empty(&carry_forward_guidance.join("; "))
        ),
    );
    Ok(Some(CompileRollbackOutcome {
        paths: restored_paths,
        snapshot_origins: origins,
        carry_forward_guidance,
    }))
}

fn emit_compile_rollback_context_carried(config: &Config, rollback: &CompileRollbackOutcome) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "compile_rollback_context_carried",
            "paths": &rollback.paths,
            "snapshot_origins": &rollback.snapshot_origins,
            "carry_forward_guidance": &rollback.carry_forward_guidance,
        }),
    );
}

fn emit_compile_rollback_skipped(
    config: &Config,
    failing_paths: &[String],
    exhausted_reason: &str,
    reason: &str,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "compile_rollback_skipped",
            "paths": failing_paths,
            "exhausted_reason": exhausted_reason,
            "reason": reason,
        }),
    );
}

fn should_run_compile_no_snapshot_narrow_retry(
    config: &Config,
    report: &VerificationReport,
    already_used: bool,
) -> bool {
    !already_used
        && !report.compile_errors.is_empty()
        && !compile_rollback_snapshot_available(config, report)
}

fn compile_rollback_snapshot_available(config: &Config, report: &VerificationReport) -> bool {
    let failing_paths = compile_error_paths(&report.compile_errors)
        .into_iter()
        .filter_map(|path| safe_source_rel_path(&path))
        .collect::<Vec<_>>();
    if failing_paths.is_empty() {
        return false;
    }
    let Some(latest_dir) = snapshot_latest_dir(config) else {
        return false;
    };
    failing_paths
        .iter()
        .all(|rel| latest_dir.join(rel).is_file())
}

fn emit_compile_no_snapshot_narrow_retry(
    config: &Config,
    attempt: usize,
    report: &VerificationReport,
    trigger: &str,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "compile_no_snapshot_narrow_retry",
            "attempt": attempt,
            "next_attempt": attempt + 1,
            "trigger": trigger,
            "compile_errors": report.compile_errors.clone(),
            "primary_reason": eval_events::body_snippet(&report.primary_reason()),
        }),
    );
}

fn phase_goal_one_liner(prompt: &str) -> String {
    let mut line = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > 180 {
        line = line.chars().take(180).collect::<String>();
        line.push_str("...");
    }
    if line.is_empty() {
        "restore the rolled-back phase intent".to_string()
    } else {
        line
    }
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

#[allow(clippy::too_many_arguments)]
fn final_acceptance_repair_prompt(
    root: &Path,
    plan: &UltraPlan,
    report: &VerificationReport,
    context: &UltraRunContext,
    repair_target: &str,
    expected_paths: &[String],
    adherence_missing: &[String],
    repair_budget: (usize, usize),
    compile_reanchored_retry: bool,
    compile_narrow_no_snapshot_retry: bool,
) -> String {
    let (attempt, max_attempts) = repair_budget;
    let expected = render_prompt_bullets(expected_paths);
    let missing = render_prompt_bullets(&report.missing_paths);
    let dependencies = render_prompt_bullets(&report.dependency_missing);
    let profile_failures = final_acceptance_model_fixable_profile_failures(report);
    let profile_failures = render_prompt_bullets(&profile_failures);
    let adherence_guidance =
        final_acceptance_adherence_guidance(report, repair_target, adherence_missing);
    let behavioral_probe_context =
        final_acceptance_behavioral_probe_context(report, expected_paths);
    let command_failures = command_failure_summaries(report);
    let command_failures = render_prompt_bullets(&command_failures);
    let compile_errors = compile_repair_prompt_section_with_root(
        Some(root),
        &report.compile_errors,
        CompileRepairPromptProtection {
            reanchored_retry: compile_reanchored_retry,
            narrow_no_snapshot_retry: compile_narrow_no_snapshot_retry,
        },
    );
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
Compile errors:\n{compile_errors}\n\n\
Command failures:\n{command_failures}\n\n\
Profile failures:\n{profile_failures}\n\n\
{behavioral_probe_context}\
{adherence_guidance}\
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
        compile_errors = compile_errors,
        command_failures = command_failures,
        profile_failures = profile_failures,
        behavioral_probe_context = behavioral_probe_context,
        adherence_guidance = adherence_guidance,
        expected = expected,
        prior_context = context.render_prompt_section(),
    )
}

fn final_acceptance_behavioral_probe_context(
    report: &VerificationReport,
    expected_paths: &[String],
) -> String {
    let Some(failure_kind) = final_acceptance_app_behavior_failure_kind(report) else {
        return String::new();
    };
    let evidence = interaction_probe_json_from_report(report);
    let dispatched_inputs = evidence
        .as_ref()
        .map(|value| raw_string_array_field_deep(value, "input_dispatches"))
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| {
            vec![
                "ArrowLeft keydown".to_string(),
                "ArrowRight keydown".to_string(),
                "Space keydown".to_string(),
                "canvas/center click".to_string(),
            ]
        });
    let mut lines = vec![
        "Interaction probe context:".to_string(),
        format!("- failure kind: {failure_kind}"),
        format!("- dispatched inputs: {}", dispatched_inputs.join(", ")),
    ];
    if let Some(value) = evidence.as_ref() {
        if let Some(mode) =
            raw_text_field_deep(value, &["probe_mode"]).filter(|mode| !mode.is_empty())
        {
            lines.push(format!("- probe mode: {mode}"));
        }
        if let Some(status) = raw_text_field_deep(value, &["contract_hook_status"])
            .filter(|status| !status.is_empty())
        {
            lines.push(format!("- contract hook status: {status}"));
        }
        if let Some(restart_present) = raw_contract_hook_bool(value, "restart_present") {
            lines.push(format!("- restart hook present: {restart_present}"));
        }
        let action_hooks = raw_string_array_field_deep(value, "action_hooks");
        if !action_hooks.is_empty() {
            lines.push(format!("- action hooks: {}", action_hooks.join(", ")));
        }
        if let Some(status) =
            raw_text_field_deep(value, &["text_entry"]).filter(|status| !status.is_empty())
        {
            lines.push(format!("- text entry: {status}"));
        }
        if let Some(target) =
            raw_text_field_deep(value, &["text_entry_target"]).filter(|target| !target.is_empty())
        {
            lines.push(format!("- text entry target: {target}"));
        }
        if let Some(token) =
            raw_text_field_deep(value, &["typed_token"]).filter(|token| !token.is_empty())
        {
            lines.push(format!("- typed token: {token}"));
        }
        if let Some(echoed) = raw_bool_field_deep(value, "token_echoed") {
            lines.push(format!("- token echoed outside input: {echoed}"));
        }
        if let Some(latency) = raw_u64_field_deep(value, "echo_latency_ms") {
            lines.push(format!("- token echo latency ms: {latency}"));
        }
        if let Some(after_reload) = raw_bool_field_deep(value, "token_echoed_after_reload") {
            lines.push(format!("- token echoed after reload: {after_reload}"));
        }
        if let Some(latency) = raw_u64_field_deep(value, "token_echo_after_reload_latency_ms") {
            lines.push(format!("- token echo after reload latency ms: {latency}"));
        }
        if let Some(changed) = raw_bool_field_deep(value, "text_input_state_change") {
            lines.push(format!("- text input state change: {changed}"));
        }
        let state_dimensions = raw_string_array_field_deep(value, "state_dimensions_changed");
        if !state_dimensions.is_empty() {
            lines.push(format!(
                "- state dimensions changed: {}",
                state_dimensions.join(", ")
            ));
        }
        if let Some(status) = raw_text_field_deep(value, &["persistence_after_reload"])
            .filter(|status| !status.is_empty())
        {
            lines.push(format!("- persistence after reload: {status}"));
        }
        if let Some(reason) = raw_text_field_deep(value, &["persistence_after_reload_reason"])
            .filter(|reason| !reason.is_empty())
        {
            lines.push(format!("- persistence after reload reason: {reason}"));
        }
        let persisted_dimensions =
            raw_string_array_field_deep(value, "persistence_changed_dimensions");
        if !persisted_dimensions.is_empty() {
            lines.push(format!(
                "- persistence dimensions checked: {}",
                persisted_dimensions.join(", ")
            ));
        }
        let info = raw_string_array_field_deep(value, "informational_failure_kinds");
        if !info.is_empty() {
            lines.push(format!(
                "- informational probe findings: {}",
                info.join(", ")
            ));
        }
        let candidates = interaction_candidate_prompt_lines(value);
        if !candidates.is_empty() {
            lines.push("- candidate table:".to_string());
            lines.extend(candidates);
        }
        for key in [
            "before_marker",
            "after_marker",
            "input_before_marker",
            "input_after_marker",
            "recovery_before_marker",
            "recovery_after_marker",
        ] {
            if let Some(marker) =
                raw_text_field_deep(value, &[key]).filter(|marker| !marker.is_empty())
            {
                lines.push(format!("- {key}: {}", prompt_marker_excerpt(&marker)));
            }
        }
    }
    if failure_kind.contains("start_transition_missing") {
        lines.push(format!(
            "- concrete requirement: {INTERACTION_START_REPAIR_REQUIREMENT}"
        ));
    }
    if failure_kind.contains("interaction_state_change_missing")
        || failure_kind.contains("input_state_change_missing_after_start")
        || failure_kind.contains("input_state_change_not_evaluated_after_start")
        || failure_kind.contains("text_input_state_change_missing")
    {
        lines.push(format!(
            "- concrete requirement: {INTERACTION_STATE_CHANGE_REPAIR_REQUIREMENT}"
        ));
    }
    if failure_kind.contains("persistence_after_reload_reset") {
        lines.push(format!(
            "- concrete requirement: {PERSISTENCE_RELOAD_REPAIR_REQUIREMENT}"
        ));
    }
    if failure_kind.contains("token_echo_after_reload_only") {
        lines.push(format!(
            "- concrete requirement: {TEXT_ECHO_AFTER_RELOAD_REPAIR_REQUIREMENT}"
        ));
    } else if failure_kind.contains("token_echo_missing") {
        lines.push(format!(
            "- concrete requirement: {TEXT_ECHO_REPAIR_REQUIREMENT}"
        ));
    }
    let route_paths = route_bound_implementation_paths(expected_paths);
    if !route_paths.is_empty() {
        lines.push("Route-bound implementation targets:".to_string());
        lines.extend(route_paths.into_iter().map(|path| format!("- {path}")));
    }
    format!("{}\n\n", lines.join("\n"))
}

fn final_acceptance_recovery_reason(
    report: &VerificationReport,
    reason: &str,
    exhausted_reason: &str,
) -> String {
    let Some(failure_kind) = final_acceptance_app_behavior_failure_kind(report) else {
        return format!("{reason}; final acceptance repair stopped: {exhausted_reason}");
    };
    let repair_targets = interaction_repair_targets_for_reason(&failure_kind).join(", ");
    let mut out = format!(
        "{failure_kind}; final acceptance repair stopped: {exhausted_reason}; repair target: {repair_targets}"
    );
    if failure_kind.contains("interaction_state_change_missing")
        || failure_kind.contains("input_state_change_missing_after_start")
        || failure_kind.contains("input_state_change_not_evaluated_after_start")
        || failure_kind.contains("text_input_state_change_missing")
    {
        out.push_str("; ");
        out.push_str(INTERACTION_STATE_CHANGE_REPAIR_REQUIREMENT);
    } else if failure_kind.contains("start_transition_missing") {
        out.push_str("; ");
        out.push_str(INTERACTION_START_REPAIR_REQUIREMENT);
    } else if failure_kind.contains("persistence_after_reload_reset") {
        out.push_str("; ");
        out.push_str(PERSISTENCE_RELOAD_REPAIR_REQUIREMENT);
    } else if failure_kind.contains("token_echo_after_reload_only") {
        out.push_str("; ");
        out.push_str(TEXT_ECHO_AFTER_RELOAD_REPAIR_REQUIREMENT);
    } else if failure_kind.contains("token_echo_missing") {
        out.push_str("; ");
        out.push_str(TEXT_ECHO_REPAIR_REQUIREMENT);
    }
    let context = final_acceptance_behavioral_probe_context(report, &[]);
    if !context.is_empty() {
        out.push('\n');
        out.push_str(context.trim());
    }
    out
}

fn final_acceptance_recovery_repair_targets(
    report: &VerificationReport,
    fallback: RepairTarget,
) -> Vec<String> {
    let mut targets = if let Some(reason) = final_acceptance_app_behavior_failure_kind(report) {
        interaction_repair_targets_for_reason(&reason)
    } else {
        vec![fallback.as_str().to_string()]
    };
    if !report.compile_errors.is_empty()
        && !targets.iter().any(|target| target == "fix_compile_error")
    {
        targets.insert(0, "fix_compile_error".to_string());
    }
    targets
}

fn final_acceptance_recovery_failure_evidence(
    report: &VerificationReport,
    reason: &str,
) -> Vec<String> {
    let mut evidence = compile_error_repair_guidance(&report.compile_errors)
        .into_iter()
        .map(|line| format!("fix_compile_error: {line}"))
        .collect::<Vec<_>>();
    if !reason.trim().is_empty() {
        evidence.push(reason.to_string());
    }
    evidence
}

fn route_bound_implementation_paths(expected_paths: &[String]) -> Vec<String> {
    let mut paths = expected_paths
        .iter()
        .filter(|path| {
            let lower = path.to_ascii_lowercase();
            lower.contains("src/app/")
                || lower.contains("app/page")
                || lower.contains("pages/")
                || lower.ends_with(".tsx")
                || lower.ends_with(".jsx")
                || lower.ends_with(".ts")
                || lower.ends_with(".js")
        })
        .cloned()
        .collect::<Vec<_>>();
    if paths.is_empty() {
        paths.extend(expected_paths.iter().cloned());
    }
    dedup_strings(paths)
}

fn interaction_probe_json_from_report(report: &VerificationReport) -> Option<Value> {
    report
        .profile_failures
        .iter()
        .find_map(|failure| failure.strip_prefix("interaction evidence path: "))
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
}

fn interaction_state_dimensions_changed_from_path(path: &str) -> Vec<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
        .map(|value| raw_string_array_field_deep(&value, "state_dimensions_changed"))
        .unwrap_or_default()
}

fn interaction_action_hooks_from_path(path: &str) -> Vec<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
        .map(|value| raw_string_array_field_deep(&value, "action_hooks"))
        .unwrap_or_default()
}

#[derive(Debug, Clone, Default)]
struct InteractionTextTelemetry {
    text_entry: String,
    text_entry_target: String,
    typed_token: String,
    token_echoed: Option<bool>,
    echo_latency_ms: Option<u64>,
    token_echoed_after_reload: Option<bool>,
    token_echo_after_reload_latency_ms: Option<u64>,
    text_input_state_change: Option<bool>,
}

fn interaction_text_telemetry_from_path(path: &str) -> InteractionTextTelemetry {
    let Some(value) = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
    else {
        return InteractionTextTelemetry::default();
    };
    InteractionTextTelemetry {
        text_entry: raw_text_field_deep(&value, &["text_entry"]).unwrap_or_default(),
        text_entry_target: raw_text_field_deep(&value, &["text_entry_target"]).unwrap_or_default(),
        typed_token: raw_text_field_deep(&value, &["typed_token"]).unwrap_or_default(),
        token_echoed: raw_bool_field_deep(&value, "token_echoed"),
        echo_latency_ms: raw_u64_field_deep(&value, "echo_latency_ms"),
        token_echoed_after_reload: raw_bool_field_deep(&value, "token_echoed_after_reload"),
        token_echo_after_reload_latency_ms: raw_u64_field_deep(
            &value,
            "token_echo_after_reload_latency_ms",
        ),
        text_input_state_change: raw_bool_field_deep(&value, "text_input_state_change"),
    }
}

fn prompt_marker_excerpt(value: &str) -> String {
    const MAX_CHARS: usize = 600;
    if value.chars().count() <= MAX_CHARS {
        return value.to_string();
    }
    let mut excerpt = value.chars().take(MAX_CHARS).collect::<String>();
    excerpt.push_str("[truncated]");
    excerpt
}

fn raw_text_field_deep(value: &Value, names: &[&str]) -> Option<String> {
    for scope in raw_value_scopes(value) {
        for name in names {
            if let Some(found) = scope.get(*name).and_then(Value::as_str) {
                return Some(found.to_string());
            }
        }
    }
    None
}

fn raw_string_array_field_deep(value: &Value, name: &str) -> Vec<String> {
    raw_value_scopes(value)
        .into_iter()
        .find_map(|scope| {
            scope
                .get(name)
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .filter(|items| !items.is_empty())
        })
        .unwrap_or_default()
}

fn raw_contract_hook_bool(value: &Value, name: &str) -> Option<bool> {
    raw_value_scopes(value).into_iter().find_map(|scope| {
        scope
            .get("contract_hooks")
            .and_then(|hooks| hooks.get(name))
            .and_then(Value::as_bool)
    })
}

fn raw_bool_field_deep(value: &Value, name: &str) -> Option<bool> {
    raw_value_scopes(value)
        .into_iter()
        .find_map(|scope| scope.get(name).and_then(Value::as_bool))
}

fn raw_u64_field_deep(value: &Value, name: &str) -> Option<u64> {
    raw_value_scopes(value)
        .into_iter()
        .find_map(|scope| scope.get(name).and_then(Value::as_u64))
}

fn interaction_candidate_prompt_lines(value: &Value) -> Vec<String> {
    raw_value_scopes(value)
        .into_iter()
        .find_map(|scope| scope.get("candidate_table").and_then(Value::as_array))
        .into_iter()
        .flatten()
        .take(4)
        .map(|candidate| {
            let rank = candidate
                .get("rank")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let text = candidate
                .get("text_excerpt")
                .and_then(Value::as_str)
                .unwrap_or("");
            let changed = candidate
                .get("changed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            format!(
                "- rank {rank}: text=\"{}\" changed={changed}",
                prompt_marker_excerpt(text)
            )
        })
        .collect()
}

fn raw_value_scopes(value: &Value) -> Vec<&Value> {
    let mut scopes = vec![value];
    if let Some(details) = value.get("details").filter(|details| details.is_object()) {
        scopes.push(details);
    }
    if let Some(details) = value
        .get("browser_details")
        .filter(|details| details.is_object())
    {
        scopes.push(details);
    }
    scopes
}

fn render_requested_features_not_detected_line(missing: &[String]) -> String {
    if missing.is_empty() {
        "Requested features not yet detected: none".to_string()
    } else {
        render_bounded_prompt_section(
            "Requested features not yet detected:",
            missing,
            None,
            ULTRA_PROMPT_GUIDANCE_MAX_LINES,
        )
    }
}

fn final_acceptance_adherence_guidance(
    report: &VerificationReport,
    repair_target: &str,
    adherence_missing: &[String],
) -> String {
    if adherence_missing.is_empty()
        || !final_acceptance_repairs_contract_evidence(report, repair_target)
    {
        return String::new();
    }
    format!(
        "Secondary plan-adherence guidance:\n- also close if in scope: {}\n\n",
        adherence_missing.join(", ")
    )
}

fn final_acceptance_repairs_contract_evidence(
    report: &VerificationReport,
    repair_target: &str,
) -> bool {
    repair_target == RepairTarget::RequiredEvidenceMissing.as_str()
        || final_acceptance_model_fixable_profile_failures(report)
            .iter()
            .any(|failure| {
                let lower = failure.to_ascii_lowercase();
                lower.contains("missing_required_evidence")
                    || lower.contains("weak_verification_evidence")
                    || lower.contains("required evidence missing")
            })
}

fn final_acceptance_model_fixable_profile_failures(report: &VerificationReport) -> Vec<String> {
    report
        .profile_failures
        .iter()
        .filter(|failure| {
            let lower = failure.to_ascii_lowercase();
            !lower.contains("browser_interaction_evidence_required")
                && !lower.contains("interaction_evidence_missing")
                && !lower.contains("interaction evidence status:")
                && !lower.contains("interaction evidence path:")
                && !lower.contains("interaction_unverified:probe_unavailable")
                && !lower.contains("unverified:")
                && !lower.contains("probe_dependency_missing")
                && !lower.contains("probe_infrastructure_failed")
                && !lower.contains("app interaction untested")
        })
        .cloned()
        .collect()
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
            step_contract_setup_authority(
                &without_setup,
                &implement,
                None,
                NodeDependencySetupAuthority::None
            ),
            NodeDependencySetupAuthority::None
        );
        assert_eq!(
            step_contract_setup_authority(
                &with_setup_implement,
                &setup_implement,
                None,
                NodeDependencySetupAuthority::None
            ),
            NodeDependencySetupAuthority::PlanSetupStep
        );
        assert_eq!(
            step_contract_setup_authority(
                &without_setup,
                &implement,
                Some("workspace-and-dependencies-setup"),
                NodeDependencySetupAuthority::None,
            ),
            NodeDependencySetupAuthority::PlanSetupStep
        );
    }

    #[test]
    fn ultra_run_setup_authority_falls_back_for_later_verify_and_contract_steps() {
        let plan = StepPlan {
            goal: "verify promoted app".to_string(),
            steps: vec![
                PlanStep {
                    id: "implement".to_string(),
                    kind: "implement".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Implement the app".to_string(),
                    expected_paths: Vec::new(),
                    verify: Vec::new(),
                },
                PlanStep {
                    id: "verify".to_string(),
                    kind: "verify".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Run build".to_string(),
                    expected_paths: Vec::new(),
                    verify: vec!["npm run build".to_string()],
                },
            ],
        };
        let implement = &plan.steps[0];
        let verify = &plan.steps[1];

        assert_eq!(
            step_verify_setup_authority(&plan, verify, NodeDependencySetupAuthority::None),
            NodeDependencySetupAuthority::None
        );
        assert_eq!(
            step_verify_setup_authority(&plan, verify, NodeDependencySetupAuthority::PlanSetupStep),
            NodeDependencySetupAuthority::PlanSetupStep
        );
        assert_eq!(
            step_contract_setup_authority(
                &plan,
                implement,
                None,
                NodeDependencySetupAuthority::PlanSetupStep
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
        let invalid = r#"{"goal":"goal","steps":[{"id":"s1","kind":"implement","expected_result":"pass","instruction":"Create app","expected_paths":["package.json"],"verify":["node check.js | node check2.js"]}]}"#;
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
    fn goal_length_retry_prompt_is_exact_and_step_preserving() {
        let mut report = PlanLintReport::pass();
        report.push("contract", "StepPlan goal is too long");
        let categories = BTreeSet::new();

        let prompt = build_lint_retry_prompt("goal", &report, 2, &categories);

        assert_eq!(prompt, "shorten goal to one sentence; keep steps unchanged");
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
            overall_goal: plan.goal.clone(),
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
    fn verifier_command_false_negative_does_not_start_implementation_repair_turn() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        std::fs::write(
            dir.path().join("usage_error.py"),
            "import sys\nsys.stderr.write('usage: fake\\ninvalid option\\n')\nsys.exit(2)\n",
        )
        .unwrap();
        let plan = StepPlan {
            goal: "Run deterministic verification".to_string(),
            steps: vec![PlanStep {
                id: "verify-usage-error".to_string(),
                kind: "verify".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Run the verifier command".to_string(),
                expected_paths: Vec::new(),
                verify: vec!["python3 usage_error.py".to_string()],
            }],
        };
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let mut fake = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Bash",
                    serde_json::json!({"command":"test -f usage_error.py"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("Verification step complete."),
        ]);

        let err = run_step_plan(&mut fake, &plan, &cfg)
            .unwrap_err()
            .to_string();

        assert_eq!(fake.messages.len(), 1, "{err}");
        assert!(
            fake.messages.iter().all(|messages| {
                !messages
                    .iter()
                    .any(|message| message.content.contains("Repair step `verify-usage-error`"))
            }),
            "{:#?}",
            fake.messages
        );
        assert!(err.contains("deterministic_verify_command_bug"), "{err}");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"reason\":\"deterministic_verify_command_bug\""));
        assert!(event_text.contains("\"repair_target\":\"verifier_command\""));
        assert!(!event_text.contains("\"reason\":\"bounded_repair_exhausted\""));
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
        let invalid = r#"{"goal":"goal","steps":[{"id":"s1","kind":"implement","expected_result":"pass","instruction":"Create app","expected_paths":["package.json"],"verify":["node check.js | node check2.js"]}]}"#;
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
    fn sanitizer_repairs_setup_dev_verify_without_retry() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let plan_json = serde_json::to_string(&StepPlan {
            goal: "Project setup phase".to_string(),
            steps: vec![
                PlanStep {
                    id: "create-manifest".to_string(),
                    kind: "implement".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Create package.json".to_string(),
                    expected_paths: vec!["package.json".to_string()],
                    verify: vec!["npm install".to_string()],
                },
                PlanStep {
                    id: "create-page".to_string(),
                    kind: "implement".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Create src/app/page.tsx".to_string(),
                    expected_paths: vec!["src/app/page.tsx".to_string()],
                    verify: vec!["npm run dev & curl http://localhost:3011".to_string()],
                },
            ],
        })
        .unwrap();
        let mut planner = FakeClient::new(vec![AssistantReply::text(plan_json)]);

        let plan = generate_step_plan(&mut planner, "Project setup phase", &cfg).unwrap();

        assert_eq!(planner.messages.len(), 1);
        assert_eq!(plan.steps[0].kind, "setup");
        assert_eq!(plan.steps[0].verify, vec!["test -f package.json"]);
        assert!(plan.steps[1].verify.is_empty());
        assert!(
            plan.steps[1]
                .instruction
                .contains("Browser readiness is verified by the runtime")
        );
        assert!(
            lint_step_plan_report_with_workspace(&plan, Some(dir.path())).is_pass(),
            "{plan:?}"
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"planner_plan_sanitized\""));
        assert!(!event_text.contains("\"event\":\"planner_error\""));
    }

    #[test]
    fn sanitizer_emits_qwen_lint_shape_repairs() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let plan_json = serde_json::to_string(&StepPlan {
            goal: "Create a Rust helper".to_string(),
            steps: vec![
                PlanStep {
                    id: "setup-project".to_string(),
                    kind: "setup".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Create Cargo.toml for the helper crate".to_string(),
                    expected_paths: vec!["Cargo.toml".to_string()],
                    verify: vec!["cargo test".to_string()],
                },
                PlanStep {
                    id: "create-helper".to_string(),
                    kind: "implement".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: format!("Create src/lib.rs. {}", "日本語".repeat(1_000)),
                    expected_paths: vec!["src/lib.rs".to_string()],
                    verify: Vec::new(),
                },
            ],
        })
        .unwrap();
        let mut planner = FakeClient::new(vec![AssistantReply::text(plan_json)]);

        let plan = generate_step_plan(&mut planner, "Create a Rust helper", &cfg).unwrap();

        assert_eq!(planner.messages.len(), 1);
        assert!(plan.steps[0].verify.is_empty());
        assert_eq!(plan.steps[1].verify, vec!["cargo test"]);
        assert_eq!(plan.steps[1].instruction, "Create src/lib.rs.");
        assert!(
            lint_step_plan_report_with_workspace(&plan, Some(dir.path())).is_pass(),
            "{plan:?}"
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"planner_plan_sanitized\""));
        assert!(event_text.contains("\"kind\":\"setup_verify_relocated\""));
        assert!(event_text.contains("\"kind\":\"instruction_truncated\""));
        assert!(!event_text.contains("\"event\":\"planner_error\""));
    }

    #[test]
    fn setup_phase_fallback_generates_profile_scaffold_after_invalid_attempts() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let goal = "Original ultra goal: Build an interactive app\n\
Profile: nextjs\n\
Style: default\n\
Intent: create\n\
Phase id: project-setup\n\
Phase task: Scaffold and initialize the Next.js project shell on port 3011";
        let mut planner = FakeClient::new(vec![
            AssistantReply::text("not a step plan"),
            AssistantReply::text("still not a step plan"),
            AssistantReply::text("no valid step plan"),
        ]);

        let plan = generate_step_plan(&mut planner, goal, &cfg).unwrap();

        let expected_paths = crate::planner::profiles::nextjs::setup_scaffold_paths(dir.path());
        assert_eq!(planner.messages.len(), 3);
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].kind, "setup");
        assert_eq!(plan.steps[0].expected_paths, expected_paths);
        assert_eq!(
            plan.steps[0].verify,
            expected_paths
                .iter()
                .map(|path| format!("test -f {path}"))
                .collect::<Vec<_>>()
        );
        for path in crate::planner::profiles::nextjs::setup_invariant_required_paths(dir.path()) {
            assert!(
                plan.steps[0].expected_paths.contains(&path),
                "fallback expected_paths must include invariant-required {path}"
            );
        }
        let instruction = &plan.steps[0].instruction;
        assert!(instruction.contains("@tailwind base"), "{instruction}");
        assert!(
            instruction.contains("@tailwind components"),
            "{instruction}"
        );
        assert!(instruction.contains("@tailwind utilities"), "{instruction}");
        assert!(instruction.contains("./globals.css"), "{instruction}");
        assert!(
            instruction.contains("exactly one Tailwind config file"),
            "{instruction}"
        );
        assert!(instruction.contains("scripts.dev"), "{instruction}");
        assert!(instruction.contains("scripts.start"), "{instruction}");
        assert!(instruction.contains("goal's port"), "{instruction}");
        assert!(
            lint_step_plan_report_with_workspace(&plan, Some(dir.path())).is_pass(),
            "{plan:?}"
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"planner_fallback_plan\""));
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
    fn step_plan_generation_retries_transient_provider_request_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut planner = FlakyClient::new(
            1,
            "transient provider unavailable",
            vec![AssistantReply::text(generated_step_plan_json("goal"))],
        );

        let plan =
            generate_step_plan(&mut planner, "goal", &config(dir.path().to_path_buf())).unwrap();

        assert_eq!(planner.messages.len(), 2);
        assert_eq!(plan.goal, "goal");
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
    fn ultra_phase_step_prompt_requires_short_goal() {
        let prompt = build_step_plan_user_prompt(
            "Original ultra goal: Build game\nPhase id: ui\nPhase task: Build the arcade UI.",
            &config(PathBuf::from("/tmp/work")),
        );

        assert!(prompt.contains("StepPlan.goal must be ONE short sentence"));
        assert!(prompt.contains("never copy the phase context"));
        assert!(prompt.contains("details belong in step instructions"));
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
    fn ultra_plan_generation_retries_transient_provider_request_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut planner = FlakyClient::new(
            1,
            "transient provider unavailable",
            vec![AssistantReply::text(generated_ultra_plan_yaml("goal"))],
        );

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
    fn echoed_ultra_phase_goal_is_sanitized_without_retry() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let long_phase_prompt = format!(
            "Original ultra goal: {}\n\
Profile: nextjs\n\
Style: default\n\
Intent: create\n\
Phase id: arcade-ui-and-local-storage\n\
Phase task: Build the arcade UI and local storage persistence. Add details in the steps.\n\n\
Unmet final requirements from earlier phases:\n- restart_or_recoverable_state_evidence\n\n\
Requested features not yet detected: keyboard, score, wave, audio\n\n\
Profile runtime contract:\n- Preserve the workspace as a real Next.js app.\n\n{}",
            "Create a polished arcade game. ".repeat(120),
            "Carry forward profile constraints. ".repeat(120)
        );
        let step_json = serde_json::to_string(&StepPlan {
            goal: long_phase_prompt.clone(),
            steps: vec![PlanStep {
                id: "create-page".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create src/app/page.tsx for the arcade phase.".to_string(),
                expected_paths: vec!["src/app/page.tsx".to_string()],
                verify: Vec::new(),
            }],
        })
        .unwrap();
        let mut planner = FakeClient::new(vec![AssistantReply::text(step_json)]);

        let plan = generate_step_plan(&mut planner, &long_phase_prompt, &cfg).unwrap();

        assert_eq!(planner.messages.len(), 1);
        assert_eq!(
            plan.goal,
            "Build the arcade UI and local storage persistence."
        );
        assert!(!plan.goal.contains("Unmet final requirements"));
        assert!(!plan.goal.contains("Requested features"));
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"planner_plan_sanitized\""));
        assert!(event_text.contains("\"kind\":\"goal_truncated\""));
        assert!(event_text.contains("\"original_len\""));
        assert!(event_text.contains("\"new_len\""));
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
        let degraded = r#"{"goal":"Build a Next.js game app","steps":[{"id":"bad","kind":"implement","expected_result":"pass","instruction":"Create app","expected_paths":["package.json"],"verify":["node check.js | node check2.js"]}]}"#;
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
    fn ultra_phase_prompt_renders_missing_plan_adherence_guidance() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "// keyboard lives pause\nexport default function Page(){ return <main><p>score</p></main>; }",
        )
        .unwrap();
        let plan = UltraPlan {
            goal: "Create an interactive browser game".to_string(),
            profile: "generic".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![crate::planner::ultra_plan::UltraPhase {
                id: "gameplay".to_string(),
                prompt:
                    "Implement keyboard controls, lives counter, pause overlay, and score display"
                        .to_string(),
            }],
        };

        let prompt = ultra_phase_prompt(
            &plan,
            &plan.phases[0],
            &config(dir.path().to_path_buf()),
            &UltraRunContext::new(Vec::new()),
        );

        let section = prompt_section_lines(&prompt, "Requested features not yet detected:");
        let section_text = section.join("\n");
        assert!(section_text.contains("- keyboard"), "{section_text}");
        assert!(section_text.contains("- lives"), "{section_text}");
        assert!(section_text.contains("- pause"), "{section_text}");
        assert!(!section_text.contains("score"), "{section_text}");
    }

    #[test]
    fn ultra_phase_prompt_elides_unmet_requirements_and_adherence_sections() {
        let dir = tempfile::tempdir().unwrap();
        let mut context = UltraRunContext::new(Vec::new());
        context.pending_capability_evidence = (0..20)
            .map(|index| format!("pending_evidence_{index:02}"))
            .collect();
        let feature_tokens = (0..30)
            .map(|index| format!("needtoken{index:02}"))
            .collect::<Vec<_>>()
            .join(" ");
        let plan = UltraPlan {
            goal: "Create a browser game".to_string(),
            profile: "generic".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![crate::planner::ultra_plan::UltraPhase {
                id: "features".to_string(),
                prompt: feature_tokens,
            }],
        };

        let prompt = ultra_phase_prompt(
            &plan,
            &plan.phases[0],
            &config(dir.path().to_path_buf()),
            &context,
        );

        let unmet = prompt_section_lines(&prompt, "Unmet final requirements from earlier phases:");
        let adherence = prompt_section_lines(&prompt, "Requested features not yet detected:");
        assert!(unmet.len() <= ULTRA_PROMPT_GUIDANCE_MAX_LINES, "{unmet:#?}");
        assert!(
            adherence.len() <= ULTRA_PROMPT_GUIDANCE_MAX_LINES,
            "{adherence:#?}"
        );
        assert!(unmet.iter().any(|line| line.contains("… and 15 more")));
        assert!(adherence.iter().any(|line| line.contains("… and 24 more")));
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
    fn uat_scenario_goal_capability_goldens_guard_distribution_drift() {
        let scenarios = [
            (
                "game",
                "あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。",
                vec![
                    "stateful_interaction",
                    "start_or_restart_flow",
                    "player_control",
                    "adversary_or_challenge",
                    "progression_or_score",
                    "failure_or_collision_rule",
                ],
            ),
            (
                "tool",
                "ローカルストレージに保存されるTodoアプリ(追加・完了・削除・フィルタ)をNext.jsアプリとして3011ポートで起動可能に開発してください。",
                vec![
                    "stateful_interaction",
                    "user_input_or_action",
                    "visible_state_change",
                    "persistence",
                ],
            ),
            (
                "content",
                "Markdownをリアルタイムプレビューできるノートアプリ(編集・保存・一覧)をNext.jsアプリとして3011ポートで開発してください。",
                vec![
                    "stateful_interaction",
                    "user_input_or_action",
                    "visible_state_change",
                    "persistence",
                ],
            ),
        ];

        for (name, goal, expected) in scenarios {
            let actual = inferred_required_capabilities("nextjs", goal);
            let expected = expected
                .into_iter()
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "scenario={name}");
            if name != "game" {
                assert!(
                    !actual.contains(&"adversary_or_challenge".to_string()),
                    "scenario={name}: {actual:?}"
                );
                assert!(
                    !actual.contains(&"failure_or_collision_rule".to_string()),
                    "scenario={name}: {actual:?}"
                );
            }
        }
    }

    #[test]
    fn todo_and_notes_capabilities_bind_to_generic_interactive_evidence() {
        for goal in [
            "ローカルストレージに保存されるTodoアプリ(追加・完了・削除・フィルタ)をNext.jsアプリとして3011ポートで起動可能に開発してください。",
            "Markdownをリアルタイムプレビューできるノートアプリ(編集・保存・一覧)をNext.jsアプリとして3011ポートで開発してください。",
        ] {
            let capabilities = inferred_required_capabilities("nextjs", goal);
            let evidence = inferred_required_evidence("nextjs", goal, &capabilities);
            assert!(evidence.contains(&"nextjs_route_evidence".to_string()));
            assert!(evidence.contains(&"visible_interactive_surface_evidence".to_string()));
            assert!(evidence.contains(&"user_input_handler_evidence".to_string()));
            assert!(evidence.contains(&"stateful_update_evidence".to_string()));
            assert!(evidence.contains(&"persistence_evidence".to_string()));
            assert!(!evidence.contains(&"challenge_or_adversary_evidence".to_string()));
            assert!(!evidence.contains(&"failure_or_collision_evidence".to_string()));
        }
    }

    #[test]
    fn generic_app_intent_binds_minimal_static_contract() {
        let goal = "ちょっとしたメモアプリを作って";
        let capabilities = inferred_required_capabilities("generic", goal);
        let evidence = inferred_required_evidence("generic", goal, &capabilities);
        let obligations = inferred_required_obligations("generic", goal, &capabilities);

        assert_eq!(
            capabilities,
            vec![GENERIC_INTERACTIVE_CONTRACT_CAPABILITY.to_string()]
        );
        assert_eq!(
            evidence,
            vec![
                "user_input_handler_evidence".to_string(),
                "stateful_update_evidence".to_string(),
                "visible_interactive_surface_evidence".to_string(),
            ]
        );
        assert_eq!(obligations, vec!["implementation".to_string()]);
    }

    #[test]
    fn promoted_contract_union_never_drops_generic_interactive_requirements() {
        let goal = "ちょっとしたメモアプリを作って";
        let generic = inferred_contract_requirements("generic", goal);
        let mut promoted = inferred_contract_requirements("nextjs", goal);

        assert!(promoted.capabilities.is_empty());
        carry_pre_promotion_contract_requirements("nextjs", goal, &generic, &mut promoted);
        merge_unique_strings(
            &mut promoted.evidence,
            &inferred_required_evidence("nextjs", goal, &promoted.capabilities),
        );

        for capability in [
            "stateful_interaction",
            "user_input_or_action",
            "visible_state_change",
        ] {
            assert!(
                promoted.capabilities.contains(&capability.to_string()),
                "{capability} missing from {promoted:?}"
            );
        }
        for evidence in GENERIC_INTERACTIVE_EVIDENCE_KEYS {
            assert!(
                promoted.evidence.contains(&evidence.to_string()),
                "{evidence} missing from {promoted:?}"
            );
        }
        assert!(
            promoted
                .evidence
                .contains(&"nextjs_route_evidence".to_string())
        );
        assert!(
            promoted
                .evidence
                .contains(&"build_command_or_dependency_missing_boundary".to_string())
        );
        assert!(
            generic
                .obligations
                .iter()
                .all(|obligation| promoted.obligations.contains(obligation))
        );
    }

    #[test]
    fn generic_script_goal_keeps_empty_contract() {
        let goal = "READMEを整形するスクリプト";
        let capabilities = inferred_required_capabilities("generic", goal);
        let evidence = inferred_required_evidence("generic", goal, &capabilities);
        let obligations = inferred_required_obligations("generic", goal, &capabilities);

        assert!(capabilities.is_empty());
        assert!(evidence.is_empty());
        assert!(obligations.is_empty());
    }

    #[test]
    fn generic_filename_app_py_does_not_bind_static_contract() {
        let goal = "Create app.py";
        let capabilities = inferred_required_capabilities("generic", goal);
        let evidence = inferred_required_evidence("generic", goal, &capabilities);
        let obligations = inferred_required_obligations("generic", goal, &capabilities);

        assert!(capabilities.is_empty());
        assert!(evidence.is_empty());
        assert!(obligations.is_empty());
    }

    #[test]
    fn generic_app_entrypoint_artifact_goal_does_not_bind_static_contract() {
        let goal = "Create app entrypoint\n\nRequired final artifacts:\n- src/app/page.tsx";
        let capabilities = inferred_required_capabilities("generic", goal);
        let evidence = inferred_required_evidence("generic", goal, &capabilities);
        let obligations = inferred_required_obligations("generic", goal, &capabilities);

        assert!(capabilities.is_empty());
        assert!(evidence.is_empty());
        assert!(obligations.is_empty());
    }

    #[test]
    fn generic_profile_ignores_known_profile_phase_prompt_for_static_contract() {
        let goal =
            "Original ultra goal: 3011 port app\nProfile: nextjs\nPhase task: Scaffold project";
        let capabilities = inferred_required_capabilities("generic", goal);
        let evidence = inferred_required_evidence("generic", goal, &capabilities);
        let obligations = inferred_required_obligations("generic", goal, &capabilities);

        assert!(capabilities.is_empty());
        assert!(evidence.is_empty());
        assert!(obligations.is_empty());
    }

    #[test]
    fn generic_contract_binding_emits_matched_intent_token() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.profile = "generic".to_string();
        let goal = "ちょっとしたメモアプリを作って";
        let capabilities = inferred_required_capabilities("generic", goal);
        let evidence = inferred_required_evidence("generic", goal, &capabilities);
        let obligations = inferred_required_obligations("generic", goal, &capabilities);

        let bound = bind_completion_contract_for_acceptance(
            &cfg,
            "ultra-plan-run",
            "generic",
            goal,
            &[],
            &capabilities,
            &evidence,
            &obligations,
        )
        .unwrap()
        .expect("generic app contract should bind");

        assert!(bound.generated);
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains(r#""event":"generic_contract_bound""#));
        assert!(event_text.contains(r#""matched_intent_token":"アプリ""#));
        assert!(event_text.contains(r#""inferred_keys":["user_input_handler_evidence","stateful_update_evidence","visible_interactive_surface_evidence"]"#));
    }

    #[test]
    fn ultra_final_acceptance_event_carries_generic_static_assurance() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.profile = "generic".to_string();
        std::fs::write(
            dir.path().join("memo.jsx"),
            r#"
import { useState } from "react";
export default function Memo() {
  const [notes, setNotes] = useState([]);
  return <form onSubmit={() => setNotes([...notes, "x"])}><input /><button>Add</button></form>;
}
"#,
        )
        .unwrap();
        let plan = UltraPlan::deterministic(
            "ちょっとしたメモアプリを作って",
            "generic",
            "default",
            "create",
        );

        let report = ultra_final_acceptance_report(&plan, &cfg).unwrap();

        assert!(report.is_pass(), "{report:?}");
        let final_acceptance = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            final_acceptance
                .get("assurance_level")
                .and_then(Value::as_str),
            Some("static")
        );
        assert_eq!(
            final_acceptance
                .get("assurance_reason")
                .and_then(Value::as_str),
            Some(eval_events::GENERIC_STATIC_ASSURANCE_REASON)
        );
    }

    #[test]
    fn generic_ultra_promotes_to_nextjs_after_workspace_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        write_fake_npm_dependency_installer(dir.path());
        let goal = "Build an interactive browser game on port 3011";
        let plan = two_phase_ultra_plan(goal, "generic");
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(single_write_step_plan_json(
                "Create a package manifest",
                "package.json",
            )),
            AssistantReply::text(generated_nextjs_artifact_plan_json(
                "Complete the promoted Next.js app",
            )),
        ]);
        let mut final_calls = nextjs_interactive_app_tool_calls(interactive_game_page_source());
        final_calls.remove(0);
        final_calls.extend(browser_release_evidence_tool_calls());
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({
                        "path": "package.json",
                        "content": nextjs_complete_package_json()
                    }),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: final_calls,
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let promotion = latest_event(&events, "profile_reinferred");
        assert_eq!(promotion.get("id").and_then(Value::as_str), Some("nextjs"));
        assert_eq!(promotion.get("at_phase").and_then(Value::as_u64), Some(1));
        assert_eq!(
            promotion.get("from").and_then(Value::as_str),
            Some("workspace")
        );
        assert!(event_array_contains(
            &promotion,
            "delta_capabilities",
            "stateful_interaction"
        ));
        let phase_two_prompt = planner_request_text(&planner, 1);
        assert!(phase_two_prompt.contains("Profile: nextjs"));
        assert!(phase_two_prompt.contains("Profile generation rules:"));
        assert!(phase_two_prompt.contains("data-anvil-action=\"primary\""));
        assert!(phase_two_prompt.contains("data-anvil-state"));
        assert!(phase_two_prompt.contains("Unmet final requirements from earlier phases:"));
        assert!(phase_two_prompt.contains("- nextjs_route_evidence"));
        let final_acceptance = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            final_acceptance.get("profile").and_then(Value::as_str),
            Some("nextjs")
        );
        assert_eq!(
            final_acceptance
                .get("assurance_level")
                .and_then(Value::as_str),
            Some("full")
        );
        assert_eq!(
            final_acceptance
                .get("browser_readiness_status")
                .and_then(Value::as_str),
            Some("passed")
        );
        assert_eq!(
            final_acceptance
                .get("interaction_evidence_status")
                .and_then(Value::as_str),
            Some("passed")
        );
        assert_eq!(
            final_acceptance
                .get("release_gate_status")
                .and_then(Value::as_str),
            Some("pass")
        );
        let complete = latest_event(&events, "ultra_plan_complete");
        assert_eq!(
            complete.get("profile").and_then(Value::as_str),
            Some("nextjs")
        );
        assert_eq!(
            complete.get("assurance_level").and_then(Value::as_str),
            Some("full")
        );
    }

    #[test]
    fn ambiguous_generic_app_promotion_keeps_union_contract_and_earns_full_after_gates() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        write_fake_npm_dependency_installer(dir.path());
        let goal = "ちょっとしたメモアプリを作って";
        let plan = two_phase_ultra_plan(goal, "generic");
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(single_write_step_plan_json(
                "Create a package manifest",
                "package.json",
            )),
            AssistantReply::text(generated_nextjs_artifact_plan_json(
                "Complete the promoted Next.js app",
            )),
        ]);
        let mut final_calls = nextjs_interactive_app_tool_calls(interactive_game_page_source());
        final_calls.remove(0);
        final_calls.extend(browser_release_evidence_tool_calls());
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({
                        "path": "package.json",
                        "content": nextjs_complete_package_json()
                    }),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: final_calls,
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let promotion = latest_event(&events, "profile_reinferred");
        assert_eq!(promotion.get("id").and_then(Value::as_str), Some("nextjs"));
        assert_eq!(
            promotion.get("contract_origin").and_then(Value::as_str),
            Some("promoted_union")
        );
        let final_acceptance = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            final_acceptance
                .get("effective_profile")
                .and_then(Value::as_str),
            Some("nextjs")
        );
        assert_eq!(
            final_acceptance
                .get("contract_origin")
                .and_then(Value::as_str),
            Some("promoted_union")
        );
        for evidence in GENERIC_INTERACTIVE_EVIDENCE_KEYS {
            assert!(
                event_array_contains(&final_acceptance, "required_evidence", evidence),
                "{evidence} missing from {final_acceptance}"
            );
        }
        for evidence in [
            "nextjs_route_evidence",
            "build_command_or_dependency_missing_boundary",
        ] {
            assert!(
                event_array_contains(&final_acceptance, "required_evidence", evidence),
                "{evidence} missing from {final_acceptance}"
            );
        }
        for capability in [
            "stateful_interaction",
            "user_input_or_action",
            "visible_state_change",
        ] {
            assert!(
                event_array_contains(&final_acceptance, "required_capabilities", capability),
                "{capability} missing from {final_acceptance}"
            );
        }
        assert_eq!(
            final_acceptance
                .get("browser_readiness_applicable")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            final_acceptance
                .get("browser_readiness_execution_status")
                .and_then(Value::as_str),
            Some("performed")
        );
        assert_eq!(
            final_acceptance
                .get("interaction_evidence_applicable")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            final_acceptance
                .get("interaction_evidence_execution_status")
                .and_then(Value::as_str),
            Some("performed")
        );
        assert_eq!(
            final_acceptance
                .get("browser_readiness_status")
                .and_then(Value::as_str),
            Some("passed")
        );
        assert_eq!(
            final_acceptance
                .get("interaction_evidence_status")
                .and_then(Value::as_str),
            Some("passed")
        );
        assert_eq!(
            final_acceptance
                .get("assurance_level")
                .and_then(Value::as_str),
            Some("full")
        );
    }

    #[cfg(unix)]
    #[test]
    fn ultra_final_acceptance_uses_effective_profile_over_stale_config_profile() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "generic".to_string();
        cfg.eval_events_path = Some(events.clone());
        let port = 3011;
        write_probe_nextjs_workspace(dir.path(), port, interactive_game_page_source());
        std::fs::write(
            dir.path().join("browser-readiness.json"),
            r#"{"ok":true,"http_status":200,"route_rendered":true}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("browser-interaction.json"),
            r#"{"ok":true,"interaction_performed":true,"start_transition":true,"input_state_change":true,"state_changed":true,"canvas_found":true}"#,
        )
        .unwrap();
        let plan = UltraPlan::deterministic(
            &explicit_port_goal("Create an interactive browser game", port),
            "next-js",
            "default",
            "create",
        );

        let report = ultra_final_acceptance_report(&plan, &cfg).unwrap();

        assert!(report.is_pass(), "{report:?}");
        let final_acceptance = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            final_acceptance.get("profile").and_then(Value::as_str),
            Some("nextjs")
        );
        assert_eq!(
            final_acceptance
                .get("effective_profile")
                .and_then(Value::as_str),
            Some("nextjs")
        );
        assert_eq!(
            final_acceptance
                .get("browser_readiness_status")
                .and_then(Value::as_str),
            Some("passed")
        );
        assert_eq!(
            final_acceptance
                .get("interaction_evidence_status")
                .and_then(Value::as_str),
            Some("passed")
        );
    }

    #[test]
    fn applicable_interaction_gates_disconnected_force_loud_failure() {
        let required_capabilities = vec![
            "stateful_interaction".to_string(),
            "user_input_or_action".to_string(),
            "visible_state_change".to_string(),
        ];
        let required_evidence = vec![
            "user_input_handler_evidence".to_string(),
            "stateful_update_evidence".to_string(),
        ];
        let mut release_gate = ReleaseGateSummary {
            status: "not_applicable".to_string(),
            reasons: Vec::new(),
            browser_readiness_status: "not_applicable".to_string(),
            browser_readiness_evidence_path: String::new(),
            interaction_evidence_status: "skipped".to_string(),
            interaction_evidence_path: String::new(),
        };
        let telemetry = acceptance_gate_telemetry(
            "nextjs",
            "Create an interactive browser app",
            &required_capabilities,
            &required_evidence,
            &release_gate,
        );

        let reason = acceptance_gates_disconnected_reason(&telemetry, &release_gate)
            .expect("disconnected gates should be loud");
        release_gate.status = "failed".to_string();
        release_gate.reasons.push(reason.clone());
        let final_acceptance_status = release_gate_final_acceptance_status(&release_gate);
        let (assurance_level, assurance_reason) = earned_assurance_for_completion(
            "nextjs",
            &required_capabilities,
            true,
            final_acceptance_status,
            &release_gate,
            &telemetry,
        );

        assert_eq!(final_acceptance_status, "incomplete");
        assert_eq!(assurance_level, "reduced");
        assert!(reason.contains("acceptance_gates_disconnected"));
        assert!(reason.contains("browser_readiness_status=not_applicable"));
        assert!(reason.contains("interaction_evidence_status=skipped"));
        assert!(assurance_reason.contains("acceptance_gates_disconnected"));
    }

    #[test]
    fn generic_ultra_promotes_to_python_cli_after_pyproject_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let goal = "Build a local text transformer";
        let plan = two_phase_ultra_plan(goal, "generic");
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(single_write_step_plan_json(
                "Create Python project metadata",
                "pyproject.toml",
            )),
            AssistantReply::text(single_write_step_plan_json(
                "Implement the promoted Python package",
                "src/text_tool/main.py",
            )),
        ]);
        let pyproject = r#"[project]
name = "text-tool"
version = "0.1.0"
"#;
        let main_py = r#"#!/usr/bin/env python3
import sys

def main() -> None:
    text = sys.stdin.read().strip()
    print(f"transformed:{text.upper()}:{len(text)}")

if __name__ == "__main__":
    main()
"#;
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"pyproject.toml","content":pyproject}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/text_tool/main.py","content":main_py}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let promotion = latest_event(&events, "profile_reinferred");
        assert_eq!(
            promotion.get("id").and_then(Value::as_str),
            Some("python-cli")
        );
        let phase_two_prompt = planner_request_text(&planner, 1);
        assert!(phase_two_prompt.contains("Profile: python-cli"));
        assert!(phase_two_prompt.contains("python -m compileall -q src"));
        let final_acceptance = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            final_acceptance.get("profile").and_then(Value::as_str),
            Some("python-cli")
        );
        assert_eq!(
            final_acceptance
                .get("assurance_level")
                .and_then(Value::as_str),
            Some("full")
        );
        assert_eq!(
            final_acceptance
                .get("profile_behavior_probe_status")
                .and_then(Value::as_str),
            Some("pass")
        );
    }

    #[test]
    fn explicit_generic_profile_does_not_promote_from_workspace_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "generic".to_string();
        cfg.profile_explicit = true;
        cfg.eval_events_path = Some(events.clone());
        let goal = "Build an interactive memo app with add and delete actions";
        let plan = two_phase_ultra_plan(goal, "generic");
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(single_write_step_plan_json(
                "Create a package manifest",
                "package.json",
            )),
            AssistantReply::text(single_write_step_plan_json(
                "Create generic app source",
                "memo.jsx",
            )),
        ]);
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({
                        "path": "package.json",
                        "content": r#"{"dependencies":{"next":"^14.2.0"}}"#
                    }),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"memo.jsx","content":generic_interactive_source()}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(!event_text.contains("\"event\":\"profile_reinferred\""));
        let phase_two_prompt = planner_request_text(&planner, 1);
        assert!(phase_two_prompt.contains("Profile: generic"));
        assert!(!phase_two_prompt.contains("Profile: nextjs"));
        let final_acceptance = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            final_acceptance.get("profile").and_then(Value::as_str),
            Some("generic")
        );
        assert_eq!(
            final_acceptance
                .get("assurance_level")
                .and_then(Value::as_str),
            Some("static")
        );
    }

    #[test]
    fn generic_ultra_without_manifest_keeps_static_tier() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let goal = "Build an interactive memo app with add and delete actions";
        let plan = two_phase_ultra_plan(goal, "generic");
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(single_write_step_plan_json("Write notes", "notes.txt")),
            AssistantReply::text(single_write_step_plan_json(
                "Create generic app source",
                "memo.jsx",
            )),
        ]);
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"notes.txt","content":"scaffold notes"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"memo.jsx","content":generic_interactive_source()}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(!event_text.contains("\"event\":\"profile_reinferred\""));
        let final_acceptance = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            final_acceptance.get("profile").and_then(Value::as_str),
            Some("generic")
        );
        assert_eq!(
            final_acceptance
                .get("assurance_level")
                .and_then(Value::as_str),
            Some("static")
        );
    }

    #[test]
    fn profile_promotion_occurs_once_and_ignores_later_manifests() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        write_fake_npm_dependency_installer(dir.path());
        let goal = "Build an interactive browser game on port 3011";
        let plan = two_phase_ultra_plan(goal, "generic");
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(single_write_step_plan_json(
                "Create a package manifest",
                "package.json",
            )),
            AssistantReply::text(generated_nextjs_artifact_plan_json(
                "Complete the promoted app and add another manifest",
            )),
        ]);
        let mut final_calls = nextjs_interactive_app_tool_calls(interactive_game_page_source());
        final_calls.remove(0);
        final_calls.push(crate::state::ToolCall::new(
            "Write",
            serde_json::json!({"path":"pyproject.toml","content":"[project]\nname = \"late-python\"\nversion = \"0.1.0\"\n"}),
        ));
        final_calls.extend(browser_release_evidence_tool_calls());
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({
                        "path": "package.json",
                        "content": nextjs_complete_package_json()
                    }),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: final_calls,
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let events_json = events_with_name(&events, "profile_reinferred");
        assert_eq!(events_json.len(), 1, "{events_json:#?}");
        assert_eq!(
            events_json[0].get("id").and_then(Value::as_str),
            Some("nextjs")
        );
        assert_ne!(
            latest_event(&events, "ultra_final_acceptance")
                .get("profile")
                .and_then(Value::as_str),
            Some("python-cli")
        );
    }

    #[test]
    #[cfg(unix)]
    fn promoted_manifest_repair_reconciles_dependencies_before_later_build_verify() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        write_fake_npm_dependency_installer(dir.path());
        let goal = "Build a static product page on port 3011";
        let plan = two_phase_ultra_plan(goal, "generic");
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(single_write_step_plan_json(
                "Create a lean package manifest",
                "package.json",
            )),
            AssistantReply::text(generated_nextjs_artifact_plan_json_with_build_verify(
                "Complete the promoted Next.js product page",
            )),
        ]);
        let page = "export default function Page(){return <main className=\"min-h-screen\"><h1>Product Page</h1><p>Ready on port 3011</p></main>;}\n";
        let mut final_calls = nextjs_interactive_app_tool_calls(page);
        final_calls.remove(0);
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({
                        "path": "package.json",
                        "content": nextjs_lean_package_json()
                    }),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: final_calls,
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let package = std::fs::read_to_string(dir.path().join("package.json")).unwrap();
        assert!(package.contains("\"autoprefixer\""), "{package}");
        assert!(dir.path().join("node_modules/autoprefixer").is_dir());
        let reconciliations = events_with_name(&events, "dependency_setup_reconciliation");
        assert!(
            reconciliations.iter().any(|event| {
                event.get("trigger").and_then(Value::as_str) == Some("promotion")
                    && event.get("status").and_then(Value::as_str) == Some("passed")
            }),
            "{reconciliations:#?}"
        );
        assert!(
            reconciliations.iter().any(|event| {
                event.get("trigger").and_then(Value::as_str) == Some("manifest_repair")
                    && event.get("status").and_then(Value::as_str) == Some("passed")
                    && event_array_contains(event, "added", "node_modules/autoprefixer")
            }),
            "{reconciliations:#?}"
        );
        let build_lifecycles = events_with_name(&events, "dependency_build_lifecycle");
        assert!(
            build_lifecycles.iter().any(|event| {
                event.get("step_id").and_then(Value::as_str) == Some("create-nextjs-artifacts")
                    && event.get("setup_status").and_then(Value::as_str) == Some("not_required")
                    && event.get("final_status").and_then(Value::as_str) == Some("passed")
            }),
            "{build_lifecycles:#?}"
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(!event_text.contains("dependency_setup_authority_required"));
    }

    #[test]
    #[cfg(unix)]
    fn ultra_run_level_authority_installs_missing_dependencies_without_current_setup_step() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        write_nextjs_profile_workspace(
            dir.path(),
            Some(nextjs_globals_css()),
            Some(nextjs_postcss_config()),
            Some(nextjs_tsconfig_json()),
        );
        crate::planner::profiles::nextjs::repair_manifest_coherence(
            dir.path(),
            "Verify promoted Next.js app",
        )
        .unwrap();
        write_fake_npm_dependency_installer(dir.path());
        let plan = StepPlan {
            goal: "Verify promoted Next.js app".to_string(),
            steps: vec![PlanStep {
                id: "later-build".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Verify the existing promoted app build".to_string(),
                expected_paths: vec!["src/app/page.tsx".to_string()],
                verify: vec!["npm run build".to_string()],
            }],
        };
        let mut run_authority = UltraRunSetupAuthorityState::default();
        run_authority.grant("profile_promotion");
        let mut session = SessionSnapshot::new();
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
            AssistantReply::text("No source changes needed."),
        ]);

        let outcome = run_step_plan_with_session_with_ui_and_run_authority(
            &mut fake,
            &mut session,
            &plan,
            &cfg,
            &NOOP_UI,
            false,
            "ultra-plan-run",
            Some("later-phase"),
            Some("Verify promoted Next.js app"),
            Some(&mut run_authority),
        )
        .unwrap();

        assert_eq!(outcome.completed_steps, 1);
        assert!(dir.path().join("node_modules/autoprefixer").is_dir());
        let build_lifecycles = events_with_name(&events, "dependency_build_lifecycle");
        assert!(
            build_lifecycles.iter().any(|event| {
                event.get("step_id").and_then(Value::as_str) == Some("later-build")
                    && event.get("setup_status").and_then(Value::as_str) == Some("passed")
                    && event.get("setup_authority").and_then(Value::as_str)
                        == Some("plan_setup_step")
                    && event.get("final_status").and_then(Value::as_str) == Some("passed")
            }),
            "{build_lifecycles:#?}"
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(!event_text.contains("dependency_setup_authority_required"));
    }

    #[test]
    fn plan_run_without_setup_step_keeps_dependency_setup_authority_none() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        write_nextjs_profile_workspace(
            dir.path(),
            Some(nextjs_globals_css()),
            Some(nextjs_postcss_config()),
            Some(nextjs_tsconfig_json()),
        );
        let plan = StepPlan {
            goal: "Verify Next.js app without setup authority".to_string(),
            steps: vec![PlanStep {
                id: "plain-build".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Verify the existing app build".to_string(),
                expected_paths: vec!["src/app/page.tsx".to_string()],
                verify: vec!["npm run build".to_string()],
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
            AssistantReply::text("No source changes needed."),
        ]);

        let err = run_step_plan(&mut fake, &plan, &cfg)
            .unwrap_err()
            .to_string();

        assert!(err.contains("dependency_setup_authority_required"), "{err}");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"setup_authority\":\"none\""));
        assert!(!event_text.contains("\"event\":\"dependency_setup_reconciliation\""));
    }

    #[test]
    fn offline_promotion_dependency_reconciliation_stops_honestly() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.offline = true;
        let goal = "Build an interactive browser game on port 3011";
        let plan = two_phase_ultra_plan(goal, "generic");
        let mut planner = FakeClient::new(vec![AssistantReply::text(single_write_step_plan_json(
            "Create a package manifest",
            "package.json",
        ))]);
        let mut execution = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: vec![crate::state::ToolCall::new(
                "Write",
                serde_json::json!({
                    "path": "package.json",
                    "content": nextjs_lean_package_json()
                }),
            )],
            prompt_tokens: None,
            completion_tokens: None,
        }]);

        let err = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg)
            .unwrap_err()
            .to_string();

        assert!(err.contains("dependency_setup_blocked_offline"), "{err}");
        let reconciliations = events_with_name(&events, "dependency_setup_reconciliation");
        assert!(
            reconciliations.iter().any(|event| {
                event.get("trigger").and_then(Value::as_str) == Some("promotion")
                    && event.get("status").and_then(Value::as_str) == Some("blocked")
                    && event.get("primary_reason").and_then(Value::as_str)
                        == Some("dependency_setup_blocked_offline")
            }),
            "{reconciliations:#?}"
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(!event_text.contains("\"event\":\"ultra_plan_complete\""));
    }

    #[test]
    fn known_profile_run_never_reinfers_profile() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        write_fake_npm_dependency_installer(dir.path());
        let goal = "Build an interactive browser game on port 3011";
        let plan = two_phase_ultra_plan(goal, "nextjs");
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(generated_nextjs_artifact_plan_json(
                "Create the Next.js app",
            )),
            AssistantReply::text(generated_nextjs_artifact_plan_json(
                "Finish the Next.js app",
            )),
        ]);
        let mut final_calls = nextjs_interactive_app_tool_calls(interactive_game_page_source());
        final_calls.push(crate::state::ToolCall::new(
            "Write",
            serde_json::json!({"path":"pyproject.toml","content":"[project]\nname = \"ignored\"\nversion = \"0.1.0\"\n"}),
        ));
        final_calls.extend(browser_release_evidence_tool_calls());
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: nextjs_interactive_app_tool_calls(interactive_game_page_source()),
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: final_calls,
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(!event_text.contains("\"event\":\"profile_reinferred\""));
        let final_acceptance = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            final_acceptance.get("profile").and_then(Value::as_str),
            Some("nextjs")
        );
        assert_eq!(
            final_acceptance
                .get("assurance_level")
                .and_then(Value::as_str),
            Some("full")
        );
    }

    #[test]
    fn ultra_run_proceeds_when_step_planner_echoes_phase_prompt_into_goal() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let plan = UltraPlan {
            goal: "Create a polished arcade game with local storage. ".repeat(120),
            profile: "generic".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "arcade-ui-and-local-storage".to_string(),
                    prompt: "Build the arcade UI and local storage persistence.".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "final-marker".to_string(),
                    prompt: "Write the final completion marker.".to_string(),
                },
            ],
        };
        let mut planner = EchoGoalPlanner::new();
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"phase-1.txt","content":"phase one\n"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({
                            "path":"arcade.jsx",
                            "content":"import { useState } from \"react\";\nexport default function Arcade(){\n  const [score, setScore] = useState(0);\n  return <form onSubmit={(event) => { event.preventDefault(); setScore(score + 1); }}><input value={score} onChange={() => setScore(score + 1)} /><button type=\"submit\">Advance</button></form>;\n}\n"
                        }),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"phase-2.txt","content":"phase two\n"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        assert_eq!(planner.messages.len(), 2);
        assert!(dir.path().join("phase-1.txt").is_file());
        assert!(dir.path().join("phase-2.txt").is_file());
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"ultra_phase_scaffold_complete\""));
        assert!(event_text.contains("\"event\":\"planner_plan_sanitized\""));
        assert!(event_text.contains("\"kind\":\"goal_truncated\""));
        assert!(!event_text.contains("phase_scaffold_error"));
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
    fn ultra_observe_no_edit_contract_repair_starts_next_phase() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.completion_contract_path = Some(write_challenge_contract_with_cap(dir.path(), 2));
        cfg.max_iterations = 8;
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
            AssistantReply::text("I inspected the page but made no edit."),
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
        assert_eq!(planner.messages.len(), 2);
        assert!(!dir.path().join(".anvil/repairs").exists());
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"reason\":\"verify_repair_no_change_observed\""));
        assert!(event_text.contains("\"phase_scope\":\"phase-one\""));
        assert!(event_text.contains("\"event\":\"ultra_phase_execute_complete\""));
        assert!(event_text.contains("\"phase_id\":\"phase-two\""));
        let phase_two_prompt = planner_request_text(&planner, 1);
        assert!(phase_two_prompt.contains("Unmet final requirements from earlier phases:"));
        assert!(phase_two_prompt.contains("- challenge_or_adversary_evidence"));
    }

    #[test]
    fn ultra_final_phase_observes_contract_debt_then_final_acceptance_enforces() {
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

        assert!(
            err.contains("ultra final acceptance repair failed"),
            "{err}"
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(!event_text.contains("\"event\":\"plan_final_contract\""));
        assert!(event_text.contains("\"event\":\"ultra_phase_complete\""));
        assert!(event_text.contains("\"phase_id\":\"phase-two\""));
        assert!(event_text.contains("\"event\":\"ultra_final_acceptance\""));
        assert!(event_text.contains("\"event\":\"ultra_final_acceptance_failed\""));
        assert!(event_text.contains("\"event\":\"final_acceptance_repair_start\""));
        assert!(event_text.contains("challenge_or_adversary_evidence"));
        assert!(!event_text.contains("\"event\":\"ultra_plan_complete\""));
    }

    #[test]
    #[cfg(unix)]
    fn ultra_final_acceptance_runs_probe_when_runtime_evidence_is_missing() {
        let _probe_guard = dev_server_probe_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let port = free_local_port();
        enable_dev_server_probe_test_override(dir.path());
        write_fake_nextjs_package_manager(dir.path(), false);
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let scaffold_plan = generated_nextjs_fixture_plan_json_with_kind(
            "Create buildable app",
            "check_scaffold.py",
            "setup",
        );
        let final_plan = final_marker_implement_step_plan_json();
        let mut planner_replies = Vec::new();
        for _ in 0..3 {
            planner_replies.push(AssistantReply::text(scaffold_plan.clone()));
        }
        for _ in 0..6 {
            planner_replies.push(AssistantReply::text(final_plan.clone()));
        }
        let mut planner = FakeClient::new(planner_replies);
        let package = format!(
            r#"{{"scripts":{{"build":"next build","dev":"next dev -p {port}"}},"dependencies":{{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"}},"devDependencies":{{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0"}}}}"#
        );
        let mut app_tool_calls =
            nextjs_interactive_app_tool_calls(interactive_game_page_without_restart_source());
        app_tool_calls[0] = crate::state::ToolCall::new(
            "Write",
            serde_json::json!({"path":"package.json","content":package}),
        );
        app_tool_calls.push(crate::state::ToolCall::new(
            "Write",
            serde_json::json!({
                "path":"interaction-evidence.json",
                "content":"{\"ok\":true,\"interaction_performed\":true,\"start_transition\":true,\"input_state_change\":true,\"input_event_observed\":true,\"state_changed\":true,\"canvas_found\":true}"
            }),
        ));
        app_tool_calls.push(crate::state::ToolCall::new(
            "Write",
            serde_json::json!({"path":"check_scaffold.py","content":"x = 1\n"}),
        ));
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: app_tool_calls,
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({
                        "path":"src/app/page.tsx",
                        "content":interactive_game_page_without_restart_source()
                    }),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("final acceptance repair could not add restart behavior"),
        ]);
        let plan = UltraPlan {
            goal: explicit_port_goal("Create an interactive browser game", port),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "build".to_string(),
                    prompt: "Create the app".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "final".to_string(),
                    prompt: "Final implementation pass".to_string(),
                },
            ],
        };

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let event_text = std::fs::read_to_string(&events).unwrap();
        let final_acceptance = latest_event(&events, "ultra_final_acceptance");
        assert!(event_text.contains("\"event\":\"ultra_phase_complete\""));
        assert!(event_text.contains("\"phase_id\":\"final\""));
        assert!(event_text.contains("\"event\":\"ultra_final_acceptance\""));
        assert!(event_text.contains("\"runtime_acceptance_status\":\"partial\""));
        assert!(event_text.contains("\"final_acceptance_status\":\"partial\""));
        assert!(event_text.contains("\"release_gate_status\":\"partial\""));
        assert!(event_text.contains("\"restart_or_recoverable_state_evidence\""));
        assert!(
            final_acceptance
                .get("unverified_evidence")
                .and_then(Value::as_array)
                .is_some_and(|items| items.iter().any(|item| item.as_str()
                    == Some("restart_or_recoverable_state_evidence:unverified:probe_unavailable")))
        );
        assert!(
            event_text.contains("\"browser_readiness_status\":\"passed\""),
            "{event_text}"
        );
        assert!(
            event_text.contains("\"interaction_evidence_status\":\"passed\""),
            "{event_text}"
        );
        assert!(
            !event_text.contains("\"browser_readiness_status\":\"not_checked\""),
            "{event_text}"
        );
        assert!(event_text.contains("\"event\":\"dev_server_lifecycle\""));
        assert!(event_text.contains("\"stage\":\"probe\""));
        assert!(!event_text.contains("\"event\":\"final_acceptance_repair_start\""));
        assert!(
            events
                .parent()
                .unwrap()
                .join("browser-readiness.json")
                .is_file()
        );
        assert!(!execution.messages.iter().flatten().any(|message| {
            message
                .content
                .contains("Repair the final acceptance failure")
        }));
    }

    #[test]
    fn ultra_probe_unavailable_weak_restart_is_partial_without_repair() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        write_fake_npm_dependency_installer(dir.path());
        let phase_plan = generated_nextjs_artifact_plan_json("Create and verify the game app");
        let mut planner_replies = Vec::new();
        for _ in 0..8 {
            planner_replies.push(AssistantReply::text(phase_plan.clone()));
        }
        let mut planner = FakeClient::new(planner_replies);
        let mut tool_calls = nextjs_interactive_app_tool_calls(
            cross_file_weak_restart_interactive_game_page_source(),
        );
        tool_calls.push(crate::state::ToolCall::new(
            "Write",
            serde_json::json!({
                "path":"src/app/gameEngine.ts",
                "content":cross_file_weak_restart_game_engine_source()
            }),
        ));
        tool_calls.push(crate::state::ToolCall::new(
            "Write",
            serde_json::json!({
                "path":"browser-readiness.json",
                "content":"{\"ok\":true,\"http_status\":200,\"route_rendered\":true}"
            }),
        ));
        let final_phase_reply = AssistantReply {
            content: String::new(),
            tool_calls: vec![
                crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({
                        "path":"src/app/page.tsx",
                        "content":cross_file_weak_restart_interactive_game_page_source()
                    }),
                ),
                crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({
                        "path":"browser-readiness.json",
                        "content":"{\"ok\":true,\"http_status\":200,\"route_rendered\":true}"
                    }),
                ),
            ],
            prompt_tokens: None,
            completion_tokens: None,
        };
        let mut execution_replies = vec![AssistantReply {
            content: String::new(),
            tool_calls,
            prompt_tokens: None,
            completion_tokens: None,
        }];
        for _ in 0..8 {
            execution_replies.push(final_phase_reply.clone());
        }
        let mut execution = FakeClient::new(execution_replies);
        let plan = UltraPlan {
            goal: "Create an interactive browser game".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "build".to_string(),
                    prompt: "Create the interactive game app".to_string(),
                },
                UltraPhase {
                    id: "final".to_string(),
                    prompt: "Verify the final interactive game app".to_string(),
                },
            ],
        };

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let event_text = std::fs::read_to_string(&events).unwrap();
        let final_acceptance = latest_event(&events, "ultra_final_acceptance");
        assert!(event_text.contains("\"event\":\"ultra_final_acceptance\""));
        assert!(event_text.contains("\"runtime_acceptance_status\":\"partial\""));
        assert!(event_text.contains("\"final_acceptance_status\":\"partial\""));
        assert!(event_text.contains("\"release_gate_status\":\"partial\""));
        assert_eq!(
            final_acceptance
                .get("assurance_level")
                .and_then(Value::as_str),
            Some("partial")
        );
        assert!(
            final_acceptance
                .get("unverified_evidence")
                .and_then(Value::as_array)
                .is_some_and(|items| items.iter().any(|item| item.as_str()
                    == Some("restart_or_recoverable_state_evidence:unverified:probe_unavailable")))
        );
        assert_eq!(
            final_acceptance
                .get("evidence_tiers")
                .and_then(|tiers| tiers.get("restart_or_recoverable_state_evidence"))
                .and_then(Value::as_str),
            Some("unverified:probe_unavailable")
        );
        assert!(
            !final_acceptance
                .get("missing_evidence")
                .and_then(Value::as_array)
                .is_some_and(|items| items
                    .iter()
                    .any(|item| item.as_str() == Some("restart_or_recoverable_state_evidence")))
        );
        assert!(event_text.contains("interaction_unverified:probe_unavailable"));
        assert!(!event_text.contains("\"event\":\"final_acceptance_repair_start\""));
        assert!(!execution.messages.iter().flatten().any(|message| {
            message
                .content
                .contains("Repair the final acceptance failure")
        }));
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
    fn ultra_final_acceptance_reports_plan_adherence_without_gating() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "generic".to_string();
        cfg.eval_events_path = Some(events.clone());
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "// canvas pause\nexport default function Page(){ return <main><button>Start</button><div>Score</div></main>; }",
        )
        .unwrap();
        let plan = UltraPlan {
            goal: "Build a simple page".to_string(),
            profile: "generic".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![UltraPhase {
                id: "gameplay".to_string(),
                prompt: "Implement canvas rendering with pause controls and score display"
                    .to_string(),
            }],
        };

        let report = ultra_final_acceptance_report(&plan, &cfg).unwrap();

        assert!(report.is_pass(), "{report:?}");
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(
            event_text.contains("\"plan_adherence_missing\""),
            "{event_text}"
        );
        assert!(event_text.contains("\"canvas\""), "{event_text}");
        assert!(event_text.contains("\"pause\""), "{event_text}");
        assert!(event_text.contains("\"score\""), "{event_text}");
        let snapshot = eval_events::latest_completion_snapshot(Some(&events));
        let projection = eval_events::project_completion(report.is_pass(), &snapshot);
        eval_events::append_completion_summary(
            Some(&events),
            "process",
            None,
            Some("/ultra-plan-run"),
            "completed",
            "",
            &projection,
        );
        let summary = std::fs::read_to_string(events.parent().unwrap().join("summary.md")).unwrap();
        assert!(summary.contains("Plan adherence:"), "{summary}");
        assert!(summary.contains("Missing tokens:"), "{summary}");
        assert!(summary.contains("- canvas"), "{summary}");
        assert!(summary.contains("- pause"), "{summary}");
        assert!(summary.contains("Status: complete"), "{summary}");
    }

    #[test]
    fn plan_adherence_missing_tokens_do_not_change_acceptance_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "generic".to_string();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page(){ return <main><p>Ready</p></main>; }",
        )
        .unwrap();
        let with_adherence_drift = UltraPlan {
            goal: "Build a simple page".to_string(),
            profile: "generic".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![UltraPhase {
                id: "final".to_string(),
                prompt: "Implement keyboard controls, lives counter, and pause mode".to_string(),
            }],
        };
        let without_adherence_drift = UltraPlan {
            phases: vec![UltraPhase {
                id: "final".to_string(),
                prompt: "Finalize the page".to_string(),
            }],
            ..with_adherence_drift.clone()
        };

        let with_report = ultra_final_acceptance_report(&with_adherence_drift, &cfg).unwrap();
        let without_report = ultra_final_acceptance_report(&without_adherence_drift, &cfg).unwrap();

        assert_eq!(with_report, without_report);
        assert!(with_report.is_pass(), "{with_report:?}");
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
                r#"{{"scripts":{{"build":"next build","dev":"next dev -p {port}","start":"next start -p {port}"}},"dependencies":{{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"}},"devDependencies":{{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0","tailwindcss":"^3.4.19","postcss":"^8.5.15","autoprefixer":"^10.4.20"}}}}"#
            ),
        )
        .unwrap();
        std::fs::write(dir.path().join("tsconfig.json"), nextjs_tsconfig_json()).unwrap();
        std::fs::write(
            dir.path().join("postcss.config.js"),
            nextjs_postcss_config(),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("tailwind.config.ts"),
            nextjs_tailwind_config_ts(),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            interactive_game_page_source(),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/layout.tsx"),
            nextjs_layout_source(),
        )
        .unwrap();
        std::fs::write(dir.path().join("src/app/globals.css"), nextjs_globals_css()).unwrap();
        std::fs::write(
            dir.path().join("src/app/global.d.ts"),
            "declare module \"*.css\";",
        )
        .unwrap();
        let plan = UltraPlan {
            goal: explicit_port_goal("Create an interactive browser game", port),
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
        let package = nextjs_complete_package_json();
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
                        serde_json::json!({"path":"tsconfig.json","content":nextjs_tsconfig_json()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"postcss.config.js","content":nextjs_postcss_config()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"tailwind.config.ts","content":nextjs_tailwind_config_ts()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/page.tsx","content":static_page}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/layout.tsx","content":nextjs_layout_source()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/globals.css","content":nextjs_globals_css()}),
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
        assert!(repair_prompt.contains("attempt: 1/2"));
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
        let package = nextjs_complete_package_json();
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
                        serde_json::json!({"path":"tsconfig.json","content":nextjs_tsconfig_json()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"postcss.config.js","content":nextjs_postcss_config()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"tailwind.config.ts","content":nextjs_tailwind_config_ts()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/page.tsx","content":static_page}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/layout.tsx","content":nextjs_layout_source()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/globals.css","content":nextjs_globals_css()}),
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
        assert!(
            err.contains("/run-ultra-plan .anvil/plans/recovery-ultra-plan-"),
            "{err}"
        );
        assert!(err.contains(".yaml"), "{err}");
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
        assert!(err.contains("Recovery UltraPlan YAML saved"), "{err}");
        assert!(err.contains(".yaml"), "{err}");
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
        assert!(!event_text.contains("\"event\":\"planner_fallback_plan\""));
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
    fn ultra_phase_scaffold_provider_error_names_terminal_reason() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let mut planner = FlakyClient::new(
            PLANNER_PROVIDER_REQUEST_ATTEMPTS,
            "transient provider unavailable",
            Vec::new(),
        );
        let mut execution = FakeClient::new(Vec::new());
        let plan = UltraPlan {
            goal: "Build an interactive web game".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "web-game-scaffold".to_string(),
                    prompt: "Scaffold the interactive web game".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "final-verify".to_string(),
                    prompt: "Verify the interactive web game".to_string(),
                },
            ],
        };

        let err = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg)
            .unwrap_err()
            .to_string();

        assert_eq!(planner.messages.len(), PLANNER_PROVIDER_REQUEST_ATTEMPTS);
        assert!(err.contains("phase scaffold failed"), "{err}");
        assert!(
            err.contains("provider request failed after 2 attempts"),
            "{err}"
        );
        assert!(err.contains("transient provider unavailable"), "{err}");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"planner_error_kind\":\"phase_scaffold_error\""));
        assert!(event_text.contains("transient provider unavailable"));
    }

    #[test]
    fn ultra_phase_execute_failure_saves_complete_recovery_yaml_in_stop_reason() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let phase_step_plan = StepPlan {
            goal: "phase one".to_string(),
            steps: vec![PlanStep {
                id: "write-phase-one".to_string(),
                kind: "implement".to_string(),
                expected_result: "phase-one.txt exists".to_string(),
                instruction: "Create phase-one.txt".to_string(),
                expected_paths: vec!["phase-one.txt".to_string()],
                verify: Vec::new(),
            }],
        };
        let mut planner = FakeClient::new(vec![AssistantReply::text(
            serde_json::to_string(&phase_step_plan).unwrap(),
        )]);
        let mut execution = FakeClient::new(Vec::new());
        let plan = UltraPlan {
            goal: "Do two phases".to_string(),
            profile: "generic".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "phase-one".to_string(),
                    prompt: "Create first artifact".to_string(),
                },
                UltraPhase {
                    id: "phase-two".to_string(),
                    prompt: "Finish second artifact".to_string(),
                },
            ],
        };

        let err = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg)
            .unwrap_err()
            .to_string();

        assert!(err.contains("phase phase-one failed"), "{err}");
        assert!(err.contains("Paths:"), "{err}");
        assert!(
            err.contains("/run-ultra-plan .anvil/plans/recovery-ultra-plan-"),
            "{err}"
        );
        assert!(err.contains(".yaml"), "{err}");
        let summary = std::fs::read_to_string(dir.path().join("summary.md")).unwrap();
        assert!(summary.contains("Failed phase:\n- phase-one"), "{summary}");
        assert!(
            summary.contains("Pending phases:\n- phase-two"),
            "{summary}"
        );
        assert!(
            summary.contains("Recovery UltraPlan YAML saved:"),
            "{summary}"
        );
        assert!(summary.contains(".yaml"), "{summary}");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"recovery_handoff_kind\":\"phase_execute_error\""));
        assert!(event_text.contains("\"recovery_ultra_plan_path\":\".anvil/plans/"));
    }

    #[test]
    fn ultra_phase_summary_notes_missing_browser_evidence_when_probe_available_unexercised() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        interaction_probe::write_test_availability_override(dir.path(), true);
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events);
        cfg.profile = "nextjs".to_string();
        let plan = UltraPlan {
            goal: "Create an interactive browser app".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "phase-one".to_string(),
                    prompt: "Create the app shell".to_string(),
                },
                UltraPhase {
                    id: "final-acceptance".to_string(),
                    prompt: "Run final browser acceptance".to_string(),
                },
            ],
        };
        let missing_paths: Vec<String> = Vec::new();
        let missing_signals: Vec<String> = Vec::new();
        let repair_targets = vec!["verifier_command".to_string()];

        let _handoff = save_ultra_phase_recovery_handoff(
            &cfg,
            &plan,
            &plan.phases[0],
            UltraPhaseRecoveryRequest {
                failure_kind: "deterministic_verify_command_bug",
                reason: "verify command malformed",
                missing_paths: &missing_paths,
                missing_signals: &missing_signals,
                repair_targets: &repair_targets,
            },
        )
        .expect("handoff saved");

        let summary = std::fs::read_to_string(dir.path().join("summary.md")).unwrap();
        assert!(
            summary.contains(
                "Browser evidence missing: run failed before final acceptance (interaction probe installed but not exercised)."
            ),
            "{summary}"
        );
    }

    #[test]
    fn ultra_phase_recovery_handoff_renders_observed_missing_evidence_keys() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let plan = challenge_ultra_plan();
        let phase = &plan.phases[0];

        let parts = save_ultra_phase_recovery_handoff(
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
        let message = eval_events::render_stop_reason(&parts);

        assert!(
            message.contains("Recovery UltraPlan YAML saved"),
            "{message}"
        );
        assert!(
            message.contains(".anvil/plans/recovery-ultra-plan-"),
            "{message}"
        );
        assert!(message.contains(".yaml"), "{message}");
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

        assert!(
            err.contains("ultra final acceptance repair failed"),
            "{err}"
        );
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
        assert!(event_text.contains("\"event\":\"ultra_phase_complete\""));
        assert!(!event_text.contains("\"event\":\"ultra_phase_failed\""));
        assert!(event_text.contains("\"event\":\"ultra_final_acceptance_failed\""));
        assert!(event_text.contains("\"event\":\"final_acceptance_repair_start\""));
        assert!(event_text.contains("\"event\":\"final_acceptance_repair_failed\""));
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
    fn ultra_partial_summary_does_not_cut_recovery_yaml_path_mid_token() {
        let recovery_path = ".anvil/plans/recovery-ultra-plan-project-setup-019f2512-5e1e-7cb3-b18a-e71b939f8810.yaml";
        let command = format!("/run-ultra-plan {recovery_path}");
        let old_cut_offset = command.find(".yaml").unwrap() + ".yam".len();
        let prefix_len = 500usize.saturating_sub(old_cut_offset);
        let reason = format!("{} {command}", "x".repeat(prefix_len.saturating_sub(1)));

        let summary = render_ultra_partial_run_summary(UltraPartialRunSummary {
            completed_phases: &[],
            failed_phase: "project-setup",
            pending_phases: &["final-verify".to_string()],
            failure_kind: "phase_scaffold_error",
            reason: &reason,
            recovery_prompt_path: ".anvil/repairs/repair-project-setup.md",
            recovery_yaml_summary: &format!("Recovery UltraPlan YAML saved: {recovery_path}"),
            prompt_command_summary: "Suggested command: unavailable",
            recovery_yaml_command_summary: &format!("Suggested YAML command: {command}"),
            recovery_artifact_check: "Recovery artifact check: prompt_parse_ok=true, yaml_parse_ok=true, command_targets_valid=true",
            browser_evidence_missing_note: None,
        });
        let failure = summary.split("Failure:\n").nth(1).expect("failure block");

        assert!(
            !failure.contains(
                "recovery-ultra-plan-project-setup-019f2512-5e1e-7cb3-b18a-e71b939f8810.yam"
            ),
            "{failure}"
        );
        for token in failure.split_whitespace() {
            if token.contains("recovery-ultra-plan-project-setup") {
                assert!(token.ends_with(".yaml"), "{token}");
            }
        }
        assert!(
            summary.contains(&format!("Suggested YAML command: {command}")),
            "{summary}"
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
        let good_package = nextjs_complete_package_json();
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
                        serde_json::json!({"path":"tsconfig.json","content":nextjs_tsconfig_json()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"postcss.config.js","content":nextjs_postcss_config()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"tailwind.config.ts","content":nextjs_tailwind_config_ts()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/page.tsx","content":"export default function Page(){return <main>Space Invaders</main>;}"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/layout.tsx","content":nextjs_layout_source()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/globals.css","content":nextjs_globals_css()}),
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
                tool_calls: vec![
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"package.json","content":good_package}),
                    ),
                    crate::state::ToolCall::new(
                        "Edit",
                        serde_json::json!({
                            "path":"src/app/page.tsx",
                            "old":"Space Invaders",
                            "new":"Space Invaders with keyboard controls, score, waves, and collisions"
                        }),
                    ),
                ],
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
                .any(|prompt| prompt.contains("Repair the final acceptance failure")),
            "{prompts:#?}"
        );
        assert!(
            prompts
                .iter()
                .any(|prompt| prompt.contains("bounded final acceptance repair")),
            "{prompts:#?}"
        );
        assert!(
            execution.messages.len() >= 3,
            "expected initial phase, follow-up phase, and repair prompts: {prompts:#?}"
        );
    }

    #[test]
    fn final_phase_profile_dependency_repair_runs_in_final_acceptance() {
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
        let package = nextjs_complete_package_json();
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
                tool_calls: vec![
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"README.md","content":"# Recovery note\nThe scaffold exists but implementation still needs task-specific gameplay."}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"package.json","content":package}),
                    ),
                ],
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
        assert!(event_text.contains("\"event\":\"ultra_phase_complete\""));
        assert!(event_text.contains("\"phase_id\":\"finish\""));
        assert!(event_text.contains("\"event\":\"ultra_final_acceptance_failed\""));
        assert!(event_text.contains("dependency missing: next"));
        assert!(event_text.contains("\"event\":\"final_acceptance_repair_start\""));
        assert!(event_text.contains("\"event\":\"final_acceptance_repair_complete\""));
        assert!(!event_text.contains("\"event\":\"profile_repair_complete\""));
        assert!(event_text.contains("\"event\":\"ultra_plan_complete\""));
    }

    #[test]
    fn ultra_plan_python_cli_profile_runs_compile_repair_and_behavior_probe() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "python-cli".to_string();
        cfg.eval_events_path = Some(events.clone());
        let scaffold_plan = r#"{"goal":"Scaffold Python CSV CLI","steps":[{"id":"scaffold","kind":"setup","expected_result":"pass","instruction":"Create pyproject.toml and src/csv_stats/main.py for a CSV file argument CLI","expected_paths":["pyproject.toml","src/csv_stats/main.py"],"verify":[]}]}"#;
        let deps_plan = r#"{"goal":"Prepare Python CLI dependencies","steps":[{"id":"deps","kind":"verify","expected_result":"pass","instruction":"Verify dependency readiness and syntax","expected_paths":["pyproject.toml","src/csv_stats/main.py"],"verify":["python -m compileall -q src"]}]}"#;
        let implement_plan = r#"{"goal":"Implement Python CSV CLI behavior","steps":[{"id":"implement","kind":"implement","expected_result":"pass","instruction":"Implement the CLI so it reads a CSV path argument, prints aggregate values, and changes output when the input file changes","expected_paths":["src/csv_stats/main.py"],"verify":["python -m compileall -q src"]}]}"#;
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(scaffold_plan),
            AssistantReply::text(deps_plan),
            AssistantReply::text(implement_plan),
        ]);
        let pyproject = r#"[project]
name = "csv-stats"
version = "0.1.0"

[project.scripts]
csv-stats = "csv_stats.main:main"
"#;
        let valid_cli = r#"#!/usr/bin/env python3
import csv
import sys
from pathlib import Path


def fmt(value: float) -> str:
    if value.is_integer():
        return str(int(value))
    return f"{value:.3f}".rstrip("0").rstrip(".")


def main() -> None:
    if len(sys.argv) != 2:
        print("usage: csv-stats <file>", file=sys.stderr)
        raise SystemExit(2)
    path = Path(sys.argv[1])
    if not path.is_file():
        print(f"missing file: {path}", file=sys.stderr)
        raise SystemExit(1)
    with path.open(newline="") as handle:
        rows = list(csv.DictReader(handle))
    numeric = {}
    for column in (rows[0].keys() if rows else []):
        values = []
        for row in rows:
            try:
                values.append(float(row[column]))
            except ValueError:
                pass
        if values:
            numeric[column] = (sum(values), sum(values) / len(values), max(values), min(values))
    if not numeric:
        print("no numeric columns", file=sys.stderr)
        raise SystemExit(1)
    print("column | sum | average | max | min")
    for column in sorted(numeric):
        total, average, maximum, minimum = numeric[column]
        print(f"{column} | {fmt(total)} | {fmt(average)} | {fmt(maximum)} | {fmt(minimum)}")

if __name__ == "__main__":
    main()
"#;
        let invalid_cli = r#"#!/usr/bin/env python3
import sys

def main() -> None:
    path = sys.argv[1]
    if path print(path)

if __name__ == "__main__":
    main()
"#;
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"pyproject.toml","content":pyproject}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/csv_stats/main.py","content":valid_cli}),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("dependency verification complete"),
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/csv_stats/main.py","content":invalid_cli}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/csv_stats/main.py","content":valid_cli}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/csv_stats/main.py","content":valid_cli}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/csv_stats/main.py","content":valid_cli}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);
        let plan = UltraPlan {
            goal: "Build a Python CLI that reads a CSV file path argument and prints sum, average, max, and min for numeric columns.".to_string(),
            profile: "python-cli".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                crate::planner::ultra_plan::UltraPhase {
                    id: "scaffold".to_string(),
                    prompt: "Scaffold the Python CLI".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "deps".to_string(),
                    prompt: "Prepare dependencies".to_string(),
                },
                crate::planner::ultra_plan::UltraPhase {
                    id: "implement".to_string(),
                    prompt: "Implement CLI behavior".to_string(),
                },
            ],
        };

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 3 phases");
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(event_text.contains("\"event\":\"ultra_phase_scaffold_complete\""));
        assert!(event_text.contains("\"event\":\"ultra_phase_plan_validated\""));
        assert!(event_text.contains("\"event\":\"ultra_phase_execute_complete\""));
        assert!(event_text.contains("\"event\":\"ultra_phase_complete\""));
        assert!(event_text.contains("\"event\":\"dependency_build_lifecycle\""));
        assert!(event_text.contains("\"mode\":\"ultra-plan-run\""));
        assert!(event_text.contains("\"lifecycle_stage\":\"dependency_setup_build\""));
        assert!(event_text.contains("\"lifecycle_stages\""));
        assert!(event_text.contains("\"verification_passed\""));
        assert!(event_text.contains("\"event\":\"step_verify_failure\""));
        assert!(event_text.contains("implementation_compile_error"));
        assert!(event_text.contains("\"event\":\"step_verify_repair\""));
        assert!(event_text.contains("\"event\":\"profile_behavior_probe\""));
        assert!(event_text.contains("\"profile\":\"python-cli\""));
        assert!(event_text.contains("\"profile_behavior_probe_status\":\"pass\""));
        assert!(event_text.contains("\"event\":\"ultra_final_acceptance\""));
        assert!(event_text.contains("\"evidence_arbitration\""));
        assert!(event_text.contains("\"browser_readiness_status\":\"not_applicable\""));
        assert!(event_text.contains("\"interaction_evidence_status\":\"not_applicable\""));
        assert!(event_text.contains("\"event\":\"ultra_plan_complete\""));
        let final_acceptance = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            final_acceptance
                .get("profile_behavior_probe_status")
                .and_then(Value::as_str),
            Some("pass")
        );
        let behavior_path = dir.path().join(".anvil/evidence/python-cli-behavior.json");
        let behavior = std::fs::read_to_string(&behavior_path).unwrap();
        let behavior_json: Value = serde_json::from_str(&behavior).unwrap();
        let details = behavior_json.get("details").expect("behavior details");
        assert!(
            behavior.contains("\"changed_by_input\": true"),
            "{behavior}"
        );
        assert_eq!(
            details.get("mode").and_then(Value::as_str),
            Some("csv_file_arg")
        );
        assert_eq!(
            details.get("argv_invocation").and_then(Value::as_bool),
            Some(true)
        );
        let first_stdout = details
            .get("first_stdout")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let second_stdout = details
            .get("second_stdout")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(first_stdout.contains("60"), "{first_stdout}");
        assert!(first_stdout.contains("20"), "{first_stdout}");
        assert!(second_stdout.contains("24"), "{second_stdout}");
        assert!(second_stdout.contains("8"), "{second_stdout}");
        assert!(
            dir.path()
                .join(".anvil/evidence/python-cli-fixtures/input-a.csv")
                .is_file()
        );
        assert!(
            dir.path()
                .join(".anvil/evidence/python-cli-fixtures/input-b.csv")
                .is_file()
        );
    }

    #[test]
    fn python_cli_failed_behavior_probe_forces_failed_gate_fields() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "python-cli".to_string();
        cfg.eval_events_path = Some(events.clone());
        std::fs::create_dir_all(dir.path().join("src/anvil_app")).unwrap();
        std::fs::write(
            dir.path().join("pyproject.toml"),
            r#"[project]
name = "anvil-app"
version = "0.1.0"

[project.scripts]
csv-stats = "anvil_app.main:main"
"#,
        )
        .unwrap();
        let entrypoint = dir.path().join("src/anvil_app/main.py");
        std::fs::write(
            &entrypoint,
            r#"#!/usr/bin/env python3
import sys

def main() -> None:
    _path = sys.argv[1] if len(sys.argv) > 1 else ""
    print("csv processed")

if __name__ == "__main__":
    main()
"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            let mut permissions = std::fs::metadata(&entrypoint).unwrap().permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
            std::fs::set_permissions(&entrypoint, permissions).unwrap();
        }
        let plan = UltraPlan {
            goal: "Build a Python CLI that reads a CSV file path argument and prints sum, average, max, and min for numeric columns.".to_string(),
            profile: "python-cli".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![crate::planner::ultra_plan::UltraPhase {
                id: "final".to_string(),
                prompt: "Final acceptance".to_string(),
            }],
        };

        let report = ultra_final_acceptance_report(&plan, &cfg).unwrap();

        assert!(!report.is_pass(), "{report:?}");
        assert!(
            report
                .primary_reason()
                .contains("python_cli_behavior_probe_failed"),
            "{report:?}"
        );
        let final_acceptance = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            final_acceptance
                .get("profile_behavior_probe_status")
                .and_then(Value::as_str),
            Some("failed")
        );
        assert_eq!(
            final_acceptance
                .get("runtime_acceptance_passed")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            final_acceptance
                .get("runtime_acceptance_status")
                .and_then(Value::as_str),
            Some("failed")
        );
        assert_eq!(
            final_acceptance
                .get("release_gate_status")
                .and_then(Value::as_str),
            Some("failed")
        );
        assert_eq!(
            final_acceptance
                .get("final_acceptance_status")
                .and_then(Value::as_str),
            Some("incomplete")
        );
        assert_ne!(
            final_acceptance
                .get("assurance_level")
                .and_then(Value::as_str),
            Some("full")
        );
        let release_gate_reasons = final_acceptance
            .get("release_gate_reasons")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            release_gate_reasons.iter().any(|reason| {
                reason
                    .as_str()
                    .is_some_and(|reason| reason.contains("profile_behavior_probe_failed"))
            }),
            "{release_gate_reasons:?}"
        );
        assert!(
            release_gate_reasons.iter().any(|reason| {
                reason.as_str().is_some_and(|reason| {
                    reason.contains(
                        "profile_behavior_probe_evidence:.anvil/evidence/python-cli-behavior.json",
                    )
                })
            }),
            "{release_gate_reasons:?}"
        );
        let behavior_probe = latest_event(&events, "profile_behavior_probe");
        assert_eq!(
            behavior_probe.get("status").and_then(Value::as_str),
            Some("failed")
        );
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
        write_fake_npm_dependency_installer(dir.path());
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
        let package = nextjs_complete_package_json();
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
                        serde_json::json!({"path":"tsconfig.json","content":nextjs_tsconfig_json()}),
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
                        serde_json::json!({"path":"tailwind.config.ts","content":nextjs_tailwind_config_ts()}),
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
        write_fake_npm_dependency_installer(dir.path());
        let step_json = generated_nextjs_artifact_plan_json("Scaffold project");
        let mut planner = FakeClient::new(
            (0..3)
                .map(|_| AssistantReply::text(step_json.clone()))
                .collect(),
        );
        let package = nextjs_complete_package_json();
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
                        serde_json::json!({"path":"postcss.config.js","content":nextjs_postcss_config()}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"tailwind.config.ts","content":nextjs_tailwind_config_ts()}),
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
        assert!(
            err.contains("/run-ultra-plan .anvil/plans/recovery-ultra-plan-"),
            "{err}"
        );
        assert!(err.contains(".yaml"), "{err}");
        assert!(dir.path().join(".anvil/repairs").is_dir());
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"profile_invariant_repair\""));
        assert!(event_text.contains("\"method\":\"deterministic\""));
        assert!(event_text.contains("\"method\":\"model\""));
        assert!(event_text.contains("\"recovery_handoff_kind\":\"profile_invariant_failure\""));
        assert!(event_text.contains("\"recovery_ultra_plan_path\":\".anvil/plans/"));
    }

    #[test]
    fn profile_invariant_handoff_uses_final_import_evidence_not_stale_postcss() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events);
        write_nextjs_profile_workspace(dir.path(), None, Some(nextjs_postcss_config()), None);
        let plan = UltraPlan {
            goal: "3011 port app".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![crate::planner::ultra_plan::UltraPhase {
                id: "scaffold".to_string(),
                prompt: "Scaffold project".to_string(),
            }],
        };
        let phase = &plan.phases[0];
        let evidence = fresh_profile_invariant_failure_evidence(
            &cfg,
            &plan,
            &ProfileSnapshot::None,
            &nextjs_scaffold_expected_paths(),
        );
        let reason = evidence.report.primary_reason();

        assert!(reason.contains("missing relative imports"), "{reason}");
        assert!(reason.contains("src/app/globals.css"), "{reason}");
        assert!(!reason.contains("PostCSS config file missing"), "{reason}");
        assert_eq!(
            reason,
            verify_profile_invariant(dir.path(), "nextjs", &plan.goal, &ProfileSnapshot::None)
                .primary_reason()
        );
        assert!(
            evidence
                .missing_paths
                .contains(&"src/app/globals.css".to_string()),
            "{:?}",
            evidence.missing_paths
        );

        let prompt = profile_invariant_model_repair_prompt(
            &plan,
            phase,
            &evidence.report,
            &UltraRunContext::default(),
            &nextjs_scaffold_expected_paths(),
            &cfg,
            None,
        );
        assert!(
            prompt.contains(
                "src/app/layout.tsx imports ./globals.css which does not exist - create src/app/globals.css"
            ),
            "{prompt}"
        );

        let _handoff = save_ultra_phase_recovery_handoff_with_evidence(
            &cfg,
            &plan,
            phase,
            UltraPhaseRecoveryRequest {
                failure_kind: "profile_invariant_failure",
                reason: &reason,
                missing_paths: &evidence.missing_paths,
                missing_signals: &[],
                repair_targets: &["profile_contract".to_string()],
            },
            &evidence.failure_evidence,
        );
        let repair_text = std::fs::read_dir(dir.path().join(".anvil/repairs"))
            .unwrap()
            .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
            .find(|text| text.contains("profile_invariant_failure"))
            .expect("profile invariant recovery prompt");
        assert!(
            repair_text.contains("Missing paths:\n- src/app/globals.css"),
            "{repair_text}"
        );
        assert!(
            repair_text.contains("src/app/layout.tsx imports ./globals.css which does not exist"),
            "{repair_text}"
        );
        assert!(
            !repair_text.contains("PostCSS config file missing"),
            "{repair_text}"
        );
    }

    #[test]
    fn profile_invariant_fresh_evidence_reason_matches_final_reverification() {
        for outcome in [
            "valid-postcss-missing-globals",
            "missing-postcss",
            "bad-tsconfig",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let cfg = config(dir.path().to_path_buf());
            let postcss = (outcome != "missing-postcss").then_some(nextjs_postcss_config());
            let tsconfig = (outcome == "bad-tsconfig")
                .then_some("{\"compilerOptions\":{\"rootDir\":\"src\"}}\n");
            let globals =
                (outcome != "valid-postcss-missing-globals").then_some(nextjs_globals_css());
            write_nextjs_profile_workspace(dir.path(), globals, postcss, tsconfig);
            let plan = UltraPlan {
                goal: "3011 port app".to_string(),
                profile: "nextjs".to_string(),
                style: "default".to_string(),
                intent: "create".to_string(),
                phases: vec![crate::planner::ultra_plan::UltraPhase {
                    id: outcome.to_string(),
                    prompt: "Simulated repair outcome".to_string(),
                }],
            };

            let evidence = fresh_profile_invariant_failure_evidence(
                &cfg,
                &plan,
                &ProfileSnapshot::None,
                &nextjs_scaffold_expected_paths(),
            );
            let reverified =
                verify_profile_invariant(dir.path(), "nextjs", &plan.goal, &ProfileSnapshot::None);

            assert_eq!(
                evidence.report.primary_reason(),
                reverified.primary_reason(),
                "outcome {outcome}"
            );
        }
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
        let page = interactive_game_page_source();
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
    #[cfg(unix)]
    fn dev_server_cleanup_kills_grandchild_process_group_without_pipe_deadlock() {
        let dir = tempfile::tempdir().unwrap();
        let port = free_local_port();
        let events = dir.path().join("events.jsonl");
        write_fake_nextjs_dev_workspace(dir.path(), port, true);
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let evidence_path = nextjs_dev_route_evidence_path(&cfg);

        let started = Instant::now();
        let evidence = run_nextjs_dev_route_probe_with_runtime(
            &cfg,
            &evidence_path,
            true,
            cleanup_dev_server_child,
            BrowserInteractionProbeOptions::default(),
            Some(port),
        );

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "cleanup should be bounded and fast"
        );
        assert_eq!(evidence.get("ok").and_then(Value::as_bool), Some(true));
        assert!(evidence_path.is_file(), "browser readiness evidence");
        assert!(evidence_path.with_file_name("dev-server.out").is_file());
        assert!(evidence_path.with_file_name("dev-server.err").is_file());
        let events_json = read_jsonl_events(&events);
        assert_eq!(
            dev_server_stage_names(&events_json),
            vec!["start", "wait", "probe", "cleanup"]
        );
        let cleanup = events_json
            .iter()
            .find(|event| {
                event.get("event").and_then(Value::as_str) == Some("dev_server_lifecycle")
                    && event.get("stage").and_then(Value::as_str) == Some("cleanup")
            })
            .expect("cleanup event");
        assert_eq!(cleanup.get("ok").and_then(Value::as_bool), Some(true));
        let pid = events_json
            .iter()
            .find(|event| {
                event.get("event").and_then(Value::as_str) == Some("dev_server_lifecycle")
                    && event.get("stage").and_then(Value::as_str) == Some("start")
            })
            .and_then(|event| event.get("pid"))
            .and_then(Value::as_u64)
            .expect("dev server pid") as u32;
        assert!(
            wait_until_process_group_gone(pid, Duration::from_secs(2)),
            "process group {pid} should be gone"
        );
    }

    #[test]
    #[cfg(unix)]
    fn dev_server_writes_readiness_before_forced_cleanup_failure() {
        let dir = tempfile::tempdir().unwrap();
        let port = free_local_port();
        let events = dir.path().join("events.jsonl");
        write_fake_nextjs_dev_workspace(dir.path(), port, false);
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let evidence_path = nextjs_dev_route_evidence_path(&cfg);

        let evidence = run_nextjs_dev_route_probe_with_runtime(
            &cfg,
            &evidence_path,
            true,
            forced_cleanup_timeout_after_real_cleanup,
            BrowserInteractionProbeOptions::default(),
            Some(port),
        );

        assert_eq!(evidence.get("ok").and_then(Value::as_bool), Some(true));
        let readiness_text =
            std::fs::read_to_string(&evidence_path).expect("readiness evidence written");
        assert!(readiness_text.contains("\"route_rendered\": true"));
        let events_json = read_jsonl_events(&events);
        assert_eq!(
            dev_server_stage_names(&events_json),
            vec!["start", "wait", "probe", "cleanup"]
        );
        let cleanup = events_json
            .iter()
            .find(|event| {
                event.get("event").and_then(Value::as_str) == Some("dev_server_lifecycle")
                    && event.get("stage").and_then(Value::as_str) == Some("cleanup")
            })
            .expect("cleanup event");
        assert_eq!(cleanup.get("ok").and_then(Value::as_bool), Some(false));
        assert_eq!(
            cleanup.get("failure_kind").and_then(Value::as_str),
            Some("dev_server_cleanup_timeout")
        );
    }

    #[test]
    #[cfg(unix)]
    fn slash_ultra_final_flow_reaches_stop_after_fake_dev_server_cleanup() {
        let _probe_guard = dev_server_probe_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let port = free_local_port();
        let events = dir.path().join(".anvil/runs/fake/events.jsonl");
        enable_dev_server_probe_test_override(dir.path());
        write_fake_nextjs_package_manager(dir.path(), false);
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let plan = UltraPlan {
            goal: explicit_port_goal("Create an interactive browser game", port),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "build".to_string(),
                    prompt: "Create the final interactive app".to_string(),
                },
                UltraPhase {
                    id: "final".to_string(),
                    prompt: "Verify final acceptance evidence".to_string(),
                },
            ],
        };
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(render_ultra_plan(&plan)),
            AssistantReply::text(generated_nextjs_artifact_plan_json_with_build_verify(
                "Create the final interactive app",
            )),
            AssistantReply::text(generated_nextjs_artifact_plan_json_with_build_verify(
                "Verify final acceptance evidence",
            )),
        ]);
        let package = format!(
            r#"{{"scripts":{{"build":"next build","dev":"next dev -p {port}","start":"next start -p {port}"}},"dependencies":{{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"}},"devDependencies":{{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0","tailwindcss":"^3.4.19","postcss":"^8.5.15","autoprefixer":"^10.4.20"}}}}"#
        );
        let mut tool_calls = nextjs_interactive_app_tool_calls(interactive_game_page_source());
        tool_calls[0] = crate::state::ToolCall::new(
            "Write",
            serde_json::json!({"path":"package.json","content":package}),
        );
        tool_calls.push(crate::state::ToolCall::new(
            "Write",
            serde_json::json!({
                "path":"interaction-evidence.json",
                "content":"{\"ok\":true,\"interaction_performed\":true,\"start_transition\":true,\"input_state_change\":true,\"input_event_observed\":true,\"state_changed\":true,\"canvas_found\":true}"
            }),
        ));
        let mut execution_replies = vec![
            AssistantReply {
                content: String::new(),
                tool_calls,
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({
                        "path":"interaction-evidence.json",
                        "content":"{\"ok\":true,\"interaction_performed\":true,\"start_transition\":true,\"input_state_change\":true,\"input_event_observed\":true,\"state_changed\":true,\"canvas_found\":true}"
                    }),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ];
        execution_replies.extend(
            (0..8).map(|_| AssistantReply::text("final app artifacts and evidence are ready")),
        );
        let mut execution = FakeClient::new(execution_replies);

        let command = format!(
            "/ultra-plan-run --profile nextjs \"Create an interactive browser game on port {port}\""
        );
        let result = crate::tui::slash::handle_command(
            &command,
            &cfg,
            &mut planner,
            &mut execution,
            &NOOP_UI,
        );
        let output = match result {
            Ok(output) => output,
            Err(err) => {
                let event_text = std::fs::read_to_string(&events).unwrap_or_default();
                panic!(
                    "slash command failed: {err}; planner_requests={}; execution_requests={}; events={event_text}",
                    planner.messages.len(),
                    execution.messages.len()
                );
            }
        };

        assert!(output.contains("ultra-plan-run complete: 2 phases"));
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(event_text.contains("\"event\":\"ultra_phase_complete\""));
        assert!(event_text.contains("\"event\":\"dev_server_lifecycle\""));
        assert!(event_text.contains("\"stage\":\"probe\""));
        assert!(event_text.contains("\"stage\":\"cleanup\""));
        assert!(event_text.contains("\"event\":\"ultra_final_acceptance\""));
        assert!(event_text.contains("\"runtime_acceptance_status\":\"pass\""));
        assert!(event_text.contains("\"final_acceptance_status\":\"full_success\""));
        assert!(event_text.contains("\"browser_readiness_status\":\"passed\""));
        assert!(event_text.contains("\"interaction_evidence_status\":\"passed\""));
        assert!(event_text.contains("\"event\":\"tui_command_stop\""));
        assert!(
            dir.path()
                .join(".anvil/runs/fake/browser-readiness.json")
                .is_file()
        );
        let summary = std::fs::read_to_string(dir.path().join(".anvil/runs/fake/summary.md"))
            .expect("summary");
        assert!(summary.contains("Runtime acceptance: pass"));
        assert!(summary.contains("Final acceptance: full_success"));
        assert!(summary.contains("Release gate: pass"));
    }

    #[test]
    #[ignore]
    #[cfg(unix)]
    #[allow(clippy::zombie_processes)]
    fn fake_dev_server_package_manager_child() {
        if std::env::var("ANVIL_FAKE_DEV_SERVER_CHILD").ok().as_deref() != Some("1") {
            return;
        }
        if std::env::var("ANVIL_FAKE_DEV_SERVER_GRANDCHILD")
            .ok()
            .as_deref()
            == Some("1")
        {
            let _ = std::process::Command::new("sh")
                .arg("-c")
                .arg("sleep 300")
                .spawn()
                .expect("spawn grandchild");
        }
        let port = std::env::var("PORT")
            .expect("PORT")
            .parse::<u16>()
            .expect("PORT number");
        let listener = std::net::TcpListener::bind(("127.0.0.1", port)).expect("bind fake server");
        listener.set_nonblocking(true).unwrap();
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut request = [0_u8; 1024];
                    let _ = stream.read(&mut request);
                    let body = "<html><body>fake next ready</body></html>";
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(err) => panic!("fake server accept failed: {err}"),
            }
        }
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
                "\"interaction_evidence_status\":\"unavailable:playwright_not_installed\""
            ),
            "{event_text}"
        );
        assert!(event_text.contains("interaction_unverified:probe_unavailable"));
        assert!(!event_text.contains("browser_interaction_evidence_required"));
    }

    #[test]
    #[cfg(unix)]
    fn fake_interaction_probe_success_passes_release_gate() {
        let _probe_guard = dev_server_probe_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let port = free_local_port();
        let events = dir.path().join(".anvil/runs/fake/events.jsonl");
        write_fake_nextjs_dev_workspace(dir.path(), port, false);
        interaction_probe::write_test_availability_override(dir.path(), true);
        interaction_probe::write_test_result_override(
            dir.path(),
            &serde_json::json!({
                "ok": true,
                "status": "passed",
                "start_transition": true,
                "input_state_evaluated_after_start": true,
                "input_state_change": true,
                "state_changed": true,
                "visible_state_changed": true,
                "steps": ["surface_visible", "start_transition", "control_input_dispatched", "input_state_evaluated_after_start", "input_state_change"],
                "before_marker": "menu",
                "after_marker": "running",
                "duration_ms": 11
            }),
        );
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events);
        let evidence_path = nextjs_dev_route_evidence_path(&cfg);

        let readiness = run_nextjs_dev_route_probe_with_runtime(
            &cfg,
            &evidence_path,
            true,
            cleanup_dev_server_child,
            BrowserInteractionProbeOptions::default(),
            Some(port),
        );

        assert_eq!(readiness.get("ok").and_then(Value::as_bool), Some(true));
        let interaction_path = evidence_path.with_file_name("browser-interaction.json");
        assert!(interaction_path.is_file(), "run interaction evidence");
        assert!(
            interaction_probe::browser_interaction_evidence_path(dir.path()).is_file(),
            "workspace interaction evidence"
        );
        let gate = browser_release_gate(&cfg);
        assert_eq!(gate.status, "pass", "{gate:?}");
        assert_eq!(gate.interaction_evidence_status, "passed");
    }

    #[test]
    #[cfg(unix)]
    fn fake_interaction_probe_failure_fails_gate_without_evidence_repair_target() {
        let _probe_guard = dev_server_probe_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let port = free_local_port();
        let events = dir.path().join(".anvil/runs/fake/events.jsonl");
        write_fake_nextjs_dev_workspace(dir.path(), port, false);
        interaction_probe::write_test_availability_override(dir.path(), true);
        interaction_probe::write_test_result_override(
            dir.path(),
            &serde_json::json!({
                "ok": false,
                "status": "failed",
                "steps": ["surface_visible", "control_input_dispatched"],
                "before_marker": "menu",
                "after_marker": "menu",
                "failure_kind": "start_transition_missing",
                "duration_ms": 13
            }),
        );
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events);
        let evidence_path = nextjs_dev_route_evidence_path(&cfg);

        let readiness = run_nextjs_dev_route_probe_with_runtime(
            &cfg,
            &evidence_path,
            true,
            cleanup_dev_server_child,
            BrowserInteractionProbeOptions::default(),
            Some(port),
        );

        assert_eq!(readiness.get("ok").and_then(Value::as_bool), Some(true));
        let gate = browser_release_gate(&cfg);
        assert_eq!(gate.status, "failed", "{gate:?}");
        assert_eq!(
            gate.reasons,
            vec!["browser_interaction_failed:start_transition_missing".to_string()]
        );
        assert_eq!(
            release_recovery_repair_targets(&gate, None),
            vec!["start_control_wiring".to_string()]
        );

        let mut report = VerificationReport::pass();
        report.push_profile_failure(format!("release gate failed: {}", gate.reasons.join("; ")));
        append_release_gate_observation_failures(&mut report, &gate);
        let plan = UltraPlan {
            goal: "Create an interactive browser game".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: Vec::new(),
        };
        let prompt = final_acceptance_repair_prompt(
            Path::new("."),
            &plan,
            &report,
            &UltraRunContext::default(),
            "capability_missing",
            &["src/app/page.tsx".to_string()],
            &[],
            (1, 2),
            false,
            false,
        );
        assert!(prompt.contains("start_transition_missing"), "{prompt}");
        assert!(!prompt.contains("interaction_evidence_missing"), "{prompt}");
        assert!(
            !prompt.contains("browser_interaction_evidence_required"),
            "{prompt}"
        );
        assert!(!prompt.contains("interaction evidence status"), "{prompt}");
    }

    #[test]
    fn interaction_probe_failure_evidence_notes_slow_cold_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("browser-interaction.json");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": true,
                "status": "passed",
                "cold_start_ms": 20_400,
                "probe_mode": "heuristic"
            }))
            .unwrap(),
        )
        .unwrap();

        let lines = interaction_probe_failure_evidence_lines(&path.display().to_string());

        assert!(
            lines.iter().any(|line| {
                line == "Note: first page load took 20s (cold start; excluded from assertions)"
            }),
            "{lines:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn ultra_final_acceptance_runs_probe_before_behavior_arbitration() {
        let _probe_guard = dev_server_probe_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let port = free_local_port();
        let events = dir.path().join(".anvil/runs/order/events.jsonl");
        enable_dev_server_probe_test_override(dir.path());
        write_probe_nextjs_workspace(dir.path(), port, interactive_game_page_source());
        interaction_probe::write_test_availability_override(dir.path(), true);
        interaction_probe::write_test_result_override(
            dir.path(),
            &interaction_state_missing_probe_result(),
        );
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let plan = UltraPlan {
            goal: explicit_port_goal("Create an interactive browser game", port),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![UltraPhase {
                id: "final".to_string(),
                prompt: "Final acceptance".to_string(),
            }],
        };

        let report = ultra_final_acceptance_report(&plan, &cfg).unwrap();

        assert!(!report.is_pass(), "{report:?}");
        assert_eq!(
            classify_repair_target(&report),
            RepairTarget::Implementation
        );
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(!event_text.contains("probe_unavailable"), "{event_text}");
        assert!(
            !event_text.contains("/setup-interaction-probe"),
            "{event_text}"
        );
        assert!(
            event_text
                .contains("browser_interaction_failed:input_state_change_missing_after_start")
        );
        let ultra = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            ultra
                .get("interaction_evidence_status")
                .and_then(Value::as_str),
            Some("failed:input_state_change_missing_after_start")
        );
        let arbitration = ultra
            .get("evidence_arbitration")
            .and_then(Value::as_object)
            .expect("evidence arbitration");
        let stateful = arbitration
            .get("stateful_update_evidence")
            .and_then(Value::as_object)
            .expect("stateful update arbitration");
        assert_eq!(
            stateful
                .get("behavioral_observation")
                .and_then(Value::as_str),
            Some("input_state_change_missing_after_start")
        );
        assert_eq!(
            stateful.get("final_tier").and_then(Value::as_str),
            Some("absent")
        );
    }

    #[test]
    #[cfg(unix)]
    #[ignore = "covered by focused final-acceptance repair/reprobe tests without intermediate phase repair"]
    fn behavioral_interaction_failure_repairs_and_reprobes_to_success() {
        let _probe_guard = dev_server_probe_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let port = free_local_port();
        let events = dir.path().join(".anvil/runs/repair-success/events.jsonl");
        enable_dev_server_probe_test_override(dir.path());
        write_probe_nextjs_workspace(dir.path(), port, hollow_canvas_game_page_source());
        interaction_probe::write_test_availability_override(dir.path(), true);
        interaction_probe::write_test_result_overrides(
            dir.path(),
            &[
                interaction_state_missing_probe_result(),
                interaction_state_changed_probe_result(),
            ],
        );
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let scaffold_plan = generated_nextjs_fixture_plan_json_with_kind(
            "Create buildable app",
            "check_scaffold.py",
            "setup",
        );
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(scaffold_plan),
            AssistantReply::text(final_marker_implement_step_plan_json()),
        ]);
        let mut execution = FakeClient::new(vec![
            probe_nextjs_scaffold_reply(port, interactive_game_page_source().to_string()),
            probe_nextjs_scaffold_reply(port, interactive_game_page_variant(1)),
            probe_nextjs_scaffold_reply(port, interactive_game_page_variant(2)),
            probe_nextjs_scaffold_reply(port, interactive_game_page_variant(3)),
            probe_nextjs_scaffold_reply(port, interactive_game_page_variant(4)),
        ]);
        let plan = UltraPlan {
            goal: "Create an interactive browser game".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "first".to_string(),
                    prompt: "First implementation pass".to_string(),
                },
                UltraPhase {
                    id: "final".to_string(),
                    prompt: "Final implementation pass".to_string(),
                },
            ],
        };

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(event_text.contains("\"event\":\"final_acceptance_repair_start\""));
        assert!(!event_text.contains("\"event\":\"final_acceptance_repair_exhausted\""));
        assert!(event_text.contains("\"event\":\"ultra_plan_complete\""));
        assert!(
            event_text
                .matches("\"event\":\"browser_interaction_probe\"")
                .count()
                >= 2,
            "{event_text}"
        );
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
            .expect("repair prompt");
        assert!(
            repair_prompt.contains(
                "ArrowLeft keydown, ArrowRight keydown, Space keydown, canvas/center click"
            )
        );
        assert!(repair_prompt.contains("player=20 score=0 health=3"));
        assert!(repair_prompt.contains(INTERACTION_STATE_CHANGE_REPAIR_REQUIREMENT));
        assert!(repair_prompt.contains("Route-bound implementation targets:"));
        assert!(repair_prompt.contains("src/app/page.tsx"));
    }

    #[test]
    #[cfg(unix)]
    #[ignore = "covered by focused final-acceptance repair/reprobe tests without intermediate phase repair"]
    fn behavioral_interaction_failure_exhausts_after_two_reprobe_cycles() {
        let _probe_guard = dev_server_probe_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let port = free_local_port();
        let events = dir.path().join(".anvil/runs/repair-exhaust/events.jsonl");
        enable_dev_server_probe_test_override(dir.path());
        write_probe_nextjs_workspace(dir.path(), port, interactive_game_page_source());
        interaction_probe::write_test_availability_override(dir.path(), true);
        interaction_probe::write_test_result_overrides(
            dir.path(),
            &[
                interaction_state_missing_probe_result(),
                interaction_state_missing_probe_result(),
                interaction_state_missing_probe_result(),
            ],
        );
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let scaffold_plan = generated_nextjs_fixture_plan_json_with_kind(
            "Create buildable app",
            "check_scaffold.py",
            "setup",
        );
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(scaffold_plan),
            AssistantReply::text(final_marker_implement_step_plan_json()),
        ]);
        let mut execution = FakeClient::new(vec![
            probe_nextjs_scaffold_reply(port, interactive_game_page_source().to_string()),
            probe_nextjs_scaffold_reply(port, interactive_game_page_variant(1)),
            probe_nextjs_scaffold_reply(port, interactive_game_page_variant(2)),
            probe_nextjs_scaffold_reply(port, interactive_game_page_variant(3)),
            probe_nextjs_scaffold_reply(port, interactive_game_page_variant(4)),
            probe_nextjs_scaffold_reply(port, interactive_game_page_variant(5)),
        ]);
        let plan = UltraPlan {
            goal: "Create an interactive browser game".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "first".to_string(),
                    prompt: "First implementation pass".to_string(),
                },
                UltraPhase {
                    id: "final".to_string(),
                    prompt: "Final implementation pass".to_string(),
                },
            ],
        };

        let err = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg)
            .unwrap_err()
            .to_string();

        assert!(
            err.contains(
                "ultra final acceptance failed after bounded repair: browser_interaction_failed:input_state_change_missing_after_start"
            ),
            "{err}"
        );
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert_eq!(
            event_text
                .matches("\"event\":\"final_acceptance_repair_start\"")
                .count(),
            2,
            "{event_text}"
        );
        assert!(event_text.contains(
            "\"failure_kind\":\"browser_interaction_failed:input_state_change_missing_after_start\""
        ));
        assert!(!event_text.contains("interaction_unverified_probe_unavailable"));
        assert!(!event_text.contains("/setup-interaction-probe"));
        let recovery_plan = assert_single_recovery_ultra_plan(dir.path());
        let recovery_text = render_ultra_plan(&recovery_plan);
        assert!(
            recovery_text.contains("input_state_render_wiring"),
            "{recovery_text}"
        );
        assert!(
            recovery_text.contains(INTERACTION_STATE_CHANGE_REPAIR_REQUIREMENT),
            "{recovery_text}"
        );
        assert!(!recovery_text.contains("/setup-interaction-probe"));
    }

    #[test]
    #[cfg(unix)]
    #[ignore = "existing probe-unavailable partial tests cover this without intermediate phase repair"]
    fn probe_unavailable_environment_remains_partial_without_final_repair() {
        let _probe_guard = dev_server_probe_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let port = free_local_port();
        let events = dir
            .path()
            .join(".anvil/runs/probe-unavailable/events.jsonl");
        enable_dev_server_probe_test_override(dir.path());
        write_probe_nextjs_workspace(dir.path(), port, interactive_game_page_source());
        interaction_probe::write_test_availability_override(dir.path(), false);
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let scaffold_plan = generated_nextjs_fixture_plan_json_with_kind(
            "Create buildable app",
            "check_scaffold.py",
            "setup",
        );
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(scaffold_plan),
            AssistantReply::text(final_marker_implement_step_plan_json()),
        ]);
        let mut execution = FakeClient::new(vec![
            probe_nextjs_scaffold_reply(port, interactive_game_page_source().to_string()),
            probe_nextjs_scaffold_reply(port, interactive_game_page_variant(1)),
            probe_nextjs_scaffold_reply(port, interactive_game_page_variant(2)),
            probe_nextjs_scaffold_reply(port, interactive_game_page_variant(3)),
        ]);
        let plan = UltraPlan {
            goal: "Create an interactive browser game".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "first".to_string(),
                    prompt: "First implementation pass".to_string(),
                },
                UltraPhase {
                    id: "final".to_string(),
                    prompt: "Final implementation pass".to_string(),
                },
            ],
        };

        let result = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap();

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(event_text.contains("\"final_acceptance_status\":\"partial\""));
        assert!(event_text.contains("interaction_unverified:probe_unavailable"));
        assert!(event_text.contains("/setup-interaction-probe"));
        assert!(!event_text.contains("\"event\":\"final_acceptance_repair_start\""));
    }

    #[test]
    #[cfg(unix)]
    fn focused_behavioral_repair_prompt_and_reprobe_passes() {
        let _probe_guard = dev_server_probe_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let port = free_local_port();
        let events = dir.path().join(".anvil/runs/focused-success/events.jsonl");
        enable_dev_server_probe_test_override(dir.path());
        write_probe_nextjs_workspace(dir.path(), port, interactive_game_page_source());
        interaction_probe::write_test_availability_override(dir.path(), true);
        interaction_probe::write_test_result_overrides(
            dir.path(),
            &[
                interaction_state_missing_probe_result(),
                interaction_state_changed_probe_result(),
            ],
        );
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let plan = UltraPlan {
            goal: explicit_port_goal("Create an interactive browser game", port),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![UltraPhase {
                id: "final".to_string(),
                prompt: "Final acceptance".to_string(),
            }],
        };
        let initial_report = ultra_final_acceptance_report(&plan, &cfg).unwrap();
        assert!(!initial_report.is_pass(), "{initial_report:?}");
        assert_eq!(
            classify_repair_target(&initial_report),
            RepairTarget::Implementation
        );
        let expected_paths =
            final_acceptance_repair_expected_paths(&plan, &cfg, &initial_report).unwrap();
        let repair_prompt = final_acceptance_repair_prompt(
            &cfg.workspace_root,
            &plan,
            &initial_report,
            &UltraRunContext::default(),
            RepairTarget::Implementation.as_str(),
            &expected_paths,
            &[],
            (1, FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS),
            false,
            false,
        );
        assert!(
            repair_prompt.contains(
                "ArrowLeft keydown, ArrowRight keydown, Space keydown, canvas/center click"
            )
        );
        assert!(repair_prompt.contains("player=20 score=0 health=3"));
        assert!(repair_prompt.contains(INTERACTION_STATE_CHANGE_REPAIR_REQUIREMENT));
        assert!(
            repair_prompt.contains("probe mode: heuristic"),
            "{repair_prompt}"
        );
        assert!(
            repair_prompt.contains("contract hook status: primary_missing"),
            "{repair_prompt}"
        );
        assert!(repair_prompt.contains("candidate table"), "{repair_prompt}");
        assert!(repair_prompt.contains("rank 2: text=\"Start\" changed=true"));
        assert!(repair_prompt.contains("src/app/page.tsx"));
        let mut fake = FakeClient::new(vec![probe_nextjs_scaffold_reply(
            port,
            interactive_game_page_variant(9),
        )]);
        let mut session = SessionSnapshot::new();
        let outcome = run_final_acceptance_repair_with_ultra_session(
            &mut fake,
            &mut session,
            &repair_prompt,
            &expected_paths,
            &cfg,
            &NOOP_UI,
        )
        .unwrap();
        assert!(!outcome.changed_paths.is_empty(), "{outcome:?}");
        clear_final_acceptance_browser_probe_evidence(&cfg);
        let repaired_report = ultra_final_acceptance_report(&plan, &cfg).unwrap();
        assert!(repaired_report.is_pass(), "{repaired_report:?}");
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(
            event_text
                .matches("\"event\":\"browser_interaction_probe\"")
                .count()
                >= 2,
            "{event_text}"
        );
        assert!(!event_text.contains("probe_unavailable"));
    }

    #[test]
    #[cfg(unix)]
    fn focused_behavioral_repair_exhaustion_handoff_uses_probe_failure() {
        let _probe_guard = dev_server_probe_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let port = free_local_port();
        let events = dir.path().join(".anvil/runs/focused-exhaust/events.jsonl");
        enable_dev_server_probe_test_override(dir.path());
        write_probe_nextjs_workspace(dir.path(), port, interactive_game_page_source());
        interaction_probe::write_test_availability_override(dir.path(), true);
        interaction_probe::write_test_result_overrides(
            dir.path(),
            &[
                interaction_state_missing_probe_result(),
                interaction_state_missing_probe_result(),
                interaction_state_missing_probe_result(),
            ],
        );
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let plan = UltraPlan {
            goal: explicit_port_goal("Create an interactive browser game", port),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![UltraPhase {
                id: "final".to_string(),
                prompt: "Final acceptance".to_string(),
            }],
        };
        let mut report = ultra_final_acceptance_report(&plan, &cfg).unwrap();
        assert!(!report.is_pass(), "{report:?}");
        let mut fake = FakeClient::new(vec![
            probe_nextjs_scaffold_reply(port, interactive_game_page_variant(10)),
            probe_nextjs_scaffold_reply(port, interactive_game_page_variant(11)),
        ]);
        let mut session = SessionSnapshot::new();
        for attempt in 1..=FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS {
            let target = classify_repair_target(&report);
            let expected_paths =
                final_acceptance_repair_expected_paths(&plan, &cfg, &report).unwrap();
            let prompt = final_acceptance_repair_prompt(
                &cfg.workspace_root,
                &plan,
                &report,
                &UltraRunContext::default(),
                target.as_str(),
                &expected_paths,
                &[],
                (attempt, FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS),
                false,
                false,
            );
            let outcome = run_final_acceptance_repair_with_ultra_session(
                &mut fake,
                &mut session,
                &prompt,
                &expected_paths,
                &cfg,
                &NOOP_UI,
            )
            .unwrap();
            assert!(!outcome.changed_paths.is_empty(), "{outcome:?}");
            clear_final_acceptance_browser_probe_evidence(&cfg);
            report = ultra_final_acceptance_report(&plan, &cfg).unwrap();
        }
        assert!(!report.is_pass(), "{report:?}");
        let failure_kind = final_acceptance_app_behavior_failure_kind(&report).unwrap();
        assert_eq!(
            failure_kind,
            "browser_interaction_failed:input_state_change_missing_after_start"
        );
        let reason =
            final_acceptance_recovery_reason(&report, &failure_kind, "bounded_repair_exhausted");
        let targets =
            final_acceptance_recovery_repair_targets(&report, classify_repair_target(&report));
        assert_eq!(targets, vec!["input_state_render_wiring".to_string()]);
        let missing_signals = verification_missing_signals(&report);
        let phase = plan.phases.last().unwrap();
        let _handoff = save_ultra_phase_recovery_handoff(
            &cfg,
            &plan,
            phase,
            UltraPhaseRecoveryRequest {
                failure_kind: &failure_kind,
                reason: &reason,
                missing_paths: &report.missing_paths,
                missing_signals: &missing_signals,
                repair_targets: &targets,
            },
        )
        .expect("handoff saved");
        let recovery_plan = assert_single_recovery_ultra_plan(dir.path());
        let recovery_text = render_ultra_plan(&recovery_plan);
        assert!(
            recovery_text.contains("input_state_render_wiring"),
            "{recovery_text}"
        );
        assert!(
            recovery_text.contains(INTERACTION_STATE_CHANGE_REPAIR_REQUIREMENT),
            "{recovery_text}"
        );
        assert!(!recovery_text.contains("/setup-interaction-probe"));
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(!event_text.contains("interaction_unverified_probe_unavailable"));
        assert!(!event_text.contains("/setup-interaction-probe"));
    }

    #[test]
    #[cfg(unix)]
    fn recovery_transition_not_observed_after_probe_success_is_implementation_failure() {
        let _probe_guard = dev_server_probe_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let port = free_local_port();
        let events = dir
            .path()
            .join(".anvil/runs/recovery-not-observed/events.jsonl");
        enable_dev_server_probe_test_override(dir.path());
        write_probe_nextjs_workspace(dir.path(), port, interactive_game_page_source());
        interaction_probe::write_test_availability_override(dir.path(), true);
        interaction_probe::write_test_result_override(
            dir.path(),
            &recovery_not_observed_probe_result(),
        );
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let plan = UltraPlan {
            goal: explicit_port_goal("Create an interactive browser game", port),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![UltraPhase {
                id: "final".to_string(),
                prompt: "Final acceptance".to_string(),
            }],
        };

        let report = ultra_final_acceptance_report(&plan, &cfg).unwrap();

        assert!(!report.is_pass(), "{report:?}");
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(
            event_text.contains("\"missing_evidence\":[\"restart_or_recoverable_state_evidence\"]")
        );
        assert!(!event_text.contains("interaction_unverified:not_observed_by_probe"));
        assert!(!event_text.contains("probe_unavailable"), "{event_text}");
        assert!(
            !event_text.contains("/setup-interaction-probe"),
            "{event_text}"
        );
        let ultra = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            ultra
                .get("evidence_tiers")
                .and_then(|tiers| tiers.get("restart_or_recoverable_state_evidence"))
                .and_then(Value::as_str),
            Some("absent")
        );
    }

    #[test]
    #[cfg(unix)]
    fn final_acceptance_repair_cycle_reprobes_restart_hook_recovery_to_pass() {
        let _probe_guard = dev_server_probe_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let port = free_local_port();
        let events = dir.path().join(".anvil/runs/restart-cycle/events.jsonl");
        enable_dev_server_probe_test_override(dir.path());
        interaction_probe::write_test_availability_override(dir.path(), true);
        interaction_probe::write_test_result_overrides(
            dir.path(),
            &[
                recovery_not_observed_probe_result(),
                interaction_state_changed_probe_result(),
            ],
        );
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let plan = UltraPlan {
            goal: explicit_port_goal("Create an interactive browser game with restart flow", port),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "first".to_string(),
                    prompt: "Scaffold the app".to_string(),
                },
                UltraPhase {
                    id: "final".to_string(),
                    prompt: "Final implementation pass".to_string(),
                },
            ],
        };
        let fixed_page = interactive_game_page_with_restart_hook_source();
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(generated_nextjs_artifact_plan_json_with_build_verify(
                "Create buildable app",
            )),
            AssistantReply::text(static_phase_step_plan_json(true)),
        ]);
        let mut execution = FakeClient::new(vec![
            probe_nextjs_scaffold_reply(
                port,
                interactive_game_page_without_restart_source().to_string(),
            ),
            read_static_page_reply(),
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/app/page.tsx","content":fixed_page}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);

        let result =
            run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap_or_else(|err| {
                let event_text = std::fs::read_to_string(&events).unwrap_or_default();
                panic!("{err}\nEvents:\n{event_text}");
            });

        assert_eq!(result, "ultra-plan-run complete: 2 phases");
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert_eq!(
            event_text
                .matches("\"event\":\"ultra_final_acceptance\"")
                .count(),
            2,
            "{event_text}"
        );
        assert!(
            event_text
                .matches("\"event\":\"browser_interaction_probe\"")
                .count()
                >= 2,
            "{event_text}"
        );
        let events_json = read_jsonl_events(&events);
        let ultra_cycles = events_json
            .iter()
            .filter(|event| {
                event.get("event").and_then(Value::as_str) == Some("ultra_final_acceptance")
            })
            .filter_map(|event| event.get("cycle_index").and_then(Value::as_u64))
            .collect::<Vec<_>>();
        assert_eq!(ultra_cycles, vec![0, 1], "{event_text}");
        let final_acceptance = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            final_acceptance
                .get("runtime_acceptance_status")
                .and_then(Value::as_str),
            Some("pass")
        );
        assert_eq!(
            final_acceptance
                .get("evidence_arbitration")
                .and_then(|arbitration| arbitration.get("restart_or_recoverable_state_evidence"))
                .and_then(|record| record.get("behavioral_observation"))
                .and_then(Value::as_str),
            Some("recovery_transition")
        );
        let cycle = latest_event(&events, "final_acceptance_cycle_complete");
        assert_eq!(cycle.get("cycle_index").and_then(Value::as_u64), Some(1));
        assert!(
            cycle
                .get("resolved_keys")
                .and_then(Value::as_array)
                .is_some_and(|items| items
                    .iter()
                    .any(|item| item.as_str() == Some("restart_or_recoverable_state_evidence"))),
            "{cycle}"
        );
        assert!(
            cycle
                .get("route_bound_changed_paths")
                .and_then(Value::as_array)
                .is_some_and(|items| items
                    .iter()
                    .any(|item| item.as_str() == Some("src/app/page.tsx"))),
            "{cycle}"
        );
        let summary = std::fs::read_to_string(events.parent().unwrap().join("summary.md")).unwrap();
        assert!(
            summary.contains("Final acceptance repair cycles:"),
            "{summary}"
        );
        assert!(
            summary.contains("restart_or_recoverable_state_evidence"),
            "{summary}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn final_acceptance_no_change_repair_skips_extra_probe_cycle() {
        let _probe_guard = dev_server_probe_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let port = free_local_port();
        let events = dir.path().join(".anvil/runs/no-change-cycle/events.jsonl");
        enable_dev_server_probe_test_override(dir.path());
        interaction_probe::write_test_availability_override(dir.path(), true);
        interaction_probe::write_test_result_override(
            dir.path(),
            &recovery_not_observed_probe_result(),
        );
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let plan = UltraPlan {
            goal: explicit_port_goal("Create an interactive browser game with restart flow", port),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "first".to_string(),
                    prompt: "Scaffold the app".to_string(),
                },
                UltraPhase {
                    id: "final".to_string(),
                    prompt: "Final implementation pass".to_string(),
                },
            ],
        };
        let unchanged_page = interactive_game_page_without_restart_source();
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(generated_nextjs_artifact_plan_json_with_build_verify(
                "Create buildable app",
            )),
            AssistantReply::text(static_phase_step_plan_json(true)),
        ]);
        let mut execution = FakeClient::new(vec![
            probe_nextjs_scaffold_reply(port, unchanged_page.to_string()),
            read_static_page_reply(),
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"src/app/page.tsx","content":unchanged_page}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ]);

        let err = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg)
            .unwrap_err()
            .to_string();

        assert!(
            err.contains("ultra final acceptance failed after bounded repair"),
            "{err}"
        );
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert_eq!(
            event_text
                .matches("\"event\":\"ultra_final_acceptance\"")
                .count(),
            1,
            "{event_text}"
        );
        assert_eq!(
            event_text
                .matches("\"event\":\"browser_interaction_probe\"")
                .count(),
            1,
            "{event_text}"
        );
        assert!(
            event_text.contains("\"reprobe_skipped\":true"),
            "{event_text}"
        );
        assert!(
            event_text.contains("\"failure_kind\":\"final_acceptance_repair_no_source_change\"")
        );
    }

    #[test]
    #[cfg(unix)]
    fn final_acceptance_budget_exhaustion_uses_last_cycle_reason() {
        let _probe_guard = dev_server_probe_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let port = free_local_port();
        let events = dir.path().join(".anvil/runs/last-cycle/events.jsonl");
        enable_dev_server_probe_test_override(dir.path());
        interaction_probe::write_test_availability_override(dir.path(), true);
        interaction_probe::write_test_result_overrides(
            dir.path(),
            &[
                interaction_state_missing_probe_result(),
                recovery_not_observed_probe_result(),
                recovery_not_observed_probe_result(),
            ],
        );
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let plan = UltraPlan {
            goal: explicit_port_goal("Create an interactive browser game with restart flow", port),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "first".to_string(),
                    prompt: "Scaffold the app".to_string(),
                },
                UltraPhase {
                    id: "final".to_string(),
                    prompt: "Final implementation pass".to_string(),
                },
            ],
        };
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(generated_nextjs_artifact_plan_json_with_build_verify(
                "Create buildable app",
            )),
            AssistantReply::text(static_phase_step_plan_json(true)),
        ]);
        let mut execution = FakeClient::new(vec![
            probe_nextjs_scaffold_reply(
                port,
                interactive_game_page_without_restart_source().to_string(),
            ),
            read_static_page_reply(),
            probe_nextjs_scaffold_reply(port, interactive_game_page_variant(101)),
            probe_nextjs_scaffold_reply(port, interactive_game_page_variant(102)),
        ]);

        let err = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg)
            .unwrap_err()
            .to_string();

        assert!(
            err.contains("restart_or_recoverable_state_evidence"),
            "{err}"
        );
        assert!(
            !err.contains("input_state_change_missing_after_start"),
            "{err}"
        );
        let exhausted = latest_event(&events, "final_acceptance_repair_exhausted");
        assert_eq!(
            exhausted.get("cycle_index").and_then(Value::as_u64),
            Some(2)
        );
        assert!(
            exhausted
                .get("primary_reason")
                .and_then(Value::as_str)
                .is_some_and(|reason| reason.contains("restart_or_recoverable_state_evidence")),
            "{exhausted}"
        );
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert_eq!(
            event_text
                .matches("\"event\":\"final_acceptance_cycle_complete\"")
                .count(),
            2,
            "{event_text}"
        );
    }

    #[test]
    fn interaction_probe_infrastructure_failure_blocks_release_without_static_override() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            interactive_game_page_source(),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("browser-readiness.json"),
            r#"{"ok":true,"http_status":200,"route_rendered":true}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("browser-interaction.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": false,
                "status": "failed",
                "interaction_success": false,
                "stage": "resolving",
                "steps": [],
                "failure_kind": "probe_dependency_missing:playwright_module_missing",
                "remediation": crate::minimal_loop::interaction_probe::INTERACTION_PROBE_SETUP_REMEDIATION
            }))
            .unwrap(),
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();

        let gate = browser_release_gate(&cfg);
        assert_eq!(gate.status, "failed", "{gate:?}");
        assert!(
            gate.reasons
                .contains(&"probe_dependency_missing:playwright_module_missing".to_string()),
            "{gate:?}"
        );
        assert!(
            gate.reasons
                .iter()
                .any(|reason| reason.contains("app interaction untested")),
            "{gate:?}"
        );
        assert!(
            gate.reasons
                .iter()
                .any(|reason| reason.contains("/setup-interaction-probe")),
            "{gate:?}"
        );
        assert_eq!(
            release_recovery_repair_targets(&gate, None),
            vec!["release_acceptance".to_string()]
        );

        let report = verify_runtime_acceptance_with_browser_dirs_and_hints(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &[],
            &["browser_interaction".to_string()],
            &[],
            &[],
            &[],
            &[],
            &[],
        );
        assert!(report.passed, "{report:?}");
        assert!(
            !report
                .missing_evidence
                .iter()
                .any(|evidence| evidence.contains("browser_interaction_failed")),
            "{report:?}"
        );
    }

    #[test]
    fn final_acceptance_repair_prompt_adds_plan_adherence_as_secondary_guidance() {
        let mut report = VerificationReport::pass();
        report.push_profile_failure(
            "missing_required_evidence:user_input_handler_evidence".to_string(),
        );
        let plan = UltraPlan {
            goal: "Create an interactive browser game".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: Vec::new(),
        };

        let prompt = final_acceptance_repair_prompt(
            Path::new("."),
            &plan,
            &report,
            &UltraRunContext::default(),
            "required_evidence_missing",
            &["src/app/page.tsx".to_string()],
            &[
                "keyboard".to_string(),
                "lives".to_string(),
                "pause".to_string(),
            ],
            (1, 1),
            false,
            false,
        );

        assert!(
            prompt.contains(
                "Profile failures:\n- missing_required_evidence:user_input_handler_evidence"
            ),
            "{prompt}"
        );
        assert!(
            prompt.contains("Secondary plan-adherence guidance:\n- also close if in scope: keyboard, lives, pause"),
            "{prompt}"
        );
    }

    #[test]
    fn route_unbound_runtime_guidance_names_file_and_remedies() {
        let report = RuntimeAcceptanceReport {
            missing_evidence: vec!["user_input_handler_evidence".to_string()],
            diagnostics: vec![
                "route_unbound_capability_artifact:src/components/SpaceInvaders.tsx".to_string(),
            ],
            ..RuntimeAcceptanceReport::default()
        };

        let guidance = runtime_acceptance_repair_guidance(&report).join("\n");

        assert!(
            guidance.contains("src/components/SpaceInvaders.tsx"),
            "{guidance}"
        );
        assert!(
            guidance.contains("import it from the route page"),
            "{guidance}"
        );
        assert!(
            guidance.contains("consolidate into page.tsx and delete the dead component"),
            "{guidance}"
        );
    }

    #[test]
    fn persistence_reset_runtime_guidance_names_reload_persistence_repair() {
        let report = RuntimeAcceptanceReport {
            missing_evidence: vec!["persistence_evidence".to_string()],
            ..RuntimeAcceptanceReport::default()
        };

        let guidance = runtime_acceptance_repair_guidance(&report).join("\n");

        assert!(
            guidance.contains("load persisted state on mount"),
            "{guidance}"
        );
        assert!(
            guidance.contains("read localStorage in initialization"),
            "{guidance}"
        );
        assert!(guidance.contains("write on mutation"), "{guidance}");
    }

    #[test]
    fn token_echo_missing_runtime_guidance_names_live_preview_repair() {
        let report = RuntimeAcceptanceReport {
            missing_evidence: vec!["live_preview_evidence".to_string()],
            ..RuntimeAcceptanceReport::default()
        };

        let guidance = runtime_acceptance_repair_guidance(&report).join("\n");

        assert!(
            guidance.contains("render the input's content reactively"),
            "{guidance}"
        );
        assert!(guidance.contains("no manual rebuild"), "{guidance}");
        assert!(
            guidance.contains("typed text must appear in the preview/list"),
            "{guidance}"
        );
    }

    #[test]
    fn token_echo_after_reload_only_repair_guidance_names_reactivity() {
        let mut report = VerificationReport::pass();
        report.push_profile_failure(
            "browser_interaction_failed:token_echo_after_reload_only".to_string(),
        );

        let guidance =
            final_acceptance_recovery_reason(&report, "acceptance failed", "repair exhausted");

        assert!(
            guidance.contains("preview renders only after reload"),
            "{guidance}"
        );
        assert!(guidance.contains("make it reactive to input"), "{guidance}");
        assert!(!guidance.contains("token never rendered"), "{guidance}");
    }

    #[test]
    fn browser_interaction_probe_options_require_reload_only_for_persistence_contract() {
        let game = browser_interaction_probe_options(
            &[
                "stateful_interaction".to_string(),
                "player_control".to_string(),
            ],
            &["stateful_update_evidence".to_string()],
        );
        assert!(
            !game.persistence_required,
            "game contracts without persistence must not reload"
        );

        let by_capability = browser_interaction_probe_options(&["persistence".to_string()], &[]);
        assert!(by_capability.persistence_required);

        let by_evidence =
            browser_interaction_probe_options(&[], &["persistence_evidence".to_string()]);
        assert!(by_evidence.persistence_required);
    }

    #[test]
    fn browser_interaction_probe_options_require_text_echo_for_preview_contracts() {
        let game = browser_interaction_probe_options(
            &[
                "stateful_interaction".to_string(),
                "player_control".to_string(),
            ],
            &["stateful_update_evidence".to_string()],
        );
        assert!(!game.text_entry_required);
        assert!(!game.token_echo_required);

        let requested_content =
            browser_interaction_probe_options(&["requested_content".to_string()], &[]);
        assert!(requested_content.text_entry_required);
        assert!(requested_content.token_echo_required);

        let live_preview =
            browser_interaction_probe_options(&[], &["live_preview_evidence".to_string()]);
        assert!(live_preview.text_entry_required);
        assert!(live_preview.token_echo_required);
    }

    #[test]
    fn non_interactive_contract_does_not_require_interaction_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config(dir.path().to_path_buf());
        let report = RuntimeAcceptanceReport {
            passed: true,
            primary_reason: "pass".to_string(),
            ..RuntimeAcceptanceReport::default()
        };

        let gate = final_acceptance_release_gate(
            &cfg,
            "nextjs",
            "Create a static about page",
            &[],
            Some(&report),
            true,
        );

        assert_eq!(gate.status, "pass");
        assert_eq!(gate.browser_readiness_status, "not_applicable");
        assert_eq!(gate.interaction_evidence_status, "not_applicable");
    }

    #[test]
    fn canvas_goal_without_canvas_surface_marker_is_partial_release_gate() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config(dir.path().to_path_buf());
        std::fs::write(
            dir.path().join("browser-readiness.json"),
            r#"{"ok":true,"http_status":200,"route_rendered":true,"has_canvas":false,"interactive_control_count":1,"title_text_excerpt":"Start"}"#,
        )
        .unwrap();
        let report = RuntimeAcceptanceReport {
            passed: true,
            primary_reason: "pass".to_string(),
            ..RuntimeAcceptanceReport::default()
        };

        let gate = final_acceptance_release_gate(
            &cfg,
            "nextjs",
            "Create a canvas-based interactive browser game",
            &["player_control".to_string()],
            Some(&report),
            true,
        );

        assert_eq!(gate.status, "partial", "{gate:?}");
        assert_eq!(gate.browser_readiness_status, "passed");
        assert!(gate.interaction_evidence_status.starts_with("unavailable:"));
        assert!(
            gate.reasons
                .iter()
                .any(|reason| reason.contains("rendered_without_expected_surface")),
            "{gate:?}"
        );
    }

    #[test]
    fn interaction_dom_surface_supersedes_empty_ssr_surface_marker() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config(dir.path().to_path_buf());
        std::fs::write(
            dir.path().join("browser-readiness.json"),
            r#"{"ok":true,"http_status":200,"route_rendered":true,"ssr_has_canvas":false,"ssr_interactive_control_count":0,"has_canvas":false,"interactive_control_count":0,"route_rendered_quality":"rendered_without_expected_surface"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("browser-interaction.json"),
            r#"{"ok":true,"status":"passed","interaction_success":true,"interaction_performed":true,"start_transition":true,"input_state_change":true,"input_event_observed":true,"state_changed":true,"post_js_has_canvas":true,"post_js_interactive_control_count":1,"canvas_found":true}"#,
        )
        .unwrap();
        let report = RuntimeAcceptanceReport {
            passed: true,
            primary_reason: "pass".to_string(),
            ..RuntimeAcceptanceReport::default()
        };

        let gate = final_acceptance_release_gate(
            &cfg,
            "nextjs",
            "Create a canvas-based interactive browser game",
            &["player_control".to_string()],
            Some(&report),
            true,
        );

        assert_eq!(gate.status, "pass", "{gate:?}");
        assert_eq!(gate.interaction_evidence_status, "passed");
        assert!(
            !gate
                .reasons
                .iter()
                .any(|reason| reason.contains("rendered_without_expected_surface")),
            "{gate:?}"
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
            r#"{"ok":true,"interaction_performed":true,"start_transition":true,"input_state_change":true,"input_event_observed":true,"state_changed":true,"canvas_found":true}"#,
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
            r#"{"ok":true,"interaction_performed":true,"start_transition":true,"input_state_change":true,"state_changed":true,"canvas_found":true}"#,
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
            r#"{"ok":true,"interaction_performed":true,"start_transition":true,"input_state_change":true,"state_changed":true,"canvas_found":false}"#,
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
                serde_json::json!({"path":"package.json","content":nextjs_complete_package_json()}),
            ),
            crate::state::ToolCall::new(
                "Write",
                serde_json::json!({"path":"tsconfig.json","content":nextjs_tsconfig_json()}),
            ),
            crate::state::ToolCall::new(
                "Write",
                serde_json::json!({"path":"postcss.config.js","content":nextjs_postcss_config()}),
            ),
            crate::state::ToolCall::new(
                "Write",
                serde_json::json!({"path":"tailwind.config.ts","content":nextjs_tailwind_config_ts()}),
            ),
            crate::state::ToolCall::new(
                "Write",
                serde_json::json!({"path":"src/app/page.tsx","content":page}),
            ),
            crate::state::ToolCall::new(
                "Write",
                serde_json::json!({"path":"src/app/layout.tsx","content":nextjs_layout_source()}),
            ),
            crate::state::ToolCall::new(
                "Write",
                serde_json::json!({"path":"src/app/globals.css","content":nextjs_globals_css()}),
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
    fn completion_verify_and_ultra_final_acceptance_emit_matching_evidence_tiers() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            serde_json::to_string_pretty(&serde_json::json!({
                "required_paths": ["src/app/page.tsx"],
                "required_evidence": [
                    "implementation_artifact",
                    "visible_interactive_surface_evidence",
                    "user_input_handler_evidence",
                    "stateful_update_evidence",
                    "challenge_or_adversary_evidence",
                    "score_or_progression_evidence",
                    "failure_or_collision_evidence",
                    "restart_or_recoverable_state_evidence"
                ],
                "verify_repair_cap": 1
            }))
            .unwrap(),
        )
        .unwrap();
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.completion_contract_path = Some(contract);

        let mut fake = FakeClient::new(vec![AssistantReply {
            content: String::new(),
            tool_calls: vec![crate::state::ToolCall::new(
                "Write",
                serde_json::json!({
                    "path": "src/app/page.tsx",
                    "content": interactive_game_page_source()
                }),
            )],
            prompt_tokens: None,
            completion_tokens: None,
        }]);
        let mut session = SessionSnapshot::new();
        run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Create the contracted game implementation in src/app/page.tsx.",
            &["src/app/page.tsx".to_string()],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Implement),
        )
        .unwrap();

        let plan = UltraPlan {
            goal: "Create the contracted game implementation".to_string(),
            profile: "generic".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![UltraPhase {
                id: "final".to_string(),
                prompt: "Final acceptance".to_string(),
            }],
        };
        let _report = ultra_final_acceptance_report(&plan, &cfg).unwrap();

        let completion = latest_event(&events, "completion_verify");
        let ultra = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            completion.get("evidence_tiers"),
            ultra.get("evidence_tiers"),
            "completion={completion:?}\nultra={ultra:?}"
        );
        assert_eq!(
            ultra
                .get("evidence_tiers")
                .and_then(|tiers| tiers.get("failure_or_collision_evidence"))
                .and_then(Value::as_str),
            Some("strong")
        );
        assert_eq!(
            ultra
                .get("evidence_tiers")
                .and_then(|tiers| tiers.get("restart_or_recoverable_state_evidence"))
                .and_then(Value::as_str),
            Some("strong")
        );
    }

    #[test]
    fn ultra_final_acceptance_records_behavioral_arbitration_per_key() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
import { useState } from "react";
export default function Page() {
  const [score, setScore] = useState(0);
  const [mode, setMode] = useState("ready");
  const fire = () => setScore((value) => value + 1);
  const initGame = () => setMode("ready");
  return <main><button onClick={fire}>Start</button><button onClick={initGame}>Restart</button><canvas />score enemy collision restart {score}{mode}</main>;
}
"#,
        )
        .unwrap();
        let contract = dir.path().join("contract.json");
        std::fs::write(
            &contract,
            serde_json::to_string_pretty(&serde_json::json!({
                "required_paths": ["src/app/page.tsx"],
                "required_evidence": [
                    "implementation_artifact",
                    "visible_interactive_surface_evidence",
                    "interactive_ui_source_evidence",
                    "non_static_screen_evidence",
                    "user_input_handler_evidence",
                    "stateful_update_evidence",
                    "failure_or_collision_evidence",
                    "restart_or_recoverable_state_evidence"
                ],
                "verify_repair_cap": 1
            }))
            .unwrap(),
        )
        .unwrap();
        interaction_probe::write_test_availability_override(dir.path(), true);
        interaction_probe::write_test_result_override(
            dir.path(),
            &serde_json::json!({
                "ok": true,
                "status": "passed",
                "interaction_success": true,
                "interaction_performed": true,
                "input_event_observed": true,
                "state_changed": true,
                "steps": [
                    "surface_visible",
                    "start_transition",
                    "control_input_dispatched",
                    "input_state_change",
                    "recovery_transition"
                ],
                "before_marker": "menu",
                "after_marker": "running",
                "input_before_marker": "running",
                "input_after_marker": "moved",
                "duration_ms": 12
            }),
        );
        let run_dir = dir.path().join(".anvil/runs/behavior");
        let interaction_path = run_dir.join("browser-interaction.json");
        let outcome = interaction_probe::probe_browser_interaction_against_running_server(
            dir.path(),
            34_099,
            &run_dir,
            &interaction_path,
            Duration::from_secs(1),
        );
        assert!(
            outcome
                .observation()
                .is_some_and(|observation| observation.ok),
            "{outcome:?}"
        );

        let events = run_dir.join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.completion_contract_path = Some(contract);
        let plan = UltraPlan {
            goal: "Create contracted implementation".to_string(),
            profile: "generic".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![UltraPhase {
                id: "final".to_string(),
                prompt: "Final acceptance".to_string(),
            }],
        };
        let report = ultra_final_acceptance_report(&plan, &cfg).unwrap();
        assert!(!report.is_pass(), "{report:?}");
        let ultra = latest_event(&events, "ultra_final_acceptance");
        assert_eq!(
            ultra
                .get("evidence_arbitration_summary")
                .and_then(Value::as_str),
            Some("behavioral (probe ok)")
        );
        let arbitration = ultra
            .get("evidence_arbitration")
            .and_then(Value::as_object)
            .expect("evidence_arbitration object");
        let collision = arbitration
            .get("failure_or_collision_evidence")
            .and_then(Value::as_object)
            .expect("collision arbitration");
        assert_eq!(
            collision.get("static_tier").and_then(Value::as_str),
            Some("weak")
        );
        assert_eq!(
            collision.get("final_tier").and_then(Value::as_str),
            Some("absent")
        );
        assert_eq!(
            collision.get("decided_by").and_then(Value::as_str),
            Some("behavioral")
        );
        assert_eq!(
            collision
                .get("behavioral_observation")
                .and_then(Value::as_str),
            Some("not_observed_by_probe")
        );
        assert_eq!(
            ultra
                .get("evidence_tiers")
                .and_then(|tiers| tiers.get("failure_or_collision_evidence"))
                .and_then(Value::as_str),
            Some("absent")
        );
        assert_eq!(
            ultra
                .get("missing_evidence")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(Value::as_str),
            Some("failure_or_collision_evidence")
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
        let replies = (0..20)
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
        assert!(err.contains("artifact_follow_through_exhausted"), "{err}");
        assert!(err.contains("missing.txt"), "{err}");
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

    struct FlakyClient {
        replies: Vec<AssistantReply>,
        messages: Vec<Vec<ConversationMessage>>,
        failures_remaining: usize,
        failure_message: String,
    }

    impl FlakyClient {
        fn new(
            failures_remaining: usize,
            failure_message: impl Into<String>,
            replies: Vec<AssistantReply>,
        ) -> Self {
            Self {
                replies,
                messages: Vec::new(),
                failures_remaining,
                failure_message: failure_message.into(),
            }
        }
    }

    impl ChatClient for FlakyClient {
        fn label(&self) -> &str {
            "flaky"
        }

        fn chat(
            &mut self,
            _model: &str,
            messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _native_tools_enabled: bool,
        ) -> anyhow::Result<AssistantReply> {
            self.messages.push(messages.to_vec());
            if self.failures_remaining > 0 {
                self.failures_remaining -= 1;
                anyhow::bail!("{}", self.failure_message);
            }
            if self.replies.is_empty() {
                anyhow::bail!("flaky client exhausted")
            }
            Ok(self.replies.remove(0))
        }
    }

    struct EchoGoalPlanner {
        messages: Vec<Vec<ConversationMessage>>,
        calls: usize,
    }

    impl EchoGoalPlanner {
        fn new() -> Self {
            Self {
                messages: Vec::new(),
                calls: 0,
            }
        }
    }

    impl ChatClient for EchoGoalPlanner {
        fn label(&self) -> &str {
            "echo-planner"
        }

        fn chat(
            &mut self,
            _model: &str,
            messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _native_tools_enabled: bool,
        ) -> anyhow::Result<AssistantReply> {
            self.messages.push(messages.to_vec());
            self.calls += 1;
            let echoed_goal = messages
                .iter()
                .rev()
                .find(|message| message.role == "user")
                .map(|message| message.content.clone())
                .unwrap_or_default();
            let path = format!("phase-{}.txt", self.calls);
            let plan = StepPlan {
                goal: echoed_goal,
                steps: vec![PlanStep {
                    id: format!("phase-{}", self.calls),
                    kind: "implement".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: format!("Create {path} for this phase."),
                    expected_paths: vec![path],
                    verify: Vec::new(),
                }],
            };
            Ok(AssistantReply::text(serde_json::to_string(&plan).unwrap()))
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

    fn two_phase_ultra_plan(goal: &str, profile: &str) -> UltraPlan {
        UltraPlan {
            goal: goal.to_string(),
            profile: profile.to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "scaffold".to_string(),
                    prompt: "Create the initial scaffold.".to_string(),
                },
                UltraPhase {
                    id: "finish".to_string(),
                    prompt: "Complete the final behavior and verification evidence.".to_string(),
                },
            ],
        }
    }

    fn single_write_step_plan_json(goal: &str, path: &str) -> String {
        serde_json::to_string(&StepPlan {
            goal: goal.to_string(),
            steps: vec![PlanStep {
                id: "write-artifact".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: format!("Create {path}."),
                expected_paths: vec![path.to_string()],
                verify: Vec::new(),
            }],
        })
        .unwrap()
    }

    fn browser_release_evidence_tool_calls() -> Vec<crate::state::ToolCall> {
        vec![
            crate::state::ToolCall::new(
                "Write",
                serde_json::json!({
                    "path": "browser-readiness.json",
                    "content": r#"{"ok":true,"http_status":200,"route_rendered":true}"#
                }),
            ),
            crate::state::ToolCall::new(
                "Write",
                serde_json::json!({
                    "path": "browser-interaction.json",
                    "content": r#"{"ok":true,"interaction_performed":true,"start_transition":true,"input_state_change":true,"state_changed":true,"canvas_found":true}"#
                }),
            ),
        ]
    }

    fn generic_interactive_source() -> &'static str {
        r#"import { useState } from "react";
export default function Memo(){
  const [items, setItems] = useState([]);
  return <form onSubmit={(event) => { event.preventDefault(); setItems([...items, "note"]); }}>
    <input onChange={() => setItems([...items, "draft"])} />
    <button type="submit">Add</button>
    <ul>{items.map((item, index) => <li key={index}>{item}</li>)}</ul>
  </form>;
}
"#
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

    fn final_marker_implement_step_plan_json() -> String {
        serde_json::to_string(&StepPlan {
            goal: "Run the final implementation pass".to_string(),
            steps: vec![PlanStep {
                id: "final-page-pass".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Update src/app/page.tsx as the final implementation artifact."
                    .to_string(),
                expected_paths: vec!["src/app/page.tsx".to_string()],
                verify: Vec::new(),
            }],
        })
        .unwrap()
    }

    fn write_challenge_contract(root: &Path) -> PathBuf {
        write_challenge_contract_with_cap(root, 1)
    }

    fn write_challenge_contract_with_cap(root: &Path, verify_repair_cap: usize) -> PathBuf {
        let path = root.join("challenge-contract.json");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&serde_json::json!({
                "required_paths": ["src/app/page.tsx"],
                "required_evidence": ["challenge_or_adversary_evidence"],
                "verify_repair_cap": verify_repair_cap
            }))
            .unwrap(),
        )
        .unwrap();
        path
    }

    fn latest_event(path: &Path, event: &str) -> Value {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .rfind(|value| value.get("event").and_then(Value::as_str) == Some(event))
            .unwrap_or_else(|| panic!("missing event {event} in {}", path.display()))
    }

    fn events_with_name(path: &Path, event: &str) -> Vec<Value> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|value| value.get("event").and_then(Value::as_str) == Some(event))
            .collect()
    }

    fn event_array_contains(value: &Value, key: &str, needle: &str) -> bool {
        value
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|item| item.as_str() == Some(needle))
    }

    fn planner_request_text(client: &FakeClient, index: usize) -> String {
        client.messages[index]
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn prompt_section_lines(prompt: &str, header: &str) -> Vec<String> {
        let mut lines = prompt.lines().skip_while(|line| *line != header);
        let Some(first) = lines.next() else {
            panic!("missing prompt section {header:?} in {prompt}");
        };
        std::iter::once(first.to_string())
            .chain(
                lines
                    .take_while(|line| !line.trim().is_empty())
                    .map(str::to_string),
            )
            .collect()
    }

    fn nextjs_scaffold_expected_paths() -> Vec<String> {
        vec![
            "package.json".to_string(),
            "tsconfig.json".to_string(),
            "postcss.config.js".to_string(),
            "tailwind.config.ts".to_string(),
            "src/app/layout.tsx".to_string(),
            "src/app/page.tsx".to_string(),
            "src/app/globals.css".to_string(),
            "src/app/global.d.ts".to_string(),
        ]
    }

    fn nextjs_complete_package_json() -> &'static str {
        r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0","tailwindcss":"^3.4.19","postcss":"^8.5.15","autoprefixer":"^10.4.20"},"scripts":{"build":"next build","dev":"next dev -p 3011","start":"next start -p 3011"}}"#
    }

    fn nextjs_lean_package_json() -> &'static str {
        r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"scripts":{"build":"next build","dev":"next dev -p 3011","start":"next start -p 3011"}}"#
    }

    fn explicit_port_goal(goal: &str, port: u16) -> String {
        format!("{goal} on port {port}")
    }

    fn nextjs_tsconfig_json() -> &'static str {
        r#"{"compilerOptions":{"target":"ES2017","lib":["dom","dom.iterable","esnext"],"allowJs":true,"skipLibCheck":true,"strict":true,"noEmit":true,"esModuleInterop":true,"module":"esnext","moduleResolution":"bundler","resolveJsonModule":true,"isolatedModules":true,"jsx":"preserve","incremental":true,"plugins":[{"name":"next"}],"baseUrl":".","paths":{"@/*":["./src/*"]}},"include":["next-env.d.ts","**/*.ts","**/*.tsx",".next/types/**/*.ts"],"exclude":["node_modules"]}"#
    }

    fn nextjs_layout_source() -> &'static str {
        "import './globals.css';\nexport default function Layout({children}:{children:React.ReactNode}){return <html><body>{children}</body></html>;}"
    }

    fn nextjs_globals_css() -> &'static str {
        "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n"
    }

    fn nextjs_tailwind_config_ts() -> &'static str {
        "import type { Config } from 'tailwindcss';\nconst config: Config = { content: ['./src/pages/**/*.{js,ts,jsx,tsx,mdx}', './src/components/**/*.{js,ts,jsx,tsx,mdx}', './src/app/**/*.{js,ts,jsx,tsx,mdx}'], theme: { extend: {} }, plugins: [] };\nexport default config;\n"
    }

    fn nextjs_postcss_config() -> &'static str {
        "module.exports = { plugins: { tailwindcss: {}, autoprefixer: {} } };\n"
    }

    fn write_nextjs_profile_workspace(
        root: &Path,
        globals_css: Option<&str>,
        postcss_config: Option<&str>,
        tsconfig_json: Option<&str>,
    ) {
        std::fs::create_dir_all(root.join("src/app")).unwrap();
        std::fs::write(root.join("package.json"), nextjs_complete_package_json()).unwrap();
        let tsconfig_json = tsconfig_json.unwrap_or(nextjs_tsconfig_json());
        std::fs::write(root.join("tsconfig.json"), tsconfig_json).unwrap();
        std::fs::write(root.join("tailwind.config.ts"), nextjs_tailwind_config_ts()).unwrap();
        if let Some(postcss_config) = postcss_config {
            std::fs::write(root.join("postcss.config.js"), postcss_config).unwrap();
        }
        std::fs::write(
            root.join("src/app/page.tsx"),
            "export default function Page(){return <main className=\"min-h-screen\">App</main>;}",
        )
        .unwrap();
        std::fs::write(root.join("src/app/layout.tsx"), nextjs_layout_source()).unwrap();
        std::fs::write(
            root.join("src/app/global.d.ts"),
            "declare module \"*.css\";\n",
        )
        .unwrap();
        if let Some(globals_css) = globals_css {
            std::fs::write(root.join("src/app/globals.css"), globals_css).unwrap();
        }
    }

    fn generated_nextjs_artifact_plan_json(goal: &str) -> String {
        let expected_paths = nextjs_scaffold_expected_paths();
        serde_json::to_string(&StepPlan {
            goal: goal.to_string(),
            steps: vec![PlanStep {
                id: "create-nextjs-artifacts".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: format!(
                    "Create a coherent Next.js scaffold with {}",
                    expected_paths.join(", ")
                ),
                expected_paths,
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
            expected_paths = nextjs_scaffold_expected_paths();
            expected_paths.push(check_path.to_string());
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
  const [gameOver, setGameOver] = useState(false);
  const [bullets, setBullets] = useState<{ x: number; y: number }[]>([]);
  const [enemies, setEnemies] = useState([{ x: 10, y: 20 }]);
  const fireBullet = () => setBullets((items) => [...items, { x: 10, y: 90 }]);
  const restart = () => {
    setGameOver(false);
    setScore(0);
    setBullets([]);
    setEnemies([{ x: 10, y: 20 }]);
  };
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") {
        fireBullet();
      }
    };
    const frame = requestAnimationFrame(() => {
      bullets.forEach((bullet) => {
        enemies.forEach((enemy) => {
          if (Math.abs(bullet.x - enemy.x) < 12 && Math.abs(bullet.y - enemy.y) < 12) {
            setGameOver(true);
            setScore((value) => value + 10);
          }
        });
      });
      setEnemies((items) => items.map((enemy) => ({ ...enemy, x: enemy.x + 1 })));
    });
    window.addEventListener("keydown", onKeyDown);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [bullets, enemies]);
  return <main><button onClick={fireBullet}>Start</button><button onClick={restart}>Restart</button><canvas /><p>score {score} enemy collision {gameOver ? "game over" : "playing"}</p></main>;
}
"#
    }

    fn cross_file_weak_restart_interactive_game_page_source() -> &'static str {
        r#""use client";
import { useEffect, useRef, useState } from "react";
import { GameEngine } from "./gameEngine";
export default function Page(){
  const engineRef = useRef(new GameEngine());
  const [score, setScore] = useState(0);
  const [gameOver, setGameOver] = useState(false);
  const [screen, setScreen] = useState("gameOver");
  const [bullets, setBullets] = useState<{ x: number; y: number }[]>([]);
  const [enemies, setEnemies] = useState([{ x: 10, y: 20 }]);
  const fireBullet = () => {
    setBullets((items) => [...items, { x: 10, y: 90 }]);
    setScore((value) => value + 10);
  };
  const startGame = () => {
    engineRef.current?.reset();
    setScreen("playing");
    setGameOver(false);
  };
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") {
        fireBullet();
      }
    };
    const frame = requestAnimationFrame(() => {
      bullets.forEach((bullet) => {
        enemies.forEach((enemy) => {
          if (Math.abs(bullet.x - enemy.x) < 12 && Math.abs(bullet.y - enemy.y) < 12) {
            setGameOver(true);
            setScore((value) => value + 25);
          }
        });
      });
      setEnemies((items) => items.map((enemy) => ({ ...enemy, x: enemy.x + 1 })));
    });
    window.addEventListener("keydown", onKeyDown);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [bullets, enemies]);
  return <main><button onClick={startGame}>Restart</button><button onClick={fireBullet}>Fire</button><canvas /><p>score {score} enemy collision {gameOver ? "game over" : screen}</p></main>;
}
"#
    }

    fn cross_file_weak_restart_game_engine_source() -> &'static str {
        r#"export class GameEngine {
  score = 10;
  actors = [{ x: 1, y: 2 }];
  reset() {
    this.score = 0;
    this.actors = [{ x: 1, y: 2 }];
  }
}
"#
    }

    fn interactive_game_page_without_restart_source() -> &'static str {
        r#""use client";
import { useEffect, useState } from "react";
export default function Page(){
  const [score, setScore] = useState(0);
  const [gameOver, setGameOver] = useState(false);
  const [bullets, setBullets] = useState<{ x: number; y: number }[]>([]);
  const [enemies, setEnemies] = useState([{ x: 10, y: 20 }]);
  const fireBullet = () => setBullets((items) => [...items, { x: 10, y: 90 }]);
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") {
        fireBullet();
      }
    };
    const frame = requestAnimationFrame(() => {
      bullets.forEach((bullet) => {
        enemies.forEach((enemy) => {
          if (Math.abs(bullet.x - enemy.x) < 12 && Math.abs(bullet.y - enemy.y) < 12) {
            setGameOver(true);
            setScore((value) => value + 10);
          }
        });
      });
      setEnemies((items) => items.map((enemy) => ({ ...enemy, x: enemy.x + 1 })));
    });
    window.addEventListener("keydown", onKeyDown);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [bullets, enemies]);
  return <main><button onClick={fireBullet}>Fire</button><canvas /><p>score {score} enemy collision {gameOver ? "game over" : "playing"}</p></main>;
}
"#
    }

    fn interactive_game_page_with_restart_hook_source() -> String {
        interactive_game_page_source().replace(
            "<button onClick={restart}>Restart</button>",
            "<button data-anvil-action=\"restart\" onClick={restart}>Restart</button>",
        )
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

    #[cfg(unix)]
    fn forced_cleanup_timeout_after_real_cleanup(
        child: Child,
        logs: &DevServerLogPaths,
    ) -> DevServerCleanup {
        let _ = cleanup_dev_server_child(child, logs);
        DevServerCleanup {
            ok: false,
            failure_kind: Some("dev_server_cleanup_timeout".to_string()),
            output_excerpt: cleanup_timeout_excerpt(
                logs,
                &["forced cleanup timeout for test".to_string()],
            ),
        }
    }

    #[cfg(unix)]
    fn enable_dev_server_probe_test_override(root: &Path) {
        std::fs::create_dir_all(root.join(".anvil")).unwrap();
        std::fs::write(root.join(".anvil/enable-dev-server-probe-tests"), "1").unwrap();
    }

    #[cfg(unix)]
    fn write_fake_nextjs_dev_workspace(root: &Path, port: u16, spawn_grandchild: bool) {
        std::fs::write(
            root.join("package.json"),
            format!(
                r#"{{"scripts":{{"dev":"next dev -p {port}","build":"next build"}},"dependencies":{{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"}}}}"#
            ),
        )
        .unwrap();
        write_fake_nextjs_package_manager(root, spawn_grandchild);
    }

    #[cfg(unix)]
    fn write_fake_nextjs_package_manager(root: &Path, spawn_grandchild: bool) {
        let bin = root.join("node_modules/.bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(root.join("node_modules/next")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/tailwindcss")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/postcss")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/autoprefixer")).unwrap();
        let exe = shell_quote(&std::env::current_exe().unwrap().display().to_string());
        let grandchild = if spawn_grandchild { "1" } else { "0" };
        let script = format!(
            "#!/bin/sh\n\
if [ \"$1\" = \"run\" ] && [ \"$2\" = \"build\" ]; then\n\
  echo \"fake build ok\"\n\
  exit 0\n\
fi\n\
if [ \"$1\" = \"run\" ] && [ \"$2\" = \"dev\" ]; then\n\
  ANVIL_FAKE_DEV_SERVER_CHILD=1 ANVIL_FAKE_DEV_SERVER_GRANDCHILD={grandchild} exec {exe} --ignored --exact planner::runner::tests::fake_dev_server_package_manager_child --nocapture\n\
fi\n\
echo \"unexpected fake npm args: $*\" >&2\n\
exit 2\n"
        );
        let path = bin.join("npm");
        std::fs::write(&path, script).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
        let next_path = bin.join("next");
        std::fs::write(&next_path, "#!/bin/sh\nexit 0\n").unwrap();
        let mut permissions = std::fs::metadata(&next_path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(next_path, permissions).unwrap();
    }

    #[cfg(unix)]
    fn write_fake_npm_dependency_installer(root: &Path) {
        let bin = root.join("node_modules/.bin");
        std::fs::create_dir_all(&bin).unwrap();
        let script = r#"#!/bin/sh
set -eu
install_pkg() {
  name="$1"
  if grep -q "\"$name\"" package.json 2>/dev/null; then
    mkdir -p "node_modules/$name"
    printf '{"name":"%s"}\n' "$name" > "node_modules/$name/package.json"
  fi
}
if [ "$1" = "install" ]; then
  mkdir -p node_modules/.bin
  install_pkg next
  install_pkg react
  install_pkg react-dom
  install_pkg typescript
  install_pkg @types/node
  install_pkg @types/react
  install_pkg @types/react-dom
  install_pkg tailwindcss
  install_pkg postcss
  install_pkg autoprefixer
  if [ -d node_modules/next ]; then
    printf '#!/bin/sh\nexit 0\n' > node_modules/.bin/next
    chmod +x node_modules/.bin/next
  fi
  printf '{"lockfileVersion":3}\n' > package-lock.json
  exit 0
fi
if [ "$1" = "run" ] && [ "$2" = "build" ]; then
  test -x node_modules/.bin/next || { echo "next missing" >&2; exit 1; }
  if grep -q "\"tailwindcss\"" package.json 2>/dev/null; then
    test -d node_modules/tailwindcss || { echo "tailwindcss missing" >&2; exit 1; }
    test -d node_modules/postcss || { echo "postcss missing" >&2; exit 1; }
    test -d node_modules/autoprefixer || { echo "autoprefixer missing" >&2; exit 1; }
  fi
  echo "fake build ok"
  exit 0
fi
echo "unexpected fake npm args: $*" >&2
exit 2
"#;
        let path = bin.join("npm");
        std::fs::write(&path, script).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }

    #[cfg(not(unix))]
    fn write_fake_npm_dependency_installer(root: &Path) {
        let bin = root.join("node_modules/.bin");
        std::fs::create_dir_all(&bin).unwrap();
        let script = r#"@echo off
setlocal
if "%1"=="install" (
  if exist package.json (
    findstr /c:"\"next\"" package.json >nul && mkdir node_modules\next 2>nul
    findstr /c:"\"tailwindcss\"" package.json >nul && mkdir node_modules\tailwindcss 2>nul
    findstr /c:"\"postcss\"" package.json >nul && mkdir node_modules\postcss 2>nul
    findstr /c:"\"autoprefixer\"" package.json >nul && mkdir node_modules\autoprefixer 2>nul
    if exist node_modules\next (
      echo @echo off> node_modules\.bin\next.cmd
      echo exit /b 0>> node_modules\.bin\next.cmd
      echo {"name":"next"}> node_modules\next\package.json
    )
    if exist node_modules\tailwindcss echo {"name":"tailwindcss"}> node_modules\tailwindcss\package.json
    if exist node_modules\postcss echo {"name":"postcss"}> node_modules\postcss\package.json
    if exist node_modules\autoprefixer echo {"name":"autoprefixer"}> node_modules\autoprefixer\package.json
  )
  echo {"lockfileVersion":3}> package-lock.json
  exit /b 0
)
if "%1"=="run" if "%2"=="build" (
  if not exist node_modules\.bin\next.cmd exit /b 1
  if exist node_modules\tailwindcss (
    if not exist node_modules\postcss exit /b 1
    if not exist node_modules\autoprefixer exit /b 1
  )
  echo fake build ok
  exit /b 0
)
echo unexpected fake npm args: %*
exit /b 2
"#;
        std::fs::write(bin.join("npm.cmd"), script).unwrap();
    }

    #[cfg(unix)]
    fn write_probe_nextjs_workspace(root: &Path, port: u16, page: &str) {
        std::fs::write(
            root.join("package.json"),
            format!(
                r#"{{"scripts":{{"build":"next build","dev":"next dev -p {port}","start":"next start -p {port}"}},"dependencies":{{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"}},"devDependencies":{{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0","tailwindcss":"^3.4.19","postcss":"^8.5.15","autoprefixer":"^10.4.20"}}}}"#
            ),
        )
        .unwrap();
        write_fake_nextjs_package_manager(root, false);
        std::fs::create_dir_all(root.join("src/app")).unwrap();
        std::fs::write(root.join("tsconfig.json"), nextjs_tsconfig_json()).unwrap();
        std::fs::write(root.join("postcss.config.js"), nextjs_postcss_config()).unwrap();
        std::fs::write(root.join("tailwind.config.ts"), nextjs_tailwind_config_ts()).unwrap();
        std::fs::write(root.join("src/app/page.tsx"), page).unwrap();
        std::fs::write(root.join("src/app/layout.tsx"), nextjs_layout_source()).unwrap();
        std::fs::write(root.join("src/app/globals.css"), nextjs_globals_css()).unwrap();
        std::fs::write(
            root.join("src/app/global.d.ts"),
            "declare module \"*.css\";\n",
        )
        .unwrap();
    }

    fn hollow_canvas_game_page_source() -> &'static str {
        r#""use client";
import { useState } from "react";
export default function Page(){
  const [mode, setMode] = useState("menu");
  return <main><button onClick={() => setMode("playing")}>Start</button><canvas /><p>score 0 health 3 {mode}</p></main>;
}
"#
    }

    fn interaction_state_missing_probe_result() -> Value {
        serde_json::json!({
            "ok": false,
            "status": "failed",
            "interaction_success": false,
            "interaction_performed": false,
            "input_event_observed": true,
            "start_transition": true,
            "input_state_evaluated_after_start": true,
            "input_state_change": false,
            "state_changed": false,
            "visible_state_changed": false,
            "probe_mode": "heuristic",
            "contract_hook_status": "primary_missing",
            "candidate_table": [
                {"rank": 1, "index": 0, "text_excerpt": "", "changed": false},
                {"rank": 2, "index": 1, "text_excerpt": "Start", "changed": true}
            ],
            "input_dispatches": [
                "ArrowLeft keydown",
                "ArrowRight keydown",
                "Space keydown",
                "canvas/center click"
            ],
            "informational_failure_kinds": ["primary_start_transition_missing"],
            "steps": ["surface_visible", "start_transition", "control_input_dispatched", "input_state_evaluated_after_start"],
            "before_marker": "screen=menu score=0 health=3",
            "after_marker": "screen=playing score=0 health=3",
            "input_before_marker": "player=20 score=0 health=3",
            "input_after_marker": "player=20 score=0 health=3",
            "recovery_transition": true,
            "recovery_transition_status": "observed",
            "failure_kind": "input_state_change_missing_after_start",
            "duration_ms": 17
        })
    }

    fn interaction_state_changed_probe_result() -> Value {
        serde_json::json!({
            "ok": true,
            "status": "passed",
            "interaction_success": true,
            "interaction_performed": true,
            "input_event_observed": true,
            "start_transition": true,
            "input_state_evaluated_after_start": true,
            "input_state_change": true,
            "state_changed": true,
            "visible_state_changed": true,
            "steps": [
                "surface_visible",
                "start_transition",
                "control_input_dispatched",
                "input_state_evaluated_after_start",
                "input_state_change",
                "recovery_transition"
            ],
            "before_marker": "screen=menu score=0 health=3",
            "after_marker": "screen=playing score=0 health=3",
            "input_before_marker": "player=20 score=0 health=3",
            "input_after_marker": "player=15 score=1 health=3",
            "recovery_transition": true,
            "recovery_transition_status": "observed",
            "duration_ms": 19
        })
    }

    fn recovery_not_observed_probe_result() -> Value {
        serde_json::json!({
            "ok": true,
            "status": "passed",
            "interaction_success": true,
            "interaction_performed": true,
            "input_event_observed": true,
            "start_transition": true,
            "input_state_evaluated_after_start": true,
            "input_state_change": true,
            "state_changed": true,
            "visible_state_changed": true,
            "steps": [
                "surface_visible",
                "start_transition",
                "control_input_dispatched",
                "input_state_evaluated_after_start",
                "input_state_change",
                "recovery_transition:not_observed"
            ],
            "before_marker": "screen=menu",
            "after_marker": "screen=playing",
            "input_before_marker": "player=20 score=0",
            "input_after_marker": "player=15 score=1",
            "recovery_before_marker": "screen=playing",
            "recovery_after_marker": "screen=playing",
            "recovery_transition": false,
            "recovery_transition_status": "not_observed",
            "duration_ms": 23
        })
    }

    #[cfg(unix)]
    fn probe_nextjs_scaffold_tool_calls(
        port: u16,
        page: &str,
        check_path: &str,
    ) -> Vec<crate::state::ToolCall> {
        vec![
            crate::state::ToolCall::new(
                "Write",
                serde_json::json!({
                    "path":"package.json",
                    "content": format!(
                        r#"{{"scripts":{{"build":"next build","dev":"next dev -p {port}","start":"next start -p {port}"}},"dependencies":{{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"}},"devDependencies":{{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0","tailwindcss":"^3.4.19","postcss":"^8.5.15","autoprefixer":"^10.4.20"}}}}"#
                    )
                }),
            ),
            crate::state::ToolCall::new(
                "Write",
                serde_json::json!({"path":"tsconfig.json","content":nextjs_tsconfig_json()}),
            ),
            crate::state::ToolCall::new(
                "Write",
                serde_json::json!({"path":"postcss.config.js","content":nextjs_postcss_config()}),
            ),
            crate::state::ToolCall::new(
                "Write",
                serde_json::json!({"path":"tailwind.config.ts","content":nextjs_tailwind_config_ts()}),
            ),
            crate::state::ToolCall::new(
                "Write",
                serde_json::json!({"path":"src/app/page.tsx","content":page}),
            ),
            crate::state::ToolCall::new(
                "Write",
                serde_json::json!({"path":"src/app/layout.tsx","content":nextjs_layout_source()}),
            ),
            crate::state::ToolCall::new(
                "Write",
                serde_json::json!({"path":"src/app/globals.css","content":nextjs_globals_css()}),
            ),
            crate::state::ToolCall::new(
                "Write",
                serde_json::json!({"path":"src/app/global.d.ts","content":"declare module \"*.css\";"}),
            ),
            crate::state::ToolCall::new(
                "Write",
                serde_json::json!({"path":check_path,"content":"x = 1\n"}),
            ),
        ]
    }

    #[cfg(unix)]
    fn probe_nextjs_scaffold_reply(port: u16, page: String) -> AssistantReply {
        AssistantReply {
            content: String::new(),
            tool_calls: probe_nextjs_scaffold_tool_calls(port, &page, "check_scaffold.py"),
            prompt_tokens: None,
            completion_tokens: None,
        }
    }

    fn interactive_game_page_variant(label: usize) -> String {
        interactive_game_page_source()
            .replace("score {score}", &format!("score {{score}} health {label}"))
    }

    #[cfg(unix)]
    fn write_compile_error_nextjs_workspace(root: &Path, port: u16) -> PathBuf {
        std::fs::create_dir_all(root.join("src/app")).unwrap();
        std::fs::create_dir_all(root.join("src/components")).unwrap();
        std::fs::create_dir_all(root.join(".anvil")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/.bin")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/next")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/tailwindcss")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/postcss")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/autoprefixer")).unwrap();
        std::fs::write(root.join(".anvil/enable-browser-probe-tests"), "1").unwrap();
        std::fs::write(
            root.join("package.json"),
            format!(
                r#"{{"scripts":{{"build":"next build","dev":"next dev -p {port}","start":"next start -p {port}"}},"dependencies":{{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"}},"devDependencies":{{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0","tailwindcss":"^3.4.19","postcss":"^8.5.15","autoprefixer":"^10.4.20"}}}}"#
            ),
        )
        .unwrap();
        std::fs::write(root.join("node_modules/next/package.json"), "{}").unwrap();
        std::fs::write(root.join("node_modules/tailwindcss/package.json"), "{}").unwrap();
        std::fs::write(root.join("node_modules/postcss/package.json"), "{}").unwrap();
        std::fs::write(root.join("node_modules/autoprefixer/package.json"), "{}").unwrap();
        let component = r#""use client";
import { useState } from "react";
export function SpaceInvaders(){
  const [score, setScore] = useState(0);
  const fire = () => setScore((value) => value + 1);
  return <main><button onClick={fire}>Fire</button><button onClick={reset}>Restart</button><canvas /><p>score {score}</p></main>;
}
"#;
        std::fs::write(root.join("src/components/SpaceInvaders.tsx"), component).unwrap();
        std::fs::write(
            root.join("src/app/page.tsx"),
            "export default function Page(){return <main><button>Plain</button></main>;}\n",
        )
        .unwrap();
        std::fs::write(root.join("src/app/layout.tsx"), nextjs_layout_source()).unwrap();
        std::fs::write(root.join("src/app/globals.css"), nextjs_globals_css()).unwrap();
        std::fs::write(
            root.join("src/app/global.d.ts"),
            "declare module \"*.css\";\n",
        )
        .unwrap();
        std::fs::write(root.join("tsconfig.json"), nextjs_tsconfig_json()).unwrap();
        std::fs::write(root.join("postcss.config.js"), nextjs_postcss_config()).unwrap();
        std::fs::write(root.join("tailwind.config.ts"), nextjs_tailwind_config_ts()).unwrap();
        let exe = shell_quote(&std::env::current_exe().unwrap().display().to_string());
        let script = format!(
            "#!/bin/sh\n\
if [ \"$1\" = \"run\" ] && [ \"$2\" = \"build\" ]; then\n\
  if grep -q 'onClick={{reset}}' src/components/SpaceInvaders.tsx && ! grep -q 'const reset' src/components/SpaceInvaders.tsx; then\n\
    echo './src/components/SpaceInvaders.tsx:137:28' >&2\n\
    echo \"Type error: Cannot find name 'reset'.\" >&2\n\
    exit 1\n\
  fi\n\
  echo 'fake build ok'\n\
  exit 0\n\
fi\n\
if [ \"$1\" = \"run\" ] && {{ [ \"$2\" = \"dev\" ] || [ \"$2\" = \"start\" ]; }}; then\n\
  ANVIL_FAKE_DEV_SERVER_CHILD=1 ANVIL_FAKE_DEV_SERVER_GRANDCHILD=0 exec {exe} --ignored --exact planner::runner::tests::fake_dev_server_package_manager_child --nocapture\n\
fi\n\
echo \"unexpected fake npm args: $*\" >&2\n\
exit 2\n"
        );
        let npm = root.join("node_modules/.bin/npm");
        std::fs::write(&npm, script).unwrap();
        let mut permissions = std::fs::metadata(&npm).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&npm, permissions).unwrap();
        let next_path = root.join("node_modules/.bin/next");
        std::fs::write(&next_path, "#!/bin/sh\nexit 0\n").unwrap();
        let mut permissions = std::fs::metadata(&next_path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(next_path, permissions).unwrap();
        let contract_path = root.join("completion-contract.json");
        std::fs::write(
            &contract_path,
            serde_json::to_string_pretty(&serde_json::json!({
                "required_paths": ["src/app/page.tsx", "src/components/SpaceInvaders.tsx"],
                "verify_commands": ["npm run build"],
                "profile": "nextjs",
                "goal": explicit_port_goal("Create an interactive browser app", port),
                "required_capabilities": ["playable_ui", "stateful_interaction"],
                "verify_repair_cap": 2
            }))
            .unwrap(),
        )
        .unwrap();
        contract_path
    }

    fn generated_nextjs_artifact_plan_json_with_build_verify(goal: &str) -> String {
        let expected_paths = nextjs_scaffold_expected_paths();
        serde_json::to_string(&StepPlan {
            goal: goal.to_string(),
            steps: vec![PlanStep {
                id: "create-nextjs-artifacts".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: format!(
                    "Create a coherent Next.js scaffold with {}",
                    expected_paths.join(", ")
                ),
                expected_paths,
                verify: vec!["npm run build".to_string()],
            }],
        })
        .unwrap()
    }

    #[cfg(unix)]
    fn static_good_page_source() -> &'static str {
        "export default function Page(){return <main>Recovered static app</main>;}\n"
    }

    #[cfg(unix)]
    fn static_broken_page_source() -> &'static str {
        "export default function Page(){\n  return (\n    <main>\n      <p>Broken</p>\nBROKEN_SYNTAX\n    </main>\n  );\n}\n"
    }

    #[cfg(unix)]
    fn write_static_compile_repair_workspace(root: &Path, page: &str) {
        std::fs::create_dir_all(root.join("src/app")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/.bin")).unwrap();
        for package in ["next", "tailwindcss", "postcss", "autoprefixer"] {
            let dir = root.join("node_modules").join(package);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("package.json"), "{}").unwrap();
        }
        std::fs::write(root.join("package.json"), nextjs_complete_package_json()).unwrap();
        std::fs::write(root.join("tsconfig.json"), nextjs_tsconfig_json()).unwrap();
        std::fs::write(root.join("postcss.config.js"), nextjs_postcss_config()).unwrap();
        std::fs::write(root.join("tailwind.config.ts"), nextjs_tailwind_config_ts()).unwrap();
        std::fs::write(root.join("src/app/page.tsx"), page).unwrap();
        std::fs::write(root.join("src/app/layout.tsx"), nextjs_layout_source()).unwrap();
        std::fs::write(root.join("src/app/globals.css"), nextjs_globals_css()).unwrap();
        std::fs::write(
            root.join("src/app/global.d.ts"),
            "declare module \"*.css\";\n",
        )
        .unwrap();
        let page_path = root.join("src/app/page.tsx");
        let script = format!(
            "#!/bin/sh\n\
if [ \"$1\" = \"run\" ] && [ \"$2\" = \"build\" ]; then\n\
  if grep -q 'BROKEN_SYNTAX' src/app/page.tsx; then\n\
    echo 'Failed to compile.' >&2\n\
    echo './src/app/page.tsx' >&2\n\
    echo 'Error:' >&2\n\
    echo \"  x Expected ';', '}}' or <eof>\" >&2\n\
    echo '   ,-[{}:12:1]' >&2\n\
    echo ' 9 |   return (' >&2\n\
    echo '10 |     <main>' >&2\n\
    echo '11 |       <p>Broken</p>' >&2\n\
    echo '12 | BROKEN_SYNTAX' >&2\n\
    echo '   | ^' >&2\n\
    exit 1\n\
  fi\n\
  echo 'fake build ok'\n\
  exit 0\n\
fi\n\
echo \"unexpected fake npm args: $*\" >&2\n\
exit 2\n",
            page_path.display()
        );
        let npm = root.join("node_modules/.bin/npm");
        std::fs::write(&npm, script).unwrap();
        let mut permissions = std::fs::metadata(&npm).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&npm, permissions).unwrap();
        let next = root.join("node_modules/.bin/next");
        std::fs::write(&next, "#!/bin/sh\nexit 0\n").unwrap();
        let mut permissions = std::fs::metadata(&next).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(next, permissions).unwrap();
    }

    fn write_static_build_contract(root: &Path) -> PathBuf {
        let path = root.join("completion-contract.json");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&serde_json::json!({
                "required_paths": ["src/app/page.tsx"],
                "verify_commands": ["npm run build"],
                "profile": "nextjs",
                "goal": "Create a static Next.js page",
                "required_capabilities": [],
                "required_evidence": ["implementation_artifact"],
                "verify_repair_cap": 2
            }))
            .unwrap(),
        )
        .unwrap();
        path
    }

    fn static_compile_repair_plan() -> UltraPlan {
        UltraPlan {
            goal: "Create a static Next.js page".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "phase-one".to_string(),
                    prompt: "Keep the current compiling static page.".to_string(),
                },
                UltraPhase {
                    id: "phase-two".to_string(),
                    prompt: "Update the page copy for the final app.".to_string(),
                },
            ],
        }
    }

    fn static_phase_step_plan_json(verify_build: bool) -> String {
        serde_json::to_string(&StepPlan {
            goal: "Create a static Next.js page".to_string(),
            steps: vec![PlanStep {
                id: if verify_build {
                    "verify-static-build".to_string()
                } else {
                    "update-static-page".to_string()
                },
                kind: if verify_build {
                    "verify".to_string()
                } else {
                    "setup".to_string()
                },
                expected_result: "static page is present".to_string(),
                instruction: if verify_build {
                    "Verify the current static Next.js page build".to_string()
                } else {
                    "Update src/app/page.tsx for the static app".to_string()
                },
                expected_paths: if verify_build {
                    Vec::new()
                } else {
                    vec!["src/app/page.tsx".to_string()]
                },
                verify: if verify_build {
                    vec!["npm run build".to_string()]
                } else {
                    Vec::new()
                },
            }],
        })
        .unwrap()
    }

    #[cfg(unix)]
    fn static_breaking_build_step_plan() -> StepPlan {
        StepPlan {
            goal: "Create a static Next.js page".to_string(),
            steps: vec![PlanStep {
                id: "break-then-verify-build".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Update src/app/page.tsx and verify the build.".to_string(),
                expected_paths: vec!["src/app/page.tsx".to_string()],
                verify: vec!["npm run build".to_string()],
            }],
        }
    }

    #[cfg(unix)]
    fn static_breaking_build_step_plan_json() -> String {
        serde_json::to_string(&static_breaking_build_step_plan()).unwrap()
    }

    fn write_static_page_reply(content: &str) -> AssistantReply {
        AssistantReply {
            content: String::new(),
            tool_calls: vec![crate::state::ToolCall::new(
                "Write",
                serde_json::json!({"path":"src/app/page.tsx","content":content}),
            )],
            prompt_tokens: None,
            completion_tokens: None,
        }
    }

    fn read_static_page_reply() -> AssistantReply {
        AssistantReply {
            content: String::new(),
            tool_calls: vec![crate::state::ToolCall::new(
                "Read",
                serde_json::json!({"path":"src/app/page.tsx"}),
            )],
            prompt_tokens: None,
            completion_tokens: None,
        }
    }

    #[cfg(unix)]
    fn bash_true_reply() -> AssistantReply {
        AssistantReply {
            content: String::new(),
            tool_calls: vec![crate::state::ToolCall::new(
                "Bash",
                serde_json::json!({"command":"true"}),
            )],
            prompt_tokens: None,
            completion_tokens: None,
        }
    }

    #[test]
    #[cfg(unix)]
    fn compile_error_final_acceptance_skips_readiness_probe() {
        let _probe_guard = dev_server_probe_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let port = free_local_port();
        let events = dir.path().join("events.jsonl");
        enable_dev_server_probe_test_override(dir.path());
        let contract = write_compile_error_nextjs_workspace(dir.path(), port);
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        cfg.completion_contract_path = Some(contract);
        let plan = UltraPlan {
            goal: explicit_port_goal("Create an interactive browser app", port),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "inspect-one".to_string(),
                    prompt: "Inspect the existing app.".to_string(),
                },
                UltraPhase {
                    id: "inspect-two".to_string(),
                    prompt: "Inspect final readiness.".to_string(),
                },
            ],
        };

        let report = ultra_final_acceptance_report(&plan, &cfg).unwrap();

        assert_eq!(
            classify_repair_target(&report),
            RepairTarget::Implementation
        );
        assert_eq!(report.compile_errors.len(), 1, "{report:?}");
        assert!(report.dependency_missing.is_empty(), "{report:?}");
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("implementation_compile_error"));
        assert!(event_text.contains("\"compile_errors\""));
        assert!(!event_text.contains("\"event\":\"browser_probe\""));
        assert!(!event_text.contains("\"event\":\"dev_server_lifecycle\""));
        assert!(!dir.path().join("browser-readiness.json").exists());
    }

    #[test]
    #[cfg(unix)]
    fn compile_error_repair_prompt_anchors_file_and_then_runs_readiness() {
        let _probe_guard = dev_server_probe_test_guard();
        let dir = tempfile::tempdir().unwrap();
        let port = free_local_port();
        let events = dir.path().join("events.jsonl");
        enable_dev_server_probe_test_override(dir.path());
        let contract = write_compile_error_nextjs_workspace(dir.path(), port);
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        cfg.completion_contract_path = Some(contract);
        let plan = UltraPlan {
            goal: explicit_port_goal("Create an interactive browser app", port),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "inspect-one".to_string(),
                    prompt: "Inspect the existing app.".to_string(),
                },
                UltraPhase {
                    id: "inspect-two".to_string(),
                    prompt: "Inspect final readiness.".to_string(),
                },
            ],
        };
        let fixed_component = r#""use client";
	import { useState } from "react";
	export function SpaceInvaders(){
	  const [score, setScore] = useState(0);
	  const fire = () => setScore((value) => value + 1);
	  const reset = () => setScore(0);
	  return <main><button onClick={() => fire()}>Fire</button><button onClick={() => reset()}>Restart</button><canvas /><p>score {score}</p></main>;
        }
        "#;
        let route_page = "import { SpaceInvaders } from '@/components/SpaceInvaders';\nexport default function Page(){return <SpaceInvaders />;}\n";
        let initial_report = ultra_final_acceptance_report(&plan, &cfg).unwrap();
        assert_eq!(
            classify_repair_target(&initial_report),
            RepairTarget::Implementation,
            "{initial_report:?}"
        );
        let expected_paths = final_acceptance_repair_expected_paths(&plan, &cfg, &initial_report)
            .expect("expected paths");
        let repair_prompt = final_acceptance_repair_prompt(
            &cfg.workspace_root,
            &plan,
            &initial_report,
            &UltraRunContext::default(),
            RepairTarget::Implementation.as_str(),
            &expected_paths,
            &[],
            (1, FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS),
            false,
            false,
        );
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/components/SpaceInvaders.tsx","content":fixed_component}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/page.tsx","content":route_page}),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("compile error repaired"),
            AssistantReply::text("compile error repaired"),
            AssistantReply::text("compile error repaired"),
            AssistantReply::text("compile error repaired"),
            AssistantReply::text("compile error repaired"),
        ]);
        let mut session = SessionSnapshot::new();

        let outcome = run_final_acceptance_repair_with_ultra_session(
            &mut execution,
            &mut session,
            &repair_prompt,
            &expected_paths,
            &cfg,
            &NOOP_UI,
        )
        .unwrap_or_else(|err| {
            let event_text = std::fs::read_to_string(&events).unwrap_or_default();
            panic!("{err}\nEvents:\n{event_text}");
        });
        assert!(
            outcome
                .changed_paths
                .contains(&"src/components/SpaceInvaders.tsx".to_string())
        );
        let repaired_report = ultra_final_acceptance_report(&plan, &cfg).unwrap();

        assert!(
            repaired_report.compile_errors.is_empty(),
            "{repaired_report:?}"
        );
        assert!(
            repair_prompt.contains("src/components/SpaceInvaders.tsx:137:28"),
            "{repair_prompt}"
        );
        assert!(
            repair_prompt.contains("Compile repair edit mandate"),
            "{repair_prompt}"
        );
        assert!(
            repair_prompt.contains("You MUST modify src/components/SpaceInvaders.tsx"),
            "{repair_prompt}"
        );
        assert!(repair_prompt.contains("define reset"), "{repair_prompt}");
        assert!(
            repair_prompt.contains("replace the reference with an existing handler"),
            "{repair_prompt}"
        );
        assert!(
            repair_prompt.contains("remove the dead code"),
            "{repair_prompt}"
        );
        assert!(
            repair_prompt.contains("the file is not imported by any route"),
            "{repair_prompt}"
        );
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(
            event_text.contains("\"event\":\"completion_verify\""),
            "{event_text}"
        );
        assert!(event_text.contains("\"ok\":true"), "{event_text}");
        assert!(
            event_text.contains("\"event\":\"browser_probe\""),
            "{event_text}"
        );
        assert!(
            event_text.contains("\"browser_readiness_status\":\"passed\""),
            "{event_text}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn compile_repair_no_edit_reanchors_then_no_snapshot_gets_narrow_retry() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join(".anvil/runs/no-snapshot/events.jsonl");
        write_static_compile_repair_workspace(dir.path(), static_good_page_source());
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "generic".to_string();
        cfg.eval_events_path = Some(events.clone());
        cfg.completion_contract_path = Some(write_static_build_contract(dir.path()));
        let plan = static_compile_repair_plan();
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(static_phase_step_plan_json(false)),
            AssistantReply::text(static_phase_step_plan_json(false)),
        ]);
        let mut execution = FakeClient::new(vec![
            write_static_page_reply(static_good_page_source()),
            write_static_page_reply(static_broken_page_source()),
            AssistantReply::text("The compile error is in src/app/page.tsx."),
            AssistantReply::text("It needs a syntax fix."),
            AssistantReply::text("The compile error remains in src/app/page.tsx."),
            AssistantReply::text("It still needs a syntax fix."),
            AssistantReply::text("Only fix the compile frame in src/app/page.tsx."),
            AssistantReply::text("No restructuring is needed."),
        ]);

        let err = run_ultra_plan(&mut planner, &mut execution, &plan, &cfg)
            .unwrap_err()
            .to_string();

        assert!(
            err.contains("ultra final acceptance failed after bounded repair"),
            "{err}"
        );
        assert!(err.contains("implementation_compile_error"), "{err}");
        assert!(err.contains("src/app/page.tsx"), "{err}");
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert_eq!(
            event_text
                .matches("\"event\":\"final_acceptance_repair_no_source_change\"")
                .count(),
            3,
            "{event_text}"
        );
        assert!(
            event_text.contains("\"compile_reanchored_retry\":true"),
            "{event_text}"
        );
        assert!(
            event_text.contains("\"event\":\"compile_no_snapshot_narrow_retry\""),
            "{event_text}"
        );
        assert!(
            event_text.contains("\"compile_narrow_no_snapshot_retry\":true"),
            "{event_text}"
        );
        assert!(
            event_text.contains("\"narrow_no_snapshot_retry\":true"),
            "{event_text}"
        );
        assert!(
            event_text.contains("\"failure_kind\":\"compile_repair_no_source_change\""),
            "{event_text}"
        );
        assert!(
            event_text.contains("\"failure_kind\":\"implementation_compile_error\""),
            "{event_text}"
        );
        assert!(
            event_text.contains("\"event\":\"compile_rollback_skipped\""),
            "{event_text}"
        );
        assert!(event_text.contains("snapshot_missing:src/app/page.tsx"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/app/page.tsx")).unwrap(),
            static_broken_page_source()
        );
    }

    #[test]
    #[cfg(unix)]
    fn step_verify_compile_repair_exhaustion_rolls_back_snapshot_and_continues() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join(".anvil/runs/step-rollback/events.jsonl");
        write_static_compile_repair_workspace(dir.path(), static_good_page_source());
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        cfg.completion_contract_path = Some(write_static_build_contract(dir.path()));
        let plan = UltraPlan {
            goal: "Create a static Next.js page".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "phase-one".to_string(),
                    prompt: "Confirm the current build is good.".to_string(),
                },
                UltraPhase {
                    id: "phase-two".to_string(),
                    prompt: "Apply a risky page update and verify the build.".to_string(),
                },
                UltraPhase {
                    id: "phase-three".to_string(),
                    prompt: "Continue after rollback with the recovered page.".to_string(),
                },
            ],
        };
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(static_phase_step_plan_json(true)),
            AssistantReply::text(static_breaking_build_step_plan_json()),
            AssistantReply::text(static_phase_step_plan_json(false)),
        ]);
        let mut execution = FakeClient::new(vec![
            read_static_page_reply(),
            write_static_page_reply(static_broken_page_source()),
            bash_true_reply(),
            AssistantReply::text("The compile error is unchanged."),
            bash_true_reply(),
            AssistantReply::text("The compile error is still unchanged."),
            bash_true_reply(),
            AssistantReply::text("The compile error remains unchanged."),
            bash_true_reply(),
            AssistantReply::text("The compile error remains unchanged again."),
            bash_true_reply(),
            AssistantReply::text("Continue after rollback."),
            write_static_page_reply(static_good_page_source()),
            read_static_page_reply(),
            AssistantReply::text("phase three complete"),
        ]);

        let result =
            run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap_or_else(|err| {
                let event_text = std::fs::read_to_string(&events).unwrap_or_default();
                panic!("{err}\nEvents:\n{event_text}");
            });

        assert!(result.contains("ultra-plan-run complete"), "{result}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/app/page.tsx")).unwrap(),
            static_good_page_source()
        );
        assert_eq!(planner.messages.len(), 3);
        let phase_three_prompt = planner_request_text(&planner, 2);
        assert!(
            phase_three_prompt.contains("Carry-forward guidance"),
            "{phase_three_prompt}"
        );
        assert!(
            phase_three_prompt
                .contains("phase phase-two changes to src/app/page.tsx were rolled back"),
            "{phase_three_prompt}"
        );
        assert!(
            phase_three_prompt.contains("Update src/app/page.tsx and verify the build."),
            "{phase_three_prompt}"
        );
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(
            event_text.contains("\"event\":\"compile_snapshot_saved\""),
            "{event_text}"
        );
        assert!(
            event_text.contains("\"event\":\"compile_rollback_applied\""),
            "{event_text}"
        );
        assert!(
            event_text.contains("\"event\":\"compile_rollback_context_carried\""),
            "{event_text}"
        );
        assert!(
            !event_text.contains("\"event\":\"recovery_prompt_saved\""),
            "{event_text}"
        );
        assert!(
            event_text.contains("\"phase_id\":\"phase-two\""),
            "{event_text}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn step_verify_compile_repair_exhaustion_without_snapshot_still_fails() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join(".anvil/runs/step-no-snapshot/events.jsonl");
        write_static_compile_repair_workspace(dir.path(), static_good_page_source());
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());
        let mut execution = FakeClient::new(vec![
            write_static_page_reply(static_broken_page_source()),
            bash_true_reply(),
            AssistantReply::text("The compile error is unchanged."),
            bash_true_reply(),
            AssistantReply::text("The compile error is still unchanged."),
            bash_true_reply(),
            AssistantReply::text("The compile error remains unchanged."),
            bash_true_reply(),
            AssistantReply::text("The compile error remains unchanged again."),
        ]);

        let err = run_step_plan(&mut execution, &static_breaking_build_step_plan(), &cfg)
            .unwrap_err()
            .to_string();

        assert!(err.contains("repair prompt saved"), "{err}");
        assert!(err.contains("implementation_compile_error"), "{err}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/app/page.tsx")).unwrap(),
            static_broken_page_source()
        );
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(
            event_text.contains("\"event\":\"compile_rollback_skipped\""),
            "{event_text}"
        );
        assert!(
            event_text.contains("snapshot_missing:src/app/page.tsx"),
            "{event_text}"
        );
        assert!(
            event_text.contains("\"event\":\"recovery_prompt_saved\""),
            "{event_text}"
        );
        assert!(
            event_text.contains("\"failure_kind\":\"verify_repair_progress_unchanged\""),
            "{event_text}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn compile_repair_exhaustion_rolls_back_last_known_good_and_continues() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join(".anvil/runs/rollback/events.jsonl");
        write_static_compile_repair_workspace(dir.path(), static_good_page_source());
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "generic".to_string();
        cfg.eval_events_path = Some(events.clone());
        cfg.completion_contract_path = Some(write_static_build_contract(dir.path()));
        let plan = static_compile_repair_plan();
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(static_phase_step_plan_json(true)),
            AssistantReply::text(static_phase_step_plan_json(false)),
        ]);
        let mut execution = FakeClient::new(vec![
            read_static_page_reply(),
            write_static_page_reply(static_broken_page_source()),
            AssistantReply::text("The compile error is in src/app/page.tsx."),
            AssistantReply::text("It needs a syntax fix."),
            AssistantReply::text("The compile error remains in src/app/page.tsx."),
            AssistantReply::text("It still needs a syntax fix."),
        ]);

        let result =
            run_ultra_plan(&mut planner, &mut execution, &plan, &cfg).unwrap_or_else(|err| {
                let event_text = std::fs::read_to_string(&events).unwrap_or_default();
                panic!("{err}\nEvents:\n{event_text}");
            });

        assert!(result.contains("ultra-plan-run complete"), "{result}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/app/page.tsx")).unwrap(),
            static_good_page_source()
        );
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(
            event_text.contains("\"event\":\"compile_snapshot_saved\""),
            "{event_text}"
        );
        assert!(
            event_text.contains("\"event\":\"compile_rollback_applied\""),
            "{event_text}"
        );
        assert!(
            event_text.contains("\"event\":\"compile_rollback_context_carried\""),
            "{event_text}"
        );
        assert!(
            event_text.contains("phase phase-two changes to src/app/page.tsx were rolled back; re-apply: Update the page copy for the final app."),
            "{event_text}"
        );
        let summary = std::fs::read_to_string(events.parent().unwrap().join("summary.md")).unwrap();
        assert!(summary.contains("Compile rollback applied:"), "{summary}");
        assert!(summary.contains("src/app/page.tsx"), "{summary}");
    }

    #[test]
    fn compile_error_recovery_handoff_orders_fix_compile_error_first() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join(".anvil/runs/recovery-order/events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events);
        let plan = static_compile_repair_plan();
        let mut report = VerificationReport::pass();
        report.compile_errors.push(CompileError {
            path: "src/app/page.tsx".to_string(),
            line: 12,
            column: 1,
            message: "Expected ';', '}' or <eof>".to_string(),
            excerpt: "12 | BROKEN_SYNTAX\n   | ^".to_string(),
            symbol: None,
            route_bound: None,
        });
        let targets =
            final_acceptance_recovery_repair_targets(&report, RepairTarget::Implementation);
        assert_eq!(
            targets.first().map(String::as_str),
            Some("fix_compile_error")
        );
        let evidence =
            final_acceptance_recovery_failure_evidence(&report, "compile_repair_no_source_change");
        assert!(
            evidence
                .first()
                .is_some_and(|line| line.contains("fix_compile_error: Compile error")),
            "{evidence:?}"
        );
        assert!(
            evidence
                .iter()
                .any(|line| line.contains("12 | BROKEN_SYNTAX"))
        );

        let _handoff = save_ultra_phase_recovery_handoff_with_evidence(
            &cfg,
            &plan,
            &plan.phases[1],
            UltraPhaseRecoveryRequest {
                failure_kind: "compile_repair_no_source_change",
                reason: "compile repair did not edit files",
                missing_paths: &[],
                missing_signals: &[],
                repair_targets: &targets,
            },
            &evidence,
        )
        .expect("recovery handoff");

        let repair_text = std::fs::read_dir(dir.path().join(".anvil/repairs"))
            .unwrap()
            .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
            .find(|text| text.contains("Failure evidence:"))
            .expect("repair prompt");
        let evidence_index = repair_text.find("fix_compile_error").unwrap();
        let target_index = repair_text
            .find("Repair targets:\n- fix_compile_error")
            .unwrap();
        assert!(evidence_index < target_index, "{repair_text}");
        let recovery_plan = assert_single_recovery_ultra_plan(dir.path());
        let repair_phase = &recovery_plan.phases[1].prompt;
        let evidence_index = repair_phase.find("fix_compile_error").unwrap();
        let target_index = repair_phase
            .find("Repair targets:\n- fix_compile_error")
            .unwrap();
        assert!(evidence_index < target_index, "{repair_phase}");
    }

    #[cfg(unix)]
    fn shell_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    #[cfg(unix)]
    static DEV_SERVER_PROBE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[cfg(unix)]
    fn dev_server_probe_test_guard() -> std::sync::MutexGuard<'static, ()> {
        DEV_SERVER_PROBE_TEST_LOCK.lock().unwrap()
    }

    #[cfg(unix)]
    static TEST_DEV_SERVER_PORT: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(34_011);

    #[cfg(unix)]
    fn free_local_port() -> u16 {
        for _ in 0..2_000 {
            let port = TEST_DEV_SERVER_PORT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if port > u16::MAX as usize {
                break;
            }
            let port = port as u16;
            if port == NEXTJS_DEV_SERVER_DEFAULT_PORT {
                continue;
            }
            if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
                return port;
            }
        }
        loop {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let port = listener.local_addr().unwrap().port();
            if port != NEXTJS_DEV_SERVER_DEFAULT_PORT {
                return port;
            }
        }
    }

    fn read_jsonl_events(path: &Path) -> Vec<Value> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect()
    }

    fn dev_server_stage_names(events: &[Value]) -> Vec<&str> {
        events
            .iter()
            .filter(|event| {
                event.get("event").and_then(Value::as_str) == Some("dev_server_lifecycle")
            })
            .filter_map(|event| event.get("stage").and_then(Value::as_str))
            .collect()
    }

    #[cfg(unix)]
    fn wait_until_process_group_gone(pgid: u32, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if !process_group_exists(pgid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        !process_group_exists(pgid)
    }

    #[cfg(unix)]
    fn process_group_exists(pgid: u32) -> bool {
        let Ok(pgid) = i32::try_from(pgid) else {
            return false;
        };
        // SAFETY: signal 0 performs existence/permission checking only, using
        // a process-group id originally emitted from a spawned child pid.
        let rc = unsafe { libc::kill(-pgid, 0) };
        if rc == 0 {
            return true;
        }
        let err = std::io::Error::last_os_error();
        err.raw_os_error() != Some(libc::ESRCH)
    }

    #[test]
    fn requested_port_overrides_dev_server_probe_spec() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"dev":"next dev","build":"next build"},"dependencies":{"next":"x","react":"x","react-dom":"x"}}"#,
        )
        .unwrap();

        let spec = load_nextjs_dev_server_probe_spec(dir.path(), Some(4000)).unwrap();

        assert_eq!(spec.port, 4000);
    }

    #[test]
    fn dev_server_probe_spec_defaults_to_3011_without_request() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"dev":"next dev -p 4010","build":"next build"},"dependencies":{"next":"x","react":"x","react-dom":"x"}}"#,
        )
        .unwrap();

        let spec = load_nextjs_dev_server_probe_spec(dir.path(), None).unwrap();

        assert_eq!(spec.port, 3011);
    }

    #[test]
    fn requested_port_telemetry_labels_nextjs_default_and_explicit_goal() {
        let default = effective_requested_port("nextjs", "ブラウザで使えるメモアプリ", None)
            .expect("nextjs default port");
        assert_eq!(default.port, 3011);
        assert_eq!(default.telemetry, "3011 (default)");

        let explicit =
            effective_requested_port("nextjs", "4000番ポートで起動", None).expect("explicit port");
        assert_eq!(explicit.port, 4000);
        assert_eq!(explicit.telemetry, "4000 (goal)");
    }

    #[test]
    fn requested_port_telemetry_leaves_python_cli_without_default_port() {
        assert_eq!(
            effective_requested_port("python-cli", "CSVを集計するCLI", None),
            None
        );
    }

    #[test]
    fn canvas_surface_gate_uses_ultra_phase_text_and_japanese_tokens() {
        let plan = UltraPlan {
            goal: "最高に面白いブラウザ体験を作る".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![UltraPhase {
                id: "implement".to_string(),
                prompt: "HTML5 Canvas のプレイ画面を実装する".to_string(),
            }],
        };
        let capabilities = vec!["stateful_interaction".to_string()];

        assert!(requires_canvas_surface(
            &ultra_plan_signal_text(&plan),
            &capabilities
        ));
        assert!(requires_canvas_surface("カンバスを描画する", &capabilities));
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
            chat_timeout_source: "override:test".to_string(),
            field_sources: crate::config::ConfigFieldSources::default(),
            chat_retries: 1,
            resume: None,
            fresh_session: false,
            no_footer: false,
            narration: crate::config::NarrationMode::Normal,
            profile: "generic".to_string(),
            profile_explicit: false,
            profile_inference: None,
            style: "default".to_string(),
            action: crate::config::Action::Repl,
        }
    }
}
