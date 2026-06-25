use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::minimal_loop::loop_run::run_session_with_required_paths_with_ui;
use crate::planner::intent::detect_intent;
use crate::planner::lint::{lint_step_plan, lint_ultra_plan};
use crate::planner::profile::{
    profile_auto_repair, profile_expected_paths, profile_guidance, profile_repair_prompt,
    verify_profile,
};
use crate::planner::repair::{build_repair_prompt, save_repair_report};
use crate::planner::step_plan::{PlanStep, StepPlan, parse_step_plan, render_step_plan};
use crate::planner::ultra_plan::{UltraPlan, parse_ultra_plan, render_ultra_plan};
use crate::planner::verify::{VerificationReport, VerifyStatus, verify_step};
use crate::providers::{ChatClient, model_for};
use crate::state::SessionSnapshot;
use crate::tui::status::UiStatus;
use crate::tui::{InteractionUi, NOOP_UI};

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
    let mut prompt = format!(
        "Create a YAML StepPlan for this goal. Return only YAML with goal and steps: {goal}"
    );
    if let Some(guidance) = profile_guidance(&config.profile, goal) {
        prompt.push_str("\n\nProfile contract:\n");
        prompt.push_str(&guidance);
        prompt.push_str(
            "\nInclude expected_paths on the final step so deterministic verification can catch missing artifacts.",
        );
    }
    let messages = vec![crate::state::ConversationMessage::user(prompt)];
    let model = model_for(config, true);
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
    let mut plan = parse_step_plan(&reply.content).unwrap_or_else(|_| StepPlan::single(goal));
    strengthen_step_plan_for_profile(&mut plan, config);
    Ok(plan)
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
    lint_step_plan(plan)?;
    let mut session = SessionSnapshot::new();
    for step in &plan.steps {
        if ui.interrupted() {
            anyhow::bail!("interrupted by user");
        }
        run_step(client, &mut session, step, config, ui)?;
    }
    Ok(format!("plan-run complete: {} steps", plan.steps.len()))
}

fn run_step(
    client: &mut dyn ChatClient,
    session: &mut SessionSnapshot,
    step: &PlanStep,
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<()> {
    run_session_with_required_paths_with_ui(
        client,
        session,
        &step.instruction,
        &step.expected_paths,
        config,
        ui,
    )?;
    let report = verify_step(&config.workspace_root, step);
    if report.is_pass() {
        return Ok(());
    }
    let repair_prompt = build_repair_prompt(&step.id, &report);
    run_session_with_required_paths_with_ui(
        client,
        session,
        &repair_prompt,
        &step.expected_paths,
        config,
        ui,
    )?;
    let retry = verify_step(&config.workspace_root, step);
    if retry.is_pass() {
        return Ok(());
    }
    save_repair_report(&config.workspace_root, &step.id, &retry)?;
    anyhow::bail!("step {} failed verification: {:?}", step.id, retry.status)
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
    let messages = vec![crate::state::ConversationMessage::user(format!(
        "Create a YAML UltraPlan for profile {} and goal: {goal}",
        config.profile
    ))];
    let model = model_for(config, true);
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
    parse_ultra_plan(&reply.content).or_else(|_| {
        Ok(UltraPlan::deterministic(
            goal,
            &config.profile,
            &config.style,
            detect_intent(goal),
        ))
    })
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
    lint_ultra_plan(plan)?;
    for (index, phase) in plan.phases.iter().enumerate() {
        if ui.interrupted() {
            anyhow::bail!("interrupted by user");
        }
        let step_plan = generate_step_plan_with_ui(planner, &phase.prompt, config, ui)?;
        save_step_plan(&config.workspace_root, &step_plan)?;
        run_step_plan_with_ui(execution, &step_plan, config, ui)
            .map_err(|err| anyhow::anyhow!("phase {} failed: {err}", phase.id))?;
        let profile_report = verify_profile(&config.workspace_root, &plan.profile, &plan.goal);
        let final_phase = index + 1 == plan.phases.len();
        if !profile_report.is_pass()
            && (final_phase
                || !matches!(
                    profile_report.status,
                    VerifyStatus::ProfileContractFailed(_)
                ))
        {
            if final_phase
                && profile_auto_repair(
                    &config.workspace_root,
                    &plan.profile,
                    &plan.goal,
                    &profile_report,
                )?
            {
                let retry = verify_profile(&config.workspace_root, &plan.profile, &plan.goal);
                if retry.is_pass() {
                    continue;
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
                let mut repair_session = SessionSnapshot::new();
                run_session_with_required_paths_with_ui(
                    execution,
                    &mut repair_session,
                    &repair_prompt,
                    &expected_paths,
                    config,
                    ui,
                )
                .map_err(|err| {
                    anyhow::anyhow!("phase {} profile repair failed: {err}", phase.id)
                })?;
                let retry = verify_profile(&config.workspace_root, &plan.profile, &plan.goal);
                if retry.is_pass() {
                    continue;
                }
                return Err(anyhow::anyhow!(
                    "phase {} profile verification failed after repair: {:?}",
                    phase.id,
                    retry.status
                ));
            }
            return Err(anyhow::anyhow!(
                "phase {} profile verification failed: {:?}",
                phase.id,
                profile_report.status
            ));
        }
    }
    Ok(format!(
        "ultra-plan-run complete: {} phases",
        plan.phases.len()
    ))
}

fn strengthen_step_plan_for_profile(plan: &mut StepPlan, config: &Config) {
    let Some(last) = plan.steps.last_mut() else {
        return;
    };
    if plan.goal.to_ascii_lowercase().contains("scaffold") {
        for path in profile_expected_paths(&config.workspace_root, &config.profile, &plan.goal) {
            if path.ends_with("package.json") && !last.expected_paths.contains(&path) {
                last.expected_paths.push(path);
            }
        }
    }
    if let Some(guidance) = profile_guidance(&config.profile, &plan.goal) {
        last.instruction = format!("{}\n\nProfile contract:\n{}", last.instruction, guidance);
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
    fn plan_artifact_saved() {
        let dir = tempfile::tempdir().unwrap();
        let plan = StepPlan::single("goal");
        let path = save_step_plan(dir.path(), &plan).unwrap();
        assert!(path.exists());
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
    fn ultra_plan_final_profile_failure_runs_repair() {
        let dir = tempfile::tempdir().unwrap();
        let step_yaml = render_step_plan(&StepPlan::single("Scaffold project"));
        let mut planner = FakeClient::new(vec![
            AssistantReply::text(step_yaml.clone()),
            AssistantReply::text(step_yaml),
        ]);
        let package = r#"{"dependencies":{"next":"x","react":"x","react-dom":"x"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#;
        let mut execution = FakeClient::new(vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![crate::state::ToolCall::new(
                    "Write",
                    serde_json::json!({"path":"package.json","content":package}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("done"),
            AssistantReply::text("done"),
            AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/page.tsx","content":"export default function Page(){return <main>Space Invaders</main>;}"}),
                    ),
                    crate::state::ToolCall::new(
                        "Write",
                        serde_json::json!({"path":"src/app/layout.tsx","content":"export default function Layout({children}:{children:React.ReactNode}){return <html><body>{children}</body></html>;}"}),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply::text("repaired"),
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
    }

    struct FakeClient {
        replies: Vec<AssistantReply>,
    }

    impl FakeClient {
        fn new(replies: Vec<AssistantReply>) -> Self {
            Self { replies }
        }
    }

    impl ChatClient for FakeClient {
        fn label(&self) -> &str {
            "fake"
        }

        fn chat(
            &mut self,
            _model: &str,
            _messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _native_tools_enabled: bool,
        ) -> anyhow::Result<AssistantReply> {
            if self.replies.is_empty() {
                anyhow::bail!("fake client exhausted")
            }
            Ok(self.replies.remove(0))
        }
    }

    fn config(root: PathBuf) -> Config {
        Config {
            workspace_root: root,
            state_dir: PathBuf::from("state"),
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
