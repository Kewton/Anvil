use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

const SNIPPET_LIMIT: usize = 500;
const SUMMARY_LIMIT: usize = 8_000;

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
    let content = summary_body(text);
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
        format!("{content}\n")
    } else {
        format!("{}\n---\n\n{content}\n", existing.trim_end())
    };
    if let Err(err) = std::fs::write(summary, combined) {
        eprintln!("warning: failed to append run summary: {err}");
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionSnapshot {
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
    pub browser_readiness_status: String,
    pub browser_readiness_evidence_path: String,
    pub interaction_evidence_status: String,
    pub interaction_evidence_path: String,
    pub recovery_prompt_path: String,
    pub recovery_ultra_plan_path: String,
    pub suggested_recovery_command: String,
    pub suggested_recovery_yaml_command: String,
    pub planner_verify_normalization_count: usize,
    pub planner_retry_count: usize,
    pub planner_quality_warning_count: usize,
    pub planner_quality_issue_count: usize,
    pub planner_repaired: bool,
    pub planner_release_risk: bool,
}

impl CompletionSnapshot {
    pub fn empty() -> Self {
        Self {
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
            browser_readiness_status: "not_applicable".to_string(),
            browser_readiness_evidence_path: String::new(),
            interaction_evidence_status: "not_applicable".to_string(),
            interaction_evidence_path: String::new(),
            recovery_prompt_path: String::new(),
            recovery_ultra_plan_path: String::new(),
            suggested_recovery_command: String::new(),
            suggested_recovery_yaml_command: String::new(),
            planner_verify_normalization_count: 0,
            planner_retry_count: 0,
            planner_quality_warning_count: 0,
            planner_quality_issue_count: 0,
            planner_repaired: false,
            planner_release_risk: false,
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
    pub browser_readiness: String,
    pub browser_readiness_evidence_path: String,
    pub interaction_evidence: String,
    pub interaction_evidence_path: String,
    pub release_quality_completion: String,
    pub next_action: String,
    pub recovery_prompt_path: String,
    pub recovery_ultra_plan_path: String,
    pub suggested_recovery_command: String,
    pub suggested_recovery_yaml_command: String,
    pub planner_verify_normalization_count: usize,
    pub planner_retry_count: usize,
    pub planner_quality_warning_count: usize,
    pub planner_quality_issue_count: usize,
    pub planner_repaired: bool,
    pub planner_release_risk: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct PlannerDiagnostics {
    verify_normalization_count: usize,
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
    events
        .iter()
        .filter_map(snapshot_from_completion_event)
        .last()
        .map(|mut snapshot| {
            snapshot.planner_verify_normalization_count = diagnostics.verify_normalization_count;
            snapshot.planner_retry_count = diagnostics.retry_count;
            snapshot.planner_quality_warning_count = diagnostics.quality_warning_count;
            snapshot.planner_quality_issue_count = diagnostics.quality_issue_count;
            snapshot.planner_repaired = diagnostics.repaired();
            snapshot.planner_release_risk = diagnostics.release_risk();
            snapshot
        })
        .unwrap_or_else(CompletionSnapshot::empty)
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
    let task_status = task_status(ok, &release_gate, &final_acceptance);
    let next_action = next_action(ok, &release_gate, &final_acceptance);
    CompletionProjection {
        status,
        command_completion,
        task_status,
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
        browser_readiness: snapshot.browser_readiness_status.clone(),
        browser_readiness_evidence_path: snapshot.browser_readiness_evidence_path.clone(),
        interaction_evidence: snapshot.interaction_evidence_status.clone(),
        interaction_evidence_path: snapshot.interaction_evidence_path.clone(),
        release_quality_completion,
        next_action,
        recovery_prompt_path: snapshot.recovery_prompt_path.clone(),
        recovery_ultra_plan_path: snapshot.recovery_ultra_plan_path.clone(),
        suggested_recovery_command: snapshot.suggested_recovery_command.clone(),
        suggested_recovery_yaml_command: snapshot.suggested_recovery_yaml_command.clone(),
        planner_verify_normalization_count: snapshot.planner_verify_normalization_count,
        planner_retry_count: snapshot.planner_retry_count,
        planner_quality_warning_count: snapshot.planner_quality_warning_count,
        planner_quality_issue_count: snapshot.planner_quality_issue_count,
        planner_repaired: snapshot.planner_repaired,
        planner_release_risk: snapshot.planner_release_risk,
    }
}

fn planner_diagnostics_from_events(events: &[Value]) -> PlannerDiagnostics {
    let mut diagnostics = PlannerDiagnostics::default();
    for event in events {
        match event.get("event").and_then(Value::as_str).unwrap_or("") {
            "planner_verify_command_normalized" => {
                diagnostics.verify_normalization_count += 1;
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
        release_gate_reasons: event
            .get("release_gate_reasons")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
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
        recovery_prompt_path: event
            .get("recovery_prompt_path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        recovery_ultra_plan_path: event
            .get("recovery_ultra_plan_path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        suggested_recovery_command: event
            .get("suggested_recovery_command")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        suggested_recovery_yaml_command: event
            .get("suggested_recovery_yaml_command")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
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
    })
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
        format!("Session/REPL status: {}", session_status(lifecycle_stage)),
    ];
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
        format!("Runtime acceptance: {}", projection.runtime_acceptance),
        format!("Final acceptance: {}", projection.final_acceptance),
        format!("Release gate: {}", projection.release_gate),
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
        format!("Next action: {}", projection.next_action),
        format!("Recovery next action: {}", projection.next_action),
        format!("Stop reason: {stop_reason}"),
    ]);
    if !projection.recovery_prompt_path.is_empty()
        || !projection.recovery_ultra_plan_path.is_empty()
        || !projection.suggested_recovery_command.is_empty()
        || !projection.suggested_recovery_yaml_command.is_empty()
    {
        lines.extend([
            "Recovery handoff:".to_string(),
            format!(
                "- Recovery prompt: {}",
                missing_if_empty(&projection.recovery_prompt_path)
            ),
            format!(
                "- Recovery UltraPlan YAML: {}",
                missing_if_empty(&projection.recovery_ultra_plan_path)
            ),
            format!(
                "- Suggested prompt command: {}",
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
        lines.push(format!("TUI command failed: {stop_reason}"));
    }
    lines.join("\n")
}

fn session_status(lifecycle_stage: &str) -> &'static str {
    match lifecycle_stage {
        "tui_command" => "repl_ready",
        "process" => "process_exited",
        _ => "unknown",
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

fn summary_body(body: &str) -> String {
    let clean = body.replace("\r\n", "\n").replace('\r', "\n");
    let clean = redact_home_paths(&clean);
    clean
        .lines()
        .map(redact_secret_like)
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .take(SUMMARY_LIMIT)
        .collect()
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
    fn default_run_events_path_uses_anvil_runs_events_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let path = default_run_events_path(dir.path());
        assert!(path.starts_with(dir.path().join(".anvil").join("runs")));
        assert_eq!(path.file_name().unwrap(), "events.jsonl");
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
}
