use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

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
const MAX_TOOL_MESSAGE_CHARS: usize = 12_000;

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
        Self {
            config,
            models,
            client,
            session_store,
            session,
            work_root,
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
                "mode={:?} auto_approve={} cwd={} session={} plan={} approx_tokens={} deferred=git,watch,testloop,tui,skills,mcp,parallel",
                self.session.mode_state.mode,
                self.config.yes_mode,
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
                    let compact_result = compact_tool_result(&tool_name, raw_result);
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
        let mut last_error = None;
        for attempt in 0..=self.config.chat_retries {
            match self.request_assistant_reply(stream_output) {
                Ok(reply) => return Ok(reply),
                Err(err) => {
                    last_error = Some(err);
                    if attempt == self.config.chat_retries {
                        break;
                    }
                    thread::sleep(Duration::from_secs((attempt + 1) as u64 * 2));
                }
            }
        }
        Err(last_error.unwrap_or_else(|| "assistant request failed".to_string()))
    }

    fn request_assistant_reply(&self, stream_output: bool) -> Result<AssistantReply, String> {
        let native_tools_enabled = should_use_native_tool_calls(&self.models.main);
        let tool_call_tag = if native_tools_enabled {
            "tool_call"
        } else {
            "anvil_tool_call"
        };

        let mut messages = Vec::new();
        messages.push(ConversationMessage::system(build_system_prompt(
            self.session.mode_state.mode,
            self.session.mode_state.active_plan_path.as_deref(),
            tool_call_tag,
            native_tools_enabled,
        )));
        if !native_tools_enabled {
            messages.push(ConversationMessage::system(
                format!(
                    "For this model, do not emit native tool_calls. When you need tools, output only <{tool_call_tag}>{{\"name\":\"Tool\",\"arguments\":{{...}}}}</{tool_call_tag}> blocks with valid JSON arguments."
                ),
            ));
        }
        if self.work_root != self.config.cwd {
            messages.push(ConversationMessage::system(format!(
                "Current project root is {}. Use relative paths from this directory unless an absolute path is easier for file tools.",
                self.work_root.display()
            )));
        }
        if let Some(nextjs_targets) = detect_nextjs_targets(&self.work_root) {
            messages.push(ConversationMessage::system(format!(
                "Detected Next.js app entry files at {} and {}. If you need to edit the UI, inspect those files first.",
                self.work_root.join(&nextjs_targets.page).display(),
                self.work_root.join(&nextjs_targets.globals).display(),
            )));
        }
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
        repo_change_progress_note(&self.work_root)
    }

    fn push_system_note(&mut self, note: String) {
        if should_skip_system_note(&self.session.messages, &note) {
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
        if name != "Bash" {
            return;
        }
        let Some(command) = arguments.get("command").and_then(serde_json::Value::as_str) else {
            return;
        };
        if !command.contains("create-next-app") {
            return;
        }

        let Some(new_root) = detect_created_project_root(result) else {
            return;
        };
        if new_root == self.work_root || !new_root.is_dir() {
            return;
        }

        self.work_root = new_root.clone();
        self.session.active_root = Some(new_root.clone());
        self.push_system_note(format!(
            "[Workspace Root Updated] Continue work inside {} and use relative paths from there.",
            new_root.display()
        ));
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

fn detect_created_project_root(tool_output: &str) -> Option<PathBuf> {
    for line in tool_output.lines() {
        let trimmed = line.trim();
        if let Some(path) = trimmed.strip_prefix("Success! Created ") {
            let (_, path) = path.rsplit_once(" at ")?;
            return Some(PathBuf::from(path.trim()));
        }
        if let Some(path) = trimmed.strip_prefix("Creating a new Next.js app in ") {
            return Some(PathBuf::from(path.trim_end_matches('.').trim()));
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RepoProgress {
    Empty,
    InitializedWithoutTests,
    InitializedWithTests,
}

fn repo_change_progress_note(root: &Path) -> String {
    match detect_repo_progress(root) {
        RepoProgress::Empty => "The requested repository change is still unfinished. If setup or scaffolding is needed, do it now. As soon as concrete project files exist, stop exploring and use Read on the implementation files, then Write or Edit them in the same turn sequence.".to_string(),
        RepoProgress::InitializedWithoutTests => "The repository is initialized but the requested implementation is still unfinished. Inspect the concrete implementation files now and make a real repository change with Write or Edit. If tests are part of the request, add or update at least one test file before finalizing.".to_string(),
        RepoProgress::InitializedWithTests => "The repository already has source and test structure. Stop describing intent and make the next concrete code change with Write or Edit on the implementation or test files now. Only finalize after the requested repository change is present.".to_string(),
    }
}

fn detect_repo_progress(root: &Path) -> RepoProgress {
    let mut has_manifest = false;
    let mut has_source = false;
    let mut has_tests = false;
    inspect_repo_tree(root, &mut has_manifest, &mut has_source, &mut has_tests);

    if !(has_manifest || has_source) {
        RepoProgress::Empty
    } else if has_tests {
        RepoProgress::InitializedWithTests
    } else {
        RepoProgress::InitializedWithoutTests
    }
}

#[derive(Debug, Clone)]
struct NextJsTargets {
    page: PathBuf,
    globals: PathBuf,
}

fn detect_nextjs_targets(root: &Path) -> Option<NextJsTargets> {
    let candidates = [
        ("src/app/page.tsx", "src/app/globals.css"),
        ("app/page.tsx", "app/globals.css"),
    ];
    for (page, globals) in candidates {
        let page_path = root.join(page);
        let globals_path = root.join(globals);
        if page_path.is_file() || globals_path.is_file() {
            return Some(NextJsTargets {
                page: PathBuf::from(page),
                globals: PathBuf::from(globals),
            });
        }
    }
    None
}

fn inspect_repo_tree(
    current: &Path,
    has_manifest: &mut bool,
    has_source: &mut bool,
    has_tests: &mut bool,
) {
    let Ok(entries) = fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if file_type.is_dir() {
            if matches!(name, "node_modules" | ".git" | ".anvil" | ".next") {
                continue;
            }
            if matches!(name, "tests" | "__tests__") {
                *has_tests = true;
            }
            inspect_repo_tree(&path, has_manifest, has_source, has_tests);
            continue;
        }
        if is_manifest_file(name) {
            *has_manifest = true;
        }
        if is_source_file(&path) {
            *has_source = true;
        }
        if is_test_file(&path) {
            *has_tests = true;
        }
    }
}

fn is_manifest_file(name: &str) -> bool {
    matches!(
        name,
        "package.json" | "Cargo.toml" | "pyproject.toml" | "go.mod" | "Gemfile" | "composer.json"
    )
}

fn is_source_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "java" | "kt" | "rb" | "php")
    )
}

fn is_test_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name.contains(".test.") || name.contains(".spec.")
}

fn format_tool_error(err: &str) -> String {
    if err.starts_with("Error:") {
        err.to_string()
    } else {
        format!("Error: {err}")
    }
}

fn should_skip_system_note(messages: &[ConversationMessage], note: &str) -> bool {
    for message in messages.iter().rev() {
        if message.role == "user" {
            break;
        }
        if message.role == "system" && message.content == note {
            return true;
        }
    }
    false
}

fn compact_tool_result(name: &str, result: String) -> String {
    if result.chars().count() <= MAX_TOOL_MESSAGE_CHARS {
        return result;
    }

    let chars = result.chars().collect::<Vec<_>>();
    let total = chars.len();
    let head_len = if name == "Read" { 8_000 } else { 10_000 }.min(total);
    let tail_len = if name == "Read" { 2_000 } else { 1_000 }.min(total.saturating_sub(head_len));
    let head = chars[..head_len].iter().collect::<String>();
    let tail = if tail_len == 0 {
        String::new()
    } else {
        chars[total - tail_len..].iter().collect::<String>()
    };

    if tail.is_empty() {
        format!("{head}\n...[truncated {} chars]", total - head_len)
    } else {
        format!(
            "{head}\n...[truncated {} chars]...\n{tail}",
            total.saturating_sub(head_len + tail_len)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RepoProgress, compact_tool_result, detect_created_project_root, detect_nextjs_targets,
        detect_repo_progress, format_tool_error, repo_change_progress_note,
        should_skip_system_note,
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
    fn detects_src_app_nextjs_targets() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page(){}",
        )
        .unwrap();
        let targets = detect_nextjs_targets(dir.path()).unwrap();
        assert_eq!(targets.page, PathBuf::from("src/app/page.tsx"));
        assert_eq!(targets.globals, PathBuf::from("src/app/globals.css"));
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
