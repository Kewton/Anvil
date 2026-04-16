use std::io::{self, IsTerminal, Write};
use std::path::Path;

use crate::agent::recovery;
use crate::config::Config;
use crate::format_model_banner;
use crate::git::checkpoint::GitCheckpointManager;
use crate::model_registry::RuntimeModels;
use crate::modes::plan_act::ExecutionMode;
use crate::ollama::client::{AssistantReply, OllamaClient};
use crate::session::compact::{
    approximate_token_count, compact_messages, compact_messages_with_strategy,
};
use crate::session::store::{ConversationMessage, SessionSnapshot, SessionStore};
use crate::stdin_prompt;
use crate::system_prompt::build_system_prompt;
use crate::tools::registry::{ToolContext, ToolRegistry};

pub struct Agent {
    config: Config,
    models: RuntimeModels,
    client: OllamaClient,
    session_store: SessionStore,
    session: SessionSnapshot,
    tool_registry: ToolRegistry,
    checkpoint_manager: GitCheckpointManager,
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
        Self {
            config,
            models,
            client,
            session_store,
            session,
            tool_registry: ToolRegistry::default(),
            checkpoint_manager,
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
        let reply = self.handle_user_message(prompt)?;
        self.persist_session()?;
        Ok(reply)
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
            let input = line.trim();
            if input.is_empty() {
                continue;
            }

            if input.starts_with('/') {
                if self.handle_command(input)? {
                    break;
                }
                continue;
            }

            let reply = self.handle_user_message(input)?;
            println!("{reply}");
            self.persist_session()?;
        }
        Ok(())
    }

    fn handle_command(&mut self, input: &str) -> Result<bool, String> {
        let (command, rest) = input.split_once(' ').unwrap_or((input, ""));
        match command {
            "/help" => {
                println!(
                    "/help /status /model /plan /approve /compact /checkpoint /rollback /yes /no /exit"
                );
            }
            "/status" => {
                println!(
                    "mode={:?} auto_approve={} session={} plan={} approx_tokens={}",
                    self.session.mode_state.mode,
                    self.config.yes_mode,
                    self.session_store.path().display(),
                    self.session
                        .mode_state
                        .active_plan_path
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "-".to_string()),
                    approximate_token_count(&self.session.messages),
                );
            }
            "/model" => {
                println!("{}", format_model_banner(&self.models));
            }
            "/yes" => {
                self.config.yes_mode = true;
                println!("auto-approve enabled");
            }
            "/no" => {
                self.config.yes_mode = false;
                println!("auto-approve disabled");
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
                    println!("already in plan mode: {current}");
                    self.persist_session()?;
                    return Ok(false);
                }
                let plan_path = self.session.mode_state.enter_plan(self.config.plan_dir())?;
                self.ensure_plan_file(&plan_path)?;
                self.push_system_note(format!(
                    "[Plan Mode] Explore with Read, Glob, and Grep. Write the plan to {}. Wait for /approve before making code changes.",
                    plan_path.display()
                ));
                println!("plan mode: {}", plan_path.display());
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
                println!("act mode");
            }
            "/compact" => {
                let changed = self.maybe_compact_session(20);
                if changed {
                    println!("session compacted");
                } else {
                    println!("session already compact");
                }
            }
            "/checkpoint" => {
                let label = if rest.trim().is_empty() {
                    "anvil-manual-checkpoint"
                } else {
                    rest.trim()
                };
                match self.checkpoint_manager.create_checkpoint(label)? {
                    Some(sha) => println!("checkpoint saved: {sha}"),
                    None => println!("no local changes to checkpoint"),
                }
                self.session.checkpoints = self.checkpoint_manager.checkpoints.clone();
            }
            "/rollback" => {
                let sha = self.checkpoint_manager.rollback_latest()?;
                self.session.checkpoints = self.checkpoint_manager.checkpoints.clone();
                println!("rolled back to checkpoint {sha}");
            }
            "/exit" | "/quit" => {
                self.persist_session()?;
                return Ok(true);
            }
            _ => println!("unknown command: {command}"),
        }

        self.persist_session()?;
        Ok(false)
    }

    fn handle_user_message(&mut self, input: &str) -> Result<String, String> {
        self.push_user_message(input.to_string());
        self.maybe_compact_session(24);

        let requires_action =
            recovery::user_prompt_requires_action(input, self.session.mode_state.mode);
        let mut tool_calls_made_this_turn = 0usize;
        let mut empty_retries = 0usize;
        let mut no_tool_retries = 0usize;

        for _ in 0..self.config.max_iterations {
            let reply = self.request_assistant_reply()?;
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

    fn request_assistant_reply(&self) -> Result<AssistantReply, String> {
        let mut messages = Vec::new();
        messages.push(ConversationMessage::system(build_system_prompt(
            self.session.mode_state.mode,
            self.session.mode_state.active_plan_path.as_deref(),
        )));
        messages.extend(self.session.messages.clone());
        self.client
            .chat(&self.models.main, &messages, self.tool_registry.specs())
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
