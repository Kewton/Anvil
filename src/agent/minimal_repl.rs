use std::io::{self, BufRead, IsTerminal, Write};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::model_registry::RuntimeModels;
use crate::ollama::client::OllamaClient;
use crate::session::store::{SessionSnapshot, SessionStore};

use super::minimal_loop::{
    MinimalChatClient, MinimalLoopConfig, completion_without_write_feedback_disabled_from_env,
    requested_artifact_feedback_disabled_from_env, run_session,
};
use super::minimal_step_runner;

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReplInput {
    Empty,
    Exit,
    Invalid(String),
    PlanSteps(String),
    PlanRun(String),
    RunPlan(String),
    UltraPlan {
        profile: minimal_step_runner::UltraProfile,
        style: minimal_step_runner::UltraPlanStyle,
        goal: String,
    },
    UltraPlanRun {
        profile: minimal_step_runner::UltraProfile,
        style: minimal_step_runner::UltraPlanStyle,
        goal: String,
    },
    RunUltraPlan(String),
    Prompt(String),
}

pub fn run(
    config: Config,
    models: RuntimeModels,
    mut client: OllamaClient,
    session_store: SessionStore,
    mut session: SessionSnapshot,
) -> Result<(), String> {
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let mut line = String::new();

    loop {
        print!("anvil> ");
        io::stdout()
            .flush()
            .map_err(|err| format!("failed to flush REPL prompt: {err}"))?;

        line.clear();
        let bytes = stdin
            .read_line(&mut line)
            .map_err(|err| format!("failed to read REPL input: {err}"))?;
        if bytes == 0 {
            break;
        }

        match parse_repl_input(&line) {
            ReplInput::Empty => continue,
            ReplInput::Exit => break,
            ReplInput::Invalid(err) => {
                eprintln!("ERROR: {err}");
            }
            ReplInput::PlanSteps(goal) => {
                let result = {
                    let _spinner = ReplSpinner::start("minimal planning");
                    minimal_step_runner::generate_step_plan(
                        &config,
                        &models.main,
                        &mut client,
                        &goal,
                    )
                };
                match result {
                    Ok(path) => println!("created step plan: {}", path.display()),
                    Err(err) => eprintln!("ERROR: {err}"),
                }
            }
            ReplInput::PlanRun(goal) => {
                let result = {
                    let _spinner = ReplSpinner::start("minimal plan-run");
                    minimal_step_runner::generate_and_run_step_plan(
                        &config,
                        &models.main,
                        &mut client,
                        &session_store,
                        &mut session,
                        &goal,
                    )
                };
                match result {
                    Ok(summary) => {
                        println!("created step plan: {}", summary.plan_path.display());
                        println!(
                            "completed {}/{} plan steps",
                            summary.steps.completed, summary.steps.total
                        );
                    }
                    Err(err) => eprintln!("ERROR: {err}"),
                }
            }
            ReplInput::UltraPlan {
                profile,
                style,
                goal,
            } => {
                let result = {
                    let _spinner = ReplSpinner::start("minimal ultra planning");
                    minimal_step_runner::generate_ultra_plan(
                        &config,
                        &models.main,
                        &mut client,
                        &goal,
                        profile,
                        style,
                    )
                };
                match result {
                    Ok(path) => println!("created ultra plan: {}", path.display()),
                    Err(err) => eprintln!("ERROR: {err}"),
                }
            }
            ReplInput::UltraPlanRun {
                profile,
                style,
                goal,
            } => {
                let result = {
                    let _spinner = ReplSpinner::start("minimal ultra plan-run");
                    minimal_step_runner::generate_and_run_ultra_plan(
                        &config,
                        &models.main,
                        &mut client,
                        &session_store,
                        &mut session,
                        &goal,
                        profile,
                        style,
                    )
                };
                match result {
                    Ok(summary) => {
                        println!("created ultra plan: {}", summary.plan_path.display());
                        println!(
                            "completed {}/{} ultra phases",
                            summary.phases.completed, summary.phases.total
                        );
                    }
                    Err(err) => eprintln!("ERROR: {err}"),
                }
            }
            ReplInput::RunUltraPlan(path) => {
                let path = std::path::PathBuf::from(path);
                match minimal_step_runner::run_ultra_plan(
                    &config,
                    &models.main,
                    &mut client,
                    &session_store,
                    &mut session,
                    &path,
                ) {
                    Ok(summary) => {
                        println!(
                            "completed {}/{} ultra phases",
                            summary.completed, summary.total
                        )
                    }
                    Err(err) => eprintln!("ERROR: {err}"),
                }
            }
            ReplInput::RunPlan(path) => {
                let path = std::path::PathBuf::from(path);
                match minimal_step_runner::run_plan(
                    &config,
                    &models.main,
                    &mut client,
                    &session_store,
                    &mut session,
                    &path,
                ) {
                    Ok(summary) => {
                        println!(
                            "completed {}/{} plan steps",
                            summary.completed, summary.total
                        )
                    }
                    Err(err) => eprintln!("ERROR: {err}"),
                }
            }
            ReplInput::Prompt(prompt) => {
                let result = {
                    let _spinner = ReplSpinner::start("minimal running");
                    run_turn(
                        &config,
                        &models.main,
                        &mut client,
                        &session_store,
                        &mut session,
                        &prompt,
                    )
                };
                let reply = match result {
                    Ok(reply) => reply,
                    Err(err) => {
                        eprintln!("ERROR: {err}");
                        continue;
                    }
                };
                if !reply.is_empty() {
                    println!("{reply}");
                }
            }
        }
    }

    Ok(())
}

struct ReplSpinner {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl ReplSpinner {
    fn start(label: &'static str) -> Self {
        let disabled = std::env::var_os("ANVIL_NO_SPINNER").is_some_and(|value| !value.is_empty());
        if disabled || !io::stderr().is_terminal() {
            return Self {
                stop: Arc::new(AtomicBool::new(true)),
                handle: None,
            };
        }

        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = stop.clone();
        let handle = thread::Builder::new()
            .name("anvil-minimal-repl-spinner".into())
            .spawn(move || {
                let frames = ["|", "/", "-", "\\"];
                let start = Instant::now();
                let mut index = 0usize;
                while !stop_for_thread.load(Ordering::SeqCst) {
                    let elapsed = start.elapsed().as_secs();
                    eprint!(
                        "\r{} {} ({}s)",
                        frames[index % frames.len()],
                        label,
                        elapsed
                    );
                    let _ = io::stderr().flush();
                    index = index.wrapping_add(1);
                    thread::sleep(Duration::from_millis(120));
                }
                eprint!("\r\x1b[2K");
                let _ = io::stderr().flush();
            })
            .ok();

        Self { stop, handle }
    }
}

impl Drop for ReplSpinner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub(crate) fn run_turn<C: MinimalChatClient>(
    config: &Config,
    model: &str,
    client: &mut C,
    session_store: &SessionStore,
    session: &mut SessionSnapshot,
    prompt: &str,
) -> Result<String, String> {
    let work_root = session
        .active_root
        .clone()
        .unwrap_or_else(|| config.cwd.clone());
    let loop_config = MinimalLoopConfig {
        work_root: work_root.clone(),
        mode: session.mode_state.mode,
        context_budget: config.context_budget,
        max_iterations: config.max_iterations,
        auto_approve: config.yes_mode,
        offline: config.offline,
        cancel_flag: None,
        completion_without_write_feedback: !completion_without_write_feedback_disabled_from_env(),
        requested_artifact_feedback: !requested_artifact_feedback_disabled_from_env(),
    };
    let reply = run_session(client, model, session, prompt, &loop_config)?;
    session.active_root = (work_root != config.cwd).then_some(work_root);
    session_store.save(session)?;
    Ok(reply)
}

fn parse_repl_input(input: &str) -> ReplInput {
    let line = input.trim_end_matches(['\r', '\n']);
    match line.trim() {
        "" => ReplInput::Empty,
        "/exit" | "/quit" => ReplInput::Exit,
        value if value.starts_with("/plan-steps ") => {
            ReplInput::PlanSteps(value["/plan-steps ".len()..].trim().to_string())
        }
        value if value.starts_with("/plan-run ") => {
            ReplInput::PlanRun(value["/plan-run ".len()..].trim().to_string())
        }
        value if value.starts_with("/run-plan ") => {
            ReplInput::RunPlan(value["/run-plan ".len()..].trim().to_string())
        }
        value if value.starts_with("/ultra-plan ") => {
            parse_ultra_goal_command(&value["/ultra-plan ".len()..], false)
        }
        value if value.starts_with("/ultra-plan-run ") => {
            parse_ultra_goal_command(&value["/ultra-plan-run ".len()..], true)
        }
        value if value.starts_with("/run-ultra-plan ") => {
            ReplInput::RunUltraPlan(value["/run-ultra-plan ".len()..].trim().to_string())
        }
        _ => ReplInput::Prompt(line.to_string()),
    }
}

fn parse_ultra_goal_command(raw: &str, run: bool) -> ReplInput {
    let mut rest = raw.trim();
    let mut style = minimal_step_runner::UltraPlanStyle::Default;
    let mut profile = minimal_step_runner::UltraProfile::Generic;
    loop {
        if let Some(after_flag) = rest.strip_prefix("--style ") {
            let mut parts = after_flag.splitn(2, char::is_whitespace);
            let Some(style_value) = parts.next() else {
                return ReplInput::Invalid("--style requires a value".to_string());
            };
            style = match style_value.parse() {
                Ok(style) => style,
                Err(err) => return ReplInput::Invalid(err),
            };
            rest = parts.next().unwrap_or("").trim();
            continue;
        }
        if let Some(after_flag) = rest.strip_prefix("--profile ") {
            let mut parts = after_flag.splitn(2, char::is_whitespace);
            let Some(profile_value) = parts.next() else {
                return ReplInput::Invalid("--profile requires a value".to_string());
            };
            profile = match profile_value.parse() {
                Ok(profile) => profile,
                Err(err) => return ReplInput::Invalid(err),
            };
            rest = parts.next().unwrap_or("").trim();
            continue;
        }
        break;
    }
    if rest.is_empty() {
        return ReplInput::Invalid("/ultra-plan requires a goal".to_string());
    }
    if run {
        ReplInput::UltraPlanRun {
            profile,
            style,
            goal: rest.to_string(),
        }
    } else {
        ReplInput::UltraPlan {
            profile,
            style,
            goal: rest.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::path::PathBuf;

    use crate::modes::plan_act::ExecutionMode;
    use crate::ollama::client::AssistantReply;
    use crate::session::store::ConversationMessage;
    use crate::tools::registry::ToolSpec;

    use super::*;

    #[derive(Default)]
    struct MockClient {
        replies: VecDeque<AssistantReply>,
        prompts_seen: Vec<String>,
    }

    impl MockClient {
        fn push_reply(&mut self, content: &str) {
            self.replies.push_back(AssistantReply {
                content: content.to_string(),
                tool_calls: Vec::new(),
                prompt_tokens: None,
                completion_tokens: None,
            });
        }
    }

    impl MinimalChatClient for MockClient {
        fn chat(
            &mut self,
            _model: &str,
            messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _native_tools_enabled: bool,
        ) -> Result<AssistantReply, String> {
            self.prompts_seen.extend(
                messages
                    .iter()
                    .filter(|message| message.role == "user")
                    .map(|message| message.content.clone()),
            );
            self.replies
                .pop_front()
                .ok_or_else(|| "no reply".to_string())
        }
    }

    fn config(cwd: PathBuf) -> Config {
        let mut config = Config::default();
        config.cwd = cwd;
        config.context_budget = 24_000;
        config.max_iterations = 2;
        config.yes_mode = true;
        config
    }

    #[test]
    fn parse_repl_input_skips_empty_and_exits() {
        assert_eq!(parse_repl_input("\n"), ReplInput::Empty);
        assert_eq!(parse_repl_input("   \n"), ReplInput::Empty);
        assert_eq!(parse_repl_input("/exit\n"), ReplInput::Exit);
        assert_eq!(parse_repl_input(" /quit \r\n"), ReplInput::Exit);
    }

    #[test]
    fn parse_repl_input_parses_step_runner_commands() {
        assert_eq!(
            parse_repl_input("/plan-steps build a game\n"),
            ReplInput::PlanSteps("build a game".to_string())
        );
        assert_eq!(
            parse_repl_input("/plan-run build a game\n"),
            ReplInput::PlanRun("build a game".to_string())
        );
        assert_eq!(
            parse_repl_input("/run-plan .anvil/plans/plan.yaml\n"),
            ReplInput::RunPlan(".anvil/plans/plan.yaml".to_string())
        );
        assert_eq!(
            parse_repl_input("/ultra-plan build a game\n"),
            ReplInput::UltraPlan {
                profile: minimal_step_runner::UltraProfile::Generic,
                style: minimal_step_runner::UltraPlanStyle::Default,
                goal: "build a game".to_string()
            }
        );
        assert_eq!(
            parse_repl_input("/ultra-plan-run --profile data-analysis --style tdd build a game\n"),
            ReplInput::UltraPlanRun {
                profile: minimal_step_runner::UltraProfile::DataAnalysis,
                style: minimal_step_runner::UltraPlanStyle::Tdd,
                goal: "build a game".to_string()
            }
        );
        assert_eq!(
            parse_repl_input("/ultra-plan-run --style test-hardening --profile python add tests\n"),
            ReplInput::UltraPlanRun {
                profile: minimal_step_runner::UltraProfile::Python,
                style: minimal_step_runner::UltraPlanStyle::TestHardening,
                goal: "add tests".to_string()
            }
        );
        assert_eq!(
            parse_repl_input("/run-ultra-plan .anvil/plans/ultra.yaml\n"),
            ReplInput::RunUltraPlan(".anvil/plans/ultra.yaml".to_string())
        );
    }

    #[test]
    fn parse_repl_input_preserves_prompt_text() {
        assert_eq!(
            parse_repl_input("  write hello  \n"),
            ReplInput::Prompt("  write hello  ".to_string())
        );
    }

    #[test]
    fn run_turn_invokes_minimal_session_and_saves_snapshot() {
        let workspace = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = SessionStore::new(state.path(), "session-1", "workspace-1");
        let mut session = SessionSnapshot::default();
        session.id = "session-1".to_string();
        session.workspace_key = "workspace-1".to_string();
        session.mode_state.mode = ExecutionMode::Plan;
        let mut client = MockClient::default();
        client.push_reply("first reply");

        let reply = run_turn(
            &config(workspace.path().to_path_buf()),
            "qwen3:8b",
            &mut client,
            &store,
            &mut session,
            "first prompt",
        )
        .unwrap();

        assert_eq!(reply, "first reply");
        assert_eq!(client.prompts_seen, vec!["first prompt"]);
        let saved = std::fs::read_to_string(store.path()).unwrap();
        assert!(saved.contains("first prompt"));
        assert!(saved.contains("first reply"));
    }
}
