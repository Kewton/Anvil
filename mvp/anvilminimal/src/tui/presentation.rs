use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};

use serde_json::Value;

use crate::config::{Config, NarrationMode};
use crate::planner::sanitizer::SanitizerReport;
use crate::planner::step_plan::StepPlan;
use crate::planner::ultra_plan::UltraPlan;
use crate::tui::OutputRenderer;

const GOAL_WIDTH: usize = 120;
const PHASE_WIDTH: usize = 96;
const STEP_WIDTH: usize = 96;
const ACTIVITY_WIDTH: usize = 140;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanProgress {
    pub completed_phases: Vec<String>,
    pub current_phase: Option<String>,
    pub current_step: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PhaseProjection {
    id: String,
    prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StepProjection {
    id: String,
    kind: String,
    instruction: String,
    verify_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UltraPlanProjection {
    goal: String,
    profile: String,
    phases: Vec<PhaseProjection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StepPlanProjection {
    phase: String,
    steps: Vec<StepProjection>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PresentationState {
    enabled: bool,
    mode: NarrationMode,
    ultra_plan: Option<UltraPlanProjection>,
    current_step_plan: Option<StepPlanProjection>,
    progress: PlanProgress,
    current_activity: Option<String>,
}

static PRESENTATION: OnceLock<Mutex<PresentationState>> = OnceLock::new();

fn global() -> &'static Mutex<PresentationState> {
    PRESENTATION.get_or_init(|| {
        Mutex::new(PresentationState {
            mode: NarrationMode::Quiet,
            ..PresentationState::default()
        })
    })
}

pub struct PresentationGuard {
    previous: PresentationState,
}

impl Drop for PresentationGuard {
    fn drop(&mut self) {
        *lock_state() = self.previous.clone();
    }
}

pub fn install(config: &Config) -> PresentationGuard {
    let mut state = lock_state();
    let previous = state.clone();
    state.enabled = !config.narration.is_quiet();
    state.mode = config.narration;
    PresentationGuard { previous }
}

pub fn installed_quiet() -> bool {
    let state = lock_state();
    !state.enabled || state.mode.is_quiet()
}

pub fn emit_ultra_plan_card(plan: &UltraPlan, progress: &PlanProgress) {
    update_ultra_plan(plan);
    update_progress(progress);
    emit_markdown(&render_ultra_plan_card(plan, progress));
}

pub fn emit_step_plan_block(
    phase_name: &str,
    plan: &StepPlan,
    sanitizer_report: Option<&SanitizerReport>,
) {
    update_step_plan(phase_name, plan);
    emit_markdown(&render_step_plan_block(phase_name, plan, sanitizer_report));
}

pub fn project_event(event: &Value) {
    let Some(line) = render_activity_line(event) else {
        update_progress_from_event(event, None);
        return;
    };
    let activity = event_updates_current_activity(event).then_some(line.clone());
    update_progress_from_event(event, activity);
    emit_markdown(&line);
}

pub fn emit_provider_turn_started(scope: &str, model: &str, deadline_secs: u64) {
    emit_activity_breadcrumb(&render_provider_turn_started(
        scope,
        model,
        deadline_secs,
        &provider_breadcrumb_context(scope),
    ));
}

pub fn emit_provider_turn_progress(elapsed_secs: u64, deadline_secs: u64) {
    emit_activity_breadcrumb(&render_provider_turn_progress(elapsed_secs, deadline_secs));
}

pub fn emit_provider_turn_completed(scope: &str, duration_secs: u64) {
    emit_activity_breadcrumb(&render_provider_turn_completed(scope, duration_secs));
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderBreadcrumbContext {
    pub phase: Option<ProviderBreadcrumbPhase>,
    pub repair_target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderBreadcrumbPhase {
    pub index: usize,
    pub total: usize,
    pub id: String,
}

pub fn render_provider_turn_started(
    scope: &str,
    model: &str,
    deadline_secs: u64,
    context: &ProviderBreadcrumbContext,
) -> String {
    let model = provider_model_label(model);
    match scope {
        "planner_step" => {
            if let Some(phase) = &context.phase {
                format!(
                    "→ planning steps for phase {}/{} {} ({model}, up to {deadline_secs}s)",
                    phase.index,
                    phase.total,
                    fit(&phase.id, 72)
                )
            } else {
                format!("→ planning steps ({model}, up to {deadline_secs}s)")
            }
        }
        "planner_ultra" => {
            format!("→ planning the overall plan ({model}, up to {deadline_secs}s)")
        }
        "executor" => format!("→ implementing (model turn: {model}, up to {deadline_secs}s)"),
        "repair" => {
            if let Some(target) = &context.repair_target {
                format!(
                    "→ repairing: {} ({model}, up to {deadline_secs}s)",
                    fit(target, 96)
                )
            } else {
                format!("→ repairing ({model}, up to {deadline_secs}s)")
            }
        }
        _ => format!("→ model turn ({model}, up to {deadline_secs}s)"),
    }
}

pub fn render_provider_turn_progress(elapsed_secs: u64, deadline_secs: u64) -> String {
    format!("… still waiting ({elapsed_secs}s/{deadline_secs}s)")
}

pub fn render_provider_turn_completed(scope: &str, duration_secs: u64) -> String {
    match scope {
        "planner_ultra" => format!("✓ overall plan ready ({duration_secs}s)"),
        "planner_step" => format!("✓ step plan ready ({duration_secs}s)"),
        "executor" => format!("✓ implementation turn finished ({duration_secs}s)"),
        "repair" => format!("✓ repair turn finished ({duration_secs}s)"),
        _ => format!("✓ model turn finished ({duration_secs}s)"),
    }
}

pub fn render_current_plan() -> String {
    let state = lock_state().clone();
    let Some(plan) = state.ultra_plan else {
        return "No active plan projection is available.".to_string();
    };
    let plan = UltraPlan {
        goal: plan.goal,
        profile: plan.profile,
        style: "projection".to_string(),
        intent: "current".to_string(),
        phases: plan
            .phases
            .into_iter()
            .map(|phase| crate::planner::ultra_plan::UltraPhase {
                id: phase.id,
                prompt: phase.prompt,
            })
            .collect(),
    };
    let mut out = render_ultra_plan_card(&plan, &state.progress);
    if let Some(step_plan) = state.current_step_plan {
        out.push_str("\n\n");
        out.push_str(&render_step_projection_block(&step_plan, &state.progress));
    }
    if let Some(activity) = state.current_activity {
        out.push_str("\n\n");
        out.push_str("Current activity: ");
        out.push_str(&fit(&activity, ACTIVITY_WIDTH));
    }
    out
}

pub fn render_status_card(config: &Config) -> String {
    let inference = match (config.profile_explicit, config.profile_inference) {
        (true, _) => "explicit".to_string(),
        (false, Some(inference)) => {
            format!("{} -> {}", inference.source.as_str(), inference.profile)
        }
        (false, None) => "none".to_string(),
    };
    let probe = match crate::minimal_loop::interaction_probe::playwright_availability(
        &config.workspace_root,
    ) {
        crate::minimal_loop::interaction_probe::ProbeAvailability::Available(resolution) => {
            format!(
                "available: playwright {} ({})",
                resolution.version, resolution.location
            )
        }
        crate::minimal_loop::interaction_probe::ProbeAvailability::Unavailable(reason) => {
            format!(
                "unavailable:{reason}; {}",
                crate::minimal_loop::interaction_probe::INTERACTION_PROBE_SETUP_REMEDIATION
            )
        }
    };
    [
        "### Status".to_string(),
        format!("- Model: {} ({})", config.model, config.field_sources.model),
        format!(
            "- Provider: {} ({})",
            provider_label(config.provider),
            config.field_sources.provider
        ),
        format!(
            "- Planner model: {} ({})",
            config.planner_model, config.field_sources.planner_model
        ),
        format!(
            "- Planner provider: {} ({})",
            provider_label(config.planner_provider),
            config.field_sources.planner_provider
        ),
        format!(
            "- Timeout: {}s ({})",
            config.chat_timeout_secs, config.field_sources.chat_timeout_secs
        ),
        format!(
            "- Context budget: {} ({})",
            config.context_budget, config.field_sources.context_budget
        ),
        format!(
            "- Profile: {} ({})",
            config.profile, config.field_sources.profile
        ),
        format!("- Inference: {inference}"),
        format!("- Port: {}", status_port(config)),
        format!("- Probe: {probe}"),
        format!(
            "- Narration: {} ({})",
            narration_label(config.narration),
            config.field_sources.narration
        ),
        format!(
            "- Footer: {} ({})",
            footer_label(config.no_footer),
            config.field_sources.footer
        ),
    ]
    .join("\n")
}

pub fn render_ultra_plan_card(plan: &UltraPlan, progress: &PlanProgress) -> String {
    let port = crate::planner::signals::requested_port_from_text(&plan.goal)
        .map(|port| port.to_string())
        .unwrap_or_else(|| "not specified".to_string());
    let assurance = if plan.profile == "generic" {
        "earned after final acceptance; static until gates run"
    } else {
        "earned after final acceptance re-verifies all gates"
    };
    let mut lines = vec![
        "### Plan".to_string(),
        format!("- Goal: {}", fit(&plan.goal, GOAL_WIDTH)),
        format!("- Profile: {}", plan.profile),
        format!("- Port: {port}"),
        format!("- Assurance expectation: {assurance}"),
        "- Phases:".to_string(),
    ];
    for (index, phase) in plan.phases.iter().enumerate() {
        let mark = phase_mark(&phase.id, progress);
        lines.push(format!(
            "{}. {} {} - {}",
            index + 1,
            mark,
            phase.id,
            fit(&phase.prompt, PHASE_WIDTH)
        ));
    }
    let skipped = progress
        .completed_phases
        .iter()
        .filter(|id| !plan.phases.iter().any(|phase| &phase.id == *id))
        .map(|id| format!("✓ {id}"))
        .collect::<Vec<_>>();
    if !skipped.is_empty() {
        lines.push(format!(
            "- Completed before this plan: {}",
            skipped.join(", ")
        ));
    }
    lines.join("\n")
}

pub fn render_step_plan_block(
    phase_name: &str,
    plan: &StepPlan,
    sanitizer_report: Option<&SanitizerReport>,
) -> String {
    let projection = step_projection(phase_name, plan);
    let mut out = render_step_projection_block(&projection, &PlanProgress::default());
    if let Some(summary) = sanitizer_report.and_then(sanitizer_summary) {
        out.push('\n');
        out.push_str("plan normalized: ");
        out.push_str(&summary);
    }
    out
}

pub fn render_activity_line(event: &Value) -> Option<String> {
    let name = event.get("event").and_then(Value::as_str)?;
    match name {
        "ultra_phase_start" => {
            let index = event
                .get("phase_index")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let total = event
                .get("total_phases")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let phase = text(event, "phase_id").unwrap_or_else(|| "unknown".to_string());
            Some(format!("── Phase {index}/{total}: {} ──", fit(&phase, 72)))
        }
        "ultra_phase_complete" => Some(phase_status_label(event, "✓ Phase")),
        "ultra_phase_failed" => {
            let mut line = phase_status_label(event, "✗ Phase");
            if let Some(reason) = text(event, "reason") {
                line.push(' ');
                line.push_str(&fit(&reason, 80));
            }
            Some(line)
        }
        "step_prompt_contract" => {
            let id = text(event, "step_id").unwrap_or_else(|| "step".to_string());
            let kind = text(event, "step_kind").unwrap_or_else(|| "step".to_string());
            Some(format!("→ step {} [{}]", fit(&id, 72), fit(&kind, 24)))
        }
        "tool_call_raw" => Some(format!("→ {}", tool_start_label(event))),
        "tool_execute" => {
            let name = text(event, "name").unwrap_or_else(|| "tool".to_string());
            let status = text(event, "status").unwrap_or_default();
            if status == "ok" {
                Some(format!("✓ {name} ok"))
            } else {
                let kind = text(event, "error_kind").unwrap_or_else(|| "error".to_string());
                Some(format!("✗ {name} {kind}"))
            }
        }
        "tool_validation_error" | "tool_policy_error" => {
            let name = text(event, "name").unwrap_or_else(|| "tool".to_string());
            let kind = text(event, "error_kind")
                .or_else(|| text(event, "policy_error_kind"))
                .unwrap_or_else(|| "validation_error".to_string());
            Some(format!("✗ {name} {kind}"))
        }
        "step_verify_failure" => {
            let reason =
                text(event, "primary_reason").unwrap_or_else(|| "verify failed".to_string());
            Some(format!("✗ verify {}", fit(&reason, 100)))
        }
        "step_verify_repair" | "verify_repair_turn" | "verify_repair_progress" => {
            let attempt = event
                .get("repair_attempt")
                .or_else(|| event.get("attempt"))
                .and_then(Value::as_u64)
                .unwrap_or(1);
            let max = event
                .get("max_repair_turns")
                .or_else(|| event.get("max"))
                .and_then(Value::as_u64)
                .unwrap_or(2);
            let target = text(event, "repair_target")
                .or_else(|| text(event, "target"))
                .unwrap_or_else(|| "repair".to_string());
            Some(format!("↻ repair {attempt}/{max}: {}", fit(&target, 96)))
        }
        "final_acceptance_repair_start" => {
            let attempt = event
                .get("repair_attempt")
                .or_else(|| event.get("attempt"))
                .and_then(Value::as_u64)
                .unwrap_or(1);
            let max = event
                .get("max_repair_turns")
                .or_else(|| event.get("max"))
                .and_then(Value::as_u64)
                .unwrap_or(2);
            let target = text(event, "repair_target")
                .or_else(|| text(event, "target"))
                .unwrap_or_else(|| "final acceptance".to_string());
            Some(format!("↻ repair {attempt}/{max}: {}", fit(&target, 96)))
        }
        "browser_probe" => {
            let status = text(event, "status").unwrap_or_default();
            if event.get("ok").and_then(Value::as_bool).unwrap_or(false) {
                let code = event
                    .get("http_status")
                    .and_then(Value::as_u64)
                    .map(|code| format!("HTTP {code}"))
                    .unwrap_or_else(|| "ok".to_string());
                Some(format!("✓ probe: {code}"))
            } else if status.is_empty() {
                Some("→ probe: navigating".to_string())
            } else {
                let kind = text(event, "failure_kind").unwrap_or(status);
                Some(format!("✗ probe: {}", fit(&kind, 96)))
            }
        }
        "browser_interaction_probe" => {
            if event.get("ok").and_then(Value::as_bool).unwrap_or(false) {
                Some("✓ probe: interaction ok".to_string())
            } else {
                let kind = text(event, "failure_kind")
                    .or_else(|| text(event, "status"))
                    .unwrap_or_else(|| "interaction failed".to_string());
                Some(format!("✗ probe: {}", fit(&kind, 96)))
            }
        }
        "phase_verification_result" => {
            let phase = text(event, "phase_id").unwrap_or_else(|| "phase".to_string());
            let mode = text(event, "phase_verification_mode")
                .unwrap_or_else(|| "verification".to_string());
            if event.get("ok").and_then(Value::as_bool).unwrap_or(false) {
                Some(format!("✓ verify {} ({mode})", fit(&phase, 72)))
            } else {
                let reason = text(event, "reason").unwrap_or_else(|| "failed".to_string());
                Some(format!(
                    "✗ verify {} ({mode}): {}",
                    fit(&phase, 56),
                    fit(&reason, 72)
                ))
            }
        }
        "ultra_final_acceptance" => {
            let status =
                text(event, "final_acceptance_status").unwrap_or_else(|| "unknown".to_string());
            if status == "pass" {
                Some("✓ final acceptance pass".to_string())
            } else {
                Some(format!("✗ final acceptance {}", fit(&status, 72)))
            }
        }
        "ultra_plan_complete" => {
            let total = event
                .get("total_phases")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            Some(format!("✓ ultra plan complete {total} phases"))
        }
        "planner_error" => {
            let kind = text(event, "kind")
                .or_else(|| text(event, "error_kind"))
                .unwrap_or_else(|| "planner_error".to_string());
            let message = text(event, "message")
                .or_else(|| text(event, "reason"))
                .unwrap_or_default();
            if message.is_empty() {
                Some(format!("✗ planner {kind}"))
            } else {
                Some(format!("✗ planner {kind}: {}", fit(&message, 96)))
            }
        }
        "provider_turn_aborted_by_user" => Some("✗ provider aborted by user".to_string()),
        "dependency_build_lifecycle" => {
            let status = text(event, "status").unwrap_or_else(|| "dependency".to_string());
            Some(format!("→ npm install {status}"))
        }
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityProjectionStatus {
    Rendered,
    Ignored,
    Unclassified,
}

pub fn activity_projection_status(event: &Value) -> ActivityProjectionStatus {
    if render_activity_line(event).is_some() {
        return ActivityProjectionStatus::Rendered;
    }
    let Some(name) = event.get("event").and_then(Value::as_str) else {
        return ActivityProjectionStatus::Unclassified;
    };
    if documented_activity_ignore_reason(name).is_some() {
        ActivityProjectionStatus::Ignored
    } else {
        ActivityProjectionStatus::Unclassified
    }
}

pub fn documented_activity_ignore_reason(name: &str) -> Option<&'static str> {
    match name {
        // Process and command lifecycle records are summarized by the terminal card.
        "run_start" | "run_stop" | "tui_command_start" | "tui_command_stop" => {
            Some("lifecycle summarized by terminal summary card")
        }
        // Planner bookkeeping is noisy in scrollback; visible plan cards cover the accepted result.
        "ultra_plan_generation_attempt"
        | "ultra_plan_raw_output_shape"
        | "ultra_plan_generation_succeeded"
        | "ultra_plan_generation_retry"
        | "ultra_plan_generation_metadata_normalized"
        | "planner_quality_warning"
        | "planner_quality_issue" => Some("planner bookkeeping covered by plan cards"),
        // Context/progress records update projections and footer state without adding useful text.
        "ultra_context_initialized"
        | "ultra_phase_context_attached"
        | "ultra_phase_scaffold_complete"
        | "ultra_phase_execute_complete"
        | "ultra_phase_plan_validated"
        | "ultra_phase_profile_check"
        | "profile_inferred"
        | "profile_reinferred"
        | "generic_contract_bound" => Some("projection state update"),
        // Provider and loop metrics are better inspected in events.jsonl than narrated line by line.
        "provider_turn_duration"
        | "loop_stop"
        | "runtime_bash_policy"
        | "tool_args_recovered"
        | "empty_response_recovered"
        | "provider_turn_timeout_recovered" => Some("low-value runtime metric"),
        // Recovery artifacts are surfaced by the summary card and resume hints.
        "recovery_prompt_saved" | "recovery_ultra_plan_saved" | "summary_written" => {
            Some("recovery artifact summarized")
        }
        _ => None,
    }
}

#[cfg(test)]
pub fn reset_for_test() {
    *lock_state() = PresentationState {
        enabled: false,
        mode: NarrationMode::Normal,
        ..PresentationState::default()
    };
}

#[cfg(test)]
pub fn set_plan_for_test(plan: &UltraPlan, progress: PlanProgress, activity: Option<String>) {
    update_ultra_plan(plan);
    update_progress(&progress);
    lock_state().current_activity = activity;
}

fn update_ultra_plan(plan: &UltraPlan) {
    let mut state = lock_state();
    state.ultra_plan = Some(UltraPlanProjection {
        goal: plan.goal.clone(),
        profile: plan.profile.clone(),
        phases: plan
            .phases
            .iter()
            .map(|phase| PhaseProjection {
                id: phase.id.clone(),
                prompt: phase.prompt.clone(),
            })
            .collect(),
    });
}

fn update_step_plan(phase_name: &str, plan: &StepPlan) {
    lock_state().current_step_plan = Some(step_projection(phase_name, plan));
}

fn update_progress(progress: &PlanProgress) {
    lock_state().progress = progress.clone();
}

fn update_progress_from_event(event: &Value, activity: Option<String>) {
    let mut state = lock_state();
    if let Some(activity) = activity {
        state.current_activity = Some(activity);
    }
    let name = event.get("event").and_then(Value::as_str).unwrap_or("");
    match name {
        "ultra_phase_start" => {
            state.progress.current_phase = text(event, "phase_id");
            state.progress.current_step = None;
        }
        "ultra_phase_complete" | "ultra_phase_execute_complete" => {
            if let Some(phase) = text(event, "phase_id")
                && !state.progress.completed_phases.contains(&phase)
            {
                state.progress.completed_phases.push(phase);
            }
        }
        "step_prompt_contract" => {
            state.progress.current_step = text(event, "step_id");
        }
        _ => {}
    }
}

fn event_updates_current_activity(event: &Value) -> bool {
    let name = event.get("event").and_then(Value::as_str).unwrap_or("");
    !matches!(
        name,
        "ultra_phase_complete"
            | "ultra_phase_execute_complete"
            | "ultra_phase_scaffold_complete"
            | "phase_verification_result"
            | "ultra_final_acceptance"
            | "ultra_plan_complete"
    )
}

fn emit_activity_breadcrumb(line: &str) {
    lock_state().current_activity = Some(line.to_string());
    emit_markdown(line);
}

fn provider_breadcrumb_context(scope: &str) -> ProviderBreadcrumbContext {
    let state = lock_state().clone();
    let phase = state.progress.current_phase.as_ref().and_then(|current| {
        let plan = state.ultra_plan.as_ref()?;
        let index = plan
            .phases
            .iter()
            .position(|phase| phase.id == *current)?
            .saturating_add(1);
        Some(ProviderBreadcrumbPhase {
            index,
            total: plan.phases.len(),
            id: current.clone(),
        })
    });
    drop(state);
    let repair_target = (scope == "repair")
        .then(|| {
            crate::tui::status_bus::snapshot_global()
                .and_then(|runtime| runtime.repair.map(|repair| repair.kind))
        })
        .flatten();
    ProviderBreadcrumbContext {
        phase,
        repair_target,
    }
}

fn provider_model_label(model: &str) -> String {
    model
        .split(':')
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(model)
        .to_string()
}

fn render_step_projection_block(plan: &StepPlanProjection, progress: &PlanProgress) -> String {
    let mut lines = vec![format!("#### Phase: {}", fit(&plan.phase, PHASE_WIDTH))];
    for (index, step) in plan.steps.iter().enumerate() {
        let mark = if progress.current_step.as_deref() == Some(step.id.as_str()) {
            "▶"
        } else {
            "○"
        };
        let verify = if step.verify_count == 0 {
            "verify 0".to_string()
        } else {
            format!("verify {}", step.verify_count)
        };
        lines.push(format!(
            "{}. {} [{}] {} ({verify})",
            index + 1,
            mark,
            step.kind,
            fit(&step.instruction, STEP_WIDTH)
        ));
    }
    lines.join("\n")
}

fn step_projection(phase_name: &str, plan: &StepPlan) -> StepPlanProjection {
    StepPlanProjection {
        phase: phase_name.to_string(),
        steps: plan
            .steps
            .iter()
            .map(|step| StepProjection {
                id: step.id.clone(),
                kind: step.kind.clone(),
                instruction: step.instruction.clone(),
                verify_count: step.verify.len(),
            })
            .collect(),
    }
}

fn sanitizer_summary(report: &SanitizerReport) -> Option<String> {
    if report.is_empty() {
        return None;
    }
    let mut counts = BTreeMap::<String, usize>::new();
    add_count(&mut counts, "goal_truncated", report.goal_truncations.len());
    add_count(
        &mut counts,
        "verify_command_normalized",
        report.normalized_commands.len(),
    );
    add_count(
        &mut counts,
        "shell_control_split",
        report.shell_control_splits.len(),
    );
    add_count(
        &mut counts,
        "command_removed",
        report.removed_commands.len(),
    );
    add_count(
        &mut counts,
        "verify_command_substituted",
        report.substituted_commands.len(),
    );
    add_count(&mut counts, "command_moved", report.moved_commands.len());
    add_count(
        &mut counts,
        "setup_verify_relocated",
        report.setup_verify_relocations.len(),
    );
    add_count(
        &mut counts,
        "command_dropped",
        report.dropped_commands.len(),
    );
    add_count(&mut counts, "step_retyped", report.retyped_steps.len());
    add_count(
        &mut counts,
        "instruction_truncated",
        report.instruction_truncations.len(),
    );
    add_count(
        &mut counts,
        "instruction_note",
        report.instruction_notes.len(),
    );
    let parts = counts
        .into_iter()
        .map(|(kind, count)| {
            if count == 1 {
                kind
            } else {
                format!("{kind} x{count}")
            }
        })
        .collect::<Vec<_>>();
    Some(parts.join(", "))
}

fn add_count(counts: &mut BTreeMap<String, usize>, key: &str, count: usize) {
    if count > 0 {
        counts.insert(key.to_string(), count);
    }
}

fn phase_mark(id: &str, progress: &PlanProgress) -> &'static str {
    if progress.completed_phases.iter().any(|phase| phase == id) {
        "✓"
    } else if progress.current_phase.as_deref() == Some(id) {
        "▶"
    } else {
        "○"
    }
}

fn status_port(config: &Config) -> String {
    if !crate::planner::profile::is_nextjs_profile(&config.profile) {
        return "not applicable".to_string();
    }
    let goal = crate::config::action_goal(&config.action).unwrap_or("");
    if let Some(port) = crate::planner::signals::requested_port_from_text(goal) {
        format!("{port} (goal)")
    } else {
        format!(
            "{} (default)",
            crate::planner::profiles::nextjs::DEFAULT_REQUESTED_PORT
        )
    }
}

fn provider_label(provider: crate::config::Provider) -> &'static str {
    match provider {
        crate::config::Provider::Ollama => "ollama",
        crate::config::Provider::Openai => "openai",
        crate::config::Provider::Gemini => "gemini",
    }
}

fn narration_label(mode: NarrationMode) -> &'static str {
    match mode {
        NarrationMode::Normal => "normal",
        NarrationMode::Quiet => "quiet",
    }
}

fn footer_label(no_footer: bool) -> &'static str {
    if no_footer { "off" } else { "on" }
}

fn tool_start_label(event: &Value) -> String {
    let name = text(event, "name").unwrap_or_else(|| "tool".to_string());
    let Some(summary) = event.get("arguments") else {
        return name;
    };
    let preview = argument_preview(summary, "path")
        .or_else(|| argument_preview(summary, "command"))
        .or_else(|| argument_preview(summary, "pattern"));
    match preview {
        Some(preview) => format!("{name} {}", fit(&preview, 96)),
        None => name,
    }
}

fn phase_status_label(event: &Value, prefix: &str) -> String {
    let index = event
        .get("phase_index")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total = event
        .get("total_phases")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let phase = text(event, "phase_id").unwrap_or_else(|| "unknown".to_string());
    if index == 0 && total == 0 {
        format!("{prefix}: {}", fit(&phase, 72))
    } else {
        format!("{prefix} {index}/{total}: {}", fit(&phase, 72))
    }
}

fn argument_preview(summary: &Value, key: &str) -> Option<String> {
    summary
        .get("argument_summaries")
        .and_then(|value| value.get(key))
        .and_then(|value| value.get("preview"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn text(event: &Value, key: &str) -> Option<String> {
    event
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn emit_markdown(text: &str) {
    let state = lock_state();
    if !state.enabled || state.mode.is_quiet() {
        return;
    }
    drop(state);
    if std::io::stdout().is_terminal() {
        let renderer = crate::tui::markdown::TerminalMarkdownRenderer::for_stdout();
        let _ = renderer.render_assistant(text);
    } else {
        let _ = crate::tui::markdown::PlainRenderer.render_assistant(text);
    }
}

fn fit(value: &str, max: usize) -> String {
    crate::util::excerpt_with_marker(&value.replace(['\n', '\r'], " "), max, "...")
}

fn lock_state() -> MutexGuard<'static, PresentationState> {
    global()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[allow(dead_code)]
fn _path_exists(path: &Path) -> bool {
    path.exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Action, ConfigFieldSources, Provider};
    use crate::planner::sanitizer::{
        SanitizedGoalTruncationRecord, SanitizedShellControlSplitRecord,
    };
    use crate::planner::step_plan::{PlanStep, StepPlan};
    use crate::planner::ultra_plan::{UltraPhase, UltraPlan};
    use serde_json::json;

    #[test]
    fn plan_card_and_step_block_snapshot_handle_japanese_goal() {
        let plan = UltraPlan {
            goal: "日本語のメモアプリを4000番ポートで作ってください".to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "setup".to_string(),
                    prompt: "package.json と Next.js の土台を作る".to_string(),
                },
                UltraPhase {
                    id: "implement".to_string(),
                    prompt: "メモ追加と永続化の UI を実装する".to_string(),
                },
            ],
        };
        let progress = PlanProgress {
            completed_phases: vec!["setup".to_string()],
            current_phase: Some("implement".to_string()),
            current_step: None,
        };
        let card = render_ultra_plan_card(&plan, &progress);

        assert!(card.contains("- Goal: 日本語のメモアプリ"), "{card}");
        assert!(card.contains("- Profile: nextjs"), "{card}");
        assert!(card.contains("- Port: 4000"), "{card}");
        assert!(card.contains("1. ✓ setup"), "{card}");
        assert!(card.contains("2. ▶ implement"), "{card}");

        let step_plan = StepPlan {
            goal: "実装".to_string(),
            steps: vec![PlanStep {
                id: "write-page".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "src/app/page.tsx にメモ UI を作る".to_string(),
                expected_paths: vec!["src/app/page.tsx".to_string()],
                verify: vec!["test -f src/app/page.tsx".to_string()],
            }],
        };
        let sanitizer = SanitizerReport {
            goal_truncations: vec![SanitizedGoalTruncationRecord {
                kind: "goal_truncated".to_string(),
                original_len: 900,
                new_len: 600,
            }],
            shell_control_splits: vec![
                SanitizedShellControlSplitRecord {
                    kind: "shell_control_split".to_string(),
                    step_id: "write-page".to_string(),
                    original_command: "test -f a && test -f b".to_string(),
                    fragments: vec!["test -f a".to_string(), "test -f b".to_string()],
                    dropped_fallback: None,
                },
                SanitizedShellControlSplitRecord {
                    kind: "shell_control_split".to_string(),
                    step_id: "write-page".to_string(),
                    original_command: "test -f c && test -f d".to_string(),
                    fragments: vec!["test -f c".to_string(), "test -f d".to_string()],
                    dropped_fallback: None,
                },
            ],
            ..SanitizerReport::default()
        };
        let block = render_step_plan_block("implement", &step_plan, Some(&sanitizer));

        assert!(block.contains("#### Phase: implement"), "{block}");
        assert!(
            block.contains("1. ○ [implement] src/app/page.tsx にメモ UI を作る (verify 1)"),
            "{block}"
        );
        assert!(
            block.contains("plan normalized: goal_truncated, shell_control_split x2"),
            "{block}"
        );
    }

    #[test]
    fn activity_narration_scripted_sequence_snapshot() {
        let events = vec![
            json!({"event": "step_prompt_contract", "step_id": "write-page", "step_kind": "implement"}),
            json!({
                "event": "tool_call_raw",
                "name": "Write",
                "arguments": {
                    "argument_summaries": {
                        "path": {"preview": "src/app/page.tsx"}
                    }
                }
            }),
            json!({"event": "tool_execute", "name": "Write", "status": "ok"}),
            json!({"event": "step_verify_failure", "primary_reason": "missing path package.json"}),
            json!({"event": "step_verify_repair", "repair_attempt": 1, "max_repair_turns": 2, "repair_target": "missing_artifact"}),
            json!({"event": "browser_probe", "ok": true, "http_status": 200, "status": "pass"}),
            json!({"event": "phase_verification_result", "phase_id": "implement", "phase_verification_mode": "final_acceptance", "ok": true}),
        ];
        let lines = events
            .iter()
            .filter_map(render_activity_line)
            .collect::<Vec<_>>();

        assert_eq!(
            lines,
            vec![
                "→ step write-page [implement]",
                "→ Write src/app/page.tsx",
                "✓ Write ok",
                "✗ verify missing path package.json",
                "↻ repair 1/2: missing_artifact",
                "✓ probe: HTTP 200",
                "✓ verify implement (final_acceptance)",
            ]
        );
    }

    #[test]
    fn activity_projection_audits_standard_ultra_event_fixture() {
        let fixture = include_str!("../../tests/fixtures/standard-ultra-events.jsonl");
        let mut total = 0usize;
        let mut classified = 0usize;
        let mut kinds = std::collections::BTreeSet::new();
        let mut unclassified = Vec::new();

        for line in fixture.lines().filter(|line| !line.trim().is_empty()) {
            let event = serde_json::from_str::<serde_json::Value>(line).unwrap();
            let kind = event
                .get("event")
                .and_then(serde_json::Value::as_str)
                .unwrap()
                .to_string();
            kinds.insert(kind.clone());
            total += 1;
            match activity_projection_status(&event) {
                ActivityProjectionStatus::Rendered | ActivityProjectionStatus::Ignored => {
                    classified += 1;
                }
                ActivityProjectionStatus::Unclassified => unclassified.push(kind),
            }
        }

        assert!(kinds.contains("tool_call_raw"));
        assert!(kinds.contains("phase_verification_result"));
        assert!(kinds.contains("browser_interaction_probe"));
        assert!(
            classified * 100 >= total * 90,
            "classified {classified}/{total}; unclassified={unclassified:?}; kinds={kinds:?}"
        );
        assert!(
            unclassified.is_empty(),
            "fixture should document every ignored event explicitly: {unclassified:?}"
        );
    }

    #[test]
    fn provider_turn_breadcrumb_snapshots_use_human_labels() {
        let context = ProviderBreadcrumbContext {
            phase: Some(ProviderBreadcrumbPhase {
                index: 2,
                total: 5,
                id: "todo-logic-storage".to_string(),
            }),
            repair_target: Some("browser readiness".to_string()),
        };

        assert_eq!(
            render_provider_turn_started("planner_step", "qwen3.6:27b-coding-nvfp4", 600, &context,),
            "→ planning steps for phase 2/5 todo-logic-storage (qwen3.6, up to 600s)"
        );
        assert_eq!(
            render_provider_turn_started("executor", "qwen3.6:27b-coding-nvfp4", 600, &context),
            "→ implementing (model turn: qwen3.6, up to 600s)"
        );
        assert_eq!(
            render_provider_turn_started("repair", "qwen3.6:27b-coding-nvfp4", 600, &context),
            "→ repairing: browser readiness (qwen3.6, up to 600s)"
        );
        assert_eq!(
            render_provider_turn_progress(120, 600),
            "… still waiting (120s/600s)"
        );
        assert_eq!(
            render_provider_turn_completed("planner_step", 98),
            "✓ step plan ready (98s)"
        );
    }

    #[test]
    fn current_plan_projection_marks_mid_run_and_post_run() {
        reset_for_test();
        let plan = UltraPlan {
            goal: "Build app".to_string(),
            profile: "generic".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "setup".to_string(),
                    prompt: "setup".to_string(),
                },
                UltraPhase {
                    id: "implement".to_string(),
                    prompt: "implement".to_string(),
                },
            ],
        };
        set_plan_for_test(
            &plan,
            PlanProgress {
                completed_phases: vec!["setup".to_string()],
                current_phase: Some("implement".to_string()),
                current_step: Some("write-page".to_string()),
            },
            Some("→ Write src/app/page.tsx".to_string()),
        );
        update_step_plan(
            "implement",
            &StepPlan {
                goal: "implement".to_string(),
                steps: vec![PlanStep {
                    id: "write-page".to_string(),
                    kind: "implement".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Write page".to_string(),
                    expected_paths: vec![],
                    verify: vec![],
                }],
            },
        );

        let mid = render_current_plan();
        assert!(mid.contains("1. ✓ setup"), "{mid}");
        assert!(mid.contains("2. ▶ implement"), "{mid}");
        assert!(mid.contains("1. ▶ [implement] Write page"), "{mid}");
        assert!(
            mid.contains("Current activity: → Write src/app/page.tsx"),
            "{mid}"
        );

        project_event(&json!({"event": "ultra_phase_complete", "phase_id": "implement"}));
        let post = render_current_plan();
        assert!(post.contains("2. ✓ implement"), "{post}");
    }

    #[test]
    fn status_card_snapshot_includes_sources_port_and_probe() {
        let mut sources = ConfigFieldSources::default();
        sources.model = "flag".to_string();
        sources.provider = "preset:local".to_string();
        sources.planner_model = "preset:local".to_string();
        sources.planner_provider = "preset:local".to_string();
        sources.chat_timeout_secs = "preset:local".to_string();
        sources.context_budget = "flag".to_string();
        sources.profile = "preset:local".to_string();
        sources.narration = "preset:local".to_string();
        sources.footer = "preset:local".to_string();
        let config = Config {
            workspace_root: std::path::PathBuf::from("."),
            state_dir: std::path::PathBuf::from("state"),
            eval_events_path: None,
            completion_contract_path: None,
            yes: true,
            offline: false,
            context_budget: 999,
            model: "flag-model".to_string(),
            provider: Provider::Ollama,
            planner_model: "planner".to_string(),
            planner_provider: Provider::Gemini,
            ollama_host: "http://localhost:11434".to_string(),
            num_predict: 100,
            max_iterations: 4,
            chat_timeout_secs: 77,
            chat_timeout_source: "preset:local".to_string(),
            field_sources: sources,
            chat_retries: 1,
            resume: None,
            fresh_session: false,
            no_footer: true,
            narration: crate::config::NarrationMode::Quiet,
            profile: "nextjs".to_string(),
            profile_explicit: true,
            profile_inference: None,
            style: "default".to_string(),
            action: Action::UltraPlanRun("4000番ポートで Web アプリ".to_string()),
        };

        let card = render_status_card(&config);

        assert!(card.contains("### Status"), "{card}");
        assert!(card.contains("- Model: flag-model (flag)"), "{card}");
        assert!(card.contains("- Provider: ollama (preset:local)"), "{card}");
        assert!(
            card.contains("- Planner provider: gemini (preset:local)"),
            "{card}"
        );
        assert!(card.contains("- Timeout: 77s (preset:local)"), "{card}");
        assert!(card.contains("- Context budget: 999 (flag)"), "{card}");
        assert!(card.contains("- Profile: nextjs (preset:local)"), "{card}");
        assert!(card.contains("- Footer: off (preset:local)"), "{card}");
        assert!(card.contains("- Inference: explicit"), "{card}");
        assert!(card.contains("- Port: 4000 (goal)"), "{card}");
        assert!(
            card.contains("unavailable:playwright_not_installed"),
            "{card}"
        );
        assert!(card.contains("/setup-interaction-probe"), "{card}");
        assert!(card.contains("- Narration: quiet (preset:local)"), "{card}");
    }
}
