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

pub(crate) fn is_profile_preset_plan(config: &Config, plan: &UltraPlan) -> bool {
    config.plan_preset == PlanPreset::Profile
        && profile_preset_ultra_plan(&config.profile, &plan.goal, &plan.style, &plan.intent)
            .is_some_and(|preset| preset == *plan)
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
    fn gemma_tier_does_not_use_profile_preset_or_emit_event() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let mut config = Config::from_cli(Cli::parse_from([
            "anvilminimal",
            "--cwd",
            &cwd,
            "--planner-model",
            "gemma4:31b-cloud",
            "--profile",
            "nextjs",
            "--ultra-plan",
            "Build a Next.js app",
        ]))
        .unwrap();
        let events_path = dir.path().join("events.jsonl");
        config.eval_events_path = Some(events_path.clone());

        let plan = maybe_preset_ultra_plan(&config, "Build a Next.js app", "create").unwrap();

        assert_eq!(config.plan_preset, PlanPreset::None);
        assert_eq!(config.field_sources.plan_preset, "default:gemma_planner");
        assert!(plan.is_none());
        let event_text = std::fs::read_to_string(events_path).unwrap_or_default();
        assert!(!event_text.contains("preset_ultra_plan_used"));
    }

    #[test]
    fn qwen27_tier_does_not_use_profile_preset_without_flag() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let mut config = Config::from_cli(Cli::parse_from([
            "anvilminimal",
            "--cwd",
            &cwd,
            "--planner-model",
            "qwen3.6:27b-coding-nvfp4",
            "--profile",
            "nextjs",
            "--ultra-plan",
            "Build a Next.js app",
        ]))
        .unwrap();
        let events_path = dir.path().join("events.jsonl");
        config.eval_events_path = Some(events_path.clone());

        let plan = maybe_preset_ultra_plan(&config, "Build a Next.js app", "create").unwrap();

        assert_eq!(config.plan_preset, PlanPreset::None);
        assert_eq!(config.plan_preset_origin(), "default");
        assert_eq!(config.field_sources.plan_preset, "default:qwen27_planner");
        assert!(plan.is_none());
        let event_text = std::fs::read_to_string(events_path).unwrap_or_default();
        assert!(!event_text.contains("preset_ultra_plan_used"));
    }

    #[test]
    fn explicit_profile_enables_qwen27_profile_preset() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let mut config = Config::from_cli(Cli::parse_from([
            "anvilminimal",
            "--cwd",
            &cwd,
            "--planner-model",
            "qwen3.6:27b-coding-nvfp4",
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

        assert_eq!(config.plan_preset, PlanPreset::Profile);
        assert_eq!(config.plan_preset_origin(), "cli");
        assert_eq!(config.field_sources.plan_preset, "flag");
        assert_eq!(plan.phases.len(), 4);
        assert!(is_profile_preset_plan(&config, &plan));
        let event_text = std::fs::read_to_string(events_path).unwrap();
        assert!(event_text.contains("\"event\":\"preset_ultra_plan_used\""));
    }

    #[test]
    fn explicit_none_remains_cli_sourced_for_qwen27() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let mut config = Config::from_cli(Cli::parse_from([
            "anvilminimal",
            "--cwd",
            &cwd,
            "--planner-model",
            "qwen3.6:27b-coding-nvfp4",
            "--profile",
            "nextjs",
            "--plan-preset",
            "none",
            "--ultra-plan",
            "Build a Next.js app",
        ]))
        .unwrap();
        let events_path = dir.path().join("events.jsonl");
        config.eval_events_path = Some(events_path.clone());

        let plan = maybe_preset_ultra_plan(&config, "Build a Next.js app", "create").unwrap();

        assert_eq!(config.plan_preset, PlanPreset::None);
        assert_eq!(config.plan_preset_origin(), "cli");
        assert_eq!(config.field_sources.plan_preset, "flag");
        assert!(plan.is_none());
        let event_text = std::fs::read_to_string(events_path).unwrap_or_default();
        assert!(!event_text.contains("preset_ultra_plan_used"));
    }
}
