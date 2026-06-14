pub const MINIMAL_FEEDBACK_PREFIX: &str = "[minimal-feedback]";

#[derive(Debug, Default, Clone)]
pub struct FeedbackState {
    empty_response_sent: bool,
    completion_without_write_sent: bool,
    requested_artifact_sent: bool,
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

    pub fn completion_without_write(&mut self) -> Option<String> {
        if self.completion_without_write_sent {
            return None;
        }
        self.completion_without_write_sent = true;
        Some(format!(
            "{MINIMAL_FEEDBACK_PREFIX}\nNo file changes have been made in this session. If the task requires creating or modifying files, do that now with Write or Edit. If no file change is needed, say so and finish."
        ))
    }

    pub fn requested_artifacts_missing(&mut self, missing_paths: &[String]) -> Option<String> {
        if self.requested_artifact_sent || missing_paths.is_empty() {
            return None;
        }
        self.requested_artifact_sent = true;
        let list = missing_paths.join(", ");
        Some(format!(
            "{MINIMAL_FEEDBACK_PREFIX}\nThe requested file(s) are still missing: {list}. Create them with Write now, or explain the blocker if they should not be created."
        ))
    }

    pub fn planned_action_without_tool(&mut self, assistant_content: &str) -> Option<String> {
        if !looks_like_planned_tool_action(assistant_content) {
            return None;
        }
        Some(format!(
            "{MINIMAL_FEEDBACK_PREFIX}\nYour previous response described a next action, but no tool call was issued. If that action is needed, call the tool now. If the work is already complete or no tool is needed, answer with completed work only."
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

    pub fn malformed_tool_call_xml_fallback(&self, error: &str) -> String {
        format!(
            "{MINIMAL_FEEDBACK_PREFIX}\nYour previous tool call was malformed and was not executed ({error}). Native tool calls are disabled for the rest of this session. Reply with exactly one complete <anvil_tool_call>{{\"name\":\"ToolName\",\"arguments\":{{...}}}}</anvil_tool_call> block, or plain text if the task is already complete."
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

fn looks_like_planned_tool_action(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    let has_future_marker = [
        "i will ",
        "i'll ",
        "let me ",
        "now let me ",
        "next, i ",
        "i am going to ",
        "i'm going to ",
        "これから",
        "今から",
        "次に",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    if !has_future_marker {
        return false;
    }

    [
        "create",
        "write",
        "edit",
        "modify",
        "read",
        "inspect",
        "check",
        "verify",
        "test",
        "run",
        "build",
        "起動",
        "作成",
        "書き",
        "編集",
        "修正",
        "確認",
        "検証",
        "実行",
        "ビルド",
    ]
    .iter()
    .any(|verb| lower.contains(verb))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_is_single_use_per_kind() {
        let mut state = FeedbackState::default();

        assert!(state.empty_response().is_some());
        assert!(state.empty_response().is_none());
        assert!(state.completion_without_write().is_some());
        assert!(state.completion_without_write().is_none());
        assert!(
            state
                .requested_artifacts_missing(&["src/main.rs".to_string()])
                .is_some()
        );
        assert!(
            state
                .requested_artifacts_missing(&["src/main.rs".to_string()])
                .is_none()
        );
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
                .malformed_tool_call_xml_fallback("tool call parser failed")
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

    #[test]
    fn planned_action_feedback_requires_future_tool_action() {
        let mut state = FeedbackState::default();
        assert!(
            state
                .planned_action_without_tool("Now let me create the Next.js app.")
                .is_some()
        );

        let mut state = FeedbackState::default();
        assert!(
            state
                .planned_action_without_tool("The files are complete and tests passed.")
                .is_none()
        );

        let mut state = FeedbackState::default();
        assert!(
            state
                .planned_action_without_tool("I will explain the design tradeoffs.")
                .is_none()
        );
    }
}
