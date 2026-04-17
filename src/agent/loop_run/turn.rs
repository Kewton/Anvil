use super::*;

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
        let turn_start_len = self.session.messages.len();

        if action_expectation == recovery::ActionExpectation::RepoChange {
            let actor_plan_note =
                orchestration::build_actor_plan(&self.client, &self.models, &self.work_root, input)
                    .ok()
                    .map(|plan| format!("[Actor Plan]\n{}", plan.summary));
            if let Some(note) = &actor_plan_note {
                self.push_system_note(note.clone());
            }

            let baseline = orchestration::capture_repo_snapshot(&self.work_root);
            match self.run_actor_loop(action_expectation, requires_action, stream_output, false) {
                Ok(reply) => return Ok(reply),
                Err(err) => {
                    let failure = orchestration::classify_failure(&err);
                    if orchestration::should_restart_actor(failure) {
                        let verification =
                            orchestration::verify_repo_progress(&baseline, &self.work_root);
                        orchestration::strip_restart_heavy_messages(
                            &mut self.session.messages,
                            turn_start_len,
                        );
                        if let Some(note) = actor_plan_note {
                            self.push_system_note(note);
                        }
                        self.push_system_note(verification.summary_note());
                        self.push_system_note(format!(
                            "[Actor Restart] Previous actor ended with {:?}. {}",
                            failure,
                            verification.restart_note()
                        ));
                        return self.run_actor_loop(
                            action_expectation,
                            requires_action,
                            stream_output,
                            verification.prefers_converged_actor(),
                        );
                    }
                    return Err(err);
                }
            }
        }

        self.run_actor_loop(action_expectation, requires_action, stream_output, false)
    }

    fn run_actor_loop(
        &mut self,
        action_expectation: recovery::ActionExpectation,
        requires_action: bool,
        stream_output: bool,
        restart_convergence_mode: bool,
    ) -> Result<String, String> {
        let mut tool_calls_made_this_turn = 0usize;
        let mut repo_edit_calls_made_this_turn = 0usize;
        let mut empty_retries = 0usize;
        let mut no_tool_retries = 0usize;
        let mut repo_change_retries = 0usize;
        let mut recent_bash_commands = Vec::<String>::new();
        let mut install_commands_seen = 0usize;

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
                let mut emitted_bash_loop_note = false;
                for tool_call in prepared_tool_calls {
                    let tool_name = tool_call.name.clone();
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
            2
        } else {
            0
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
