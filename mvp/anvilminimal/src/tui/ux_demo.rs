use std::io::{self, IsTerminal};
use std::thread;
use std::time::Duration;

use serde_json::json;

use crate::config::{Config, NarrationMode};
use crate::eval_events::{CompletionSnapshot, project_completion, render_terminal_summary_card};
use crate::planner::step_plan::{PlanStep, StepPlan};
use crate::planner::ultra_plan::{UltraPhase, UltraPlan};
use crate::tui::OutputRenderer;
use crate::tui::footer::{Footer, build_live_footer_lines};
use crate::tui::status::UiStatus;
use crate::tui::status_bus::{self, ProviderTurnStatus, RuntimeStatus, StatusTime};

pub fn run(config: &Config) -> anyhow::Result<()> {
    print!(
        "{}",
        crate::tui::banner::render_startup_banner(
            config,
            crate::tui::banner::BannerStyle::Legacy4Line,
        )
    );
    let mut demo_config = config.clone();
    demo_config.narration = NarrationMode::Normal;
    let _presentation = crate::tui::presentation::install(&demo_config);
    let footer = Footer::start(&demo_config);
    let plan = demo_plan(&demo_config);
    let step_plan = demo_step_plan();

    crate::tui::presentation::emit_ultra_plan_card(
        &plan,
        &crate::tui::presentation::PlanProgress::default(),
    );
    emit_demo_event(json!({
        "event": "ultra_phase_start",
        "phase_id": "scaffold",
        "phase_index": 1,
        "total_phases": 2,
    }));
    crate::tui::presentation::emit_step_plan_block("scaffold", &step_plan, None);
    emit_demo_event(json!({
        "event": "tool_call_raw",
        "name": "Write",
        "arguments": {"argument_summaries": {"path": {"preview": "src/app/page.tsx"}}},
    }));
    emit_demo_event(json!({"event": "tool_execute", "name": "Write", "status": "ok"}));
    emit_demo_event(json!({
        "event": "step_verify_failure",
        "primary_reason": "missing path browser-readiness.json",
    }));
    emit_demo_event(json!({
        "event": "step_verify_repair",
        "repair_attempt": 1,
        "max_repair_turns": 2,
        "repair_target": "browser evidence",
    }));

    let fast = std::env::var_os("ANVIL_UX_DEMO_FAST").is_some();
    let total_secs = if fast { 2 } else { 20 };
    let interrupt_at = if fast { 1 } else { 10 };
    status_bus::publish_provider_started("planner_step", 600);
    for second in 0..=total_secs {
        if second == interrupt_at {
            status_bus::publish_interrupt_requested();
        }
        if (0..=2).contains(&second) || second == interrupt_at {
            render_demo_markdown(&footer_snapshot(
                &demo_config,
                second,
                second >= interrupt_at,
            ))?;
        }
        if second < total_secs {
            thread::sleep(if fast {
                Duration::from_millis(10)
            } else {
                Duration::from_secs(1)
            });
        }
    }
    status_bus::publish_provider_finished(Duration::from_secs(total_secs));
    drop(footer);

    render_demo_markdown(&demo_summary_card())?;
    Ok(())
}

pub fn render_scripted_demo_text(config: &Config) -> String {
    let plan = demo_plan(config);
    let step_plan = demo_step_plan();
    let mut out = crate::tui::banner::render_startup_banner(
        config,
        crate::tui::banner::BannerStyle::Legacy4Line,
    );
    out.push_str(&crate::tui::presentation::render_ultra_plan_card(
        &plan,
        &crate::tui::presentation::PlanProgress::default(),
    ));
    out.push('\n');
    if let Some(line) = crate::tui::presentation::render_activity_line(
        &json!({"event": "ultra_phase_start", "phase_id": "scaffold", "phase_index": 1, "total_phases": 2}),
    ) {
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str(&crate::tui::presentation::render_step_plan_block(
        "scaffold", &step_plan, None,
    ));
    out.push('\n');
    for event in [
        json!({"event": "tool_call_raw", "name": "Write", "arguments": {"argument_summaries": {"path": {"preview": "src/app/page.tsx"}}}}),
        json!({"event": "tool_execute", "name": "Write", "status": "ok"}),
        json!({"event": "step_verify_failure", "primary_reason": "missing path browser-readiness.json"}),
        json!({"event": "step_verify_repair", "repair_attempt": 1, "max_repair_turns": 2, "repair_target": "browser evidence"}),
    ] {
        if let Some(line) = crate::tui::presentation::render_activity_line(&event) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out.push_str(&footer_snapshot(config, 2, false));
    out.push('\n');
    out.push_str(&footer_snapshot(config, 10, true));
    out.push('\n');
    out.push_str(&demo_summary_card());
    out
}

fn demo_plan(config: &Config) -> UltraPlan {
    UltraPlan {
        goal: "UX demo: build a tiny web app and recover from an interruption".to_string(),
        profile: config.profile.clone(),
        style: config.style.clone(),
        intent: "demo".to_string(),
        phases: vec![
            UltraPhase {
                id: "scaffold".to_string(),
                prompt: "Create the app shell and first route".to_string(),
            },
            UltraPhase {
                id: "verify".to_string(),
                prompt: "Run browser checks, repair evidence, and summarize recovery".to_string(),
            },
        ],
    }
}

fn demo_step_plan() -> StepPlan {
    StepPlan {
        goal: "Create the app shell".to_string(),
        steps: vec![PlanStep {
            id: "write-page".to_string(),
            kind: "implement".to_string(),
            instruction: "Write src/app/page.tsx and supporting styles".to_string(),
            expected_paths: vec!["src/app/page.tsx".to_string()],
            verify: vec!["test -f src/app/page.tsx".to_string()],
            expected_result: "pass".to_string(),
        }],
    }
}

fn emit_demo_event(event: serde_json::Value) {
    crate::eval_events::emit(None, event);
}

fn footer_snapshot(config: &Config, elapsed_secs: u64, interrupted: bool) -> String {
    let runtime = RuntimeStatus {
        provider: Some(ProviderTurnStatus {
            scope: "planner_step".to_string(),
            deadline_secs: 600,
            started_at: StatusTime::ZERO,
        }),
        interrupt_requested: interrupted,
        ..RuntimeStatus::default()
    };
    build_live_footer_lines(
        &UiStatus::from_config(config),
        &runtime,
        StatusTime::from_secs(elapsed_secs),
        120,
        false,
    )
    .join("\n")
}

fn demo_summary_card() -> String {
    let mut snapshot = CompletionSnapshot::empty();
    snapshot.profile = "generic".to_string();
    snapshot.effective_profile = "generic".to_string();
    snapshot.runtime_acceptance_status = "partial".to_string();
    snapshot.final_acceptance_status = "interrupted".to_string();
    snapshot.release_gate_status = "partial".to_string();
    snapshot.assurance_level = "partial".to_string();
    snapshot.assurance_reason = "demo interrupted during provider turn".to_string();
    snapshot.recovery_prompt_path = ".anvil/repairs/ux-demo-recovery.md".to_string();
    snapshot.recovery_ultra_plan_path = ".anvil/plans/ux-demo-recovery.yaml".to_string();
    snapshot.suggested_recovery_yaml_command =
        "/run-ultra-plan .anvil/plans/ux-demo-recovery.yaml".to_string();
    let mut projection = project_completion(false, &snapshot);
    projection.status = "interrupted".to_string();
    projection.command_completion = "interrupted".to_string();
    projection.task_status = "interrupted".to_string();
    projection.next_action = "resume_or_rerun_command".to_string();
    render_terminal_summary_card(
        None,
        "interrupted by user during demo provider turn",
        &projection,
    )
}

fn render_demo_markdown(text: &str) -> anyhow::Result<()> {
    if io::stdout().is_terminal() {
        let renderer = crate::tui::markdown::TerminalMarkdownRenderer::for_stdout();
        renderer.render_assistant(text)
    } else {
        crate::tui::markdown::PlainRenderer.render_assistant(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Action, ConfigFieldSources, Provider};

    fn config() -> Config {
        Config {
            workspace_root: std::path::PathBuf::from("/tmp/anvilminimal"),
            state_dir: std::path::PathBuf::from("state"),
            eval_events_path: None,
            completion_contract_path: None,
            yes: true,
            offline: true,
            context_budget: 65_536,
            model: "m".to_string(),
            provider: Provider::Ollama,
            planner_model: "pm".to_string(),
            planner_provider: Provider::Ollama,
            ollama_host: "http://localhost:11434".to_string(),
            num_predict: 100,
            max_iterations: 4,
            chat_timeout_secs: 600,
            chat_timeout_source: "default:local_provider".to_string(),
            field_sources: ConfigFieldSources::default(),
            chat_retries: 1,
            resume: None,
            fresh_session: false,
            no_footer: false,
            narration: NarrationMode::Normal,
            profile: "generic".to_string(),
            profile_explicit: false,
            profile_inference: None,
            style: "default".to_string(),
            action: Action::UxDemo,
        }
    }

    #[test]
    fn scripted_demo_contains_full_visual_journey() {
        let text = render_scripted_demo_text(&config());

        for needle in [
            "anvilminimal",
            "### Plan",
            "── Phase 1/2: scaffold ──",
            "#### Phase: scaffold",
            "→ Write src/app/page.tsx",
            "✓ Write ok",
            "✗ verify missing path browser-readiness.json",
            "↻ repair 1/2: browser evidence",
            "provider 2s/600s (planner_step)",
            "[stopping: aborting current operation…]",
            "### Terminal summary",
            "- Status: interrupted",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
        }
    }
}
