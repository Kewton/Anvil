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
        let mut line = format!(
            "Time profile: provider {} [planner {} / executor {} / repair {}] · installs {} · builds {} · probe {} · total {}",
            percent(provider, total),
            percent(self.provider.planner_ms(), total),
            percent(self.provider.executor_ms, total),
            percent(self.provider.repair_ms, total),
            percent(self.installs_ms, total),
            percent(self.builds_ms, total),
            percent(self.probe_ms, total),
            format_duration(total),
        );
        if self.tokens.prompt_eval_observed || self.tokens.eval_observed {
            line.push_str(&format!(
                " · tokens prompt_eval={} eval={}",
                observed_u64(
                    self.tokens.prompt_eval_observed,
                    self.tokens.prompt_eval_count
                ),
                observed_u64(self.tokens.eval_observed, self.tokens.eval_count),
            ));
        }
        line
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
}

pub fn aggregate_events(events: &[Value]) -> TimeProfile {
    let mut profile = TimeProfile::default();
    let mut phases: BTreeMap<String, PhaseTimeProfile> = BTreeMap::new();
    let mut current_phase = "unscoped".to_string();

    for event in events {
        if let Some(phase) = event_phase(event) {
            current_phase = phase;
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
            "{}\n\n{}",
            profile.summary_line(),
            profile.phase_table_markdown()
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

fn observed_u64(observed: bool, value: u64) -> String {
    if observed {
        value.to_string()
    } else {
        "n/a".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregates_time_profile_from_existing_event_stream() {
        let events = vec![
            json!({"event": "ultra_phase_start", "phase_id": "setup"}),
            json!({"event": "provider_turn_duration", "caller_scope": "planner_ultra", "duration_ms": 10_000, "estimated_prompt_tokens_sent": 1000, "prompt_eval_count": 800, "eval_count": 100}),
            json!({"event": "dependency_build_lifecycle", "setup_duration_ms": 20_000}),
            json!({"event": "ultra_phase_start", "phase_id": "play"}),
            json!({"event": "provider_turn_duration", "caller_scope": "executor", "duration_ms": 30_000, "estimated_prompt_tokens_sent": 2000, "prompt_eval_count": 1200, "eval_count": 200}),
            json!({"event": "browser_probe", "elapsed_ms": 5_000}),
            json!({"event": "browser_interaction_probe", "duration_ms": 7_000}),
        ];

        let profile = aggregate_events(&events);

        assert_eq!(profile.total_ms(), 72_000);
        assert_eq!(profile.provider.planner_ultra_ms, 10_000);
        assert_eq!(profile.provider.executor_ms, 30_000);
        assert_eq!(profile.installs_ms, 20_000);
        assert_eq!(profile.probe_ms, 12_000);
        assert_eq!(profile.tokens.prompt_eval_count, 2_000);
        assert_eq!(profile.tokens.eval_count, 300);
        let summary = profile.summary_line();
        assert!(summary.contains("Time profile: provider 56%"));
        assert!(summary.contains("tokens prompt_eval=2000 eval=300"));
        let table = profile.phase_table_markdown();
        assert!(table.contains("| setup | 30s | 10s | 20s | 0s | 0s | 0s |"));
        assert!(table.contains("| play | 42s | 30s | 0s | 0s | 12s | 0s |"));
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
        assert!(!profile.summary_line().contains("tokens prompt_eval"));
    }
}
