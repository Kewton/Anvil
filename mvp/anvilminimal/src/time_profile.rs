use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderScopeTotals {
    pub planner_ultra_ms: u64,
    pub planner_step_ms: u64,
    pub executor_ms: u64,
    pub repair_ms: u64,
}

impl ProviderScopeTotals {
    pub fn total_ms(&self) -> u64 {
        self.planner_ultra_ms
            .saturating_add(self.planner_step_ms)
            .saturating_add(self.executor_ms)
            .saturating_add(self.repair_ms)
    }

    pub fn planner_ms(&self) -> u64 {
        self.planner_ultra_ms.saturating_add(self.planner_step_ms)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenTotals {
    pub estimated_prompt_tokens_sent: u64,
    pub prompt_eval_count: u64,
    pub eval_count: u64,
    pub prompt_eval_observed: bool,
    pub eval_observed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderDurationTotals {
    pub prompt_eval_duration: u64,
    pub eval_duration: u64,
    pub load_duration: u64,
    pub total_duration: u64,
    pub prompt_eval_observed: bool,
    pub eval_observed: bool,
    pub load_observed: bool,
    pub total_observed: bool,
}

impl ProviderDurationTotals {
    pub fn provider_total_duration(&self) -> u64 {
        self.total_duration.max(
            self.prompt_eval_duration
                .saturating_add(self.eval_duration)
                .saturating_add(self.load_duration),
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GenerationScopeTotals {
    pub eval_count: u64,
    pub eval_observed: bool,
    pub duration_ms: u64,
    pub turn_count: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GenerationTurnTypeTotals {
    pub eval_count: u64,
    pub eval_observed: bool,
    pub duration_ms: u64,
    pub turn_count: u64,
    pub write_bytes: u64,
    pub edit_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GenerationProfileTotals {
    pub scopes: BTreeMap<String, GenerationScopeTotals>,
    pub turn_types: BTreeMap<String, GenerationTurnTypeTotals>,
}

#[derive(Debug, Clone, Default)]
struct PendingGenerationTurn {
    scope: String,
    eval_count: Option<u64>,
    duration_ms: u64,
    saw_write: bool,
    saw_edit: bool,
    saw_tool_call: bool,
    write_bytes: u64,
    edit_bytes: u64,
}

impl PendingGenerationTurn {
    fn new(scope: String, eval_count: Option<u64>, duration_ms: u64) -> Self {
        Self {
            scope,
            eval_count,
            duration_ms,
            saw_write: false,
            saw_edit: false,
            saw_tool_call: false,
            write_bytes: 0,
            edit_bytes: 0,
        }
    }

    fn observe_tool_call(&mut self, event: &Value) {
        let Some(name) = event.get("name").and_then(Value::as_str) else {
            self.saw_tool_call = true;
            return;
        };
        match name {
            "Write" => {
                self.saw_write = true;
                self.write_bytes = self
                    .write_bytes
                    .saturating_add(argument_string_len(event, "content").unwrap_or(0));
            }
            "Edit" => {
                self.saw_edit = true;
                self.edit_bytes = self
                    .edit_bytes
                    .saturating_add(argument_string_len(event, "new_string").unwrap_or(0));
            }
            _ => {
                self.saw_tool_call = true;
            }
        }
    }

    fn turn_kind(&self) -> &'static str {
        if self.saw_write {
            "full-file Write"
        } else if self.saw_edit {
            "Edit"
        } else if self.saw_tool_call {
            "tool-call"
        } else {
            "prose-only"
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PhaseTimeProfile {
    pub phase: String,
    pub provider_ms: u64,
    pub installs_ms: u64,
    pub builds_ms: u64,
    pub probe_ms: u64,
    pub other_ms: u64,
}

impl PhaseTimeProfile {
    pub fn total_ms(&self) -> u64 {
        self.provider_ms
            .saturating_add(self.installs_ms)
            .saturating_add(self.builds_ms)
            .saturating_add(self.probe_ms)
            .saturating_add(self.other_ms)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimeProfile {
    pub provider: ProviderScopeTotals,
    pub provider_durations: ProviderDurationTotals,
    pub generation: GenerationProfileTotals,
    pub installs_ms: u64,
    pub builds_ms: u64,
    pub probe_ms: u64,
    pub other_ms: u64,
    pub tokens: TokenTotals,
    pub phases: Vec<PhaseTimeProfile>,
}

impl TimeProfile {
    pub fn total_ms(&self) -> u64 {
        self.provider
            .total_ms()
            .saturating_add(self.installs_ms)
            .saturating_add(self.builds_ms)
            .saturating_add(self.probe_ms)
            .saturating_add(self.other_ms)
    }

    pub fn to_event_json(&self) -> Value {
        json!({
            "total_ms": self.total_ms(),
            "provider_ms": self.provider.total_ms(),
            "planner_ultra_ms": self.provider.planner_ultra_ms,
            "planner_step_ms": self.provider.planner_step_ms,
            "planner_ms": self.provider.planner_ms(),
            "executor_ms": self.provider.executor_ms,
            "repair_ms": self.provider.repair_ms,
            "provider_prompt_eval_duration": if self.provider_durations.prompt_eval_observed {
                Value::from(self.provider_durations.prompt_eval_duration)
            } else {
                Value::Null
            },
            "provider_eval_duration": if self.provider_durations.eval_observed {
                Value::from(self.provider_durations.eval_duration)
            } else {
                Value::Null
            },
            "provider_load_duration": if self.provider_durations.load_observed {
                Value::from(self.provider_durations.load_duration)
            } else {
                Value::Null
            },
            "provider_total_duration": if self.provider_durations.total_observed {
                Value::from(self.provider_durations.total_duration)
            } else {
                Value::Null
            },
            "installs_ms": self.installs_ms,
            "builds_ms": self.builds_ms,
            "probe_ms": self.probe_ms,
            "other_ms": self.other_ms,
            "estimated_prompt_tokens_sent": self.tokens.estimated_prompt_tokens_sent,
            "prompt_eval_count": if self.tokens.prompt_eval_observed {
                Value::from(self.tokens.prompt_eval_count)
            } else {
                Value::Null
            },
            "eval_count": if self.tokens.eval_observed {
                Value::from(self.tokens.eval_count)
            } else {
                Value::Null
            },
        })
    }

    pub fn summary_line(&self) -> String {
        let total = self.total_ms();
        let provider = self.provider.total_ms();
        let provider_duration_total = self.provider_durations.provider_total_duration();
        let provider_breakdown = if provider_duration_total > 0 {
            format!(
                " [prefill {} · generation {} · load {}]",
                percent(
                    self.provider_durations.prompt_eval_duration,
                    provider_duration_total
                ),
                percent(
                    self.provider_durations.eval_duration,
                    provider_duration_total
                ),
                percent(
                    self.provider_durations.load_duration,
                    provider_duration_total
                ),
            )
        } else {
            String::new()
        };
        format!(
            "Time profile: provider {}{} · installs {} · builds {} · probe {} · total {}",
            percent(provider, total),
            provider_breakdown,
            percent(self.installs_ms, total),
            percent(self.builds_ms, total),
            percent(self.probe_ms, total),
            format_duration(total),
        )
    }

    pub fn phase_table_markdown(&self) -> String {
        let mut lines = vec![
            "Time profile by phase:".to_string(),
            "| Phase | Total | Provider | Installs | Builds | Probe | Other |".to_string(),
            "| --- | ---: | ---: | ---: | ---: | ---: | ---: |".to_string(),
        ];
        if self.phases.is_empty() {
            lines.push("| unscoped | 0s | 0s | 0s | 0s | 0s | 0s |".to_string());
            return lines.join("\n");
        }
        for phase in &self.phases {
            lines.push(format!(
                "| {} | {} | {} | {} | {} | {} | {} |",
                phase.phase,
                format_duration(phase.total_ms()),
                format_duration(phase.provider_ms),
                format_duration(phase.installs_ms),
                format_duration(phase.builds_ms),
                format_duration(phase.probe_ms),
                format_duration(phase.other_ms),
            ));
        }
        lines.join("\n")
    }

    pub fn generation_profile_markdown(&self) -> String {
        let mut lines = vec![
            "Generation profile (duration-weighted eval tokens):".to_string(),
            "| Caller scope | Eval tokens | Duration | Turns |".to_string(),
            "| --- | ---: | ---: | ---: |".to_string(),
        ];
        if self.generation.scopes.is_empty() {
            lines.push("| none | n/a | 0s | 0 |".to_string());
        } else {
            for (scope, totals) in &self.generation.scopes {
                lines.push(format!(
                    "| {} | {} | {} | {} |",
                    scope,
                    display_eval_count(totals.eval_observed, totals.eval_count),
                    format_duration(totals.duration_ms),
                    totals.turn_count,
                ));
            }
        }

        lines.push(String::new());
        lines.push(
            "| Turn type | Eval tokens | Duration | Turns | Write bytes | Edit bytes |".to_string(),
        );
        lines.push("| --- | ---: | ---: | ---: | ---: | ---: |".to_string());
        if self.generation.turn_types.is_empty() {
            lines.push("| none | n/a | 0s | 0 | 0B | 0B |".to_string());
        } else {
            for (turn_type, totals) in &self.generation.turn_types {
                lines.push(format!(
                    "| {} | {} | {} | {} | {} | {} |",
                    turn_type,
                    display_eval_count(totals.eval_observed, totals.eval_count),
                    format_duration(totals.duration_ms),
                    totals.turn_count,
                    format_bytes(totals.write_bytes),
                    format_bytes(totals.edit_bytes),
                ));
            }
        }
        lines.join("\n")
    }
}

pub fn aggregate_events(events: &[Value]) -> TimeProfile {
    let mut profile = TimeProfile::default();
    let mut phases: BTreeMap<String, PhaseTimeProfile> = BTreeMap::new();
    let mut current_phase = "unscoped".to_string();
    let mut current_acceptance_repair = false;
    let mut pending_generation_turn: Option<PendingGenerationTurn> = None;

    for event in events {
        let event_name = event.get("event").and_then(Value::as_str).unwrap_or("");
        if let Some(phase) = event_phase(event) {
            current_phase = phase;
        }
        if event_name == "final_acceptance_repair_start" {
            current_acceptance_repair = true;
        }
        if matches!(
            event_name,
            "final_acceptance_repair_complete"
                | "final_acceptance_repair_failed"
                | "final_acceptance_repair_exhausted"
        ) {
            current_acceptance_repair = false;
        }
        if event_name == "provider_turn_duration" {
            if let Some(turn) = pending_generation_turn.take() {
                finalize_generation_turn(&mut profile.generation, turn);
            }
            let scope = generation_scope_label(
                event
                    .get("caller_scope")
                    .and_then(Value::as_str)
                    .unwrap_or("executor"),
                current_acceptance_repair,
            );
            let eval_count = event.get("eval_count").and_then(Value::as_u64);
            let duration_ms = duration_field(event, "duration_ms").unwrap_or(0);
            pending_generation_turn = Some(PendingGenerationTurn::new(
                scope.to_string(),
                eval_count,
                duration_ms,
            ));
        } else if event_name == "tool_call_raw"
            && let Some(turn) = pending_generation_turn.as_mut()
        {
            turn.observe_tool_call(event);
        }
        match event_category_duration(event) {
            Some((TimeCategory::Provider(scope), duration)) => {
                match scope {
                    "planner_ultra" => {
                        profile.provider.planner_ultra_ms =
                            profile.provider.planner_ultra_ms.saturating_add(duration);
                    }
                    "planner_step" => {
                        profile.provider.planner_step_ms =
                            profile.provider.planner_step_ms.saturating_add(duration);
                    }
                    "repair" => {
                        profile.provider.repair_ms =
                            profile.provider.repair_ms.saturating_add(duration);
                    }
                    _ => {
                        profile.provider.executor_ms =
                            profile.provider.executor_ms.saturating_add(duration);
                    }
                }
                add_provider_duration_totals(&mut profile.provider_durations, event);
                let phase = phase_entry(&mut phases, &current_phase);
                phase.provider_ms = phase.provider_ms.saturating_add(duration);
                add_token_totals(&mut profile.tokens, event);
            }
            Some((TimeCategory::Installs, duration)) => {
                profile.installs_ms = profile.installs_ms.saturating_add(duration);
                let phase = phase_entry(&mut phases, &current_phase);
                phase.installs_ms = phase.installs_ms.saturating_add(duration);
            }
            Some((TimeCategory::Builds, duration)) => {
                profile.builds_ms = profile.builds_ms.saturating_add(duration);
                let phase = phase_entry(&mut phases, &current_phase);
                phase.builds_ms = phase.builds_ms.saturating_add(duration);
            }
            Some((TimeCategory::Probe, duration)) => {
                profile.probe_ms = profile.probe_ms.saturating_add(duration);
                let phase = phase_entry(&mut phases, &current_phase);
                phase.probe_ms = phase.probe_ms.saturating_add(duration);
            }
            Some((TimeCategory::Other, duration)) => {
                profile.other_ms = profile.other_ms.saturating_add(duration);
                let phase = phase_entry(&mut phases, &current_phase);
                phase.other_ms = phase.other_ms.saturating_add(duration);
            }
            None => {}
        }
    }

    if let Some(turn) = pending_generation_turn.take() {
        finalize_generation_turn(&mut profile.generation, turn);
    }

    profile.phases = phases
        .into_values()
        .filter(|phase| phase.total_ms() > 0)
        .collect();
    profile
}

pub fn aggregate_event_path(path: Option<&Path>) -> TimeProfile {
    let Some(path) = path else {
        return TimeProfile::default();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return TimeProfile::default();
    };
    let events = text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect::<Vec<_>>();
    aggregate_events(&events)
}

pub fn render_summary_sections(events: &[Value]) -> Option<String> {
    let profile = aggregate_events(events);
    (profile.total_ms() > 0).then(|| {
        format!(
            "{}\n\n{}\n\n{}",
            profile.summary_line(),
            profile.phase_table_markdown(),
            profile.generation_profile_markdown()
        )
    })
}

pub fn format_duration(ms: u64) -> String {
    let secs = (ms.saturating_add(999)) / 1_000;
    let minutes = secs / 60;
    let seconds = secs % 60;
    if minutes == 0 {
        format!("{seconds}s")
    } else {
        format!("{minutes}m{seconds:02}s")
    }
}

fn event_phase(event: &Value) -> Option<String> {
    event
        .get("phase_id")
        .or_else(|| event.get("failed_phase_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimeCategory<'a> {
    Provider(&'a str),
    Installs,
    Builds,
    Probe,
    Other,
}

fn event_category_duration(event: &Value) -> Option<(TimeCategory<'_>, u64)> {
    let name = event.get("event").and_then(Value::as_str).unwrap_or("");
    if name == "provider_turn_duration" {
        return Some((
            TimeCategory::Provider(
                event
                    .get("caller_scope")
                    .and_then(Value::as_str)
                    .unwrap_or("executor"),
            ),
            duration_field(event, "duration_ms")?,
        ));
    }
    if matches!(
        name,
        "browser_probe" | "browser_interaction_probe" | "profile_behavior_probe"
    ) {
        return Some((TimeCategory::Probe, any_duration(event)?));
    }
    if name == "dependency_build_lifecycle"
        && let Some(duration) = duration_field(event, "setup_duration_ms")
    {
        return Some((TimeCategory::Installs, duration));
    }
    let command = event
        .get("command")
        .or_else(|| event.get("setup_command"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    if command_mentions_install(&command)
        && let Some(duration) = any_duration(event)
    {
        return Some((TimeCategory::Installs, duration));
    }
    if command_mentions_build(&command)
        && let Some(duration) = any_duration(event)
    {
        return Some((TimeCategory::Builds, duration));
    }
    if matches!(
        name,
        "verify_command_timeout"
            | "profile_invariant_repair_build_verify"
            | "dev_server_lifecycle"
            | "step_wall_clock_exhausted"
    ) && let Some(duration) = any_duration(event)
    {
        return Some((TimeCategory::Other, duration));
    }
    None
}

fn command_mentions_install(command: &str) -> bool {
    command.contains("npm install")
        || command.contains("npm ci")
        || command.contains("pnpm install")
        || command.contains("yarn install")
        || command.contains("pip install")
}

fn command_mentions_build(command: &str) -> bool {
    command.contains("npm run build")
        || command.contains("pnpm build")
        || command.contains("yarn build")
        || command.contains("next build")
        || command.contains("cargo build")
}

fn any_duration(event: &Value) -> Option<u64> {
    duration_field(event, "duration_ms")
        .or_else(|| duration_field(event, "elapsed_ms"))
        .or_else(|| duration_field(event, "setup_duration_ms"))
}

fn duration_field(event: &Value, key: &str) -> Option<u64> {
    event.get(key).and_then(Value::as_u64)
}

fn argument_string_len(event: &Value, key: &str) -> Option<u64> {
    event
        .get("arguments")
        .and_then(|value| value.get("argument_summaries"))
        .and_then(|summaries| summaries.get(key))
        .and_then(|value| value.get("string_len"))
        .and_then(Value::as_u64)
}

fn generation_scope_label(scope: &str, current_acceptance_repair: bool) -> &'static str {
    if current_acceptance_repair && scope == "repair" {
        "acceptance-repair"
    } else if matches!(scope, "planner_ultra" | "planner_step") {
        "planner"
    } else if scope == "repair" {
        "repair"
    } else {
        "executor"
    }
}

fn finalize_generation_turn(generation: &mut GenerationProfileTotals, turn: PendingGenerationTurn) {
    let scope = generation.scopes.entry(turn.scope.clone()).or_default();
    scope.turn_count = scope.turn_count.saturating_add(1);
    scope.duration_ms = scope.duration_ms.saturating_add(turn.duration_ms);
    if let Some(eval_count) = turn.eval_count {
        scope.eval_observed = true;
        scope.eval_count = scope.eval_count.saturating_add(eval_count);
    }

    let turn_kind = turn.turn_kind().to_string();
    let bucket = generation.turn_types.entry(turn_kind).or_default();
    bucket.turn_count = bucket.turn_count.saturating_add(1);
    bucket.duration_ms = bucket.duration_ms.saturating_add(turn.duration_ms);
    if let Some(eval_count) = turn.eval_count {
        bucket.eval_observed = true;
        bucket.eval_count = bucket.eval_count.saturating_add(eval_count);
    }
    bucket.write_bytes = bucket.write_bytes.saturating_add(turn.write_bytes);
    bucket.edit_bytes = bucket.edit_bytes.saturating_add(turn.edit_bytes);
}

fn display_eval_count(observed: bool, value: u64) -> String {
    if observed {
        value.to_string()
    } else {
        "n/a".to_string()
    }
}

fn format_bytes(bytes: u64) -> String {
    format!("{bytes}B")
}

fn add_provider_duration_totals(totals: &mut ProviderDurationTotals, event: &Value) {
    if let Some(value) = event.get("prompt_eval_duration").and_then(Value::as_u64) {
        totals.prompt_eval_observed = true;
        totals.prompt_eval_duration = totals.prompt_eval_duration.saturating_add(value);
    }
    if let Some(value) = event.get("eval_duration").and_then(Value::as_u64) {
        totals.eval_observed = true;
        totals.eval_duration = totals.eval_duration.saturating_add(value);
    }
    if let Some(value) = event.get("load_duration").and_then(Value::as_u64) {
        totals.load_observed = true;
        totals.load_duration = totals.load_duration.saturating_add(value);
    }
    if let Some(value) = event.get("total_duration").and_then(Value::as_u64) {
        totals.total_observed = true;
        totals.total_duration = totals.total_duration.saturating_add(value);
    }
}

fn add_token_totals(totals: &mut TokenTotals, event: &Value) {
    if let Some(value) = event
        .get("estimated_prompt_tokens_sent")
        .and_then(Value::as_u64)
    {
        totals.estimated_prompt_tokens_sent =
            totals.estimated_prompt_tokens_sent.saturating_add(value);
    }
    if let Some(value) = event.get("prompt_eval_count").and_then(Value::as_u64) {
        totals.prompt_eval_observed = true;
        totals.prompt_eval_count = totals.prompt_eval_count.saturating_add(value);
    }
    if let Some(value) = event.get("eval_count").and_then(Value::as_u64) {
        totals.eval_observed = true;
        totals.eval_count = totals.eval_count.saturating_add(value);
    }
}

fn phase_entry<'a>(
    phases: &'a mut BTreeMap<String, PhaseTimeProfile>,
    phase: &str,
) -> &'a mut PhaseTimeProfile {
    phases
        .entry(phase.to_string())
        .or_insert_with(|| PhaseTimeProfile {
            phase: phase.to_string(),
            ..PhaseTimeProfile::default()
        })
}

fn percent(part: u64, total: u64) -> String {
    if total == 0 {
        "0%".to_string()
    } else {
        format!(
            "{}%",
            (part.saturating_mul(100).saturating_add(total / 2)) / total
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregates_time_profile_from_existing_event_stream() {
        let events = vec![
            json!({"event": "ultra_phase_start", "phase_id": "setup"}),
            json!({"event": "provider_turn_duration", "caller_scope": "planner_ultra", "duration_ms": 10_000, "estimated_prompt_tokens_sent": 1000, "prompt_eval_count": 800, "eval_count": 100, "prompt_eval_duration": 4_000_000_000u64, "eval_duration": 5_000_000_000u64, "load_duration": 1_000_000_000u64, "total_duration": 10_000_000_000u64}),
            json!({"event": "dependency_build_lifecycle", "setup_duration_ms": 20_000}),
            json!({"event": "ultra_phase_start", "phase_id": "play"}),
            json!({"event": "provider_turn_duration", "caller_scope": "executor", "duration_ms": 30_000, "estimated_prompt_tokens_sent": 2000, "prompt_eval_count": 1200, "eval_count": 200, "prompt_eval_duration": 6_000_000_000u64, "eval_duration": 20_000_000_000u64, "load_duration": 4_000_000_000u64, "total_duration": 30_000_000_000u64}),
            json!({"event": "browser_probe", "elapsed_ms": 5_000}),
            json!({"event": "browser_interaction_probe", "duration_ms": 7_000}),
        ];

        let profile = aggregate_events(&events);

        assert_eq!(profile.total_ms(), 72_000);
        assert_eq!(profile.provider.planner_ultra_ms, 10_000);
        assert_eq!(profile.provider.executor_ms, 30_000);
        assert_eq!(
            profile.provider_durations.prompt_eval_duration,
            10_000_000_000
        );
        assert_eq!(profile.provider_durations.eval_duration, 25_000_000_000);
        assert_eq!(profile.provider_durations.load_duration, 5_000_000_000);
        assert_eq!(profile.installs_ms, 20_000);
        assert_eq!(profile.probe_ms, 12_000);
        assert_eq!(profile.tokens.prompt_eval_count, 2_000);
        assert_eq!(profile.tokens.eval_count, 300);
        let summary = profile.summary_line();
        assert!(summary.contains("Time profile: provider 56%"));
        assert!(summary.contains("[prefill 25% · generation 63% · load 13%]"));
        let table = profile.phase_table_markdown();
        assert!(table.contains("| setup | 30s | 10s | 20s | 0s | 0s | 0s |"));
        assert!(table.contains("| play | 42s | 30s | 0s | 0s | 12s | 0s |"));
    }

    #[test]
    fn generation_profile_renders_scope_and_turn_type_breakdown() {
        let events = vec![
            json!({
                "event": "provider_turn_duration",
                "caller_scope": "planner_step",
                "duration_ms": 1_000,
                "eval_count": 10
            }),
            json!({
                "event": "provider_turn_duration",
                "caller_scope": "executor",
                "duration_ms": 2_000,
                "eval_count": 20
            }),
            json!({
                "event": "tool_call_raw",
                "name": "Write",
                "arguments": {
                    "argument_summaries": {
                        "content": {
                            "string_len": 128,
                            "preview": "<omitted>"
                        }
                    }
                }
            }),
            json!({
                "event": "provider_turn_duration",
                "caller_scope": "repair",
                "duration_ms": 3_000,
                "eval_count": 30
            }),
            json!({
                "event": "tool_call_raw",
                "name": "Edit",
                "arguments": {
                    "argument_summaries": {
                        "new_string": {
                            "string_len": 45,
                            "preview": "<omitted>"
                        }
                    }
                }
            }),
            json!({"event": "final_acceptance_repair_start"}),
            json!({
                "event": "provider_turn_duration",
                "caller_scope": "repair",
                "duration_ms": 4_000,
                "eval_count": 40
            }),
            json!({
                "event": "tool_call_raw",
                "name": "Bash",
                "arguments": {
                    "argument_summaries": {
                        "command": {
                            "string_len": 16,
                            "preview": "npm run build"
                        }
                    }
                }
            }),
            json!({"event": "final_acceptance_repair_complete"}),
        ];

        let profile = aggregate_events(&events);
        let generation = &profile.generation;
        assert_eq!(generation.scopes["planner"].eval_count, 10);
        assert_eq!(generation.scopes["planner"].turn_count, 1);
        assert_eq!(generation.scopes["executor"].eval_count, 20);
        assert_eq!(generation.scopes["executor"].turn_count, 1);
        assert_eq!(generation.scopes["repair"].eval_count, 30);
        assert_eq!(generation.scopes["repair"].turn_count, 1);
        assert_eq!(generation.scopes["acceptance-repair"].eval_count, 40);
        assert_eq!(generation.scopes["acceptance-repair"].turn_count, 1);

        assert_eq!(generation.turn_types["prose-only"].turn_count, 1);
        assert_eq!(generation.turn_types["full-file Write"].turn_count, 1);
        assert_eq!(generation.turn_types["full-file Write"].write_bytes, 128);
        assert_eq!(generation.turn_types["Edit"].turn_count, 1);
        assert_eq!(generation.turn_types["Edit"].edit_bytes, 45);
        assert_eq!(generation.turn_types["tool-call"].turn_count, 1);

        let block = profile.generation_profile_markdown();
        assert!(block.contains("Generation profile (duration-weighted eval tokens):"));
        assert!(block.contains("| planner | 10 | 1s | 1 |"));
        assert!(block.contains("| acceptance-repair | 40 | 4s | 1 |"));
        assert!(block.contains("| full-file Write | 20 | 2s | 1 | 128B | 0B |"));
        assert!(block.contains("| Edit | 30 | 3s | 1 | 0B | 45B |"));
        assert!(block.contains("| tool-call | 40 | 4s | 1 | 0B | 0B |"));
    }

    #[test]
    fn missing_provider_token_fields_degrade_gracefully() {
        let events = vec![json!({
            "event": "provider_turn_duration",
            "caller_scope": "repair",
            "duration_ms": 1_500
        })];

        let profile = aggregate_events(&events);

        assert_eq!(profile.total_ms(), 1_500);
        assert!(!profile.tokens.prompt_eval_observed);
        assert!(!profile.summary_line().contains("[prefill"));
        let block = profile.generation_profile_markdown();
        assert!(block.contains("| repair | n/a | 2s | 1 |"), "{block}");
        assert!(
            block.contains("| prose-only | n/a | 2s | 1 | 0B | 0B |"),
            "{block}"
        );
    }
}
