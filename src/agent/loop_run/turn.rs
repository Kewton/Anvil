use super::*;

/// Maximum number of characters of tool-call arguments retained in trace logs.
const LOG_ARGS_MAX_CHARS: usize = 200;

/// UTF-8-safe truncation: keeps at most `max` characters and appends `...`
/// when the input was longer. Never splits a multi-byte code point.
fn truncate(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((byte_idx, _)) => {
            let mut out = String::with_capacity(byte_idx + 3);
            out.push_str(&s[..byte_idx]);
            out.push_str("...");
            out
        }
        None => s.to_string(),
    }
}

impl Agent {
    pub(super) fn handle_user_message(
        &mut self,
        input: &str,
        stream_output: bool,
    ) -> Result<String, String> {
        self.push_user_message(input.to_string());
        self.maybe_compact_session(DEFAULT_KEEP_TAIL);

        let action_expectation =
            recovery::classify_action_expectation(input, self.session.mode_state.mode);
        let requires_action = action_expectation != recovery::ActionExpectation::None;

        self.run_actor_loop(action_expectation, requires_action, stream_output, false)
    }

    fn run_actor_loop(
        &mut self,
        action_expectation: recovery::ActionExpectation,
        requires_action: bool,
        stream_output: bool,
        restart_convergence_mode: bool,
    ) -> Result<String, String> {
        let use_color = io::stdout().is_terminal() && !no_color_requested();
        let mut tool_calls_made_this_turn = 0usize;
        let mut repo_edit_calls_made_this_turn = 0usize;
        let mut empty_retries = 0usize;
        let mut no_tool_retries = 0usize;
        let mut repo_change_retries = 0usize;
        let mut recent_bash_commands = Vec::<String>::new();
        let mut install_commands_seen = 0usize;

        for iter_count in 0..self.config.max_iterations {
            let approx_tokens = approximate_token_count(&self.session.messages);
            tracing::debug!(iter = iter_count, tokens = approx_tokens, "iter");
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
                let mut emitted_bash_loop_note = false;
                for tool_call in prepared_tool_calls {
                    let tool_name = tool_call.name.clone();
                    let args_str = tool_call.arguments.to_string();
                    tracing::debug!(
                        tool = %tool_name,
                        args = %truncate(&args_str, LOG_ARGS_MAX_CHARS),
                        "tool call"
                    );
                    let progress = format_progress_line(
                        &tool_name,
                        &tool_call.arguments,
                        iter_count + 1,
                        self.config.max_iterations,
                        &self.work_root,
                        use_color,
                    );
                    println!("{progress}");
                    let _ = io::stdout().flush();
                    let raw_result = if recovery::should_block_restart_discovery(
                        &tool_name,
                        restart_convergence_mode && repo_edit_calls_made_this_turn == 0,
                    ) {
                        recovery::broad_restart_discovery_error(&tool_name)
                    } else if tool_name == "Bash" {
                        let command = tool_call
                            .arguments
                            .get("command")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let block_as_loop = recovery::should_block_bash_command(
                            &command,
                            &recent_bash_commands,
                            install_commands_seen,
                        );
                        recent_bash_commands.push(command.clone());
                        if recovery::is_dependency_install_command(&command) {
                            install_commands_seen += 1;
                        }
                        if block_as_loop {
                            emitted_bash_loop_note = true;
                            recovery::repeated_bash_error(&command)
                        } else {
                            self.execute_tool_call(&tool_name, &tool_call.arguments)
                        }
                    } else {
                        self.execute_tool_call(&tool_name, &tool_call.arguments)
                    };
                    let compact_result = prompting::compact_tool_result(&tool_name, raw_result);
                    self.session
                        .messages
                        .push(ConversationMessage::tool(tool_name, compact_result));
                }
                if emitted_bash_loop_note {
                    self.push_system_note(recovery::install_loop_recovery_note());
                }
                let compacted = self.maybe_compact_late_turn_session(
                    tool_calls_made_this_turn,
                    repo_edit_calls_made_this_turn,
                );
                if !compacted {
                    self.maybe_compact_session(DEFAULT_KEEP_TAIL);
                }
                continue;
            }

            let final_reply = reply.content.trim().to_string();
            if final_reply.is_empty() {
                if action_expectation == recovery::ActionExpectation::RepoChange {
                    repo_change_retries += 1;
                    if repo_change_retries >= 3 {
                        return Err(
                            "assistant kept stopping before making the requested repository edits"
                                .to_string(),
                        );
                    }
                    self.push_system_note(recovery::repo_change_recovery_note(repo_change_retries));
                } else {
                    empty_retries += 1;
                    if empty_retries >= 3 {
                        return Err("assistant returned empty responses repeatedly".to_string());
                    }
                    self.push_system_note(recovery::empty_response_recovery_note(
                        empty_retries,
                        requires_action,
                    ));
                }
                continue;
            }

            if requires_action && tool_calls_made_this_turn == 0 {
                if action_expectation == recovery::ActionExpectation::RepoChange {
                    repo_change_retries += 1;
                    if repo_change_retries >= 3 {
                        return Err(
                            "assistant kept stopping before making the requested repository edits"
                                .to_string(),
                        );
                    }
                    self.push_system_note(recovery::repo_change_recovery_note(repo_change_retries));
                } else {
                    no_tool_retries += 1;
                    if no_tool_retries >= 3 {
                        return Err(
                            "assistant kept describing actions without using tools to perform them"
                                .to_string(),
                        );
                    }
                    self.push_system_note(recovery::no_tool_recovery_note(no_tool_retries));
                }
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
                self.push_system_note(recovery::repo_change_recovery_note(repo_change_retries));
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
        let mut extra_transport_retries = if self.session.messages.len() >= 12 {
            4
        } else {
            2
        };
        let mut transport_retry_count = 0usize;
        loop {
            match self.request_assistant_reply(stream_output) {
                Ok(reply) => return Ok(reply),
                Err(err) => {
                    if self.native_tools_enabled
                        && !downgraded_native_tools
                        && lifecycle::is_native_tool_parser_failure(&err)
                    {
                        downgraded_native_tools = true;
                        self.disable_native_tools_for_session();
                        continue;
                    }
                    if lifecycle::is_transport_error(&err) && extra_transport_retries > 0 {
                        transport_retry_count += 1;
                        extra_transport_retries -= 1;
                        thread::sleep(Duration::from_secs((transport_retry_count as u64) * 4));
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
        let protocol =
            prompting::ToolProtocol::from_native_tools_enabled(self.native_tools_enabled);
        let native_tools_enabled = protocol.native_tools_enabled();
        let messages = self.build_request_messages(protocol);

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

    fn build_request_messages(
        &self,
        protocol: prompting::ToolProtocol,
    ) -> Vec<ConversationMessage> {
        let mut messages = Vec::new();

        messages.push(ConversationMessage::system(build_system_prompt(
            self.session.mode_state.mode,
            self.session.mode_state.active_plan_path.as_deref(),
            protocol,
        )));
        messages.extend(prompting::runtime_context_messages(
            &self.config.cwd,
            &self.work_root,
            protocol,
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
            Err(err) => lifecycle::format_tool_error(&err),
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

    pub(super) fn push_system_note(&mut self, note: String) {
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
}

/// Returns true when the environment requests that color output be suppressed
/// (https://no-color.org/): `NO_COLOR` is set to any non-empty value.
fn no_color_requested() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
}

/// Replace C0 control characters and DEL with spaces, then trim trailing
/// whitespace. Required for model-derived text so that newlines or ANSI escape
/// sequences cannot be injected into the terminal.
fn sanitize_for_progress(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if (ch as u32) < 0x20 || ch == '\u{007F}' {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out.trim_end().to_string()
}

const COLOR_GREEN: &str = "\x1b[32m";
const COLOR_CYAN: &str = "\x1b[36m";
const COLOR_YELLOW: &str = "\x1b[33m";
const COLOR_MAGENTA: &str = "\x1b[35m";
const COLOR_BLUE: &str = "\x1b[34m";
const COLOR_RESET: &str = "\x1b[0m";

fn tool_color(tool_name: &str) -> &'static str {
    match tool_name {
        "Write" => COLOR_GREEN,
        "Read" => COLOR_CYAN,
        "Edit" => COLOR_YELLOW,
        "Bash" => COLOR_MAGENTA,
        "Glob" | "Grep" => COLOR_BLUE,
        _ => "",
    }
}

fn paint(s: &str, color: &str, use_color: bool) -> String {
    if use_color && !color.is_empty() {
        format!("{color}{s}{COLOR_RESET}")
    } else {
        s.to_string()
    }
}

/// Returns `(display_str, extra)` for the progress line. `display_str` is the
/// main single-line description (path / command / pattern); `extra` is an
/// optional parenthesized suffix (e.g. `"5B"` for Write byte count). Paths are
/// made relative to `work_root` when possible. All model-derived strings pass
/// through `sanitize_for_progress` to prevent terminal injection.
fn tool_display(
    tool_name: &str,
    arguments: &serde_json::Value,
    work_root: &std::path::Path,
) -> (String, Option<String>) {
    let str_arg = |key: &str| -> &str {
        arguments
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
    };
    let relativize = |path: &str| -> String {
        std::path::Path::new(path)
            .strip_prefix(work_root)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| path.to_string())
    };

    match tool_name {
        "Write" => {
            let display = sanitize_for_progress(&relativize(str_arg("path")));
            let extra = arguments
                .get("content")
                .and_then(serde_json::Value::as_str)
                .map(|c| format!("{}B", c.len()));
            (display, extra)
        }
        "Edit" | "Read" => (sanitize_for_progress(&relativize(str_arg("path"))), None),
        "Bash" => {
            let sanitized = sanitize_for_progress(str_arg("command"));
            (truncate(&sanitized, 57), None)
        }
        "Glob" | "Grep" => (sanitize_for_progress(str_arg("pattern")), None),
        _ => (sanitize_for_progress(tool_name), None),
    }
}

/// Format a single-line per-iteration progress line. ANSI color is only
/// applied to the tool name when `use_color` is true.
pub(super) fn format_progress_line(
    tool_name: &str,
    arguments: &serde_json::Value,
    iter_human: usize,
    max_iterations: usize,
    work_root: &std::path::Path,
    use_color: bool,
) -> String {
    let (display_str, extra) = tool_display(tool_name, arguments, work_root);
    // Sanitize before painting so an adversarial tool_name cannot inject escapes.
    let safe_tool_name = sanitize_for_progress(tool_name);
    let painted_tool = paint(&safe_tool_name, tool_color(tool_name), use_color);
    let extra_part = extra.map(|e| format!(" ({e})")).unwrap_or_default();
    format!("[iter {iter_human}/{max_iterations}]  {painted_tool}  {display_str}{extra_part}")
}

#[cfg(test)]
mod truncate_tests {
    use super::truncate;

    #[test]
    fn preserves_short_strings_verbatim() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("exact", 5), "exact");
    }

    #[test]
    fn truncates_long_strings_with_ellipsis() {
        assert_eq!(truncate("abcdefgh", 3), "abc...");
    }

    #[test]
    fn never_splits_multibyte_code_points() {
        // Each Japanese char is 3 bytes in UTF-8; taking 2 must not slice mid-char.
        assert_eq!(truncate("あいうえお", 2), "あい...");
    }
}

#[cfg(test)]
mod progress_tests {
    use super::{format_progress_line, sanitize_for_progress, tool_display};
    use serde_json::json;
    use std::path::PathBuf;

    #[test]
    fn sanitize_removes_newline() {
        assert_eq!(sanitize_for_progress("hello\nworld"), "hello world");
    }

    #[test]
    fn sanitize_removes_escape() {
        assert_eq!(sanitize_for_progress("red\x1b[31m!"), "red [31m!");
    }

    #[test]
    fn sanitize_passthrough_normal() {
        assert_eq!(sanitize_for_progress("hello world"), "hello world");
    }

    #[test]
    fn tool_display_write_ascii() {
        let work_root = PathBuf::from("/work");
        let args = json!({"path": "/work/src/foo.rs", "content": "hello"});
        let (display, extra) = tool_display("Write", &args, &work_root);
        assert_eq!(display, "src/foo.rs");
        assert_eq!(extra, Some("5B".to_string()));
    }

    #[test]
    fn tool_display_write_non_ascii() {
        let work_root = PathBuf::from("/work");
        let args = json!({"path": "/work/a.txt", "content": "日本語"});
        let (_, extra) = tool_display("Write", &args, &work_root);
        // "日本語" is 9 bytes in UTF-8
        assert_eq!(extra, Some("9B".to_string()));
    }

    #[test]
    fn tool_display_bash_short() {
        let work_root = PathBuf::from("/work");
        let cmd = "cargo test";
        let args = json!({"command": cmd});
        let (display, _) = tool_display("Bash", &args, &work_root);
        assert_eq!(display, cmd);
    }

    #[test]
    fn tool_display_bash_long() {
        let work_root = PathBuf::from("/work");
        let cmd = "a".repeat(61);
        let args = json!({"command": cmd});
        let (display, _) = tool_display("Bash", &args, &work_root);
        assert_eq!(display.len(), 60); // 57 chars + "..."
        assert!(display.ends_with("..."));
    }

    #[test]
    fn tool_display_path_relative() {
        let work_root = PathBuf::from("/work");
        let args = json!({"path": "/work/src/lib.rs"});
        let (display, _) = tool_display("Read", &args, &work_root);
        assert_eq!(display, "src/lib.rs");
    }

    #[test]
    fn tool_display_path_outside() {
        let work_root = PathBuf::from("/work");
        let args = json!({"path": "/tmp/outside.txt"});
        let (display, _) = tool_display("Read", &args, &work_root);
        assert_eq!(display, "/tmp/outside.txt");
    }

    #[test]
    fn progress_line_iter_1indexed() {
        let work_root = PathBuf::from("/work");
        let args = json!({"path": "/work/a.txt", "content": "x"});
        let line = format_progress_line("Write", &args, 1, 12, &work_root, false);
        assert!(line.starts_with("[iter 1/12]"));
    }

    #[test]
    fn progress_line_no_color_no_escape() {
        let work_root = PathBuf::from("/work");
        let args = json!({"command": "ls"});
        let line = format_progress_line("Bash", &args, 1, 12, &work_root, false);
        assert!(!line.contains('\x1b'));
    }

    #[test]
    fn progress_line_color_prefix_invariant() {
        let work_root = PathBuf::from("/work");
        let args = json!({"command": "ls"});
        let line = format_progress_line("Bash", &args, 1, 12, &work_root, true);
        assert!(line.starts_with("[iter "));
    }
}
