use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

const SNIPPET_LIMIT: usize = 500;
const SUMMARY_LIMIT: usize = 8_000;
pub const GENERIC_REDUCED_ASSURANCE_REASON: &str =
    "generic profile — no capability contract, no behavioral verification";
pub const GENERIC_STATIC_ASSURANCE_REASON: &str = "generic profile — minimal interactive contract verified statically; no behavioral verification";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StopReasonParts {
    pub free_text: String,
    pub paths: Vec<String>,
    pub commands: Vec<String>,
}

impl StopReasonParts {
    pub fn free_text(value: impl Into<String>) -> Self {
        Self {
            free_text: value.into(),
            ..Self::default()
        }
    }
}

pub fn render_stop_reason(parts: &StopReasonParts) -> String {
    let mut lines = Vec::new();
    let free_text = body_snippet_whole_tokens(parts.free_text.trim());
    if !free_text.is_empty() {
        lines.push(free_text);
    }
    append_stop_reason_section(&mut lines, "Paths", &parts.paths);
    append_stop_reason_section(&mut lines, "Commands", &parts.commands);
    if lines.is_empty() {
        "unknown".to_string()
    } else {
        lines.join("\n")
    }
}

pub fn render_stop_reason_text(value: &str) -> String {
    render_stop_reason(&parse_stop_reason_parts(value))
}

fn parse_stop_reason_parts(value: &str) -> StopReasonParts {
    let mut parts = StopReasonParts::default();
    let mut free_lines = Vec::new();
    let mut section = "";
    for line in value.lines() {
        let trimmed = line.trim();
        match trimmed {
            "Paths:" => {
                section = "paths";
                continue;
            }
            "Commands:" => {
                section = "commands";
                continue;
            }
            _ => {}
        }
        match section {
            "paths" => {
                if let Some(item) = trimmed.strip_prefix("- ") {
                    parts.paths.push(item.to_string());
                } else if !trimmed.is_empty() {
                    parts.paths.push(trimmed.to_string());
                }
            }
            "commands" => {
                if let Some(item) = trimmed.strip_prefix("- ") {
                    parts.commands.push(item.to_string());
                } else if !trimmed.is_empty() {
                    parts.commands.push(trimmed.to_string());
                }
            }
            _ => free_lines.push(line.to_string()),
        }
    }
    parts.free_text = free_lines.join("\n");
    parts
}

fn append_stop_reason_section(lines: &mut Vec<String>, label: &str, values: &[String]) {
    let values = values
        .iter()
        .map(|value| redact_stop_reason_detail(value))
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>();
    if values.is_empty() {
        return;
    }
    lines.push(format!("{label}:"));
    lines.extend(values.into_iter().map(|value| format!("- {value}")));
}

fn redact_stop_reason_detail(value: &str) -> String {
    redact_home_paths(&redact_secret_like(value)).replace(['\n', '\r'], " ")
}

pub fn path_from_env() -> Option<PathBuf> {
    std::env::var_os("ANVIL_EVAL_EVENTS")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub fn path_from_env_or_default(root: &Path) -> Option<PathBuf> {
    if let Some(path) = path_from_env() {
        return Some(path);
    }
    Some(default_run_events_path(root))
}

pub fn default_run_events_path(root: &Path) -> PathBuf {
    root.join(".anvil")
        .join("runs")
        .join(uuid::Uuid::now_v7().to_string())
        .join("events.jsonl")
}

pub fn is_eval_events_override() -> bool {
    path_from_env().is_some()
}

pub fn emit(path: Option<&Path>, mut event: Value) {
    crate::tui::status_bus::publish_eval_projection(&event);
    crate::tui::presentation::project_event(&event);
    let Some(path) = path else {
        return;
    };
    if let Value::Object(ref mut object) = event {
        object
            .entry("schema_version")
            .or_insert_with(|| Value::String("1".to_string()));
    }
    if let Err(err) = append(path, &event) {
        eprintln!("warning: failed to write ANVIL_EVAL_EVENTS: {err}");
    }
}

fn append(path: &Path, event: &Value) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", serde_json::to_string(event)?)?;
    Ok(())
}

pub fn write_run_summary(path: Option<&Path>, text: &str) {
    let Some(path) = path else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };
    let summary = parent.join("summary.md");
    let content = summary_document(text);
    if let Err(err) = std::fs::create_dir_all(parent) {
        eprintln!("warning: failed to create run summary directory: {err}");
        return;
    }
    if let Err(err) = std::fs::write(summary, format!("{content}\n")) {
        eprintln!("warning: failed to write run summary: {err}");
    }
}

pub fn append_run_summary(path: Option<&Path>, text: &str) {
    let Some(path) = path else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };
    let summary = parent.join("summary.md");
    let content = summary_body(text);
    if let Err(err) = std::fs::create_dir_all(parent) {
        eprintln!("warning: failed to create run summary directory: {err}");
        return;
    }
    let existing = std::fs::read_to_string(&summary).unwrap_or_default();
    let combined = if existing.trim().is_empty() {
        format!("{}\n", summary_document(text))
    } else {
        format!("{}\n---\n\n{content}\n", existing.trim_end())
    };
    if let Err(err) = std::fs::write(summary, combined) {
        eprintln!("warning: failed to append run summary: {err}");
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionSnapshot {
    pub profile: String,
    pub effective_profile: String,
    pub contract_origin: String,
    pub assurance_level: String,
    pub assurance_reason: String,
    pub profile_inferred: String,
    pub profile_inference_source: String,
    pub requested_port: String,
    pub runtime_acceptance_status: String,
    pub final_acceptance_status: String,
    pub release_gate_status: String,
    pub completion_contract_verification_enabled: bool,
    pub completion_contract_path_merge_enabled: bool,
    pub completion_contract_path: String,
    pub completion_contract_generated: bool,
    pub external_contract_checked: bool,
    pub external_contract_ok: bool,
    pub release_gate_reasons: Vec<String>,
    pub unverified_evidence: Vec<String>,
    pub browser_readiness_applicable: bool,
    pub browser_readiness_execution_status: String,
    pub browser_readiness_status: String,
    pub browser_readiness_evidence_path: String,
    pub interaction_evidence_applicable: bool,
    pub interaction_evidence_execution_status: String,
    pub interaction_evidence_status: String,
    pub interaction_evidence_path: String,
    pub state_dimensions_changed: Vec<String>,
    pub action_hooks: Vec<String>,
    pub surface_fit_summary: String,
    pub surface_fit_guidance: String,
    pub text_entry_target: String,
    pub typed_token: String,
    pub token_echoed: String,
    pub text_input_state_change: String,
    pub persistence_after_reload: String,
    pub persistence_after_reload_reason: String,
    pub evidence_arbitration_summary: String,
    pub recovery_prompt_path: String,
    pub recovery_ultra_plan_path: String,
    pub suggested_recovery_command: String,
    pub suggested_recovery_yaml_command: String,
    pub plan_adherence_present: Vec<String>,
    pub plan_adherence_missing: Vec<String>,
    pub planner_verify_normalization_count: usize,
    pub planner_retry_count: usize,
    pub planner_quality_warning_count: usize,
    pub planner_quality_issue_count: usize,
    pub planner_repaired: bool,
    pub planner_release_risk: bool,
    pub display_normalization_count: usize,
    pub display_salvaged_count: usize,
    pub display_substituted_count: usize,
    pub context_truncation_warning_count: usize,
    pub compile_rollback_summaries: Vec<String>,
}

impl CompletionSnapshot {
    pub fn empty() -> Self {
        Self {
            profile: String::new(),
            effective_profile: String::new(),
            contract_origin: "initial".to_string(),
            assurance_level: String::new(),
            assurance_reason: String::new(),
            profile_inferred: String::new(),
            profile_inference_source: String::new(),
            requested_port: String::new(),
            runtime_acceptance_status: "not_checked".to_string(),
            final_acceptance_status: "not_checked".to_string(),
            release_gate_status: "not_applicable".to_string(),
            completion_contract_verification_enabled: false,
            completion_contract_path_merge_enabled: false,
            completion_contract_path: String::new(),
            completion_contract_generated: false,
            external_contract_checked: false,
            external_contract_ok: false,
            release_gate_reasons: Vec::new(),
            unverified_evidence: Vec::new(),
            browser_readiness_applicable: false,
            browser_readiness_execution_status: "not_applicable".to_string(),
            browser_readiness_status: "not_applicable".to_string(),
            browser_readiness_evidence_path: String::new(),
            interaction_evidence_applicable: false,
            interaction_evidence_execution_status: "not_applicable".to_string(),
            interaction_evidence_status: "not_applicable".to_string(),
            interaction_evidence_path: String::new(),
            state_dimensions_changed: Vec::new(),
            action_hooks: Vec::new(),
            surface_fit_summary: String::new(),
            surface_fit_guidance: String::new(),
            text_entry_target: String::new(),
            typed_token: String::new(),
            token_echoed: String::new(),
            text_input_state_change: String::new(),
            persistence_after_reload: "not_applicable".to_string(),
            persistence_after_reload_reason: String::new(),
            evidence_arbitration_summary: String::new(),
            recovery_prompt_path: String::new(),
            recovery_ultra_plan_path: String::new(),
            suggested_recovery_command: String::new(),
            suggested_recovery_yaml_command: String::new(),
            plan_adherence_present: Vec::new(),
            plan_adherence_missing: Vec::new(),
            planner_verify_normalization_count: 0,
            planner_retry_count: 0,
            planner_quality_warning_count: 0,
            planner_quality_issue_count: 0,
            planner_repaired: false,
            planner_release_risk: false,
            display_normalization_count: 0,
            display_salvaged_count: 0,
            display_substituted_count: 0,
            context_truncation_warning_count: 0,
            compile_rollback_summaries: Vec::new(),
        }
    }

    pub fn has_release_signal(&self) -> bool {
        self.final_acceptance_status != "not_checked"
            || !matches!(
                self.release_gate_status.as_str(),
                "" | "not_applicable" | "not_checked"
            )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionProjection {
    pub status: String,
    pub command_completion: String,
    pub task_status: String,
    pub profile: String,
    pub effective_profile: String,
    pub contract_origin: String,
    pub assurance_level: String,
    pub assurance_reason: String,
    pub profile_inferred: String,
    pub profile_inference_source: String,
    pub requested_port: String,
    pub runtime_acceptance: String,
    pub final_acceptance: String,
    pub release_gate: String,
    pub completion_contract_verification_enabled: bool,
    pub completion_contract_path_merge_enabled: bool,
    pub completion_contract_path: String,
    pub completion_contract_generated: bool,
    pub external_contract_checked: bool,
    pub external_contract_ok: bool,
    pub release_gate_reasons: Vec<String>,
    pub unverified_evidence: Vec<String>,
    pub browser_readiness_applicable: bool,
    pub browser_readiness_execution_status: String,
    pub browser_readiness: String,
    pub browser_readiness_evidence_path: String,
    pub interaction_evidence_applicable: bool,
    pub interaction_evidence_execution_status: String,
    pub interaction_evidence: String,
    pub interaction_evidence_path: String,
    pub state_dimensions_changed: Vec<String>,
    pub action_hooks: Vec<String>,
    pub surface_fit_summary: String,
    pub surface_fit_guidance: String,
    pub text_entry_target: String,
    pub typed_token: String,
    pub token_echoed: String,
    pub text_input_state_change: String,
    pub persistence_after_reload: String,
    pub persistence_after_reload_reason: String,
    pub evidence_arbitration_summary: String,
    pub release_quality_completion: String,
    pub next_action: String,
    pub recovery_prompt_path: String,
    pub recovery_ultra_plan_path: String,
    pub suggested_recovery_command: String,
    pub suggested_recovery_yaml_command: String,
    pub plan_adherence_present: Vec<String>,
    pub plan_adherence_missing: Vec<String>,
    pub planner_verify_normalization_count: usize,
    pub planner_retry_count: usize,
    pub planner_quality_warning_count: usize,
    pub planner_quality_issue_count: usize,
    pub planner_repaired: bool,
    pub planner_release_risk: bool,
    pub display_normalization_count: usize,
    pub display_salvaged_count: usize,
    pub display_substituted_count: usize,
    pub context_truncation_warning_count: usize,
    pub compile_rollback_summaries: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct PlannerDiagnostics {
    verify_normalization_count: usize,
    display_normalization_count: usize,
    display_salvaged_count: usize,
    display_substituted_count: usize,
    context_truncation_warning_count: usize,
    retry_count: usize,
    quality_warning_count: usize,
    quality_issue_count: usize,
}

impl PlannerDiagnostics {
    fn repaired(self) -> bool {
        self.verify_normalization_count > 0 || self.retry_count > 0
    }

    fn release_risk(self) -> bool {
        self.repaired() || self.quality_warning_count > 0 || self.quality_issue_count > 0
    }
}

pub fn latest_completion_snapshot(path: Option<&Path>) -> CompletionSnapshot {
    let Some(path) = path else {
        return CompletionSnapshot::empty();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return CompletionSnapshot::empty();
    };
    let events = text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect::<Vec<_>>();
    let diagnostics = planner_diagnostics_from_events(&events);
    let recovery_fields = latest_recovery_fields(&events);
    let persistence_fields = latest_persistence_fields(&events);
    let mut snapshot = CompletionSnapshot::empty();
    let mut latest_completion_index = None;
    for (index, event) in events.iter().enumerate() {
        if let Some(next) = snapshot_from_completion_event(event) {
            snapshot = next;
            latest_completion_index = Some(index);
        }
    }
    if let Some(profile) = latest_lifecycle_profile_fields(&events) {
        profile.apply_to(&mut snapshot);
    }
    if let Some(reinference) = latest_profile_reinference_after(&events, latest_completion_index) {
        reinference.apply_to(&mut snapshot);
    }
    if snapshot.requested_port.is_empty()
        && let Some(requested_port) = latest_requested_port(&events)
    {
        snapshot.requested_port = requested_port;
    }
    snapshot.planner_verify_normalization_count = diagnostics.verify_normalization_count;
    snapshot.planner_retry_count = diagnostics.retry_count;
    snapshot.planner_quality_warning_count = diagnostics.quality_warning_count;
    snapshot.planner_quality_issue_count = diagnostics.quality_issue_count;
    snapshot.planner_repaired = diagnostics.repaired();
    snapshot.planner_release_risk = diagnostics.release_risk();
    snapshot.display_normalization_count = diagnostics.display_normalization_count;
    snapshot.display_salvaged_count = diagnostics.display_salvaged_count;
    snapshot.display_substituted_count = diagnostics.display_substituted_count;
    snapshot.context_truncation_warning_count = diagnostics.context_truncation_warning_count;
    snapshot.compile_rollback_summaries = compile_rollback_summaries_from_events(&events);
    recovery_fields.apply_to(&mut snapshot);
    persistence_fields.apply_to(&mut snapshot);
    snapshot
}

pub fn project_completion(ok: bool, snapshot: &CompletionSnapshot) -> CompletionProjection {
    let command_completion = if ok { "completed" } else { "failed" }.to_string();
    let release_gate = if snapshot.release_gate_status.is_empty() {
        "not_applicable".to_string()
    } else {
        snapshot.release_gate_status.clone()
    };
    let final_acceptance = if snapshot.final_acceptance_status.is_empty() {
        "not_checked".to_string()
    } else {
        snapshot.final_acceptance_status.clone()
    };
    let runtime_acceptance = if snapshot.runtime_acceptance_status.is_empty() {
        "not_checked".to_string()
    } else {
        snapshot.runtime_acceptance_status.clone()
    };
    let release_quality_completion = release_quality_completion(&release_gate, &final_acceptance);
    let status = terminal_status(ok, &release_gate, &final_acceptance);
    let interaction_unverified =
        interaction_unverified_probe_unavailable(&release_gate, &snapshot.release_gate_reasons);
    let (assurance_level, assurance_reason) =
        projected_assurance_from_snapshot(snapshot, &release_gate, &final_acceptance);
    let base_task_status = task_status(ok, &release_gate, &final_acceptance);
    let task_status = if ok && interaction_unverified {
        "partial (interaction unverified)".to_string()
    } else if ok && assurance_level == "static" && base_task_status == "complete" {
        "completed (static assurance)".to_string()
    } else if ok && assurance_level == "reduced" && base_task_status == "complete" {
        "completed (reduced assurance)".to_string()
    } else {
        base_task_status
    };
    let next_action = if ok && interaction_unverified {
        "run_setup_interaction_probe_to_enable_interaction_release_checks".to_string()
    } else {
        next_action(ok, &release_gate, &final_acceptance)
    };
    CompletionProjection {
        status,
        command_completion,
        task_status,
        profile: snapshot.profile.clone(),
        effective_profile: snapshot_effective_profile(snapshot),
        contract_origin: snapshot.contract_origin.clone(),
        assurance_level,
        assurance_reason,
        profile_inferred: snapshot.profile_inferred.clone(),
        profile_inference_source: snapshot.profile_inference_source.clone(),
        requested_port: snapshot.requested_port.clone(),
        runtime_acceptance,
        final_acceptance,
        release_gate,
        completion_contract_verification_enabled: snapshot.completion_contract_verification_enabled,
        completion_contract_path_merge_enabled: snapshot.completion_contract_path_merge_enabled,
        completion_contract_path: snapshot.completion_contract_path.clone(),
        completion_contract_generated: snapshot.completion_contract_generated,
        external_contract_checked: snapshot.external_contract_checked,
        external_contract_ok: snapshot.external_contract_ok,
        release_gate_reasons: snapshot.release_gate_reasons.clone(),
        unverified_evidence: snapshot.unverified_evidence.clone(),
        browser_readiness_applicable: snapshot.browser_readiness_applicable,
        browser_readiness_execution_status: snapshot.browser_readiness_execution_status.clone(),
        browser_readiness: snapshot.browser_readiness_status.clone(),
        browser_readiness_evidence_path: snapshot.browser_readiness_evidence_path.clone(),
        interaction_evidence_applicable: snapshot.interaction_evidence_applicable,
        interaction_evidence_execution_status: snapshot
            .interaction_evidence_execution_status
            .clone(),
        interaction_evidence: snapshot.interaction_evidence_status.clone(),
        interaction_evidence_path: snapshot.interaction_evidence_path.clone(),
        state_dimensions_changed: snapshot.state_dimensions_changed.clone(),
        action_hooks: snapshot.action_hooks.clone(),
        surface_fit_summary: snapshot.surface_fit_summary.clone(),
        surface_fit_guidance: snapshot.surface_fit_guidance.clone(),
        text_entry_target: snapshot.text_entry_target.clone(),
        typed_token: snapshot.typed_token.clone(),
        token_echoed: snapshot.token_echoed.clone(),
        text_input_state_change: snapshot.text_input_state_change.clone(),
        persistence_after_reload: snapshot.persistence_after_reload.clone(),
        persistence_after_reload_reason: snapshot.persistence_after_reload_reason.clone(),
        evidence_arbitration_summary: snapshot.evidence_arbitration_summary.clone(),
        release_quality_completion,
        next_action,
        recovery_prompt_path: snapshot.recovery_prompt_path.clone(),
        recovery_ultra_plan_path: snapshot.recovery_ultra_plan_path.clone(),
        suggested_recovery_command: snapshot.suggested_recovery_command.clone(),
        suggested_recovery_yaml_command: snapshot.suggested_recovery_yaml_command.clone(),
        plan_adherence_present: snapshot.plan_adherence_present.clone(),
        plan_adherence_missing: snapshot.plan_adherence_missing.clone(),
        planner_verify_normalization_count: snapshot.planner_verify_normalization_count,
        planner_retry_count: snapshot.planner_retry_count,
        planner_quality_warning_count: snapshot.planner_quality_warning_count,
        planner_quality_issue_count: snapshot.planner_quality_issue_count,
        planner_repaired: snapshot.planner_repaired,
        planner_release_risk: snapshot.planner_release_risk,
        display_normalization_count: snapshot.display_normalization_count,
        display_salvaged_count: snapshot.display_salvaged_count,
        display_substituted_count: snapshot.display_substituted_count,
        context_truncation_warning_count: snapshot.context_truncation_warning_count,
        compile_rollback_summaries: snapshot.compile_rollback_summaries.clone(),
    }
}

fn snapshot_effective_profile(snapshot: &CompletionSnapshot) -> String {
    if snapshot.effective_profile.trim().is_empty() {
        snapshot.profile.clone()
    } else {
        snapshot.effective_profile.clone()
    }
}

fn projected_assurance_from_snapshot(
    snapshot: &CompletionSnapshot,
    release_gate: &str,
    final_acceptance: &str,
) -> (String, String) {
    let mut level = snapshot.assurance_level.clone();
    let mut reason = snapshot.assurance_reason.clone();
    if level != "full" {
        return (level, reason);
    }
    let effective_profile = snapshot_effective_profile(snapshot);
    if effective_profile.trim().is_empty() {
        return (
            "partial".to_string(),
            "effective_profile_unknown".to_string(),
        );
    }
    if final_acceptance == "partial" || release_gate == "partial" {
        return (
            "partial".to_string(),
            snapshot
                .release_gate_reasons
                .first()
                .cloned()
                .unwrap_or_else(|| "acceptance_partial".to_string()),
        );
    }
    if final_acceptance != "full_success" || release_gate == "failed" {
        level = "partial".to_string();
        reason = snapshot
            .release_gate_reasons
            .first()
            .cloned()
            .unwrap_or_else(|| "acceptance_not_full_success".to_string());
        return (level, reason);
    }
    if !snapshot.completion_contract_verification_enabled && !snapshot.external_contract_checked {
        return (
            "partial".to_string(),
            "completion_contract_not_bound".to_string(),
        );
    }
    if snapshot.browser_readiness_applicable
        && snapshot.browser_readiness_execution_status != "performed"
    {
        return (
            "partial".to_string(),
            format!(
                "browser_readiness_not_performed:{}",
                snapshot.browser_readiness_execution_status
            ),
        );
    }
    if snapshot.interaction_evidence_applicable
        && snapshot.interaction_evidence_execution_status != "performed"
    {
        return (
            "partial".to_string(),
            format!(
                "interaction_evidence_not_performed:{}",
                snapshot.interaction_evidence_execution_status
            ),
        );
    }
    (level, reason)
}

fn compile_rollback_summaries_from_events(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .filter(|event| {
            event.get("event").and_then(Value::as_str) == Some("compile_rollback_applied")
        })
        .filter_map(|event| {
            let paths = event_string_array(event, "paths");
            if paths.is_empty() {
                return None;
            }
            let origins = event_string_array(event, "snapshot_origins");
            let carry = event_string_array(event, "carry_forward_guidance");
            let mut summary = format!("paths: {}", paths.join(", "));
            if !origins.is_empty() {
                summary.push_str(&format!("; snapshot origin: {}", origins.join(", ")));
            }
            if !carry.is_empty() {
                summary.push_str(&format!("; carry-forward: {}", carry.join("; ")));
            }
            Some(summary)
        })
        .collect()
}

fn interaction_unverified_probe_unavailable(release_gate: &str, reasons: &[String]) -> bool {
    release_gate == "partial"
        && reasons
            .iter()
            .any(|reason| reason.contains("interaction_unverified:probe_unavailable"))
}

fn planner_diagnostics_from_events(events: &[Value]) -> PlannerDiagnostics {
    let mut diagnostics = PlannerDiagnostics::default();
    for event in events {
        match event.get("event").and_then(Value::as_str).unwrap_or("") {
            "planner_verify_command_normalized" => {
                diagnostics.verify_normalization_count += 1;
                diagnostics.display_normalization_count += 1;
            }
            "tool_args_path_normalized"
            | "verify_command_normalized_at_runtime"
            | "side_effect_path_dropped"
            | "ultra_plan_generation_metadata_normalized" => {
                diagnostics.display_normalization_count += 1;
            }
            "tool_args_path_salvaged" => {
                diagnostics.display_salvaged_count += 1;
            }
            "verify_command_substituted" => {
                diagnostics.display_substituted_count += 1;
            }
            "context_truncation_suspected" => {
                diagnostics.context_truncation_warning_count += 1;
            }
            "planner_quality_retry"
            | "planner_quality_retry_degraded"
            | "planner_quality_retry_exhausted"
            | "ultra_plan_generation_retry" => {
                diagnostics.retry_count += 1;
            }
            "planner_error" if event.get("planner_error_kind").is_some() => {
                diagnostics.retry_count += 1;
            }
            "planner_quality_warning" => {
                diagnostics.quality_warning_count += 1;
            }
            "planner_quality_issue" => {
                diagnostics.quality_issue_count += 1;
            }
            _ => {}
        }
    }
    diagnostics
}

#[derive(Debug, Clone, Default)]
struct RecoveryFields {
    recovery_prompt_path: String,
    recovery_ultra_plan_path: String,
    suggested_recovery_command: String,
    suggested_recovery_yaml_command: String,
}

impl RecoveryFields {
    fn apply_to(&self, snapshot: &mut CompletionSnapshot) {
        if !self.recovery_prompt_path.is_empty() {
            snapshot.recovery_prompt_path = self.recovery_prompt_path.clone();
        }
        if !self.recovery_ultra_plan_path.is_empty() {
            snapshot.recovery_ultra_plan_path = self.recovery_ultra_plan_path.clone();
        }
        if !self.suggested_recovery_command.is_empty() {
            snapshot.suggested_recovery_command = self.suggested_recovery_command.clone();
        }
        if !self.suggested_recovery_yaml_command.is_empty() {
            snapshot.suggested_recovery_yaml_command = self.suggested_recovery_yaml_command.clone();
        }
    }
}

#[derive(Debug, Clone, Default)]
struct PersistenceFields {
    persistence_after_reload: String,
    persistence_after_reload_reason: String,
}

impl PersistenceFields {
    fn apply_to(&self, snapshot: &mut CompletionSnapshot) {
        if !self.persistence_after_reload.is_empty() {
            snapshot.persistence_after_reload = self.persistence_after_reload.clone();
        }
        if !self.persistence_after_reload_reason.is_empty() {
            snapshot.persistence_after_reload_reason = self.persistence_after_reload_reason.clone();
        }
    }
}

fn latest_persistence_fields(events: &[Value]) -> PersistenceFields {
    let mut fields = PersistenceFields::default();
    for event in events.iter().rev() {
        if fields.persistence_after_reload.is_empty()
            && let Some(value) = non_empty_event_field(event, "persistence_after_reload")
        {
            fields.persistence_after_reload = value.to_string();
        }
        if fields.persistence_after_reload_reason.is_empty()
            && let Some(value) = non_empty_event_field(event, "persistence_after_reload_reason")
        {
            fields.persistence_after_reload_reason = value.to_string();
        }
        if !fields.persistence_after_reload.is_empty()
            && !fields.persistence_after_reload_reason.is_empty()
        {
            break;
        }
    }
    fields
}

fn latest_recovery_fields(events: &[Value]) -> RecoveryFields {
    let mut fields = RecoveryFields::default();
    for event in events.iter().rev() {
        if fields.recovery_prompt_path.is_empty()
            && let Some(value) = non_empty_event_field(event, "recovery_prompt_path")
        {
            fields.recovery_prompt_path = handoff_display_value(value);
        }
        if fields.recovery_ultra_plan_path.is_empty()
            && let Some(value) = non_empty_event_field(event, "recovery_ultra_plan_path")
        {
            fields.recovery_ultra_plan_path = handoff_display_value(value);
        }
        if fields.suggested_recovery_command.is_empty()
            && let Some(value) = non_empty_event_field(event, "suggested_recovery_command")
        {
            fields.suggested_recovery_command = normalize_handoff_display_text(value.to_string());
        }
        if fields.suggested_recovery_yaml_command.is_empty()
            && let Some(value) = non_empty_event_field(event, "suggested_recovery_yaml_command")
        {
            fields.suggested_recovery_yaml_command =
                normalize_handoff_display_text(value.to_string());
        }
        if !fields.recovery_prompt_path.is_empty()
            && !fields.recovery_ultra_plan_path.is_empty()
            && !fields.suggested_recovery_command.is_empty()
            && !fields.suggested_recovery_yaml_command.is_empty()
        {
            break;
        }
    }
    fields
}

fn non_empty_event_field<'a>(event: &'a Value, key: &str) -> Option<&'a str> {
    event
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub fn append_completion_summary(
    path: Option<&Path>,
    lifecycle_stage: &str,
    action: Option<&str>,
    command: Option<&str>,
    stop_reason: &str,
    failure_kind: &str,
    projection: &CompletionProjection,
) {
    append_run_summary(
        path,
        &render_completion_summary(
            lifecycle_stage,
            action,
            command,
            stop_reason,
            failure_kind,
            projection,
        ),
    );
}

#[allow(clippy::too_many_arguments)]
pub fn write_tui_command_completion_summary(
    path: Option<&Path>,
    command: &str,
    stop_reason: &str,
    failure_kind: &str,
    terminal_status: &str,
    projection: &CompletionProjection,
) {
    let events = read_event_values(path);
    let previous_summary = read_run_summary(path).unwrap_or_default();
    write_run_summary(
        path,
        &render_tui_command_completion_summary(
            command,
            stop_reason,
            failure_kind,
            terminal_status,
            projection,
            &events,
            &previous_summary,
        ),
    );
}

pub fn render_tui_completion_output(output: &str, projection: &CompletionProjection) -> String {
    if projection.release_gate == "not_applicable"
        && projection.final_acceptance == "not_checked"
        && projection.runtime_acceptance == "not_checked"
    {
        return output.to_string();
    }
    let mut output = format!(
        "{}\n\nCommand status: {}\nCommand completion: {}\nTask status: {}\nRuntime acceptance: {}\nFinal acceptance: {}\nRelease gate: {}\ncompletion_contract_verification_enabled={}\nexternal_contract_checked={}\nPlanner diagnostics: normalizations={} retries={} quality_warnings={} quality_issues={}\nPlanner release risk: {}\nNext action: {}",
        output,
        projection.command_completion,
        projection.command_completion,
        projection.task_status,
        projection.runtime_acceptance,
        projection.final_acceptance,
        projection.release_gate,
        projection.completion_contract_verification_enabled,
        projection.external_contract_checked,
        projection.planner_verify_normalization_count,
        projection.planner_retry_count,
        projection.planner_quality_warning_count,
        projection.planner_quality_issue_count,
        projection.planner_release_risk,
        projection.next_action
    );
    if !projection.assurance_level.is_empty() {
        output.push_str("\nAssurance: ");
        output.push_str(&projection.assurance_level);
        if !projection.assurance_reason.is_empty() {
            output.push_str(" (");
            output.push_str(&projection.assurance_reason);
            output.push(')');
        }
    }
    if !projection.recovery_ultra_plan_path.is_empty()
        || !projection.suggested_recovery_yaml_command.is_empty()
    {
        output.push_str("\nRecovery UltraPlan: ");
        output.push_str(missing_if_empty(&projection.recovery_ultra_plan_path));
        if !projection.suggested_recovery_yaml_command.is_empty() {
            output.push_str("\nSuggested recovery command: ");
            output.push_str(&projection.suggested_recovery_yaml_command);
        }
    } else if !projection.recovery_prompt_path.is_empty()
        || !projection.suggested_recovery_command.is_empty()
    {
        output.push_str("\nRecovery prompt: ");
        output.push_str(missing_if_empty(&projection.recovery_prompt_path));
        if !projection.suggested_recovery_command.is_empty() {
            output.push_str("\nSuggested recovery command: ");
            output.push_str(&projection.suggested_recovery_command);
        }
    }
    output
}

pub fn render_terminal_summary_card(
    path: Option<&Path>,
    primary_stop_reason: &str,
    projection: &CompletionProjection,
) -> String {
    let events = read_event_values(path);
    let summary_text = read_run_summary(path).unwrap_or_default();
    let (completed, total) = phase_counts_from_events(&events)
        .or_else(|| phase_counts_from_summary(&summary_text))
        .unwrap_or((None, None));
    let mut lines = vec![
        "### Terminal summary".to_string(),
        format!(
            "- Status: {} · Assurance: {}",
            projection.status,
            assurance_display(projection)
        ),
        "| Gate | Recorded value |".to_string(),
        "| --- | --- |".to_string(),
        format!("| task_status | {} |", projection.task_status),
        format!("| release_gate_status | {} |", projection.release_gate),
        format!(
            "| final_acceptance_status | {} |",
            projection.final_acceptance
        ),
        format!(
            "| browser_readiness_status | execution={} status={} |",
            projection.browser_readiness_execution_status, projection.browser_readiness
        ),
        format!(
            "| interaction_evidence_status | execution={} status={} |",
            projection.interaction_evidence_execution_status, projection.interaction_evidence
        ),
        format!(
            "| persistence_after_reload | {} |",
            persistence_display(projection)
        ),
        format!(
            "- Primary stop reason: {}",
            terminal_card_stop_reason(primary_stop_reason)
        ),
        format!(
            "- Phases completed: {}",
            phase_count_display(completed, total)
        ),
    ];
    if let Some(resumed_from) = latest_event_field(&events, &["resumed_from"]) {
        lines.push(format!("- Resumed from: {resumed_from}"));
    }
    if let Some(telemetry) = terminal_card_telemetry(projection) {
        lines.push(format!("- Telemetry: {telemetry}"));
    }
    if !projection.recovery_prompt_path.is_empty() {
        lines.push(format!(
            "- Recovery prompt: {}",
            projection.recovery_prompt_path
        ));
    }
    if !projection.recovery_ultra_plan_path.is_empty() {
        lines.push(format!(
            "- Recovery UltraPlan: {}",
            projection.recovery_ultra_plan_path
        ));
        if let Some(command) = resume_command_for_run(path) {
            lines.push(format!("- resume: {command}"));
        }
    }
    if !projection.suggested_recovery_command.is_empty() {
        lines.push(format!(
            "- Suggested command: {}",
            projection.suggested_recovery_command
        ));
    }
    if !projection.suggested_recovery_yaml_command.is_empty() {
        lines.push(format!(
            "- Suggested YAML command: {}",
            projection.suggested_recovery_yaml_command
        ));
    }
    lines.push(format!("- Next action: {}", projection.next_action));
    lines.truncate(25);
    lines.join("\n")
}

fn resume_command_for_run(path: Option<&Path>) -> Option<String> {
    let run_dir = path.and_then(Path::parent)?;
    let is_run_dir = run_dir
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .is_some_and(|value| value == "runs");
    if !is_run_dir {
        return None;
    }
    let run_id = run_dir
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())?;
    let short = crate::util::truncate_at_char_boundary(run_id, 8);
    Some(format!("/resume {short}"))
}

fn assurance_display(projection: &CompletionProjection) -> String {
    if projection.assurance_level.is_empty() {
        return "missing".to_string();
    }
    if projection.assurance_reason.is_empty() {
        projection.assurance_level.clone()
    } else {
        format!(
            "{} ({})",
            projection.assurance_level, projection.assurance_reason
        )
    }
}

fn persistence_display(projection: &CompletionProjection) -> String {
    if projection.persistence_after_reload.is_empty() {
        return "not_applicable".to_string();
    }
    if projection.persistence_after_reload_reason.is_empty() {
        projection.persistence_after_reload.clone()
    } else {
        format!(
            "{} ({})",
            projection.persistence_after_reload, projection.persistence_after_reload_reason
        )
    }
}

fn terminal_card_stop_reason(value: &str) -> String {
    let rendered = render_stop_reason_text(value);
    let first_line = rendered.lines().next().unwrap_or("unknown");
    body_snippet_whole_tokens(first_line)
}

fn phase_count_display(completed: Option<usize>, total: Option<usize>) -> String {
    match (completed, total) {
        (Some(completed), Some(total)) => format!("{completed}/{total}"),
        (Some(completed), None) => format!("{completed}/unknown"),
        (None, Some(total)) => format!("unknown/{total}"),
        (None, None) => "unknown".to_string(),
    }
}

fn terminal_card_telemetry(projection: &CompletionProjection) -> Option<String> {
    let mut parts = Vec::new();
    if projection.display_normalization_count > 0 {
        parts.push(format!(
            "normalized={}",
            projection.display_normalization_count
        ));
    }
    if projection.display_salvaged_count > 0 {
        parts.push(format!("salvaged={}", projection.display_salvaged_count));
    }
    if projection.display_substituted_count > 0 {
        parts.push(format!(
            "substituted={}",
            projection.display_substituted_count
        ));
    }
    if !projection.compile_rollback_summaries.is_empty() {
        parts.push(format!(
            "rollbacks={}",
            projection.compile_rollback_summaries.len()
        ));
    }
    if projection.context_truncation_warning_count > 0 {
        parts.push(format!(
            "context_truncation_suspected={}",
            projection.context_truncation_warning_count
        ));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

pub fn render_tui_command_failure_block(
    path: Option<&Path>,
    stop_reason: &str,
    projection: &CompletionProjection,
) -> String {
    let events = read_event_values(path);
    let summary_text = read_run_summary(path).unwrap_or_default();
    let failed_phase = latest_event_field(
        &events,
        &["failed_phase_id", "failed_phase", "phase_id", "step_id"],
    )
    .or_else(|| failed_phase_from_summary(&summary_text))
    .unwrap_or_else(|| "unknown".to_string());
    let (completed_phases, total_phases) = phase_counts_from_events(&events)
        .or_else(|| phase_counts_from_summary(&summary_text))
        .unwrap_or((None, None));
    let primary_stop_reason = normalize_handoff_display_text(tui_primary_stop_reason(
        &events,
        &summary_text,
        stop_reason,
    ));
    let suggested_recovery_command = latest_event_field(&events, &["suggested_recovery_command"])
        .filter(|value| !value.is_empty())
        .or_else(|| non_empty(projection.suggested_recovery_command.clone()))
        .or_else(|| {
            summary_value(
                &summary_text,
                &[
                    "Suggested prompt command:",
                    "Suggested command:",
                    "Recovery prompt command:",
                    "Suggested recovery command:",
                ],
            )
        })
        .map(normalize_handoff_display_text)
        .unwrap_or_default();
    let suggested_recovery_yaml_command =
        latest_event_field(&events, &["suggested_recovery_yaml_command"])
            .filter(|value| !value.is_empty())
            .or_else(|| non_empty(projection.suggested_recovery_yaml_command.clone()))
            .or_else(|| {
                summary_value(
                    &summary_text,
                    &["Suggested YAML command:", "Recovery UltraPlan command:"],
                )
            })
            .map(normalize_handoff_display_text)
            .unwrap_or_default();
    let summary_path = run_summary_path(path)
        .map(|path| handoff_display_path(&path))
        .unwrap_or_default();
    crate::tui::status::render_task_failure_block(&crate::tui::status::TaskFailureBlock {
        task_status: projection.task_status.clone(),
        failed_phase,
        completed_phases,
        total_phases,
        primary_stop_reason,
        recovery_prompt_command: suggested_recovery_command,
        recovery_ultra_plan_command: suggested_recovery_yaml_command,
        summary_path,
    })
}

pub fn render_tui_command_incomplete_notice(
    path: Option<&Path>,
    projection: &CompletionProjection,
) -> Option<String> {
    if projection.command_completion != "completed" || !command_returned_incomplete(projection) {
        return None;
    }
    Some(crate::tui::status::render_task_incomplete_notice(
        &crate::tui::status::TaskIncompleteNotice {
            status: projection.status.clone(),
            task_status: projection.task_status.clone(),
            summary_path: run_summary_path(path)
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
        },
    ))
}

fn read_event_values(path: Option<&Path>) -> Vec<Value> {
    let Some(path) = path else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

fn read_run_summary(path: Option<&Path>) -> Option<String> {
    std::fs::read_to_string(run_summary_path(path)?).ok()
}

fn run_summary_path(path: Option<&Path>) -> Option<PathBuf> {
    path.and_then(Path::parent)
        .map(|parent| parent.join("summary.md"))
}

fn latest_event_field(events: &[Value], keys: &[&str]) -> Option<String> {
    events.iter().rev().find_map(|event| {
        keys.iter().find_map(|key| {
            event
                .get(*key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        })
    })
}

fn phase_counts_from_events(events: &[Value]) -> Option<(Option<usize>, Option<usize>)> {
    for event in events.iter().rev() {
        let completed_ids = event
            .get("completed_phase_ids")
            .and_then(Value::as_array)
            .map(Vec::len);
        let pending_ids = event
            .get("pending_phase_ids")
            .and_then(Value::as_array)
            .map(Vec::len);
        if completed_ids.is_some() || pending_ids.is_some() {
            let total = completed_ids
                .zip(pending_ids)
                .map(|(completed, pending)| completed + 1 + pending);
            return Some((completed_ids, total));
        }
        if let Some(total) = event.get("total_phases").and_then(Value::as_u64) {
            let completed = event
                .get("completed_phase_count")
                .and_then(Value::as_u64)
                .map(|value| value as usize)
                .or_else(|| {
                    event
                        .get("phase_index")
                        .and_then(Value::as_u64)
                        .map(|value| value.saturating_sub(1) as usize)
                });
            return Some((completed, Some(total as usize)));
        }
        if let Some(completed) = event.get("completed_phase_count").and_then(Value::as_u64) {
            return Some((Some(completed as usize), None));
        }
    }
    None
}

fn phase_counts_from_summary(text: &str) -> Option<(Option<usize>, Option<usize>)> {
    let completed = summary_section_count(text, "Completed phases:");
    let pending = summary_section_count(text, "Pending phases:");
    if completed.is_none() && pending.is_none() {
        return None;
    }
    let total = completed
        .zip(pending)
        .map(|(completed, pending)| completed + 1 + pending);
    Some((completed, total))
}

fn summary_section_count(text: &str, header: &str) -> Option<usize> {
    let mut in_section = false;
    let mut saw_bullet = false;
    let mut count = 0usize;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == header {
            in_section = true;
            continue;
        }
        if !in_section {
            continue;
        }
        if trimmed.is_empty() || (trimmed.ends_with(':') && !trimmed.starts_with("- ")) {
            break;
        }
        if let Some(item) = trimmed.strip_prefix("- ") {
            saw_bullet = true;
            if item.trim() != "none" {
                count += 1;
            }
        }
    }
    saw_bullet.then_some(count)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PhaseBreakdown {
    completed: Vec<String>,
    failed: Vec<String>,
    pending: Vec<String>,
}

impl PhaseBreakdown {
    fn is_empty(&self) -> bool {
        self.completed.is_empty() && self.failed.is_empty() && self.pending.is_empty()
    }
}

fn phase_breakdown_for_tui_summary(
    events: &[Value],
    previous_summary: &str,
    terminal_status: &str,
) -> PhaseBreakdown {
    let mut breakdown = phase_breakdown_from_events(events, terminal_status);
    let previous = phase_breakdown_from_summary(previous_summary);
    if breakdown.is_empty() {
        return previous;
    }
    if breakdown.completed.is_empty() {
        breakdown.completed = previous.completed;
    }
    if breakdown.failed.is_empty() {
        breakdown.failed = previous.failed;
    }
    if breakdown.pending.is_empty() {
        breakdown.pending = previous.pending;
    }
    breakdown
}

fn phase_breakdown_from_events(events: &[Value], terminal_status: &str) -> PhaseBreakdown {
    let mut completed = Vec::new();
    let mut failed = Vec::new();
    let mut explicit_completed: Option<Vec<String>> = None;
    let mut explicit_pending: Option<Vec<String>> = None;
    let mut started_by_index = BTreeMap::new();
    let mut total_phases: Option<usize> = None;
    let mut last_phase: Option<String> = None;
    let mut last_index: Option<usize> = None;

    for event in events {
        let name = event.get("event").and_then(Value::as_str).unwrap_or("");
        if let Some(total) = event.get("total_phases").and_then(Value::as_u64) {
            total_phases = Some(total as usize);
        }
        let index = event
            .get("phase_index")
            .and_then(Value::as_u64)
            .map(|value| value as usize);
        let phase = event
            .get("phase_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);

        if name.starts_with("ultra_phase_")
            && let Some(phase) = phase.clone()
        {
            last_phase = Some(phase.clone());
            if let Some(index) = index {
                last_index = Some(index);
                started_by_index.entry(index).or_insert(phase);
            }
        }

        match name {
            "ultra_phase_complete" => {
                if let Some(phase) = phase {
                    push_unique_phase(&mut completed, phase);
                }
            }
            "ultra_phase_failed" => {
                if let Some(phase) = phase {
                    push_unique_phase(&mut failed, phase);
                }
            }
            _ => {}
        }

        let completed_ids = string_array_field(event, "completed_phase_ids");
        if !completed_ids.is_empty() {
            explicit_completed = Some(completed_ids);
        }
        let pending_ids = string_array_field(event, "pending_phase_ids");
        if !pending_ids.is_empty()
            || event
                .get("pending_phase_ids")
                .and_then(Value::as_array)
                .is_some()
        {
            explicit_pending = Some(pending_ids);
        }
        for key in ["failed_phase_id", "failed_phase"] {
            if let Some(value) = event
                .get(key)
                .and_then(Value::as_str)
                .and_then(clean_phase_value)
            {
                push_unique_phase(&mut failed, value);
            }
        }
    }

    if let Some(values) = explicit_completed {
        completed = values;
    }
    let mut pending = explicit_pending.unwrap_or_default();

    if terminal_status != "completed"
        && failed.is_empty()
        && let Some(phase) = last_phase.as_ref()
        && !contains_phase(&completed, phase)
    {
        push_unique_phase(&mut failed, phase.clone());
    }

    if pending.is_empty()
        && terminal_status != "completed"
        && let (Some(total), Some(index)) = (total_phases, last_index)
    {
        for pending_index in index.saturating_add(1)..=total {
            let value = started_by_index
                .get(&pending_index)
                .cloned()
                .unwrap_or_else(|| format!("phase {pending_index}"));
            push_unique_phase(&mut pending, value);
        }
    }

    if pending.is_empty()
        && terminal_status == "completed"
        && let Some(total) = total_phases
    {
        for pending_index in 1..=total {
            let value = started_by_index
                .get(&pending_index)
                .cloned()
                .unwrap_or_else(|| format!("phase {pending_index}"));
            if !contains_phase(&completed, &value) && !contains_phase(&failed, &value) {
                push_unique_phase(&mut pending, value);
            }
        }
    }

    PhaseBreakdown {
        completed,
        failed,
        pending,
    }
}

fn phase_breakdown_from_summary(text: &str) -> PhaseBreakdown {
    let mut failed = summary_section_items(text, "Failed phases:");
    if failed.is_empty() {
        failed = summary_section_items(text, "Failed phase:");
    }
    PhaseBreakdown {
        completed: summary_section_items(text, "Completed phases:"),
        failed,
        pending: summary_section_items(text, "Pending phases:"),
    }
}

fn summary_section_items(text: &str, header: &str) -> Vec<String> {
    let mut in_section = false;
    let mut items = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == header {
            in_section = true;
            continue;
        }
        if !in_section {
            continue;
        }
        if trimmed.is_empty() || (trimmed.ends_with(':') && !trimmed.starts_with("- ")) {
            break;
        }
        if let Some(value) = phase_from_bullet(trimmed) {
            push_unique_phase(&mut items, value);
        }
    }
    items
}

fn string_array_field(event: &Value, key: &str) -> Vec<String> {
    event
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            let mut values = Vec::new();
            for item in items {
                if let Some(value) = item.as_str().and_then(clean_phase_value) {
                    push_unique_phase(&mut values, value);
                }
            }
            values
        })
        .unwrap_or_default()
}

fn push_unique_phase(values: &mut Vec<String>, value: String) {
    let Some(value) = clean_phase_value(&value) else {
        return;
    };
    if !contains_phase(values, &value) {
        values.push(value);
    }
}

fn contains_phase(values: &[String], value: &str) -> bool {
    values.iter().any(|existing| existing == value)
}

fn failed_phase_from_summary(text: &str) -> Option<String> {
    let mut next_line_is_failed_phase = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if next_line_is_failed_phase {
            if let Some(value) = phase_from_bullet(trimmed) {
                return Some(value);
            }
            if !trimmed.is_empty() {
                next_line_is_failed_phase = false;
            }
        }
        if trimmed == "Failed phase:" {
            next_line_is_failed_phase = true;
            continue;
        }
        for prefix in ["Failed phase:", "- Failed phase:", "- Last failed phase:"] {
            if let Some(value) = trimmed.strip_prefix(prefix)
                && let Some(value) = clean_phase_value(value)
            {
                return Some(value);
            }
        }
    }
    None
}

fn phase_from_bullet(line: &str) -> Option<String> {
    line.strip_prefix("- ").and_then(clean_phase_value)
}

fn clean_phase_value(value: &str) -> Option<String> {
    let value = value
        .split_once(" (")
        .map(|(phase, _)| phase)
        .unwrap_or(value)
        .trim()
        .trim_matches('`');
    (!value.is_empty() && value != "none").then(|| value.to_string())
}

fn tui_primary_stop_reason(events: &[Value], summary_text: &str, stop_reason: &str) -> String {
    let missing_evidence = latest_missing_evidence_keys(events, stop_reason);
    if !missing_evidence.is_empty() {
        return missing_evidence.join(", ");
    }
    latest_event_field(events, &["primary_reason", "stop_reason", "reason"])
        .or_else(|| summary_value(summary_text, &["Failure:", "Stop reason:"]))
        .unwrap_or_else(|| stop_reason.to_string())
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn summary_value(text: &str, prefixes: &[&str]) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim();
        let trimmed = trimmed.strip_prefix("- ").unwrap_or(trimmed).trim();
        for prefix in prefixes {
            if let Some(value) = trimmed.strip_prefix(prefix) {
                let value = value.trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn handoff_display_path(path: &Path) -> String {
    crate::planner::repair::workspace_relative_handoff_path(path)
}

fn handoff_display_value(value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else {
        handoff_display_path(Path::new(value))
    }
}

fn normalize_handoff_display_text(value: String) -> String {
    if !value.contains(".anvil") {
        return value;
    }
    value
        .split_whitespace()
        .map(normalize_handoff_display_token)
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_handoff_display_token(token: &str) -> String {
    let Some(index) = token.find(".anvil") else {
        return token.to_string();
    };
    let (prefix, rest) = token.split_at(index);
    let mut path = rest.to_string();
    let mut trailing = Vec::new();
    while path.len() > ".anvil".len() {
        let Some(ch) = path.chars().next_back() else {
            break;
        };
        if !matches!(ch, '"' | '\'' | ')' | ']' | ',' | ';') {
            break;
        }
        path.pop();
        trailing.push(ch);
    }
    let preserved_prefix = prefix
        .chars()
        .filter(|ch| matches!(*ch, '"' | '\'' | '(' | '['))
        .collect::<String>();
    let trailing = trailing.into_iter().rev().collect::<String>();
    format!(
        "{}{}{}",
        preserved_prefix,
        handoff_display_path(Path::new(&path)),
        trailing
    )
}

fn command_returned_incomplete(projection: &CompletionProjection) -> bool {
    projection.status.starts_with("incomplete")
        || projection.status.contains("partial")
        || matches!(
            projection.task_status.as_str(),
            "partial" | "incomplete" | "failed"
        )
}

fn latest_missing_evidence_keys(events: &[Value], stop_reason: &str) -> Vec<String> {
    for event in events.iter().rev() {
        let mut keys = event
            .get("missing_evidence")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if keys.is_empty()
            && let Some(profile_failures) = event.get("profile_failures").and_then(Value::as_array)
        {
            for failure in profile_failures.iter().filter_map(Value::as_str) {
                keys.extend(missing_evidence_keys_from_text(failure));
            }
        }
        if keys.is_empty()
            && let Some(reason) = event.get("primary_reason").and_then(Value::as_str)
        {
            keys.extend(missing_evidence_keys_from_text(reason));
        }
        if keys.is_empty()
            && let Some(reason) = event.get("stop_reason").and_then(Value::as_str)
        {
            keys.extend(missing_evidence_keys_from_text(reason));
        }
        keys = normalize_unique_strings(keys);
        if !keys.is_empty() {
            return keys;
        }
    }
    normalize_unique_strings(missing_evidence_keys_from_text(stop_reason))
}

fn missing_evidence_keys_from_text(text: &str) -> Vec<String> {
    let Some((_, rest)) = text.split_once("missing_required_evidence:") else {
        return Vec::new();
    };
    let end = rest
        .find(|ch: char| ch.is_whitespace() || matches!(ch, ';' | ')' | ']'))
        .unwrap_or(rest.len());
    rest[..end]
        .split(',')
        .map(|value| {
            value
                .trim()
                .trim_matches(|ch: char| matches!(ch, '.' | ':'))
        })
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn normalize_unique_strings(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn snapshot_from_completion_event(event: &Value) -> Option<CompletionSnapshot> {
    let name = event.get("event")?.as_str()?;
    if !matches!(
        name,
        "plan_final_contract" | "ultra_final_acceptance" | "tui_command_stop" | "run_stop"
    ) {
        return None;
    }
    if !has_completion_fields(event) {
        return None;
    }
    Some(CompletionSnapshot {
        profile: event
            .get("profile")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        effective_profile: event
            .get("effective_profile")
            .and_then(Value::as_str)
            .or_else(|| event.get("profile").and_then(Value::as_str))
            .unwrap_or("")
            .to_string(),
        contract_origin: event
            .get("contract_origin")
            .and_then(Value::as_str)
            .unwrap_or("initial")
            .to_string(),
        assurance_level: event
            .get("assurance_level")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        assurance_reason: event
            .get("assurance_reason")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        profile_inferred: event
            .get("profile_inferred")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        profile_inference_source: event
            .get("profile_inference_source")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        requested_port: event
            .get("requested_port")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        runtime_acceptance_status: event
            .get("runtime_acceptance_status")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| runtime_acceptance_status_from_bool(event))
            .unwrap_or_else(|| "not_checked".to_string()),
        final_acceptance_status: event
            .get("final_acceptance_status")
            .and_then(Value::as_str)
            .unwrap_or("not_checked")
            .to_string(),
        release_gate_status: event
            .get("release_gate_status")
            .and_then(Value::as_str)
            .unwrap_or("not_applicable")
            .to_string(),
        completion_contract_verification_enabled: event
            .get("completion_contract_verification_enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        completion_contract_path_merge_enabled: event
            .get("completion_contract_path_merge_enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        completion_contract_path: event
            .get("completion_contract_path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        completion_contract_generated: event
            .get("completion_contract_generated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        external_contract_checked: event
            .get("external_contract_checked")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        external_contract_ok: event
            .get("external_contract_ok")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        release_gate_reasons: event_string_array(event, "release_gate_reasons"),
        unverified_evidence: event_string_array(event, "unverified_evidence"),
        browser_readiness_applicable: event
            .get("browser_readiness_applicable")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        browser_readiness_execution_status: event
            .get("browser_readiness_execution_status")
            .and_then(Value::as_str)
            .unwrap_or("not_applicable")
            .to_string(),
        browser_readiness_status: event
            .get("browser_readiness_status")
            .and_then(Value::as_str)
            .unwrap_or("not_applicable")
            .to_string(),
        browser_readiness_evidence_path: event
            .get("browser_readiness_evidence_path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        interaction_evidence_applicable: event
            .get("interaction_evidence_applicable")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        interaction_evidence_execution_status: event
            .get("interaction_evidence_execution_status")
            .and_then(Value::as_str)
            .unwrap_or("not_applicable")
            .to_string(),
        interaction_evidence_status: event
            .get("interaction_evidence_status")
            .and_then(Value::as_str)
            .unwrap_or("not_applicable")
            .to_string(),
        interaction_evidence_path: event
            .get("interaction_evidence_path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        state_dimensions_changed: event_string_array(event, "state_dimensions_changed"),
        action_hooks: event_string_array(event, "action_hooks"),
        surface_fit_summary: event
            .get("surface_fit_summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        surface_fit_guidance: event
            .get("surface_fit_guidance")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        text_entry_target: event
            .get("text_entry_target")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        typed_token: event
            .get("typed_token")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        token_echoed: event_bool_or_string(event, "token_echoed"),
        text_input_state_change: event_bool_or_string(event, "text_input_state_change"),
        persistence_after_reload: event
            .get("persistence_after_reload")
            .and_then(Value::as_str)
            .unwrap_or("not_applicable")
            .to_string(),
        persistence_after_reload_reason: event
            .get("persistence_after_reload_reason")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        evidence_arbitration_summary: event
            .get("evidence_arbitration_summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        recovery_prompt_path: event
            .get("recovery_prompt_path")
            .and_then(Value::as_str)
            .map(handoff_display_value)
            .unwrap_or_default(),
        recovery_ultra_plan_path: event
            .get("recovery_ultra_plan_path")
            .and_then(Value::as_str)
            .map(handoff_display_value)
            .unwrap_or_default(),
        suggested_recovery_command: event
            .get("suggested_recovery_command")
            .and_then(Value::as_str)
            .map(|value| normalize_handoff_display_text(value.to_string()))
            .unwrap_or_default(),
        suggested_recovery_yaml_command: event
            .get("suggested_recovery_yaml_command")
            .and_then(Value::as_str)
            .map(|value| normalize_handoff_display_text(value.to_string()))
            .unwrap_or_default(),
        plan_adherence_present: event_string_array(event, "plan_adherence_present"),
        plan_adherence_missing: event_string_array(event, "plan_adherence_missing"),
        planner_verify_normalization_count: event
            .get("planner_verify_normalization_count")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize,
        planner_retry_count: event
            .get("planner_retry_count")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize,
        planner_quality_warning_count: event
            .get("planner_quality_warning_count")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize,
        planner_quality_issue_count: event
            .get("planner_quality_issue_count")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize,
        planner_repaired: event
            .get("planner_repaired")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        planner_release_risk: event
            .get("planner_release_risk")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        display_normalization_count: 0,
        display_salvaged_count: 0,
        display_substituted_count: 0,
        context_truncation_warning_count: 0,
        compile_rollback_summaries: Vec::new(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProfileReinferenceFields {
    from_profile: String,
    to_profile: String,
    source: String,
    at_phase: Option<u64>,
    requested_port: String,
    contract_origin: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LifecycleProfileFields {
    profile: String,
    effective_profile: String,
    profile_inferred: String,
    profile_inference_source: String,
    requested_port: String,
    contract_origin: String,
}

impl LifecycleProfileFields {
    fn apply_to(self, snapshot: &mut CompletionSnapshot) {
        let current =
            crate::planner::profile::canonical_profile_name(&snapshot_effective_profile(snapshot));
        let incoming = crate::planner::profile::canonical_profile_name(&self.effective_profile);
        let should_replace_profile = snapshot.profile.trim().is_empty()
            || (current == "generic" && known_non_generic_profile(&incoming));
        if should_replace_profile {
            snapshot.profile = self.profile.clone();
            snapshot.effective_profile = self.effective_profile.clone();
        } else if snapshot.effective_profile.trim().is_empty()
            && !snapshot.profile.trim().is_empty()
        {
            snapshot.effective_profile = snapshot.profile.clone();
        }
        if snapshot.profile_inferred.trim().is_empty() && !self.profile_inferred.is_empty() {
            snapshot.profile_inferred = self.profile_inferred;
        }
        if snapshot.profile_inference_source.trim().is_empty()
            && !self.profile_inference_source.is_empty()
        {
            snapshot.profile_inference_source = self.profile_inference_source;
        }
        if snapshot.requested_port.trim().is_empty() && !self.requested_port.is_empty() {
            snapshot.requested_port = self.requested_port;
        }
        if snapshot.contract_origin == "initial" && !self.contract_origin.is_empty() {
            snapshot.contract_origin = self.contract_origin;
        }
    }
}

fn latest_lifecycle_profile_fields(events: &[Value]) -> Option<LifecycleProfileFields> {
    events.iter().rev().find_map(lifecycle_profile_fields)
}

fn lifecycle_profile_fields(event: &Value) -> Option<LifecycleProfileFields> {
    let name = event.get("event").and_then(Value::as_str)?;
    if !matches!(
        name,
        "run_start"
            | "tui_command_start"
            | "ultra_context_initialized"
            | "ultra_plan_generation_attempt"
            | "ultra_plan_saved"
            | "recovery_prompt_saved"
            | "resume_start"
    ) {
        return None;
    }
    let profile = event
        .get("effective_profile")
        .or_else(|| event.get("profile"))
        .or_else(|| event.get("recovery_profile"))
        .or_else(|| event.get("resume_effective_profile"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    Some(LifecycleProfileFields {
        profile: profile.to_string(),
        effective_profile: profile.to_string(),
        profile_inferred: event
            .get("profile_inferred")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        profile_inference_source: event
            .get("profile_inference_source")
            .or_else(|| event.get("from"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        requested_port: event
            .get("requested_port")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| {
                event
                    .get("requested_port_value")
                    .and_then(Value::as_u64)
                    .map(|value| value.to_string())
            })
            .unwrap_or_default(),
        contract_origin: event
            .get("contract_origin")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    })
}

fn known_non_generic_profile(profile: &str) -> bool {
    crate::planner::profile::domain_profile(profile).id() != "generic"
}

impl ProfileReinferenceFields {
    fn apply_to(self, snapshot: &mut CompletionSnapshot) {
        snapshot.profile = self.to_profile.clone();
        snapshot.effective_profile = self.to_profile.clone();
        snapshot.profile_inferred = self.to_profile;
        snapshot.profile_inference_source = self.source;
        if !self.contract_origin.is_empty() {
            snapshot.contract_origin = self.contract_origin;
        }
        if !self.requested_port.is_empty() {
            snapshot.requested_port = self.requested_port;
        }
    }
}

fn latest_profile_reinference_after(
    events: &[Value],
    latest_completion_index: Option<usize>,
) -> Option<ProfileReinferenceFields> {
    let min_index = latest_completion_index.map(|index| index + 1).unwrap_or(0);
    events
        .iter()
        .enumerate()
        .rev()
        .find(|(index, event)| {
            *index >= min_index
                && event
                    .get("event")
                    .and_then(Value::as_str)
                    .is_some_and(|name| name == "profile_reinferred")
        })
        .and_then(|(_, event)| profile_reinference_fields(event))
}

fn profile_reinference_fields(event: &Value) -> Option<ProfileReinferenceFields> {
    let to_profile = event
        .get("to_profile")
        .or_else(|| event.get("profile"))
        .or_else(|| event.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?
        .to_string();
    Some(ProfileReinferenceFields {
        from_profile: event
            .get("from_profile")
            .and_then(Value::as_str)
            .unwrap_or("generic")
            .to_string(),
        to_profile,
        source: event
            .get("from")
            .and_then(Value::as_str)
            .unwrap_or("workspace")
            .to_string(),
        at_phase: event.get("at_phase").and_then(Value::as_u64),
        requested_port: event
            .get("requested_port")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        contract_origin: event
            .get("contract_origin")
            .and_then(Value::as_str)
            .unwrap_or("promoted_union")
            .to_string(),
    })
}

fn latest_requested_port(events: &[Value]) -> Option<String> {
    events
        .iter()
        .rev()
        .find_map(|event| event.get("requested_port").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn event_string_array(event: &Value, field: &str) -> Vec<String> {
    event
        .get(field)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn event_bool_or_string(event: &Value, field: &str) -> String {
    event
        .get(field)
        .and_then(|value| {
            value
                .as_bool()
                .map(|flag| flag.to_string())
                .or_else(|| value.as_str().map(ToOwned::to_owned))
        })
        .unwrap_or_default()
}

fn has_completion_fields(event: &Value) -> bool {
    event.get("final_acceptance_status").is_some()
        || event.get("release_gate_status").is_some()
        || event.get("runtime_acceptance_status").is_some()
        || event.get("runtime_acceptance_passed").is_some()
}

fn runtime_acceptance_status_from_bool(event: &Value) -> Option<String> {
    event
        .get("runtime_acceptance_passed")
        .and_then(Value::as_bool)
        .map(|passed| {
            if passed {
                "pass".to_string()
            } else {
                "failed".to_string()
            }
        })
}

fn terminal_status(ok: bool, release_gate: &str, final_acceptance: &str) -> String {
    if !ok {
        return "incomplete".to_string();
    }
    match release_gate {
        "partial" => "complete_with_partial_release_gate".to_string(),
        "failed" => "incomplete_release_gate_failed".to_string(),
        "pass" | "not_applicable" | "not_checked" | "" => match final_acceptance {
            "partial" => "complete_with_partial_release_gate".to_string(),
            "incomplete" | "failed" => "incomplete".to_string(),
            _ => "complete".to_string(),
        },
        _ => "incomplete".to_string(),
    }
}

fn task_status(ok: bool, release_gate: &str, final_acceptance: &str) -> String {
    if !ok {
        return "failed".to_string();
    }
    match release_gate {
        "partial" => "partial".to_string(),
        "failed" => "failed".to_string(),
        "pass" => "complete".to_string(),
        "not_applicable" | "not_checked" | "" => match final_acceptance {
            "partial" => "partial".to_string(),
            "incomplete" => "incomplete".to_string(),
            "failed" => "failed".to_string(),
            _ => "complete".to_string(),
        },
        _ => "incomplete".to_string(),
    }
}

fn release_quality_completion(release_gate: &str, final_acceptance: &str) -> String {
    match release_gate {
        "pass" | "not_applicable" => "release_ready".to_string(),
        "partial" => "partial".to_string(),
        "failed" => "failed".to_string(),
        _ if final_acceptance == "partial" => "partial".to_string(),
        _ if matches!(final_acceptance, "incomplete" | "failed") => "failed".to_string(),
        _ => "not_checked".to_string(),
    }
}

fn next_action(ok: bool, release_gate: &str, final_acceptance: &str) -> String {
    if !ok {
        return "fix_command_failure".to_string();
    }
    match release_gate {
        "partial" => "collect_missing_release_evidence_or_continue_release_recovery".to_string(),
        "failed" => "repair_release_gate_failure".to_string(),
        _ if final_acceptance == "partial" => {
            "collect_missing_final_acceptance_evidence".to_string()
        }
        _ if matches!(final_acceptance, "incomplete" | "failed") => {
            "repair_final_acceptance_failure".to_string()
        }
        _ => "none".to_string(),
    }
}

fn render_completion_summary(
    lifecycle_stage: &str,
    action: Option<&str>,
    command: Option<&str>,
    stop_reason: &str,
    failure_kind: &str,
    projection: &CompletionProjection,
) -> String {
    let mut lines = vec![
        format!("Status: {}", projection.status),
        format!("Lifecycle: {lifecycle_stage}"),
        format!("Process: {}", process_lifecycle_status(projection)),
        format!("Session/REPL status: {}", session_status(lifecycle_stage)),
    ];
    let stop_reason = render_stop_reason_text(stop_reason);
    if let Some(action) = action {
        lines.push(format!("Action: {action}"));
    }
    if let Some(command) = command {
        lines.push(format!("Command: {command}"));
    }
    lines.extend([
        format!("Command status: {}", projection.command_completion),
        format!("Command completion: {}", projection.command_completion),
        format!("Task status: {}", projection.task_status),
        format!(
            "Effective profile: {}",
            missing_if_empty(&projection.effective_profile)
        ),
        format!("Contract origin: {}", projection.contract_origin),
        format!("Runtime acceptance: {}", projection.runtime_acceptance),
        format!("Final acceptance: {}", projection.final_acceptance),
        format!("Release gate: {}", projection.release_gate),
        format!("Requested port: {}", missing_if_empty(&projection.requested_port)),
        format!(
            "Evidence arbitration: {}",
            missing_if_empty(&projection.evidence_arbitration_summary)
        ),
        format!(
            "completion_contract_verification_enabled={}",
            projection.completion_contract_verification_enabled
        ),
        format!(
            "completion_contract_path_merge_enabled={}",
            projection.completion_contract_path_merge_enabled
        ),
        format!(
            "completion_contract_path={}",
            missing_if_empty(&projection.completion_contract_path)
        ),
        format!(
            "completion_contract_generated={}",
            projection.completion_contract_generated
        ),
        format!(
            "external_contract_checked={}",
            projection.external_contract_checked
        ),
        format!("external_contract_ok={}", projection.external_contract_ok),
        format!(
            "browser_readiness_applicable={}",
            projection.browser_readiness_applicable
        ),
        format!(
            "browser_readiness_execution_status={}",
            projection.browser_readiness_execution_status
        ),
        format!(
            "interaction_evidence_applicable={}",
            projection.interaction_evidence_applicable
        ),
        format!(
            "interaction_evidence_execution_status={}",
            projection.interaction_evidence_execution_status
        ),
        format!("Planner repaired: {}", projection.planner_repaired),
        format!("Planner release risk: {}", projection.planner_release_risk),
        format!(
            "Planner diagnostics: normalizations={} retries={} quality_warnings={} quality_issues={}",
            projection.planner_verify_normalization_count,
            projection.planner_retry_count,
            projection.planner_quality_warning_count,
            projection.planner_quality_issue_count
        ),
        format!(
            "Context truncation warning: {}",
            if projection.context_truncation_warning_count > 0 {
                format!(
                    "suspected (warnings={})",
                    projection.context_truncation_warning_count
                )
            } else {
                "none".to_string()
            }
        ),
        format!(
            "Release quality completion: {}",
            projection.release_quality_completion
        ),
        "Release gate reasons:".to_string(),
        render_summary_bullets(&projection.release_gate_reasons),
        format!("Browser readiness: {}", projection.browser_readiness),
        format!(
            "Browser readiness evidence: {}",
            missing_if_empty(&projection.browser_readiness_evidence_path)
        ),
        format!("Interaction evidence: {}", projection.interaction_evidence),
        format!(
            "Interaction evidence path: {}",
            missing_if_empty(&projection.interaction_evidence_path)
        ),
        format!(
            "State dimensions changed: {}",
            if projection.state_dimensions_changed.is_empty() {
                "none".to_string()
            } else {
                projection.state_dimensions_changed.join(", ")
            }
        ),
        format!(
            "Action hooks: {}",
            if projection.action_hooks.is_empty() {
                "none".to_string()
            } else {
                projection.action_hooks.join(", ")
            }
        ),
        format!(
            "Surface fit: {}",
            missing_if_empty(&projection.surface_fit_summary)
        ),
        format!(
            "Text entry target: {}",
            missing_if_empty(&projection.text_entry_target)
        ),
        format!("Typed token: {}", missing_if_empty(&projection.typed_token)),
        format!(
            "Token echoed: {}",
            missing_if_empty(&projection.token_echoed)
        ),
        format!(
            "Text input state change: {}",
            missing_if_empty(&projection.text_input_state_change)
        ),
        format!("Next action: {}", projection.next_action),
        format!("Recovery next action: {}", projection.next_action),
        format!("Stop reason: {stop_reason}"),
    ]);
    if !projection.profile.is_empty() {
        lines.push(format!("Profile: {}", projection.profile));
    }
    if !projection.profile_inferred.is_empty() {
        lines.push(format!(
            "profile_inferred: {} (from: {})",
            projection.profile_inferred, projection.profile_inference_source
        ));
    }
    if !projection.assurance_level.is_empty() {
        let suffix = if projection.assurance_reason.is_empty() {
            String::new()
        } else {
            format!(" ({})", projection.assurance_reason)
        };
        lines.push(format!(
            "Assurance: {}{}",
            projection.assurance_level, suffix
        ));
    }
    if !projection.unverified_evidence.is_empty() {
        lines.push("Unverified (probe required):".to_string());
        lines.push(render_summary_bullets(&projection.unverified_evidence));
        if projection
            .unverified_evidence
            .iter()
            .any(|evidence| evidence.contains(":unverified:probe_unavailable"))
        {
            lines.push(
                crate::minimal_loop::interaction_probe::INTERACTION_PROBE_SETUP_REMEDIATION
                    .to_string(),
            );
        }
        if projection
            .unverified_evidence
            .iter()
            .any(|evidence| evidence.contains(":unverified:terminal_state_not_reached"))
        {
            lines.push(
                "Restart verification: either expose an in-play restart control, or accept the partial classification (the restart exists but cannot be behaviorally verified by the generic probe)."
                    .to_string(),
            );
        }
    }
    if !projection.surface_fit_guidance.is_empty() {
        lines.push(format!(
            "Surface fit guidance: {}",
            projection.surface_fit_guidance
        ));
    }
    if interaction_unverified_probe_unavailable(
        &projection.release_gate,
        &projection.release_gate_reasons,
    ) {
        lines.push(
            format!(
                "Interaction verification: interaction_unverified:probe_unavailable; {}.",
                crate::minimal_loop::interaction_probe::INTERACTION_PROBE_SETUP_REMEDIATION
            )
            .to_string(),
        );
    }
    if projection
        .evidence_arbitration_summary
        .starts_with("static (probe infrastructure failure:")
        || projection.release_gate_reasons.iter().any(|reason| {
            reason.contains("probe_dependency_missing")
                || reason.contains("probe_infrastructure_failed")
        })
    {
        lines.push(
            "Interaction verification: app interaction untested (probe infrastructure failure)."
                .to_string(),
        );
    }
    let host_env_contamination = crate::minimal_loop::verifier_env::host_env_contamination();
    if !host_env_contamination.is_empty() {
        lines.push(format!(
            "Host env: {} detected (verifiers ran with a cleaned environment)",
            host_env_contamination.join(", ")
        ));
    }
    if !projection.plan_adherence_present.is_empty()
        || !projection.plan_adherence_missing.is_empty()
    {
        lines.extend([
            "Plan adherence:".to_string(),
            "Present tokens:".to_string(),
            render_summary_bullets(&projection.plan_adherence_present),
            "Missing tokens:".to_string(),
            render_summary_bullets(&projection.plan_adherence_missing),
        ]);
    }
    if !projection.compile_rollback_summaries.is_empty() {
        lines.push("Compile rollback applied:".to_string());
        lines.push(render_summary_bullets(
            &projection.compile_rollback_summaries,
        ));
    }
    if !projection.recovery_prompt_path.is_empty()
        || !projection.recovery_ultra_plan_path.is_empty()
        || !projection.suggested_recovery_command.is_empty()
        || !projection.suggested_recovery_yaml_command.is_empty()
    {
        lines.extend([
            "Recovery handoff:".to_string(),
            format!(
                "- Recovery prompt saved: {}",
                missing_if_empty(&projection.recovery_prompt_path)
            ),
            format!(
                "- Recovery UltraPlan YAML saved: {}",
                missing_if_empty(&projection.recovery_ultra_plan_path)
            ),
            format!(
                "- Suggested command: {}",
                missing_if_empty(&projection.suggested_recovery_command)
            ),
            format!(
                "- Suggested YAML command: {}",
                missing_if_empty(&projection.suggested_recovery_yaml_command)
            ),
        ]);
    }
    if !failure_kind.is_empty() {
        lines.push(format!("Failure kind: {failure_kind}"));
    }
    if lifecycle_stage == "tui_command" && projection.command_completion == "failed" {
        let first_stop_line = stop_reason.lines().next().unwrap_or("unknown");
        lines.push(format!(
            "TUI command failed: {}",
            body_snippet(first_stop_line)
        ));
    }
    lines.join("\n")
}

fn render_tui_command_completion_summary(
    command: &str,
    stop_reason: &str,
    failure_kind: &str,
    terminal_status: &str,
    projection: &CompletionProjection,
    events: &[Value],
    previous_summary: &str,
) -> String {
    let mut summary_projection = projection.clone();
    summary_projection.status = terminal_status.to_string();
    summary_projection.command_completion = terminal_status.to_string();
    if terminal_status != "completed" && terminal_status != "partial" {
        summary_projection.task_status = terminal_status.to_string();
        summary_projection.next_action = match terminal_status {
            "interrupted" => "resume_or_rerun_command".to_string(),
            "aborted" => "inspect_summary_and_resume_or_rerun".to_string(),
            _ => "fix_command_failure".to_string(),
        };
    }
    let mut lines = render_completion_summary(
        "tui_command",
        None,
        Some(command),
        stop_reason,
        failure_kind,
        &summary_projection,
    )
    .lines()
    .map(ToOwned::to_owned)
    .collect::<Vec<_>>();
    let insert_at = lines
        .iter()
        .position(|line| line.starts_with("Lifecycle:"))
        .unwrap_or(lines.len());
    lines.insert(
        insert_at,
        format!("Completion status: {}", projection.status),
    );
    if let Some(line) = latest_profile_promotion_summary_line(events)
        && !lines.iter().any(|existing| existing == &line)
    {
        let profile_index = lines
            .iter()
            .position(|line| line.starts_with("Profile:"))
            .unwrap_or(lines.len());
        lines.insert(profile_index, line);
    }
    lines.push(String::new());
    lines.push(render_phase_breakdown_for_summary(
        &phase_breakdown_for_tui_summary(events, previous_summary, terminal_status),
        terminal_status,
    ));
    lines.join("\n")
}

fn latest_profile_promotion_summary_line(events: &[Value]) -> Option<String> {
    let fields = events
        .iter()
        .rev()
        .find(|event| {
            event
                .get("event")
                .and_then(Value::as_str)
                .is_some_and(|name| name == "profile_reinferred")
        })
        .and_then(profile_reinference_fields)?;
    let phase = fields
        .at_phase
        .map(|phase| format!(", phase {phase}"))
        .unwrap_or_default();
    Some(format!(
        "Profile promoted: {} -> {} ({} evidence{})",
        fields.from_profile, fields.to_profile, fields.source, phase
    ))
}

fn render_phase_breakdown_for_summary(breakdown: &PhaseBreakdown, terminal_status: &str) -> String {
    let failed_status = match terminal_status {
        "aborted" | "interrupted" => terminal_status,
        _ => "failed",
    };
    [
        "Completed phases:".to_string(),
        render_phase_bullets_with_status(&breakdown.completed, "completed"),
        String::new(),
        "Failed phases:".to_string(),
        render_phase_bullets_with_status(&breakdown.failed, failed_status),
        String::new(),
        "Pending phases:".to_string(),
        render_phase_bullets_with_status(&breakdown.pending, "pending"),
    ]
    .join("\n")
}

fn render_phase_bullets_with_status(items: &[String], status: &str) -> String {
    if items.is_empty() {
        "- none".to_string()
    } else {
        items
            .iter()
            .map(|item| format!("- {item} ({status})"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn session_status(lifecycle_stage: &str) -> &'static str {
    match lifecycle_stage {
        "tui_command" => "repl_ready",
        "process" => "process_exited",
        _ => "unknown",
    }
}

fn process_lifecycle_status(projection: &CompletionProjection) -> String {
    if matches!(
        projection.command_completion.as_str(),
        "aborted" | "interrupted"
    ) {
        projection.command_completion.clone()
    } else {
        "REPL exited cleanly (not task status)".to_string()
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

fn missing_if_empty(value: &str) -> &str {
    if value.is_empty() { "missing" } else { value }
}

pub fn argument_shape(arguments: &Value) -> Value {
    match arguments {
        Value::Object(map) => {
            let mut keys = map.keys().cloned().collect::<Vec<_>>();
            keys.sort();
            let mut summaries = serde_json::Map::new();
            for key in &keys {
                if let Some(value) = map.get(key) {
                    summaries.insert(key.clone(), argument_value_summary(key, value));
                }
            }
            json!({
                "arguments_type": "object",
                "argument_keys": keys,
                "argument_summaries": summaries,
            })
        }
        Value::String(value) => json!({
            "arguments_type": "string",
            "argument_len": value.chars().count(),
            "argument_preview": safe_preview(value),
        }),
        Value::Array(values) => json!({
            "arguments_type": "array",
            "argument_len": values.len(),
        }),
        Value::Null => json!({
            "arguments_type": "null",
        }),
        Value::Bool(_) => json!({
            "arguments_type": "bool",
        }),
        Value::Number(_) => json!({
            "arguments_type": "number",
        }),
    }
}

pub fn body_snippet(body: &str) -> String {
    let mut clean = body.replace('\n', " ");
    clean = clean.replace('\r', " ");
    clean = redact_secret_like(&clean);
    clean = redact_home_paths(&clean);
    clean.chars().take(SNIPPET_LIMIT).collect()
}

pub fn body_snippet_whole_tokens(body: &str) -> String {
    let mut clean = body.replace('\n', " ");
    clean = clean.replace('\r', " ");
    clean = redact_secret_like(&clean);
    clean = redact_home_paths(&clean);
    truncate_whole_tokens(&clean, SNIPPET_LIMIT)
}

fn truncate_whole_tokens(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let mut out = String::new();
    let mut len = 0usize;
    for token in value.split_whitespace() {
        let token_len = token.chars().count();
        let next_len = len + usize::from(!out.is_empty()) + token_len;
        if next_len > limit {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
            len += 1;
        }
        out.push_str(token);
        len += token_len;
    }
    if out.is_empty() {
        value.chars().take(limit).collect()
    } else {
        out
    }
}

fn summary_body(body: &str) -> String {
    let clean = body.replace("\r\n", "\n").replace('\r', "\n");
    let clean = redact_home_paths(&clean);
    let mut out = String::new();
    let mut len = 0usize;
    for line in clean.lines().map(redact_secret_like) {
        let line_len = line.chars().count();
        let next_len = len + usize::from(!out.is_empty()) + line_len;
        if next_len > SUMMARY_LIMIT {
            break;
        }
        if !out.is_empty() {
            out.push('\n');
            len += 1;
        }
        out.push_str(&line);
        len += line_len;
    }
    out
}

fn summary_document(body: &str) -> String {
    let content = summary_body(body);
    if content.is_empty() {
        crate::build_info::summary_line()
    } else {
        format!("{}\n{content}", crate::build_info::summary_line())
    }
}

fn argument_value_summary(key: &str, value: &Value) -> Value {
    match value {
        Value::String(text) => {
            if key == "content" {
                json!({
                    "type": "string",
                    "string_len": text.chars().count(),
                    "preview": "<omitted>",
                })
            } else if matches!(key, "path" | "pattern" | "glob" | "command") {
                json!({
                    "type": "string",
                    "string_len": text.chars().count(),
                    "preview": safe_preview(text),
                })
            } else {
                json!({
                    "type": "string",
                    "string_len": text.chars().count(),
                })
            }
        }
        Value::Array(values) => json!({"type": "array", "len": values.len()}),
        Value::Object(map) => json!({"type": "object", "keys": map.len()}),
        Value::Bool(_) => json!({"type": "bool"}),
        Value::Number(_) => json!({"type": "number"}),
        Value::Null => json!({"type": "null"}),
    }
}

fn safe_preview(value: &str) -> String {
    let mut clean = value.replace('\n', "\\n").replace('\r', "\\r");
    clean = redact_secret_like(&clean);
    clean = redact_home_paths(&clean);
    clean.chars().take(120).collect()
}

fn redact_secret_like(value: &str) -> String {
    value
        .split_whitespace()
        .map(|part| {
            if part.starts_with("sk-")
                || part.starts_with("AIza")
                || part.to_ascii_lowercase().contains("api_key")
            {
                "<redacted>"
            } else {
                part
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn redact_home_paths(value: &str) -> String {
    let mut out = value.to_string();
    for prefix in ["/Users/", "/home/"] {
        let mut search_from = 0usize;
        loop {
            let Some(relative_start) = out[search_from..].find(prefix) else {
                break;
            };
            let start = search_from + relative_start;
            let name_start = start + prefix.len();
            let Some(rest_end) = out[name_start..].find('/') else {
                break;
            };
            let name_end = name_start + rest_end;
            if &out[name_start..name_end] == "<user>" {
                search_from = name_end;
                continue;
            }
            out.replace_range(name_start..name_end, "<user>");
            search_from = name_start + "<user>".len();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_jsonl_without_prompt_payloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        emit(
            Some(&path),
            json!({"event":"tool_call_raw","name":"Grep","arguments": argument_shape(&json!({"pattern":"sk-test","content":"do not persist"}))}),
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("\"event\":\"tool_call_raw\""));
        assert!(text.contains("\"argument_keys\":[\"content\",\"pattern\"]"));
        assert!(text.contains("<redacted>"));
        assert!(!text.contains("do not persist"));
    }

    #[test]
    fn body_snippet_truncates_and_redacts_secret_like_values() {
        let snippet = body_snippet(&format!(
            "api_key sk-test /Users/example/project {}",
            "x".repeat(700)
        ));
        assert!(snippet.contains("<redacted>"));
        assert!(snippet.contains("/Users/<user>/project"));
        assert!(snippet.chars().count() <= SNIPPET_LIMIT);
    }

    #[test]
    fn body_snippet_and_summary_body_handle_multibyte_caps() {
        let body = format!("{}{}", "日本語".repeat(220), "除外");
        let snippet = body_snippet(&body);
        assert!(snippet.chars().count() <= SNIPPET_LIMIT);
        let summary = summary_body(&format!("{}\n{}", "日本語".repeat(1_000), body));
        assert!(summary.chars().count() <= SUMMARY_LIMIT);
    }

    #[test]
    fn stop_reason_renderer_preserves_path_and_command_lines() {
        let recovery_yaml =
            ".anvil/plans/recovery-ultra-plan-final-acceptance-test0703-002-long-name.yaml";
        let command = format!("/run-ultra-plan {recovery_yaml}");
        let rendered = render_stop_reason(&StopReasonParts {
            free_text: format!("failure {}", "x ".repeat(2_000)),
            paths: vec![format!("recovery YAML saved: {recovery_yaml}")],
            commands: vec![format!("suggested YAML command: {command}")],
        });

        assert!(
            rendered.contains(&format!("- recovery YAML saved: {recovery_yaml}")),
            "{rendered}"
        );
        assert!(
            rendered.contains(&format!("- suggested YAML command: {command}")),
            "{rendered}"
        );
        assert!(!rendered.contains(".yam\n"), "{rendered}");
        assert!(!rendered.contains("recovery-ultr\n"), "{rendered}");
    }

    #[test]
    fn default_run_events_path_uses_anvil_runs_events_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let path = default_run_events_path(dir.path());
        assert!(path.starts_with(dir.path().join(".anvil").join("runs")));
        assert_eq!(path.file_name().unwrap(), "events.jsonl");
    }

    #[test]
    fn context_truncation_warning_projects_to_summary_and_terminal_card() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        emit(
            Some(&path),
            json!({
                "event": "context_truncation_suspected",
                "level": "WARNING",
                "caller_scope": "executor",
                "provider": "ollama",
                "model": "gemma4:31b-cloud",
                "estimated_prompt_tokens_sent": 4096,
                "prompt_eval_count": 1024,
                "eval_count": 128,
                "finish_reason": "length",
                "persistent_undercut_count": 2,
            }),
        );

        let snapshot = latest_completion_snapshot(Some(&path));
        assert_eq!(snapshot.context_truncation_warning_count, 1);
        let projection = project_completion(false, &snapshot);
        assert_eq!(projection.context_truncation_warning_count, 1);
        let summary = render_completion_summary(
            "tui_command",
            None,
            Some("/run-ultra"),
            "context telemetry",
            "",
            &projection,
        );
        assert!(
            summary.contains("Context truncation warning: suspected (warnings=1)"),
            "{summary}"
        );
        let telemetry = terminal_card_telemetry(&projection).unwrap_or_default();
        assert!(
            telemetry.contains("context_truncation_suspected=1"),
            "{telemetry}"
        );
    }

    #[test]
    fn run_summary_preserves_human_readable_sections_and_appends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".anvil/runs/test/events.jsonl");
        write_run_summary(
            Some(&path),
            "Status: incomplete\nCompleted phases:\n- scaffold\napi_key sk-test",
        );
        append_run_summary(Some(&path), "TUI command failed: phase failed");
        let summary = std::fs::read_to_string(path.parent().unwrap().join("summary.md")).unwrap();
        assert!(summary.contains("Status: incomplete\nCompleted phases:\n- scaffold"));
        assert!(summary.contains("---\n\nTUI command failed: phase failed"));
        assert!(summary.contains("<redacted>"));
        assert!(!summary.contains("sk-test"));
    }

    #[test]
    fn completion_projection_renders_contract_binding_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        emit(
            Some(&path),
            json!({
                "event": "ultra_final_acceptance",
                "runtime_acceptance_status": "pass",
                "final_acceptance_status": "partial",
                "release_gate_status": "partial",
                "completion_contract_verification_enabled": true,
                "completion_contract_path_merge_enabled": true,
                "completion_contract_path": ".anvil/runs/test/completion-contract-ultra-plan-run.json",
                "completion_contract_generated": true,
                "external_contract_checked": true,
                "external_contract_ok": true,
            }),
        );
        let snapshot = latest_completion_snapshot(Some(&path));
        let projection = project_completion(true, &snapshot);
        let tui = render_tui_completion_output("done", &projection);
        assert!(tui.contains("Command status: completed"));
        assert!(tui.contains("Task status: partial"));
        assert!(tui.contains("completion_contract_verification_enabled=true"));
        assert!(tui.contains("external_contract_checked=true"));
        let summary = render_completion_summary(
            "tui_command",
            None,
            Some("/ultra-plan-run"),
            "completed",
            "",
            &projection,
        );
        assert!(summary.contains("Session/REPL status: repl_ready"));
        assert!(summary.contains("Command status: completed"));
        assert!(summary.contains("Task status: partial"));
        assert!(summary.contains("completion_contract_verification_enabled=true"));
        assert!(summary.contains("completion_contract_path_merge_enabled=true"));
        assert!(summary.contains("external_contract_checked=true"));
        assert!(summary.contains("external_contract_ok=true"));
    }

    #[test]
    fn latest_completion_snapshot_reflects_profile_reinferred_after_generic_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        emit(
            Some(&path),
            json!({
                "event": "profile_reinferred",
                "from_profile": "generic",
                "to_profile": "nextjs",
                "from": "workspace",
                "at_phase": 1,
                "requested_port": "3011 (goal)",
                "contract_origin": "promoted_union",
            }),
        );

        let snapshot = latest_completion_snapshot(Some(&path));

        assert_eq!(snapshot.profile, "nextjs");
        assert_eq!(snapshot.effective_profile, "nextjs");
        assert_eq!(snapshot.profile_inferred, "nextjs");
        assert_eq!(snapshot.profile_inference_source, "workspace");
        assert_eq!(snapshot.requested_port, "3011 (goal)");
        assert_eq!(snapshot.contract_origin, "promoted_union");
        assert!(snapshot.assurance_level.is_empty());
        assert!(snapshot.assurance_reason.is_empty());
    }

    #[test]
    fn latest_completion_snapshot_uses_known_profile_from_early_lifecycle_events() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        emit(
            Some(&path),
            json!({
                "event": "tui_command_start",
                "command": "/ultra-plan-run",
                "profile": "nextjs",
                "style": "default",
            }),
        );
        emit(
            Some(&path),
            json!({
                "event": "ultra_context_initialized",
                "profile": "nextjs",
                "requested_port": "3011 (goal)",
            }),
        );
        emit(
            Some(&path),
            json!({
                "event": "planner_error",
                "planner_error_kind": "verify_command_policy_error",
            }),
        );

        let snapshot = latest_completion_snapshot(Some(&path));
        let projection = project_completion(false, &snapshot);

        assert_eq!(snapshot.profile, "nextjs");
        assert_eq!(snapshot.effective_profile, "nextjs");
        assert_eq!(snapshot.requested_port, "3011 (goal)");
        assert_eq!(projection.effective_profile, "nextjs");
    }

    #[test]
    fn completion_projection_downgrades_full_when_applicable_gates_are_disconnected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        emit(
            Some(&path),
            json!({
                "event": "ultra_final_acceptance",
                "profile": "nextjs",
                "effective_profile": "nextjs",
                "contract_origin": "promoted_union",
                "runtime_acceptance_status": "pass",
                "final_acceptance_status": "full_success",
                "release_gate_status": "not_applicable",
                "completion_contract_verification_enabled": true,
                "external_contract_checked": true,
                "external_contract_ok": true,
                "assurance_level": "full",
                "browser_readiness_applicable": true,
                "browser_readiness_execution_status": "disconnected",
                "browser_readiness_status": "not_applicable",
                "interaction_evidence_applicable": true,
                "interaction_evidence_execution_status": "disconnected",
                "interaction_evidence_status": "not_applicable",
            }),
        );

        let snapshot = latest_completion_snapshot(Some(&path));
        let projection = project_completion(true, &snapshot);

        assert_eq!(projection.effective_profile, "nextjs");
        assert_eq!(projection.contract_origin, "promoted_union");
        assert_eq!(projection.assurance_level, "partial");
        assert_eq!(
            projection.assurance_reason,
            "browser_readiness_not_performed:disconnected"
        );
    }

    #[test]
    fn explicit_known_profile_failed_projection_is_partial_not_reduced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        emit(
            Some(&path),
            json!({
                "event": "ultra_final_acceptance",
                "profile": "nextjs",
                "effective_profile": "nextjs",
                "runtime_acceptance_status": "failed",
                "final_acceptance_status": "failed",
                "release_gate_status": "failed",
                "release_gate_reasons": ["browser_interaction_failed:canvas_blank"],
                "assurance_level": "full",
                "completion_contract_verification_enabled": true,
                "external_contract_checked": true,
                "external_contract_ok": false,
                "browser_readiness_applicable": true,
                "browser_readiness_execution_status": "performed",
                "browser_readiness_status": "passed",
                "interaction_evidence_applicable": true,
                "interaction_evidence_execution_status": "performed_failed",
                "interaction_evidence_status": "failed:canvas_blank",
            }),
        );

        let snapshot = latest_completion_snapshot(Some(&path));
        let projection = project_completion(false, &snapshot);
        let summary = render_completion_summary(
            "process",
            Some("UltraPlanRun"),
            None,
            "failed",
            "direct_cli_command_failed",
            &projection,
        );

        assert_eq!(projection.effective_profile, "nextjs");
        assert_eq!(projection.status, "incomplete");
        assert_eq!(projection.task_status, "failed");
        assert_eq!(projection.assurance_level, "partial");
        assert!(!summary.contains("Assurance: reduced"), "{summary}");
        assert!(summary.contains("Task status: failed"), "{summary}");
        assert!(summary.contains("Final acceptance: failed"), "{summary}");
    }

    #[test]
    fn completion_projection_renders_planner_diagnostics_as_release_risk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        emit(
            Some(&path),
            json!({
                "event": "planner_verify_command_normalized",
                "planner_stage": "verify_policy",
            }),
        );
        emit(
            Some(&path),
            json!({
                "event": "planner_error",
                "planner_error_kind": "verify_command_policy_error",
            }),
        );
        emit(
            Some(&path),
            json!({
                "event": "planner_quality_warning",
                "planner_error_kind": "planner_quality_warning",
            }),
        );
        emit(
            Some(&path),
            json!({
                "event": "ultra_final_acceptance",
                "runtime_acceptance_status": "pass",
                "final_acceptance_status": "partial",
                "release_gate_status": "partial",
            }),
        );
        let snapshot = latest_completion_snapshot(Some(&path));
        let projection = project_completion(true, &snapshot);
        let tui = render_tui_completion_output("done", &projection);
        assert!(tui.contains("Task status: partial"));
        assert!(tui.contains(
            "Planner diagnostics: normalizations=1 retries=1 quality_warnings=1 quality_issues=0"
        ));
        assert!(tui.contains("Planner release risk: true"));
        let summary = render_completion_summary(
            "tui_command",
            None,
            Some("/ultra-plan-run"),
            "completed",
            "",
            &projection,
        );
        assert!(summary.contains("Task status: partial"));
        assert!(summary.contains(
            "Recovery next action: collect_missing_release_evidence_or_continue_release_recovery"
        ));
        assert!(summary.contains("Planner repaired: true"));
        assert!(summary.contains("Planner release risk: true"));
        assert!(summary.contains(
            "Planner diagnostics: normalizations=1 retries=1 quality_warnings=1 quality_issues=0"
        ));
    }

    #[test]
    fn completion_snapshot_uses_latest_non_empty_recovery_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        emit(
            Some(&path),
            json!({
                "event": "loop_stop",
                "runtime_acceptance_status": "failed",
                "final_acceptance_status": "failed",
                "release_gate_status": "failed",
                "recovery_prompt_path": "/tmp/work/.anvil/repairs/old.md",
                "recovery_ultra_plan_path": "/tmp/work/.anvil/plans/old.yaml",
                "suggested_recovery_command": "/ultra-plan-run --profile nextjs \"$(cat /tmp/work/.anvil/repairs/old.md)\"",
                "suggested_recovery_yaml_command": "/run-ultra-plan /tmp/work/.anvil/plans/old.yaml",
            }),
        );
        emit(
            Some(&path),
            json!({
                "event": "recovery_prompt_saved",
                "recovery_prompt_path": "/tmp/work/.anvil/repairs/new.md",
                "recovery_ultra_plan_path": "/tmp/work/.anvil/plans/new.yaml",
                "suggested_recovery_command": "/ultra-plan-run --profile nextjs \"$(cat /tmp/work/.anvil/repairs/new.md)\"",
                "suggested_recovery_yaml_command": "/run-ultra-plan /tmp/work/.anvil/plans/new.yaml",
            }),
        );
        emit(
            Some(&path),
            json!({
                "event": "ultra_phase_failed",
                "recovery_prompt_path": "",
                "recovery_ultra_plan_path": "",
                "suggested_recovery_command": "",
                "suggested_recovery_yaml_command": "",
            }),
        );

        let snapshot = latest_completion_snapshot(Some(&path));

        assert_eq!(snapshot.recovery_prompt_path, ".anvil/repairs/new.md");
        assert_eq!(snapshot.recovery_ultra_plan_path, ".anvil/plans/new.yaml");
        assert_eq!(
            snapshot.suggested_recovery_command,
            "/ultra-plan-run --profile nextjs \"$(cat .anvil/repairs/new.md)\""
        );
        assert_eq!(
            snapshot.suggested_recovery_yaml_command,
            "/run-ultra-plan .anvil/plans/new.yaml"
        );
    }

    #[test]
    fn completion_snapshot_can_be_recovery_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        emit(
            Some(&path),
            json!({
                "event": "recovery_prompt_saved",
                "recovery_prompt_path": "/tmp/work/.anvil/repairs/phase.md",
                "recovery_ultra_plan_path": "/tmp/work/.anvil/plans/phase.yaml",
                "suggested_recovery_command": "/ultra-plan-run --profile nextjs \"$(cat /tmp/work/.anvil/repairs/phase.md)\"",
                "suggested_recovery_yaml_command": "/run-ultra-plan /tmp/work/.anvil/plans/phase.yaml",
            }),
        );

        let snapshot = latest_completion_snapshot(Some(&path));

        assert_eq!(snapshot.final_acceptance_status, "not_checked");
        assert_eq!(snapshot.recovery_prompt_path, ".anvil/repairs/phase.md");
        assert_eq!(snapshot.recovery_ultra_plan_path, ".anvil/plans/phase.yaml");
    }

    #[test]
    fn side_effect_path_drop_counts_as_display_normalization_metric() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        emit(
            Some(&path),
            json!({
                "event": "side_effect_path_dropped",
                "path": "node_modules",
                "tier": "unambiguous",
            }),
        );

        let snapshot = latest_completion_snapshot(Some(&path));

        assert_eq!(snapshot.display_normalization_count, 1);
        assert_eq!(snapshot.display_salvaged_count, 0);
    }

    #[test]
    fn summary_keeps_recovery_command_lines_intact_with_long_failure_reason() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".anvil/runs/test/events.jsonl");
        let mut snapshot = CompletionSnapshot::empty();
        snapshot.recovery_prompt_path = ".anvil/repairs/repair-long.md".to_string();
        snapshot.recovery_ultra_plan_path = ".anvil/plans/recovery-long.yaml".to_string();
        let long_arg = "x".repeat(700);
        snapshot.suggested_recovery_command = format!(
            "/ultra-plan-run --profile nextjs \"$(cat .anvil/repairs/repair-long.md)\" --note {long_arg}"
        );
        snapshot.suggested_recovery_yaml_command =
            format!("/run-ultra-plan .anvil/plans/recovery-long.yaml --note {long_arg}");
        let projection = project_completion(false, &snapshot);
        let long_reason = format!("failure {}", "y".repeat(20_000));
        let summary = render_completion_summary(
            "tui_command",
            None,
            Some("/ultra-plan-run"),
            &long_reason,
            "tui_command_failed",
            &projection,
        );

        write_run_summary(Some(&path), &summary);

        let written = std::fs::read_to_string(path.parent().unwrap().join("summary.md")).unwrap();
        let suggested_line = format!(
            "- Suggested command: {}",
            snapshot.suggested_recovery_command
        );
        let suggested_yaml_line = format!(
            "- Suggested YAML command: {}",
            snapshot.suggested_recovery_yaml_command
        );
        assert!(written.contains(&suggested_line), "{written}");
        assert!(written.contains(&suggested_yaml_line), "{written}");
        assert!(
            written
                .lines()
                .any(|line| line == suggested_line && line.starts_with("- Suggested command: /")),
            "{written}"
        );
        assert!(
            written.lines().any(|line| line == suggested_yaml_line
                && line.starts_with("- Suggested YAML command: /")),
            "{written}"
        );
        assert!(
            written.contains("Process: REPL exited cleanly (not task status)"),
            "{written}"
        );
        assert!(!written.contains(&"y".repeat(1_000)));
        assert!(written.contains("Stop reason: failure"));
    }

    #[test]
    fn completion_summary_renders_plan_adherence_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        emit(
            Some(&path),
            json!({
                "event": "ultra_final_acceptance",
                "runtime_acceptance_status": "pass",
                "final_acceptance_status": "pass",
                "release_gate_status": "pass",
                "plan_adherence_present": ["score"],
                "plan_adherence_missing": ["canvas", "pause"],
            }),
        );
        let snapshot = latest_completion_snapshot(Some(&path));
        let projection = project_completion(true, &snapshot);
        let summary = render_completion_summary(
            "tui_command",
            None,
            Some("/ultra-plan-run"),
            "completed",
            "",
            &projection,
        );

        assert!(summary.contains("Plan adherence:"), "{summary}");
        assert!(summary.contains("Present tokens:\n- score"), "{summary}");
        assert!(
            summary.contains("Missing tokens:\n- canvas\n- pause"),
            "{summary}"
        );
        assert!(summary.contains("Status: complete"), "{summary}");
    }

    #[test]
    fn completion_summary_renders_evidence_arbitration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        emit(
            Some(&path),
            json!({
                "event": "ultra_final_acceptance",
                "runtime_acceptance_status": "pass",
                "final_acceptance_status": "pass",
                "release_gate_status": "pass",
                "evidence_arbitration_summary": "behavioral (probe ok)",
                "state_dimensions_changed": ["player", "score"],
                "action_hooks": ["primary", "restart"],
                "surface_fit_summary": "canvas overflows viewport (right: 22px)",
                "surface_fit_guidance": "canvas overflows the viewport by 22px; consider responsive sizing",
                "text_entry_target": "textarea:data-anvil-action=input",
                "typed_token": "anvil-note",
                "token_echoed": true,
                "text_input_state_change": true,
            }),
        );
        let snapshot = latest_completion_snapshot(Some(&path));
        let projection = project_completion(true, &snapshot);
        let summary = render_completion_summary(
            "tui_command",
            None,
            Some("/ultra-plan-run"),
            "completed",
            "",
            &projection,
        );

        assert!(
            summary.contains("Evidence arbitration: behavioral (probe ok)"),
            "{summary}"
        );
        assert!(
            summary.contains("State dimensions changed: player, score"),
            "{summary}"
        );
        assert!(
            summary.contains("Action hooks: primary, restart"),
            "{summary}"
        );
        assert!(
            summary.contains("Surface fit: canvas overflows viewport (right: 22px)"),
            "{summary}"
        );
        assert!(
            summary.contains(
                "Surface fit guidance: canvas overflows the viewport by 22px; consider responsive sizing"
            ),
            "{summary}"
        );
        assert!(
            summary.contains("Text entry target: textarea:data-anvil-action=input"),
            "{summary}"
        );
        assert!(summary.contains("Typed token: anvil-note"), "{summary}");
        assert!(summary.contains("Token echoed: true"), "{summary}");
        assert!(
            summary.contains("Text input state change: true"),
            "{summary}"
        );
    }

    #[test]
    fn completion_summary_renders_restart_terminal_partial_guidance_without_probe_setup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        emit(
            Some(&path),
            json!({
                "event": "ultra_final_acceptance",
                "runtime_acceptance_status": "partial",
                "final_acceptance_status": "partial",
                "release_gate_status": "partial",
                "release_gate_reasons": ["interaction_unverified:terminal_state_not_reached"],
                "unverified_evidence": [
                    "restart_or_recoverable_state_evidence:unverified:terminal_state_not_reached"
                ],
            }),
        );
        let snapshot = latest_completion_snapshot(Some(&path));
        let projection = project_completion(true, &snapshot);
        let summary = render_completion_summary(
            "tui_command",
            None,
            Some("/ultra-plan-run"),
            "completed",
            "",
            &projection,
        );

        assert!(
            summary.contains(
                "Restart verification: either expose an in-play restart control, or accept the partial classification"
            ),
            "{summary}"
        );
        assert!(
            !summary.contains(
                crate::minimal_loop::interaction_probe::INTERACTION_PROBE_SETUP_REMEDIATION
            ),
            "{summary}"
        );
    }

    #[test]
    fn completion_summary_renders_probe_infrastructure_failure_as_untested() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        emit(
            Some(&path),
            json!({
                "event": "ultra_final_acceptance",
                "runtime_acceptance_status": "pass",
                "final_acceptance_status": "failed",
                "release_gate_status": "failed",
                "release_gate_reasons": [
                    "probe_dependency_missing:playwright_module_missing",
                    "app interaction untested (probe infrastructure failure: probe_dependency_missing:playwright_module_missing)"
                ],
                "evidence_arbitration_summary": "static (probe infrastructure failure: probe_dependency_missing:playwright_module_missing)",
            }),
        );
        let snapshot = latest_completion_snapshot(Some(&path));
        let projection = project_completion(false, &snapshot);
        let summary = render_completion_summary(
            "tui_command",
            None,
            Some("/ultra-plan-run"),
            "failed",
            "",
            &projection,
        );

        assert!(
            summary.contains(
                "Evidence arbitration: static (probe infrastructure failure: probe_dependency_missing:playwright_module_missing)"
            ),
            "{summary}"
        );
        assert!(
            summary.contains(
                "Interaction verification: app interaction untested (probe infrastructure failure)."
            ),
            "{summary}"
        );
    }

    #[test]
    fn terminal_summary_card_snapshot_full_projection() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        emit(
            Some(&path),
            json!({
                "event": "ultra_phase_start",
                "phase_id": "final",
                "phase_index": 2,
                "total_phases": 2,
            }),
        );
        emit(
            Some(&path),
            json!({
                "event": "browser_interaction_probe",
                "persistence_after_reload": "preserved",
            }),
        );
        emit(
            Some(&path),
            json!({
                "event": "ultra_final_acceptance",
                "profile": "nextjs",
                "effective_profile": "nextjs",
                "runtime_acceptance_status": "pass",
                "final_acceptance_status": "full_success",
                "release_gate_status": "pass",
                "assurance_level": "full",
                "completion_contract_verification_enabled": true,
                "external_contract_checked": true,
                "external_contract_ok": true,
                "browser_readiness_execution_status": "performed",
                "browser_readiness_status": "pass",
                "interaction_evidence_execution_status": "performed",
                "interaction_evidence_status": "pass",
            }),
        );

        let snapshot = latest_completion_snapshot(Some(&path));
        let projection = project_completion(true, &snapshot);
        let card = render_terminal_summary_card(Some(&path), "completed", &projection);

        assert!(card.contains("### Terminal summary"), "{card}");
        assert!(
            card.contains("- Status: complete · Assurance: full"),
            "{card}"
        );
        assert!(card.contains("| task_status | complete |"), "{card}");
        assert!(card.contains("| release_gate_status | pass |"), "{card}");
        assert!(
            card.contains("| final_acceptance_status | full_success |"),
            "{card}"
        );
        assert!(
            card.contains("| browser_readiness_status | execution=performed status=pass |"),
            "{card}"
        );
        assert!(
            card.contains("| interaction_evidence_status | execution=performed status=pass |"),
            "{card}"
        );
        assert!(
            card.contains("| persistence_after_reload | preserved |"),
            "{card}"
        );
        assert!(card.contains("- Phases completed: 1/2"), "{card}");
        assert!(card.contains("- Next action: none"), "{card}");
        assert!(card.lines().count() <= 25, "{card}");
    }

    #[test]
    fn terminal_summary_card_snapshot_partial_failed_and_interrupted_projections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".anvil/runs/018f7777-summary/events.jsonl");
        let recovery = ".anvil/plans/recovery-ultra-plan-日本語ディレクトリ-final-acceptance.yaml";
        emit(
            Some(&path),
            json!({
                "event": "resume_start",
                "resumed_from": "018f6666",
            }),
        );
        emit(
            Some(&path),
            json!({
                "event": "planner_verify_command_normalized",
            }),
        );
        emit(
            Some(&path),
            json!({
                "event": "tool_args_path_salvaged",
            }),
        );
        emit(
            Some(&path),
            json!({
                "event": "verify_command_substituted",
            }),
        );
        emit(
            Some(&path),
            json!({
                "event": "compile_rollback_applied",
                "paths": ["src/app/page.tsx"],
            }),
        );
        emit(
            Some(&path),
            json!({
                "event": "ultra_partial_artifact_summary",
                "completed_phase_ids": ["setup"],
                "failed_phase_id": "acceptance",
                "pending_phase_ids": ["release"],
            }),
        );
        emit(
            Some(&path),
            json!({
                "event": "ultra_final_acceptance",
                "runtime_acceptance_status": "pass",
                "final_acceptance_status": "partial",
                "release_gate_status": "partial",
                "release_gate_reasons": ["interaction_unverified:probe_unavailable"],
                "browser_readiness_execution_status": "performed",
                "browser_readiness_status": "pass",
                "interaction_evidence_execution_status": "disconnected",
                "interaction_evidence_status": "not_applicable",
                "persistence_after_reload": "not_evaluated",
                "persistence_after_reload_reason": "no_mutation_observed",
                "recovery_ultra_plan_path": recovery,
                "suggested_recovery_yaml_command": format!("/run-ultra-plan {recovery}"),
            }),
        );

        let snapshot = latest_completion_snapshot(Some(&path));
        let partial = project_completion(true, &snapshot);
        let partial_card = render_terminal_summary_card(Some(&path), "partial gate", &partial);
        assert!(
            partial_card.contains("| release_gate_status | partial |"),
            "{partial_card}"
        );
        assert!(
            partial_card.contains("| final_acceptance_status | partial |"),
            "{partial_card}"
        );
        assert!(
            partial_card
                .contains("| persistence_after_reload | not_evaluated (no_mutation_observed) |"),
            "{partial_card}"
        );
        assert!(
            partial_card.contains("Telemetry: normalized=1 salvaged=1 substituted=1 rollbacks=1"),
            "{partial_card}"
        );
        assert!(partial_card.contains(recovery), "{partial_card}");
        assert!(
            partial_card.contains("- resume: /resume 018f7777"),
            "{partial_card}"
        );
        assert!(
            partial_card.contains("- Resumed from: 018f6666"),
            "{partial_card}"
        );
        assert!(
            !partial_card.contains("recovery-ultra-plan-日本語ディレクトリ-final-acceptance.ya\n")
        );

        let failed = project_completion(false, &snapshot);
        let failed_card = render_terminal_summary_card(Some(&path), "failed release", &failed);
        assert!(
            failed_card.contains("- Status: incomplete"),
            "{failed_card}"
        );
        assert!(
            failed_card.contains("| task_status | failed |"),
            "{failed_card}"
        );
        assert!(
            failed_card.contains("- Next action: fix_command_failure"),
            "{failed_card}"
        );

        let mut interrupted = failed.clone();
        interrupted.status = "interrupted".to_string();
        interrupted.command_completion = "interrupted".to_string();
        interrupted.task_status = "interrupted".to_string();
        interrupted.next_action = "resume_or_rerun_command".to_string();
        let interrupted_card =
            render_terminal_summary_card(Some(&path), "interrupted by user", &interrupted);
        assert!(
            interrupted_card.contains("- Status: interrupted"),
            "{interrupted_card}"
        );
        assert!(
            interrupted_card.contains("| task_status | interrupted |"),
            "{interrupted_card}"
        );
        assert!(
            interrupted_card.contains("- Next action: resume_or_rerun_command"),
            "{interrupted_card}"
        );
        assert!(interrupted_card.lines().count() <= 25, "{interrupted_card}");
    }

    #[test]
    fn tui_failure_block_renders_phase_evidence_and_recovery_commands() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        let workspace = dir.path().join("workspace");
        let prompt_path = workspace.join(".anvil/repairs/repair-project-setup.md");
        let recovery_plan_path =
            workspace.join(".anvil/plans/recovery-ultra-plan-project-setup.yaml");
        emit(
            Some(&path),
            json!({
                "event": "completion_verify",
                "missing_evidence": [
                    "challenge_or_adversary_evidence",
                    "failure_or_collision_evidence",
                    "restart_or_recoverable_state_evidence"
                ],
            }),
        );
        emit(
            Some(&path),
            json!({
                "event": "ultra_partial_artifact_summary",
                "completed_phase_ids": ["scaffold"],
                "failed_phase_id": "project-setup",
                "pending_phase_ids": ["final-verify"],
                "recovery_prompt_path": prompt_path.display().to_string(),
                "recovery_ultra_plan_path": recovery_plan_path.display().to_string(),
                "suggested_recovery_command": format!(
                    "/ultra-plan-run --profile nextjs \"$(cat {})\"",
                    prompt_path.display()
                ),
                "suggested_recovery_yaml_command": format!(
                    "/run-ultra-plan {}",
                    recovery_plan_path.display()
                ),
            }),
        );
        let projection = project_completion(false, &CompletionSnapshot::empty());
        let block = render_tui_command_failure_block(
            Some(&path),
            "phase project-setup failed",
            &projection,
        );
        assert!(block.contains("TASK FAILED (process exited normally)"));
        assert!(block.contains("Task status: failed"));
        assert!(block.contains("Failed phase: project-setup"));
        assert!(block.contains("Phases completed: 1/3"));
        assert!(block.contains("Primary stop reason:"));
        assert!(block.contains("challenge_or_adversary_evidence"));
        assert!(block.contains("failure_or_collision_evidence"));
        assert!(block.contains("restart_or_recoverable_state_evidence"));
        assert!(block.contains("Recovery prompt command: /ultra-plan-run --profile nextjs"));
        assert!(block.contains("$(cat .anvil/repairs/repair-project-setup.md)"));
        assert!(block.contains(
            "Recovery UltraPlan command: /run-ultra-plan .anvil/plans/recovery-ultra-plan-project-setup.yaml"
        ));
        assert!(!block.contains(&workspace.display().to_string()), "{block}");
        assert!(block.contains("Run summary:"));
        assert!(block.contains("summary.md"));
    }
}
