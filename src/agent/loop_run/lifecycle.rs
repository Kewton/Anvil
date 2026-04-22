use super::*;

impl Agent {
    pub(super) fn current_plan_contents(&self) -> Result<Option<String>, String> {
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

    pub(super) fn persist_session(&mut self) -> Result<(), String> {
        self.session.active_root = if self.work_root == self.config.cwd {
            None
        } else {
            Some(self.work_root.clone())
        };
        self.session_store.save(&self.session)
    }

    pub(super) fn maybe_compact_session(&mut self, keep_tail: usize) -> bool {
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

    pub(super) fn maybe_compact_late_turn_session(
        &mut self,
        tool_calls_this_turn: usize,
        repo_edit_calls_this_turn: usize,
    ) -> bool {
        if !should_compact_late_turn(
            &self.session.messages,
            self.config.context_budget,
            tool_calls_this_turn,
            repo_edit_calls_this_turn,
        ) {
            return false;
        }

        self.maybe_compact_session(crate::agent::loop_run::LATE_TURN_KEEP_TAIL)
    }

    pub(super) fn ensure_plan_file(&self, plan_path: &Path) -> Result<(), String> {
        if plan_path.exists() {
            return Ok(());
        }
        if let Some(parent) = plan_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
        }
        std::fs::write(
            plan_path,
            "# Plan\n\n## Goal\n- \n\n## Constraints\n- \n\n## Deliverables\n- \n\n## Acceptance Criteria\n- \n\n## Execution Plan\n1. First slice:\n2. Next phases:\n3. Review checkpoint:\n\n## Verification Plan\n- \n\n## Risks / Fallbacks\n- \n",
        )
        .map_err(|err| format!("failed to create plan file {}: {err}", plan_path.display()))
    }

    pub(super) fn maybe_update_work_root(
        &mut self,
        name: &str,
        arguments: &serde_json::Value,
        result: &str,
    ) {
        let Some(new_root) = prompting::detect_scaffold_root(name, arguments, result) else {
            return;
        };
        if new_root == self.work_root || !new_root.is_dir() {
            return;
        }

        self.apply_scaffold_root(new_root);
    }

    pub(super) fn disable_native_tools_for_session(&mut self) {
        self.native_tools_enabled = false;
        self.session.native_tools_disabled = true;
        self.push_system_note(prompting::ToolProtocol::TaggedXml.parser_downgrade_notice());
    }

    fn apply_scaffold_root(&mut self, new_root: PathBuf) {
        self.work_root = new_root.clone();
        self.session.active_root = Some(new_root.clone());
        self.push_system_note(format!(
            "[Workspace Root Updated] Continue work inside {} and use relative paths from there.",
            new_root.display()
        ));
    }
}

pub(super) fn should_compact(
    messages: &[ConversationMessage],
    context_budget: usize,
    keep_tail: usize,
) -> bool {
    messages.len() > keep_tail + 4 || approximate_token_count(messages) > context_budget
}

pub(super) fn should_compact_late_turn(
    messages: &[ConversationMessage],
    context_budget: usize,
    tool_calls_this_turn: usize,
    repo_edit_calls_this_turn: usize,
) -> bool {
    if repo_edit_calls_this_turn == 0 && tool_calls_this_turn < 4 {
        return false;
    }

    messages.len() >= 16 || approximate_token_count(messages) > context_budget.saturating_mul(3) / 5
}

pub(super) fn is_native_tool_parser_failure(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("native tool parser failed")
        || lower.contains("unexpected end element")
        || lower.contains("unexpected eof")
}

pub(super) fn is_tool_call_format_error(error: &str) -> bool {
    error
        .to_ascii_lowercase()
        .contains("tool call parser failed:")
}

pub(super) fn is_transport_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("failed to contact ollama chat api")
        || lower.contains("error sending request for url")
        || lower.contains("connection reset")
        || lower.contains("connection refused")
        || lower.contains("broken pipe")
        || lower.contains("ollama /api/chat failed: 5")
        || lower.contains("ollama /api/chat failed: 429")
}

pub(super) fn plan_is_substantive(contents: &str) -> bool {
    let meaningful_lines = contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !matches!(*line, "# Plan" | "## Goal" | "## Findings" | "## Steps"))
        .filter(|line| *line != "-" && *line != "1.")
        .count();
    meaningful_lines >= 2
}

pub(super) fn format_tool_error(err: &str) -> String {
    if err.starts_with("Error:") {
        err.to_string()
    } else {
        format!("Error: {err}")
    }
}
