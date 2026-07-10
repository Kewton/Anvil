use serde_json::json;

use crate::config::{Config, PlanPreset};
use crate::eval_events;
use crate::planner::lint::lint_ultra_plan_report;
use crate::planner::profile::profile_preset_ultra_plan;
use crate::planner::ultra_plan::UltraPlan;

pub(crate) fn maybe_preset_ultra_plan(
    config: &Config,
    goal: &str,
    intent: &str,
) -> anyhow::Result<Option<UltraPlan>> {
    if config.plan_preset != PlanPreset::Profile {
        return Ok(None);
    }
    let Some(plan) = profile_preset_ultra_plan(&config.profile, goal, &config.style, intent) else {
        return Ok(None);
    };
    let report = lint_ultra_plan_report(&plan);
    if !report.is_pass() {
        anyhow::bail!("preset UltraPlan failed lint: {}", report.primary_message());
    }
    emit_preset_ultra_plan_used(config, &plan);
    Ok(Some(plan))
}

fn emit_preset_ultra_plan_used(config: &Config, plan: &UltraPlan) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "preset_ultra_plan_used",
            "profile": plan.profile.as_str(),
            "template_id": template_id(plan),
            "phase_count": plan.phases.len(),
            "planner_skipped": true,
        }),
    );
}

fn template_id(plan: &UltraPlan) -> String {
    format!("{}-{}-{}-ultra", plan.profile, plan.intent, plan.style)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;

    #[test]
    fn opt_in_absent_does_not_use_profile_preset_or_emit_event() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let mut config = Config::from_cli(Cli::parse_from([
            "anvilminimal",
            "--cwd",
            &cwd,
            "--profile",
            "nextjs",
            "--ultra-plan",
            "Build a Next.js app",
        ]))
        .unwrap();
        let events_path = dir.path().join("events.jsonl");
        config.eval_events_path = Some(events_path.clone());

        let plan = maybe_preset_ultra_plan(&config, "Build a Next.js app", "create").unwrap();

        assert!(plan.is_none());
        let event_text = std::fs::read_to_string(events_path).unwrap_or_default();
        assert!(!event_text.contains("preset_ultra_plan_used"));
    }

    #[test]
    fn profile_preset_emits_event_and_returns_plan() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let mut config = Config::from_cli(Cli::parse_from([
            "anvilminimal",
            "--cwd",
            &cwd,
            "--profile",
            "nextjs",
            "--plan-preset",
            "profile",
            "--ultra-plan",
            "Build a Next.js app",
        ]))
        .unwrap();
        let events_path = dir.path().join("events.jsonl");
        config.eval_events_path = Some(events_path.clone());

        let plan = maybe_preset_ultra_plan(&config, "Build a Next.js app", "create")
            .unwrap()
            .unwrap();

        assert_eq!(plan.phases.len(), 4);
        let event_text = std::fs::read_to_string(events_path).unwrap();
        assert!(event_text.contains("\"event\":\"preset_ultra_plan_used\""));
        assert!(event_text.contains("\"profile\":\"nextjs\""));
        assert!(event_text.contains("\"template_id\":\"nextjs-create-default-ultra\""));
        assert!(event_text.contains("\"planner_skipped\":true"));
    }
}
