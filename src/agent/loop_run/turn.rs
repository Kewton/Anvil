use super::interrupt::{InterruptEnv, InterruptMonitor};
use super::spinner::{Spinner, SpinnerStopSignal};
use super::summary::{ExitReason, LoopResult, LoopStats};
use super::*;
use crate::agent::orchestration::{RepoVerification, capture_repo_snapshot, verify_repo_progress};
use std::collections::HashSet;
use std::time::Instant;

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

fn build_stats(
    accumulated: Vec<RepoVerification>,
    final_verif: RepoVerification,
    iter_used: usize,
    iter_max: usize,
    duration_secs: u64,
) -> LoopStats {
    let mut all_changed: HashSet<String> = HashSet::new();
    let mut impl_changed = 0usize;
    let mut test_changed = 0usize;
    let mut setup_changed = 0usize;
    let mut deleted_changed = 0usize;

    for verif in accumulated.iter().chain(std::iter::once(&final_verif)) {
        for f in &verif.changed_files {
            all_changed.insert(f.clone());
        }
        impl_changed += verif.implementation_files_changed;
        test_changed += verif.test_files_changed;
        setup_changed += verif.setup_files_changed;
        deleted_changed += verif.deleted_files_changed;
    }

    let total_changed = impl_changed + test_changed + setup_changed + deleted_changed;
    let mut changed_files: Vec<String> = all_changed.into_iter().collect();
    changed_files.sort();
    changed_files.truncate(16);

    LoopStats {
        iter_used,
        iter_max,
        duration_secs,
        changed_files,
        total_changed,
    }
}

impl Agent {
    pub(super) fn handle_user_message(&mut self, input: &str, stream_output: bool) -> LoopResult {
        // Start the ESC interrupt monitor for the duration of this turn only —
        // rustyline owns raw mode during the REPL line-edit, so the monitor
        // must live strictly inside `handle_user_message`. Drop at function
        // exit disables raw mode deterministically (AC-2 / AC-3 / R1 / R2).
        let env = InterruptEnv::detect();
        let mut monitor = InterruptMonitor::start(&env);
        self.run_turn(input, stream_output, &mut monitor)
    }

    fn run_turn(
        &mut self,
        input: &str,
        stream_output: bool,
        monitor: &mut InterruptMonitor,
    ) -> LoopResult {
        self.push_user_message(input.to_string());
        self.maybe_compact_session(DEFAULT_KEEP_TAIL);

        let action_expectation =
            recovery::classify_action_expectation(input, self.session.mode_state.mode);
        let requires_action = action_expectation != recovery::ActionExpectation::None;

        self.run_actor_loop(
            action_expectation,
            requires_action,
            stream_output,
            false,
            monitor,
        )
    }

    fn run_actor_loop(
        &mut self,
        action_expectation: recovery::ActionExpectation,
        requires_action: bool,
        stream_output: bool,
        restart_convergence_mode: bool,
        monitor: &mut InterruptMonitor,
    ) -> LoopResult {
        let use_color = io::stdout().is_terminal() && !no_color_requested();
        let use_unicode = unicode_supported();
        let start = Instant::now();
        let mut before_snapshot = capture_repo_snapshot(&self.work_root);
        let mut accumulated: Vec<RepoVerification> = Vec::new();
        let mut last_known_root = self.work_root.clone();

        let mut tool_calls_made_this_turn = 0usize;
        let mut repo_edit_calls_made_this_turn = 0usize;
        let mut empty_retries = 0usize;
        let mut no_tool_retries = 0usize;
        let mut repo_change_retries = 0usize;
        let mut recent_bash_commands = Vec::<String>::new();
        let mut install_commands_seen = 0usize;

        let mut exit_reason = ExitReason::MaxIterations;
        let mut error_text = String::new();
        let mut last_iter = 0usize;
        let mut final_prose = String::new();

        let interrupt_flag = monitor.flag();

        'outer: for iter_count in 0..self.config.max_iterations {
            last_iter = iter_count + 1;
            let approx_tokens = approximate_token_count(&self.session.messages);
            tracing::debug!(iter = iter_count, tokens = approx_tokens, "iter");
            // Publish per-turn token count to the footer (issue #430, AC12).
            // Reuses the value we just computed — O(1), no second walk over
            // `messages`. No-op when the footer handle is disabled.
            self.footer.publish_tokens(approx_tokens);

            // Boundary 1: before requesting the next assistant reply. Lets us
            // bail out between iterations without starting a fresh LLM call.
            if interrupt_flag.is_set() {
                exit_reason = ExitReason::Interrupted;
                break 'outer;
            }

            let reply = match self.request_assistant_reply_with_retry(stream_output) {
                Ok(r) => r,
                Err(err) => {
                    exit_reason = ExitReason::TransportError;
                    error_text = err;
                    break 'outer;
                }
            };

            // Boundary 2: right after the Ollama response completes. This is
            // the AC-10 checkpoint — mid-flight cancel is out of scope.
            if interrupt_flag.is_set() {
                exit_reason = ExitReason::Interrupted;
                break 'outer;
            }

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
                        use_unicode,
                    );
                    println!("{progress}");
                    let _ = io::stdout().flush();
                    // approve-guard: tools Bash/Write/Edit may invoke an
                    // interactive approve prompt in `tools/registry.rs`. We
                    // must not let the spinner write to stderr while stdin is
                    // being read. Skip spinner in that narrow case; RAII
                    // scope ends when execute_tool_call returns for all
                    // other branches.
                    let needs_approve_prompt =
                        matches!(tool_name.as_str(), "Bash" | "Write" | "Edit")
                            && !self.config.yes_mode
                            && io::stdin().is_terminal();
                    let start_spinner_for_exec = !needs_approve_prompt;
                    // Yield raw mode to the approve `stdin().read_line` and park
                    // the daemon thread until `resume()` is called. Idempotent,
                    // so a tool that never triggers the prompt is unaffected.
                    if needs_approve_prompt {
                        monitor.pause();
                    }
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
                        } else if start_spinner_for_exec {
                            let _sp = Spinner::start(format!("running {tool_name}..."));
                            self.execute_tool_call(&tool_name, &tool_call.arguments)
                        } else {
                            self.execute_tool_call(&tool_name, &tool_call.arguments)
                        }
                    } else if start_spinner_for_exec {
                        let _sp = Spinner::start(format!("running {tool_name}..."));
                        self.execute_tool_call(&tool_name, &tool_call.arguments)
                    } else {
                        self.execute_tool_call(&tool_name, &tool_call.arguments)
                    };
                    if needs_approve_prompt {
                        monitor.resume();
                    }

                    // detect work_root change after each tool execution
                    if self.work_root != last_known_root {
                        let verif = verify_repo_progress(&before_snapshot, &last_known_root);
                        accumulated.push(verif);
                        before_snapshot = capture_repo_snapshot(&self.work_root);
                        last_known_root = self.work_root.clone();
                    }

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
                // Boundary 3: after tool messages have been pushed and the
                // session has been compacted, so `persist_session` (called by
                // `process_line`) can save a consistent snapshot the user can
                // `--resume` from. `Condvar` wake on drop makes this cheap.
                if interrupt_flag.is_set() {
                    exit_reason = ExitReason::Interrupted;
                    break 'outer;
                }
                continue;
            }

            let final_reply = reply.content.trim().to_string();
            if final_reply.is_empty() {
                if action_expectation == recovery::ActionExpectation::RepoChange {
                    repo_change_retries += 1;
                    if repo_change_retries >= 3 {
                        exit_reason = ExitReason::MissingRepoEdits;
                        error_text = exit_reason.default_error_text().to_string();
                        break 'outer;
                    }
                    self.push_system_note(recovery::repo_change_recovery_note(repo_change_retries));
                } else {
                    empty_retries += 1;
                    if empty_retries >= 3 {
                        exit_reason = ExitReason::EmptyResponses;
                        error_text = exit_reason.default_error_text().to_string();
                        break 'outer;
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
                        exit_reason = ExitReason::MissingRepoEdits;
                        error_text = exit_reason.default_error_text().to_string();
                        break 'outer;
                    }
                    self.push_system_note(recovery::repo_change_recovery_note(repo_change_retries));
                } else {
                    no_tool_retries += 1;
                    if no_tool_retries >= 3 {
                        exit_reason = ExitReason::NoToolCalls;
                        error_text = exit_reason.default_error_text().to_string();
                        break 'outer;
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
                    exit_reason = ExitReason::MissingRepoEdits;
                    error_text = exit_reason.default_error_text().to_string();
                    break 'outer;
                }
                self.push_system_note(recovery::repo_change_recovery_note(repo_change_retries));
                continue;
            }

            // Done
            final_prose = final_reply;
            exit_reason = ExitReason::Done;
            break 'outer;
        }

        // Single exit point: compute stats and return LoopResult
        let duration_secs = start.elapsed().as_secs();
        let final_verif = verify_repo_progress(&before_snapshot, &self.work_root);
        let stats = build_stats(
            accumulated,
            final_verif,
            last_iter.min(self.config.max_iterations),
            self.config.max_iterations,
            duration_secs,
        );

        if exit_reason.is_success() {
            self.session.messages.push(ConversationMessage::assistant(
                final_prose.clone(),
                Vec::new(),
            ));
            Ok((final_prose, stats))
        } else {
            if error_text.is_empty() {
                error_text = exit_reason.default_error_text().to_string();
            }
            Err((exit_reason, error_text, stats))
        }
    }

    fn request_assistant_reply_with_retry(
        &mut self,
        stream_output: bool,
    ) -> Result<AssistantReply, String> {
        // Start spinner once at function entry; retries share the same
        // animation (no flicker between attempts). Dropped automatically on
        // function exit (Ok / Err / early-return), clearing the line.
        let sp = Spinner::start(format!("thinking... ({})", self.models.main));
        let mut downgraded_native_tools = false;
        let mut retries_remaining = self.config.chat_retries;
        let mut extra_transport_retries = if self.session.messages.len() >= 12 {
            4
        } else {
            2
        };
        let mut transport_retry_count = 0usize;
        loop {
            // Only streaming paths need first-chunk stop; oneshot blocks until
            // the whole reply is assembled so Drop is sufficient.
            let stop_signal = if stream_output {
                sp.stop_signal()
            } else {
                None
            };
            match self.request_assistant_reply(stream_output, stop_signal) {
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

    fn request_assistant_reply(
        &self,
        stream_output: bool,
        stop_signal: Option<SpinnerStopSignal>,
    ) -> Result<AssistantReply, String> {
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
                        // First chunk: stop spinner immediately (stop flag +
                        // Condvar notify) so no spinner residue appears before
                        // "assistant> ". Safe when `stop_signal` is None.
                        if let Some(sig) = &stop_signal {
                            sig.trigger();
                        }
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
pub(super) fn no_color_requested() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
}

fn is_utf8_locale(lang: &str) -> bool {
    let lower = lang.to_ascii_lowercase();
    lower
        .split(['.', '_', '@', ';', ',', ' '])
        .any(|t| t == "utf-8" || t == "utf8")
}

fn unicode_supported() -> bool {
    if std::env::var_os("ANVIL_NO_EMOJI").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    for key in ["LC_ALL", "LC_CTYPE", "LANG"] {
        if let Ok(v) = std::env::var(key)
            && is_utf8_locale(&v)
        {
            return true;
        }
    }
    false
}

/// Replace control characters (C0, DEL, and C1) with spaces, then trim trailing
/// whitespace. Required for model-derived text so that newlines or ANSI escape
/// sequences cannot be injected into the terminal. C1 (`U+0080..U+009F`) is
/// included because some terminals interpret 8-bit CSI (`U+009B`) and OSC
/// (`U+009D`) equivalently to `ESC [` and `ESC ]`.
fn sanitize_for_progress(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        let cp = ch as u32;
        if cp < 0x20 || cp == 0x7F || (0x80..=0x9F).contains(&cp) {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out.trim_end().to_string()
}

const COLOR_RESET: &str = "\x1b[0m";

fn tool_color(tool_name: &str) -> &'static str {
    match tool_name {
        "Write" => "\x1b[38;5;198m",
        "Read" => "\x1b[38;5;87m",
        "Edit" => "\x1b[38;5;208m",
        "Bash" => "\x1b[38;5;226m",
        "Glob" => "\x1b[38;5;51m",
        "Grep" => "\x1b[38;5;39m",
        _ => "\x1b[38;5;245m",
    }
}

fn tool_emoji(tool_name: &str) -> &'static str {
    match tool_name {
        "Write" => "✏️",
        "Read" => "📄",
        "Edit" => "📝",
        "Bash" => "⚡",
        "Glob" => "🔍",
        "Grep" => "🔎",
        _ => "🔧",
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
/// applied to the tool name when `use_color` is true, and emoji is prepended
/// when `use_unicode` is true.
pub(super) fn format_progress_line(
    tool_name: &str,
    arguments: &serde_json::Value,
    iter_human: usize,
    max_iterations: usize,
    work_root: &std::path::Path,
    use_color: bool,
    use_unicode: bool,
) -> String {
    let (display_str, extra) = tool_display(tool_name, arguments, work_root);
    // Sanitize before painting so an adversarial tool_name cannot inject escapes.
    // emoji は &'static str ハードコードなので再 sanitize は不要。
    let safe_tool_name = sanitize_for_progress(tool_name);
    let label = if use_unicode {
        format!("{} {}", tool_emoji(tool_name), safe_tool_name)
    } else {
        safe_tool_name
    };
    let painted = paint(&label, tool_color(tool_name), use_color);
    let extra_part = extra.map(|e| format!(" ({e})")).unwrap_or_default();
    format!("[iter {iter_human}/{max_iterations}]  {painted}  {display_str}{extra_part}")
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
    use super::{
        format_progress_line, is_utf8_locale, sanitize_for_progress, tool_color, tool_display,
        tool_emoji, unicode_supported,
    };
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Mutex;

    static ENV_GUARD: Mutex<()> = Mutex::new(());

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
        let line = format_progress_line("Write", &args, 1, 12, &work_root, false, false);
        assert!(line.starts_with("[iter 1/12]"));
    }

    #[test]
    fn progress_line_no_color_no_escape() {
        let work_root = PathBuf::from("/work");
        let args = json!({"command": "ls"});
        let line = format_progress_line("Bash", &args, 1, 12, &work_root, false, false);
        assert!(!line.contains('\x1b'));
    }

    #[test]
    fn progress_line_color_prefix_invariant() {
        let work_root = PathBuf::from("/work");
        let args = json!({"command": "ls"});
        let line = format_progress_line("Bash", &args, 1, 12, &work_root, true, false);
        assert!(line.starts_with("[iter "));
    }

    #[test]
    fn tool_style_all_mappings() {
        let cases: &[(&str, &str, &str)] = &[
            ("Write", "\x1b[38;5;198m", "✏\u{fe0f}"),
            ("Read", "\x1b[38;5;87m", "📄"),
            ("Edit", "\x1b[38;5;208m", "📝"),
            ("Bash", "\x1b[38;5;226m", "⚡"),
            ("Glob", "\x1b[38;5;51m", "🔍"),
            ("Grep", "\x1b[38;5;39m", "🔎"),
            ("Unknown", "\x1b[38;5;245m", "🔧"),
        ];
        for (name, expected_color, expected_emoji) in cases {
            assert_eq!(tool_color(name), *expected_color, "color for {name}");
            assert_eq!(tool_emoji(name), *expected_emoji, "emoji for {name}");
        }
    }

    #[test]
    fn progress_line_emoji_and_color_for_bash() {
        let work_root = PathBuf::from("/work");
        let args = json!({"command": "ls"});
        let line = format_progress_line("Bash", &args, 1, 12, &work_root, true, true);
        let color_idx = line.find("\x1b[38;5;226m").expect("color present");
        let emoji_idx = line.find('⚡').expect("emoji present");
        let reset_idx = line.find("\x1b[0m").expect("reset present");
        assert!(color_idx < emoji_idx, "color before emoji");
        assert!(emoji_idx < reset_idx, "emoji before reset");
    }

    #[test]
    fn progress_line_no_color_but_unicode_emits_emoji() {
        let work_root = PathBuf::from("/work");
        let args = json!({"command": "ls"});
        let line = format_progress_line("Bash", &args, 1, 12, &work_root, false, true);
        assert!(line.contains('⚡'));
        assert!(!line.contains('\x1b'));
    }

    #[test]
    fn progress_line_unicode_off_no_emoji() {
        let work_root = PathBuf::from("/work");
        let args = json!({"command": "ls"});
        let line = format_progress_line("Bash", &args, 1, 12, &work_root, true, false);
        assert!(!line.contains('⚡'));
    }

    #[test]
    fn is_utf8_locale_table() {
        let true_cases = [
            "en_US.UTF-8",
            "en_US.utf-8",
            "C.UTF8",
            "C.utf8",
            "ja_JP.UTF-8@Modifier",
            "en_US.UTF-8;POSIX",
        ];
        let false_cases = [
            "",
            "C",
            "POSIX",
            "en_US.utf-800",
            "xutf8x",
            "utf-88",
            "en_US.ISO-8859-1",
        ];
        for c in true_cases {
            assert!(is_utf8_locale(c), "expected true for {c:?}");
        }
        for c in false_cases {
            assert!(!is_utf8_locale(c), "expected false for {c:?}");
        }
    }

    fn set_or_remove(key: &str, value: Option<&str>) {
        unsafe {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }

    fn snapshot_and_clear(keys: &[&str]) -> Vec<(String, Option<String>)> {
        keys.iter()
            .map(|k| {
                let prior = std::env::var(k).ok();
                unsafe {
                    std::env::remove_var(k);
                }
                ((*k).to_string(), prior)
            })
            .collect()
    }

    fn restore(snapshot: Vec<(String, Option<String>)>) {
        for (k, v) in snapshot {
            set_or_remove(&k, v.as_deref());
        }
    }

    #[test]
    fn unicode_supported_respects_anvil_no_emoji() {
        let _g = ENV_GUARD.lock().unwrap();
        let keys = ["ANVIL_NO_EMOJI", "LC_ALL", "LC_CTYPE", "LANG"];
        let snap = snapshot_and_clear(&keys);
        set_or_remove("LANG", Some("en_US.UTF-8"));
        set_or_remove("ANVIL_NO_EMOJI", Some("1"));
        assert!(!unicode_supported());
        restore(snap);
    }

    #[test]
    fn unicode_supported_empty_env_returns_false() {
        let _g = ENV_GUARD.lock().unwrap();
        let keys = ["ANVIL_NO_EMOJI", "LC_ALL", "LC_CTYPE", "LANG"];
        let snap = snapshot_and_clear(&keys);
        assert!(!unicode_supported());
        restore(snap);
    }
}
