use super::*;
use crate::config::LogLevel;

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

    pub fn run_repl(&mut self) -> Result<(), String> {
        println!("anvil {}", env!("CARGO_PKG_VERSION"));
        println!("{}", format_model_banner(&self.models));
        println!(
            "mode={:?} cwd={}",
            self.session.mode_state.mode,
            self.work_root.display()
        );
        if self.config.log_level >= LogLevel::Verbose
            && let Some(path) = crate::logging::llm_io_log_path()
        {
            println!("llm log={}", path.display());
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
