use super::summary::{ExitReason, format_run_summary};
use super::*;
use crate::config::LogLevel;

/// First 8 characters of a session id, or the full id if shorter. Used for
/// banner display so users get a stable short handle without exposing the
/// whole UUID.
pub fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// Print the REPL / resume startup banner to stdout. The `fresh` and
/// `resumed` flags drive the `[state]` suffix so users can tell at a glance
/// which session they are in.
#[allow(clippy::too_many_arguments)]
pub fn print_startup_banner(
    version: &str,
    model_banner: &str,
    mode: &crate::modes::plan_act::ExecutionMode,
    work_root: &std::path::Path,
    session_id_short: &str,
    message_count: usize,
    fresh: bool,
    resumed: bool,
    log_level: LogLevel,
) {
    println!("anvil {version}");
    println!("{model_banner}");
    println!("mode={mode:?} cwd={}", work_root.display());
    let state = if fresh {
        "fresh"
    } else if resumed {
        "resumed"
    } else {
        "continued"
    };
    println!("session={session_id_short} messages={message_count} [{state}]");
    if log_level >= LogLevel::Verbose
        && let Some(path) = crate::logging::llm_io_log_path()
    {
        println!("llm log={}", path.display());
    }
}

/// Oneshot startup banner: single stderr line so script output on stdout stays
/// clean.
pub fn print_startup_banner_stderr_oneshot(
    session_id_short: &str,
    message_count: usize,
    fresh: bool,
    resumed: bool,
) {
    let state = if fresh {
        "fresh"
    } else if resumed {
        "resumed"
    } else {
        "continued"
    };
    eprintln!("anvil session={session_id_short} messages={message_count} [{state}]");
}

impl Agent {
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

    /// Replay the last user turn and then continue in the REPL loop.
    /// Used by `--resume` / `--resume <ID>` after the session has been loaded
    /// and reconciled. The caller is responsible for printing the startup
    /// banner; this method does not re-print it.
    pub fn run_resume(&mut self, replay_prompt: &str) -> Result<(), String> {
        match self.process_line(replay_prompt, self.config.stream)? {
            AgentEvent::Continue(Some(message)) => {
                if !self.config.stream {
                    println!("{message}");
                }
            }
            AgentEvent::Continue(None) => {}
            AgentEvent::Exit => return Ok(()),
        }
        self.run_repl_loop()
    }

    /// REPL body without the startup banner. Separated from `run_repl` so that
    /// `run_cli` can print the banner once (in a single location) and have
    /// both the fresh-REPL and resumed-REPL paths share the same loop.
    pub fn run_repl_loop(&mut self) -> Result<(), String> {
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

    /// Backwards-compatible wrapper that prints the startup banner and then
    /// runs the REPL loop. Kept for consumers that still call `run_repl`
    /// directly; `run_cli` no longer uses it because the banner is printed
    /// one level up for consistency across REPL / oneshot / resume.
    pub fn run_repl(&mut self) -> Result<(), String> {
        print_startup_banner(
            env!("CARGO_PKG_VERSION"),
            &format_model_banner(&self.models),
            &self.session.mode_state.mode,
            &self.work_root,
            short_id(&self.session.id),
            self.session.messages.len(),
            self.config.fresh_session,
            false,
            self.config.log_level,
        );
        self.run_repl_loop()
    }

    pub fn process_line(&mut self, input: &str, stream_output: bool) -> Result<AgentEvent, String> {
        if input.trim().is_empty() {
            return Ok(AgentEvent::Continue(None));
        }

        let outcome = if input.starts_with('/') {
            self.handle_command(input)
        } else {
            match self.handle_user_message(input, stream_output) {
                Ok((prose, stats)) => {
                    if !stream_output {
                        println!("{prose}");
                    }
                    let summary = format_run_summary(ExitReason::Done, &stats);
                    println!();
                    println!("{summary}");
                    Ok(AgentEvent::Continue(None))
                }
                Err((reason, error_text, stats)) => {
                    let summary = format_run_summary(reason, &stats);
                    println!();
                    println!("{summary}");
                    Err(error_text)
                }
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
                "/help /status /model /plan /approve /compact /logs /yes /no /exit".to_string(),
            ))),
            "/status" => Ok(AgentEvent::Continue(Some(format!(
                "mode={:?} auto_approve={} native_tools={} cwd={} session={} plan={} approx_tokens={} log_level={} core_only=true",
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
                self.config.log_level,
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
                let plan_path = self
                    .session
                    .mode_state
                    .enter_plan(self.session_store.plan_dir())?;
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
                if !lifecycle::plan_is_substantive(&plan_contents) {
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
            "/logs" => {
                let args: Vec<&str> = input
                    .trim_start_matches("/logs")
                    .split_whitespace()
                    .collect();
                match args.as_slice() {
                    ["path"] => {
                        let path = self.session_store.log_dir().display().to_string();
                        Ok(AgentEvent::Continue(Some(format!("log dir: {path}"))))
                    }
                    ["path", session_id] => match uuid::Uuid::parse_str(session_id) {
                        Err(_) => Ok(AgentEvent::Continue(Some(format!(
                            "invalid session id: {session_id}"
                        )))),
                        Ok(_) => match self.session_store.log_dir_for(session_id) {
                            Some(path) => Ok(AgentEvent::Continue(Some(format!(
                                "log dir: {}",
                                path.display()
                            )))),
                            None => Ok(AgentEvent::Continue(Some(format!(
                                "session not found: {session_id}"
                            )))),
                        },
                    },
                    _ => Ok(AgentEvent::Continue(Some(
                        "/logs path [<session_id>] — show log dir".to_string(),
                    ))),
                }
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
}
