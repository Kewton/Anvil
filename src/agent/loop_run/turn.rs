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
