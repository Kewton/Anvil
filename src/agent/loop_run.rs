use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use crate::agent::prompting;
use crate::agent::recovery;
use crate::config::Config;
use crate::format_model_banner;
use crate::model_registry::RuntimeModels;
use crate::modes::plan_act::ExecutionMode;
use crate::ollama::client::{AssistantReply, OllamaClient, should_use_native_tool_calls};
use crate::ollama::xml_fallback::ToolCall;
use crate::safety::path_guard::resolve_user_path;
use crate::session::compact::{
    approximate_token_count, compact_messages, compact_messages_with_strategy,
};
use crate::session::store::{ConversationMessage, SessionSnapshot, SessionStore};
use crate::stdin_prompt;
use crate::system_prompt::build_system_prompt;
use crate::tools::registry::{ToolContext, ToolRegistry};

const DEFAULT_KEEP_TAIL: usize = 24;

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
    work_root: PathBuf,
    native_tools_enabled: bool,
    tool_registry: ToolRegistry,
}

impl Agent {
    pub fn new(
        config: Config,
        models: RuntimeModels,
        client: OllamaClient,
        session_store: SessionStore,
        session: SessionSnapshot,
    ) -> Self {
        let work_root = session
            .active_root
            .clone()
            .unwrap_or_else(|| config.cwd.clone());
        let native_tools_enabled =
            should_use_native_tool_calls(&models.main) && !session.native_tools_disabled;
        Self {
            config,
            models,
            client,
            session_store,
            session,
            work_root,
            native_tools_enabled,
            tool_registry: ToolRegistry::default(),
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
            self.work_root.display()
        );
        if self.config.debug
            && let Some(path) = crate::logging::llm_io_log_path()
        {
            println!("debug llm log={}", path.display());
        }

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

        let outcome = if input.starts_with('/') {
            self.handle_command(input)
        } else {
            match self.handle_user_message(input, stream_output) {
                Ok(reply) => {
                    if stream_output {
                        Ok(AgentEvent::Continue(None))
                    } else {
                        Ok(AgentEvent::Continue(Some(reply)))
                    }
                }
                Err(err) => Err(err),
            }
        };

        let persist_error = self.persist_session().err();
        match (outcome, persist_error) {
            (Ok(event), None) => Ok(event),
            (Ok(_), Some(err)) => Err(err),
            (Err(err), None) => Err(err),
            (Err(err), Some(persist_err)) => Err(format!(
                "{err} (also failed to persist session: {persist_err})"
            )),
        }
    }

    fn handle_command(&mut self, input: &str) -> Result<AgentEvent, String> {
        let (command, _) = input.split_once(' ').unwrap_or((input, ""));
        match command {
            "/help" => Ok(AgentEvent::Continue(Some(
                "/help /status /model /plan /approve /compact /yes /no /exit".to_string(),
            ))),
            "/status" => Ok(AgentEvent::Continue(Some(format!(
                "mode={:?} auto_approve={} native_tools={} cwd={} session={} plan={} approx_tokens={} deferred=git,watch,testloop,tui,skills,mcp,parallel",
                self.session.mode_state.mode,
                self.config.yes_mode,
                self.native_tools_enabled,
                self.work_root.display(),
                self.session_store.path().display(),
                self.session
                    .mode_state
                    .active_plan_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "-".to_string()),
                approximate_token_count(&self.session.messages),
            )))),
            "/model" => Ok(AgentEvent::Continue(Some(format_model_banner(
                &self.models,
            )))),
            "/yes" => {
                self.config.yes_mode = true;
                Ok(AgentEvent::Continue(Some(
                    "auto-approve enabled".to_string(),
                )))
            }
            "/no" => {
                self.config.yes_mode = false;
                Ok(AgentEvent::Continue(Some(
                    "auto-approve disabled".to_string(),
                )))
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
            "/checkpoint" | "/rollback" | "/watch" | "/autotest" | "/skills" | "/skill"
            | "/mcp" | "/parallel" => Ok(AgentEvent::Continue(Some(format!(
                "{command} is unavailable in the v0.1.0 core rebuild"
            )))),
            "/exit" | "/quit" => Ok(AgentEvent::Exit),
            _ => Ok(AgentEvent::Continue(Some(format!(
                "unknown command: {command}"
            )))),
        }
    }

    fn handle_user_message(&mut self, input: &str, stream_output: bool) -> Result<String, String> {
        self.push_user_message(input.to_string());
        self.maybe_compact_session(DEFAULT_KEEP_TAIL);

        let action_expectation =
            recovery::classify_action_expectation(input, self.session.mode_state.mode);
        let requires_action = action_expectation != recovery::ActionExpectation::None;
        let mut tool_calls_made_this_turn = 0usize;
        let mut repo_edit_calls_made_this_turn = 0usize;
        let mut empty_retries = 0usize;
        let mut no_tool_retries = 0usize;
        let mut repo_change_retries = 0usize;

        for _ in 0..self.config.max_iterations {
            let reply = self.request_assistant_reply_with_retry(stream_output)?;
            let prepared_tool_calls = reply
                .tool_calls
                .into_iter()
                .map(|tool_call| self.prepare_tool_call(tool_call))
                .collect::<Vec<_>>();

            if !prepared_tool_calls.is_empty() {
                tool_calls_made_this_turn += prepared_tool_calls.len();
                repo_edit_calls_made_this_turn += prepared_tool_calls
                    .iter()
                    .filter(|tool_call| recovery::tool_call_counts_as_repo_edit(&tool_call.name))
                    .count();
                empty_retries = 0;
                no_tool_retries = 0;
                if repo_edit_calls_made_this_turn > 0 {
                    repo_change_retries = 0;
                }

                self.session.messages.push(ConversationMessage::assistant(
                    reply.content,
                    prepared_tool_calls.clone(),
                ));
                for tool_call in prepared_tool_calls {
                    let tool_name = tool_call.name.clone();
                    let raw_result = self.execute_tool_call(&tool_name, &tool_call.arguments);
                    let compact_result = prompting::compact_tool_result(&tool_name, raw_result);
                    self.session
                        .messages
                        .push(ConversationMessage::tool(tool_name, compact_result));
                }
                self.maybe_compact_session(DEFAULT_KEEP_TAIL);
                continue;
            }

            let final_reply = reply.content.trim().to_string();
            if final_reply.is_empty() {
                empty_retries += 1;
                if empty_retries >= 3 {
                    return Err("assistant returned empty responses repeatedly".to_string());
                }
                self.push_system_note(
                    if action_expectation == recovery::ActionExpectation::RepoChange {
                        self.next_repo_change_note()
                    } else {
                        recovery::empty_response_recovery_note(empty_retries, requires_action)
                    },
                );
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
                self.push_system_note(
                    if action_expectation == recovery::ActionExpectation::RepoChange {
                        self.next_repo_change_note()
                    } else {
                        recovery::no_tool_recovery_note(no_tool_retries)
                    },
                );
                continue;
            }

            if action_expectation == recovery::ActionExpectation::RepoChange
                && repo_edit_calls_made_this_turn == 0
            {
                repo_change_retries += 1;
                if repo_change_retries >= 3 {
                    return Err(
                        "assistant kept stopping before making the requested repository edits"
                            .to_string(),
                    );
                }
                self.push_system_note(self.next_repo_change_note());
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

    fn request_assistant_reply_with_retry(
        &mut self,
        stream_output: bool,
    ) -> Result<AssistantReply, String> {
        let mut downgraded_native_tools = false;
        let mut retries_remaining = self.config.chat_retries;
        loop {
            match self.request_assistant_reply(stream_output) {
                Ok(reply) => return Ok(reply),
                Err(err) => {
                    if self.native_tools_enabled
                        && !downgraded_native_tools
                        && is_native_tool_parser_failure(&err)
                    {
                        downgraded_native_tools = true;
                        self.disable_native_tools_for_session();
                        continue;
                    }
                    if retries_remaining == 0 {
                        return Err(err);
                    }
                    let sleep_secs = (self.config.chat_retries - retries_remaining + 1) as u64 * 2;
                    retries_remaining -= 1;
                    thread::sleep(Duration::from_secs(sleep_secs));
                }
            }
        }
    }

    fn request_assistant_reply(&self, stream_output: bool) -> Result<AssistantReply, String> {
        let native_tools_enabled = self.native_tools_enabled;
        let messages = self.build_request_messages(native_tools_enabled);

        if stream_output {
            let mut first_chunk = true;
            let reply = self.client.chat_streaming_with_mode(
                &self.models.main,
                &messages,
                self.tool_registry.specs(),
                native_tools_enabled,
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
            self.client.chat_with_mode(
                &self.models.main,
                &messages,
                self.tool_registry.specs(),
                native_tools_enabled,
            )
        }
    }

    fn build_request_messages(&self, native_tools_enabled: bool) -> Vec<ConversationMessage> {
        let mut messages = Vec::new();
        let tool_call_tag = if native_tools_enabled {
            "tool_call"
        } else {
            "anvil_tool_call"
        };

        messages.push(ConversationMessage::system(build_system_prompt(
            self.session.mode_state.mode,
            self.session.mode_state.active_plan_path.as_deref(),
            tool_call_tag,
            native_tools_enabled,
        )));
        messages.extend(prompting::runtime_context_messages(
            &self.config.cwd,
            &self.work_root,
            tool_call_tag,
            native_tools_enabled,
        ));
        messages.extend(self.session.messages.clone());
        messages
    }

    fn execute_tool_call(&mut self, name: &str, arguments: &serde_json::Value) -> String {
        let context = ToolContext {
            root: self.work_root.clone(),
            mode: self.session.mode_state.mode,
            plan_path: self.session.mode_state.active_plan_path.clone(),
            auto_approve: self.config.yes_mode,
            interactive_approval: io::stdin().is_terminal(),
        };
        match self.tool_registry.execute(name, arguments, &context) {
            Ok(result) => {
                self.maybe_update_work_root(name, arguments, &result);
                result
            }
            Err(err) => format_tool_error(&err),
        }
    }

    fn prepare_tool_call(&self, mut tool_call: ToolCall) -> ToolCall {
        if matches!(tool_call.name.as_str(), "Read" | "Write" | "Edit")
            && let Some(arguments) = tool_call.arguments.as_object_mut()
            && let Some(raw_path) = arguments.get("path").and_then(serde_json::Value::as_str)
            && let Ok(resolved) = resolve_user_path(&self.work_root, raw_path)
        {
            arguments.insert(
                "path".to_string(),
                serde_json::Value::String(resolved.display().to_string()),
            );
        }
        tool_call
    }

    fn next_repo_change_note(&self) -> String {
        prompting::repo_change_progress_note(&self.work_root)
    }

    fn push_system_note(&mut self, note: String) {
        if prompting::should_skip_system_note(&self.session.messages, &note) {
            return;
        }
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
        self.session.active_root = if self.work_root == self.config.cwd {
            None
        } else {
            Some(self.work_root.clone())
        };
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

    fn maybe_update_work_root(&mut self, name: &str, arguments: &serde_json::Value, result: &str) {
        let Some(new_root) = prompting::detect_scaffold_root(name, arguments, result) else {
            return;
        };
        if new_root == self.work_root || !new_root.is_dir() {
            return;
        }

        self.apply_scaffold_root(new_root);
    }

    fn apply_scaffold_root(&mut self, new_root: PathBuf) {
        self.work_root = new_root.clone();
        self.session.active_root = Some(new_root.clone());
        self.reset_after_scaffold();
        self.push_system_note(format!(
            "[Workspace Root Updated] Continue work inside {} and use relative paths from there.",
            new_root.display()
        ));
    }

    fn disable_native_tools_for_session(&mut self) {
        self.native_tools_enabled = false;
        self.session.native_tools_disabled = true;
        self.push_system_note(
            "[Runtime Notice] Native tool calling is disabled for this session because the model/runtime parser rejected a tool-call response. Use only <anvil_tool_call>{\"name\":\"Tool\",\"arguments\":{...}}</anvil_tool_call> blocks with valid JSON arguments."
                .to_string(),
        );
    }

    fn reset_after_scaffold(&mut self) {
        self.session.messages =
            prompting::reset_messages_after_scaffold(&self.work_root, &self.session.messages);
    }
}

fn should_compact(
    messages: &[ConversationMessage],
    context_budget: usize,
    keep_tail: usize,
) -> bool {
    messages.len() > keep_tail + 4 || approximate_token_count(messages) > context_budget
}

fn is_native_tool_parser_failure(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("native tool parser failed")
        || lower.contains("unexpected end element")
        || lower.contains("unexpected eof")
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

fn format_tool_error(err: &str) -> String {
    if err.starts_with("Error:") {
        err.to_string()
    } else {
        format!("Error: {err}")
    }
}

#[cfg(test)]
mod tests {
    use super::format_tool_error;
    use crate::agent::prompting::{
        RepoProgress, compact_tool_result, detect_created_project_root, detect_repo_progress,
        repo_change_progress_note, should_skip_system_note,
    };
    use crate::session::store::ConversationMessage;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn detects_created_next_app_root_from_success_line() {
        let output = "Success! Created space-invaders at /tmp/work/space-invaders";
        assert_eq!(
            detect_created_project_root(output),
            Some(PathBuf::from("/tmp/work/space-invaders"))
        );
    }

    #[test]
    fn detects_created_next_app_root_from_create_line() {
        let output = "Creating a new Next.js app in /tmp/work/space-invaders.";
        assert_eq!(
            detect_created_project_root(output),
            Some(PathBuf::from("/tmp/work/space-invaders"))
        );
    }

    #[test]
    fn tool_errors_are_formatted_for_model_recovery() {
        assert_eq!(
            format_tool_error("failed to stat /tmp/missing: nope"),
            "Error: failed to stat /tmp/missing: nope"
        );
        assert_eq!(
            format_tool_error("Error: file not found"),
            "Error: file not found"
        );
    }

    #[test]
    fn read_results_are_compacted_for_transcript() {
        let long = "a".repeat(20_000);
        let compacted = compact_tool_result("Read", long);
        assert!(compacted.contains("[truncated"));
        assert!(compacted.len() < 13_000);
    }

    #[test]
    fn repo_progress_is_empty_without_manifest_or_source() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "hello").unwrap();
        assert_eq!(detect_repo_progress(dir.path()), RepoProgress::Empty);
    }

    #[test]
    fn repo_progress_detects_initialized_repo_without_tests() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("package.json"), "{\"name\":\"demo\"}").unwrap();
        std::fs::write(dir.path().join("src/main.ts"), "console.log('hi');").unwrap();
        assert_eq!(
            detect_repo_progress(dir.path()),
            RepoProgress::InitializedWithoutTests
        );
    }

    #[test]
    fn repo_progress_detects_tests_and_note_stays_generic() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("src.rs"), "fn main() {}").unwrap();
        std::fs::write(
            dir.path().join("tests/app.test.ts"),
            "it('works', () => expect(true).toBe(true));",
        )
        .unwrap();
        assert_eq!(
            detect_repo_progress(dir.path()),
            RepoProgress::InitializedWithTests
        );
        let note = repo_change_progress_note(dir.path());
        assert!(note.contains("implementation or test files"));
        assert!(!note.contains("Next.js"));
        assert!(!note.contains("space-invaders"));
    }

    #[test]
    fn skips_duplicate_consecutive_system_note() {
        let messages = vec![ConversationMessage::system("same note".to_string())];
        assert!(should_skip_system_note(&messages, "same note"));
        assert!(!should_skip_system_note(&messages, "other note"));
    }

    #[test]
    fn skips_duplicate_system_note_with_tool_results_between() {
        let messages = vec![
            ConversationMessage::system("same note".to_string()),
            ConversationMessage::assistant(String::new(), Vec::new()),
            ConversationMessage::tool("Read".to_string(), "file contents".to_string()),
        ];
        assert!(should_skip_system_note(&messages, "same note"));
    }
}
