pub const MINIMAL_FEEDBACK_PREFIX: &str = "[minimal-feedback]";

#[derive(Debug, Default, Clone)]
pub struct FeedbackState {
    empty_response_sent: bool,
    missing_tool_sent: bool,
    edit_anchor_sent: bool,
}

impl FeedbackState {
    pub fn empty_response(&mut self) -> Option<String> {
        if self.empty_response_sent {
            return None;
        }
        self.empty_response_sent = true;
        Some(format!(
            "{MINIMAL_FEEDBACK_PREFIX}\nYour previous response was empty. Continue the task now. Use a tool if repository facts or files are needed."
        ))
    }

    pub fn missing_tool_call(&mut self, user_prompt: &str) -> Option<String> {
        if self.missing_tool_sent || !looks_like_repo_action(user_prompt) {
            return None;
        }
        self.missing_tool_sent = true;
        Some(format!(
            "{MINIMAL_FEEDBACK_PREFIX}\nThe user asked for a repository change. Use exactly one relevant tool call next instead of only describing the work."
        ))
    }

    pub fn edit_anchor_mismatch(&mut self, error: &str) -> Option<String> {
        if self.edit_anchor_sent || !is_edit_anchor_mismatch(error) {
            return None;
        }
        self.edit_anchor_sent = true;
        Some(format!(
            "{MINIMAL_FEEDBACK_PREFIX}\nYour Edit anchor did not match the file. Read the target file again, then issue a smaller exact Edit using text that is currently present."
        ))
    }

    pub fn malformed_tool_call(&self, error: &str) -> String {
        format!(
            "{MINIMAL_FEEDBACK_PREFIX}\nYour previous tool call was malformed and was not executed ({error}). Reply with exactly one complete <anvil_tool_call>{{\"name\":\"ToolName\",\"arguments\":{{...}}}}</anvil_tool_call> block, or plain text if the task is already complete."
        )
    }
}

fn is_edit_anchor_mismatch(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("target text not found") || lower.contains("old_string was not found")
}

fn looks_like_repo_action(prompt: &str) -> bool {
    let lower = prompt.to_ascii_lowercase();
    [
        "edit",
        "write",
        "create",
        "modify",
        "fix",
        "implement",
        "add",
        "delete",
        "update",
        "refactor",
        "test",
        "修正",
        "実装",
        "作成",
        "追加",
        "変更",
        "削除",
        "更新",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_is_single_use_per_kind() {
        let mut state = FeedbackState::default();

        assert!(state.empty_response().is_some());
        assert!(state.empty_response().is_none());
        assert!(state.missing_tool_call("fix src/lib.rs").is_some());
        assert!(state.missing_tool_call("fix src/lib.rs").is_none());
        assert!(
            state
                .edit_anchor_mismatch("target text not found in src/lib.rs")
                .is_some()
        );
        assert!(
            state
                .edit_anchor_mismatch("target text not found in src/lib.rs")
                .is_none()
        );
        assert!(
            state
                .malformed_tool_call("tool call parser failed")
                .starts_with(MINIMAL_FEEDBACK_PREFIX)
        );
    }

    #[test]
    fn missing_tool_feedback_only_for_action_prompts() {
        let mut state = FeedbackState::default();

        assert!(
            state
                .missing_tool_call("explain this architecture")
                .is_none()
        );
        assert!(state.missing_tool_call("README を修正して").is_some());
    }
}
