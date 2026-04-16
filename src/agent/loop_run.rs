use std::io::{self, IsTerminal, Write};
use std::path::Path;

use crate::agent::{parallel, recovery};
use crate::config::Config;
use crate::format_model_banner;
use crate::git::checkpoint::GitCheckpointManager;
use crate::mcp::client::McpRegistry;
use crate::model_registry::RuntimeModels;
use crate::modes::plan_act::ExecutionMode;
use crate::ollama::client::{AssistantReply, OllamaClient};
use crate::session::compact::{
    approximate_token_count, compact_messages, compact_messages_with_strategy,
};
use crate::session::store::{ConversationMessage, SessionSnapshot, SessionStore};
use crate::skills::loader::SkillLibrary;
use crate::stdin_prompt;
use crate::system_prompt::build_system_prompt;
use crate::testloop::auto_test::AutoTestRunner;
use crate::tools::registry::{ToolContext, ToolRegistry};
use crate::watch::file_watcher::FileWatcher;

pub enum AgentEvent {
    Continue(Option<String>),
    Exit,
}

pub struct Agent {
    config: Config,
    models: RuntimeModels,
    client: OllamaClient,
    session_store: SessionStore,
    session: SessionSnapshot,
    tool_registry: ToolRegistry,
    checkpoint_manager: GitCheckpointManager,
    watcher: Option<FileWatcher>,
    auto_test: AutoTestRunner,
    skills: SkillLibrary,
    mcp: McpRegistry,
}

impl Agent {
    pub fn new(
        config: Config,
        models: RuntimeModels,
        client: OllamaClient,
        session_store: SessionStore,
        session: SessionSnapshot,
    ) -> Self {
        let checkpoints = session.checkpoints.clone();
        let checkpoint_manager = GitCheckpointManager::new(config.cwd.clone(), checkpoints);
        let watcher = if config.watch {
            FileWatcher::new(config.cwd.clone()).ok()
        } else {
            None
        };
        let auto_test = AutoTestRunner::new(config.auto_test_command.clone());
        let skills = SkillLibrary::new(config.cwd.join(".anvil").join("skills"));
        let mcp = McpRegistry::new(config.cwd.join(".anvil").join("mcp.json"));
        Self {
            config,
            models,
            client,
            session_store,
            session,
            tool_registry: ToolRegistry::default(),
            checkpoint_manager,
            watcher,
            auto_test,
            skills,
            mcp,
        }
    }

    pub fn initial_prompt_from_cli_or_stdin(&self) -> Result<Option<String>, String> {
        if let Some(prompt) = &self.config.prompt {
            return Ok(Some(prompt.clone()));
        }
        if self.config.oneshot {
            return stdin_prompt();
        }
        stdin_prompt()
    }

    pub fn run_oneshot(&mut self, prompt: &str) -> Result<String, String> {
        match self.process_line(prompt, false)? {
            AgentEvent::Continue(Some(message)) => Ok(message),
            AgentEvent::Continue(None) => Ok(String::new()),
            AgentEvent::Exit => Ok(String::new()),
        }
    }

    pub fn run_repl(&mut self) -> Result<(), String> {
        println!("anvil {}", env!("CARGO_PKG_VERSION"));
        println!("{}", format_model_banner(&self.models));
        println!(
            "mode={:?} cwd={}",
            self.session.mode_state.mode,
            self.config.cwd.display()
        );

        let mut line = String::new();
        loop {
            if let Some(background) = self.poll_background_tasks()? {
                println!("{background}");
            }
            print!("anvil> ");
            io::stdout()
                .flush()
                .map_err(|err| format!("failed to flush stdout: {err}"))?;
            line.clear();
            let bytes = io::stdin()
                .read_line(&mut line)
                .map_err(|err| format!("failed to read line: {err}"))?;
            if bytes == 0 {
                break;
            }
            match self.process_line(line.trim(), self.config.stream)? {
                AgentEvent::Continue(Some(message)) => println!("{message}"),
                AgentEvent::Continue(None) => {}
                AgentEvent::Exit => break,
            }
        }
        Ok(())
    }

    pub fn process_line(&mut self, input: &str, stream_output: bool) -> Result<AgentEvent, String> {
        if input.trim().is_empty() {
            return Ok(AgentEvent::Continue(None));
        }

        if input.starts_with('/') {
            let outcome = self.handle_command(input)?;
            self.persist_session()?;
            return Ok(outcome);
        }

        let reply = self.handle_user_message(input, stream_output)?;
        self.persist_session()?;
        if stream_output {
            Ok(AgentEvent::Continue(None))
        } else {
            Ok(AgentEvent::Continue(Some(reply)))
        }
    }

    fn handle_command(&mut self, input: &str) -> Result<AgentEvent, String> {
        let (command, rest) = input.split_once(' ').unwrap_or((input, ""));
        match command {
            "/help" => Ok(AgentEvent::Continue(Some(
                "/help /status /model /plan /approve /compact /checkpoint /rollback /yes /no /watch /autotest /skills /skill <name> /mcp /parallel t1 || t2 /exit".to_string(),
            ))),
            "/status" => Ok(AgentEvent::Continue(Some(format!(
                "mode={:?} auto_approve={} watch={} auto_test={} session={} plan={} approx_tokens={}",
                self.session.mode_state.mode,
                self.config.yes_mode,
                self.watcher.is_some(),
                self.auto_test.command().unwrap_or("-"),
                self.session_store.path().display(),
                self.session
                    .mode_state
                    .active_plan_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "-".to_string()),
                approximate_token_count(&self.session.messages),
            )))),
            "/model" => Ok(AgentEvent::Continue(Some(format_model_banner(&self.models)))),
            "/yes" => {
                self.config.yes_mode = true;
                Ok(AgentEvent::Continue(Some("auto-approve enabled".to_string())))
            }
            "/no" => {
                self.config.yes_mode = false;
                Ok(AgentEvent::Continue(Some("auto-approve disabled".to_string())))
            }
            "/plan" => {
                if self.session.mode_state.mode == ExecutionMode::Plan {
                    let current = self
                        .session
                        .mode_state
                        .active_plan_path
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "-".to_string());
                    return Ok(AgentEvent::Continue(Some(format!(
                        "already in plan mode: {current}"
                    ))));
                }
                let plan_path = self.session.mode_state.enter_plan(self.config.plan_dir())?;
                self.ensure_plan_file(&plan_path)?;
                self.push_system_note(format!(
                    "[Plan Mode] Explore with Read, Glob, and Grep. Write the plan to {}. Wait for /approve before making code changes.",
                    plan_path.display()
                ));
                Ok(AgentEvent::Continue(Some(format!(
                    "plan mode: {}",
                    plan_path.display()
                ))))
            }
            "/approve" | "/act" => {
                if self.session.mode_state.mode != ExecutionMode::Plan {
                    return Err("approve is only available from plan mode".to_string());
                }
                let plan_contents = self
                    .current_plan_contents()?
                    .ok_or_else(|| "plan file is missing".to_string())?;
                if !plan_is_substantive(&plan_contents) {
                    return Err("plan file is empty or still template-only".to_string());
                }
                self.session.mode_state.approve();
                if self.checkpoint_manager.is_git_repo() {
                    let _ = self
                        .checkpoint_manager
                        .create_checkpoint("anvil-plan-to-act");
                }
                self.push_system_note(format!(
                    "[Act Mode] Implement the following plan step by step.\n\n{}",
                    plan_contents
                ));
                Ok(AgentEvent::Continue(Some("act mode".to_string())))
            }
            "/compact" => {
                let changed = self.maybe_compact_session(20);
                Ok(AgentEvent::Continue(Some(if changed {
                    "session compacted".to_string()
                } else {
                    "session already compact".to_string()
                })))
            }
            "/checkpoint" => {
                let label = if rest.trim().is_empty() {
                    "anvil-manual-checkpoint"
                } else {
                    rest.trim()
                };
                let message = match self.checkpoint_manager.create_checkpoint(label)? {
                    Some(sha) => format!("checkpoint saved: {sha}"),
                    None => "no local changes to checkpoint".to_string(),
                };
                self.session.checkpoints = self.checkpoint_manager.checkpoints.clone();
                Ok(AgentEvent::Continue(Some(message)))
            }
            "/rollback" => {
                let sha = self.checkpoint_manager.rollback_latest()?;
                self.session.checkpoints = self.checkpoint_manager.checkpoints.clone();
                Ok(AgentEvent::Continue(Some(format!(
                    "rolled back to checkpoint {sha}"
                ))))
            }
            "/watch" => {
                if self.watcher.is_some() {
                    self.watcher = None;
                    Ok(AgentEvent::Continue(Some("file watcher disabled".to_string())))
                } else {
                    self.watcher = Some(FileWatcher::new(self.config.cwd.clone())?);
                    Ok(AgentEvent::Continue(Some("file watcher enabled".to_string())))
                }
            }
            "/autotest" => {
                let trimmed = rest.trim();
                if trimmed.is_empty() {
                    if self.auto_test.is_enabled() {
                        self.auto_test.set_command(None);
                        Ok(AgentEvent::Continue(Some("auto-test disabled".to_string())))
                    } else {
                        Err("usage: /autotest <command>".to_string())
                    }
                } else {
                    self.auto_test.set_command(Some(trimmed.to_string()));
                    Ok(AgentEvent::Continue(Some(format!(
                        "auto-test command set: {trimmed}"
                    ))))
                }
            }
            "/skills" => {
                let skills = self.skills.list()?;
                let message = if skills.is_empty() {
                    format!("no skills found in {}", self.skills.root().display())
                } else {
                    format!("skills: {}", skills.join(", "))
                };
                Ok(AgentEvent::Continue(Some(message)))
            }
            "/skill" => {
                let name = rest.trim();
                if name.is_empty() {
                    return Err("usage: /skill <name>".to_string());
                }
                let skill = self.skills.load(name)?;
                self.push_system_note(format!(
                    "[Skill:{}]\n{}",
                    skill.name,
                    skill.content.chars().take(8_000).collect::<String>()
                ));
                Ok(AgentEvent::Continue(Some(format!(
                    "loaded skill: {}",
                    skill.path.display()
                ))))
            }
            "/mcp" => Ok(AgentEvent::Continue(Some(
                self.mcp.status_lines()?.join("\n"),
            ))),
            "/parallel" => {
                let tasks = rest
                    .split("||")
                    .map(str::trim)
                    .filter(|task| !task.is_empty())
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                let model = self
                    .models
                    .sidecar
                    .clone()
                    .unwrap_or_else(|| self.models.main.clone());
                let outputs = parallel::run_parallel_analysis(self.client.clone(), model, tasks)?;
                let message = outputs
                    .into_iter()
                    .enumerate()
                    .map(|(index, output)| format!("[worker {}]\n{}", index + 1, output.trim()))
                    .collect::<Vec<_>>()
                    .join("\n\n");
                Ok(AgentEvent::Continue(Some(message)))
            }
            "/exit" | "/quit" => Ok(AgentEvent::Exit),
            _ => Ok(AgentEvent::Continue(Some(format!(
                "unknown command: {command}"
            )))),
        }
    }

    fn handle_user_message(&mut self, input: &str, stream_output: bool) -> Result<String, String> {
        self.push_user_message(input.to_string());
        self.maybe_compact_session(24);

        let requires_action =
            recovery::user_prompt_requires_action(input, self.session.mode_state.mode);
        let mut tool_calls_made_this_turn = 0usize;
        let mut empty_retries = 0usize;
        let mut no_tool_retries = 0usize;

        for _ in 0..self.config.max_iterations {
            let reply = self.request_assistant_reply(stream_output)?;
            if !reply.tool_calls.is_empty() {
                tool_calls_made_this_turn += reply.tool_calls.len();
                empty_retries = 0;
                no_tool_retries = 0;
                self.session.messages.push(ConversationMessage::assistant(
                    reply.content,
                    reply.tool_calls.clone(),
                ));
                for tool_call in reply.tool_calls {
                    let result = self.execute_tool_call(&tool_call.name, &tool_call.arguments)?;
                    self.session
                        .messages
                        .push(ConversationMessage::tool(tool_call.name, result));
                }
                self.maybe_compact_session(24);
                continue;
            }

            let final_reply = reply.content.trim().to_string();
            if final_reply.is_empty() {
                empty_retries += 1;
                if empty_retries >= 3 {
                    return Err("assistant returned empty responses repeatedly".to_string());
                }
                self.push_system_note(recovery::empty_response_recovery_note(
                    empty_retries,
                    requires_action,
                ));
                continue;
            }

            if requires_action && tool_calls_made_this_turn == 0 {
                no_tool_retries += 1;
                if no_tool_retries >= 3 {
                    return Err(
                        "assistant kept describing actions without using tools to perform them"
                            .to_string(),
                    );
                }
                self.push_system_note(recovery::no_tool_recovery_note(no_tool_retries));
                continue;
            }

            self.session.messages.push(ConversationMessage::assistant(
                final_reply.clone(),
                Vec::new(),
            ));
            return Ok(final_reply);
        }

        Err("assistant did not finish within max iterations".to_string())
    }

    fn request_assistant_reply(&self, stream_output: bool) -> Result<AssistantReply, String> {
        let mut messages = Vec::new();
        messages.push(ConversationMessage::system(build_system_prompt(
            self.session.mode_state.mode,
            self.session.mode_state.active_plan_path.as_deref(),
        )));
        messages.extend(self.session.messages.clone());

        if stream_output {
            let mut first_chunk = true;
            let reply = self.client.chat_streaming(
                &self.models.main,
                &messages,
                self.tool_registry.specs(),
                |chunk| {
                    if first_chunk {
                        print!("assistant> ");
                        first_chunk = false;
                    }
                    print!("{chunk}");
                    let _ = io::stdout().flush();
                },
            )?;
            if !first_chunk {
                println!();
            }
            Ok(reply)
        } else {
            self.client
                .chat(&self.models.main, &messages, self.tool_registry.specs())
        }
    }

    fn execute_tool_call(
        &self,
        name: &str,
        arguments: &serde_json::Value,
    ) -> Result<String, String> {
        let context = ToolContext {
            root: self.config.cwd.clone(),
            mode: self.session.mode_state.mode,
            plan_path: self.session.mode_state.active_plan_path.clone(),
            auto_approve: self.config.yes_mode,
            interactive_approval: io::stdin().is_terminal(),
        };
        self.tool_registry.execute(name, arguments, &context)
    }

    fn push_system_note(&mut self, note: String) {
        self.session
            .messages
            .push(ConversationMessage::system(note));
    }

    fn push_user_message(&mut self, content: String) {
        self.session
            .messages
            .push(ConversationMessage::user(content));
    }

    fn current_plan_contents(&self) -> Result<Option<String>, String> {
        let Some(path) = &self.session.mode_state.active_plan_path else {
            return Ok(None);
        };
        if !path.exists() {
            return Ok(None);
        }
        let contents = std::fs::read_to_string(path)
            .map_err(|err| format!("failed to read plan {}: {err}", path.display()))?;
        Ok(Some(contents))
    }

    fn persist_session(&mut self) -> Result<(), String> {
        self.session.checkpoints = self.checkpoint_manager.checkpoints.clone();
        self.session_store.save(&self.session)
    }

    fn maybe_compact_session(&mut self, keep_tail: usize) -> bool {
        if !should_compact(
            &self.session.messages,
            self.config.context_budget,
            keep_tail,
        ) {
            return false;
        }

        if let Some(sidecar) = self.models.sidecar.clone() {
            compact_messages_with_strategy(&mut self.session.messages, keep_tail, |head| {
                self.client.summarize_conversation(&sidecar, head)
            })
            .unwrap_or_else(|_| compact_messages(&mut self.session.messages, keep_tail))
        } else {
            compact_messages(&mut self.session.messages, keep_tail)
        }
    }

    fn ensure_plan_file(&self, plan_path: &Path) -> Result<(), String> {
        if plan_path.exists() {
            return Ok(());
        }
        if let Some(parent) = plan_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
        }
        std::fs::write(
            plan_path,
            "# Plan\n\n## Goal\n- \n\n## Findings\n- \n\n## Steps\n1. \n",
        )
        .map_err(|err| format!("failed to create plan file {}: {err}", plan_path.display()))
    }

    fn poll_background_tasks(&mut self) -> Result<Option<String>, String> {
        let Some(watcher) = &mut self.watcher else {
            return Ok(None);
        };
        let changes = watcher.poll_changes()?;
        if changes.is_empty() {
            return Ok(None);
        }

        let mut lines = vec![format!("watcher:\n{}", changes.join("\n"))];
        if let Some(result) = self.auto_test.run_if_enabled(&self.config.cwd)? {
            lines.push(format!(
                "auto-test: {} exit={}\n{}",
                result.command, result.exit_code, result.output
            ));
        }
        Ok(Some(lines.join("\n\n")))
    }
}

fn should_compact(
    messages: &[ConversationMessage],
    context_budget: usize,
    keep_tail: usize,
) -> bool {
    messages.len() > keep_tail + 4 || approximate_token_count(messages) > context_budget
}

fn plan_is_substantive(contents: &str) -> bool {
    let meaningful_lines = contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !matches!(*line, "# Plan" | "## Goal" | "## Findings" | "## Steps"))
        .filter(|line| *line != "-" && *line != "1.")
        .count();
    meaningful_lines >= 2
}
