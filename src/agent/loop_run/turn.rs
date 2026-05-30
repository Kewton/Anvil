use super::workspace_walk::workspace_appears_empty;
use super::*;

use super::quality::repo_change_request_text;

/// Maximum number of characters of tool-call arguments retained in trace logs.
pub(super) const LOG_ARGS_MAX_CHARS: usize = 200;

// Issue #634: SSOT for specialized-fallback ログ event 名。emit 側 / test 側の
// 双方が参照し、typo による検証無効化を防ぐ。文字列値そのものは既存テスト互換の
// ため不変。`EVENT_DETERMINISTIC_PYTHON_TEST_FALLBACK` は本 Issue で新規追加。
pub(super) const PLAN_REPEATED_EXPLORATION_BLOCK_THRESHOLD: usize = 2;
pub(super) const TASK_CONTRACT_VERIFIER_ATTEMPT_LIMIT: usize = 3;
pub(super) const TASK_CONTRACT_VERIFIER_REPAIR_ATTEMPT_LIMIT: usize = 6;
pub(super) const USER_INTERRUPT_ERROR: &str = "__anvil_user_interrupt__";

#[derive(Debug, Clone, Copy)]
pub(super) struct AssistantReplyRetryState {
    pub(super) downgraded_native_tools: bool,
    pub(super) retries_remaining: usize,
    pub(super) tool_call_format_retries_remaining: usize,
    pub(super) extra_transport_retries: usize,
    pub(super) transport_retry_count: usize,
    pub(super) focused_edit_timeout_retry_count: usize,
    pub(super) tool_call_format_retry_count: usize,
}

pub(super) enum AssistantReplyRetryDecision {
    Retry,
    ReturnReply(AssistantReply),
    Fail(String),
}

impl AssistantReplyRetryState {
    pub(super) fn new(chat_retries: usize, message_count: usize) -> Self {
        Self {
            downgraded_native_tools: false,
            retries_remaining: chat_retries,
            tool_call_format_retries_remaining: 2,
            extra_transport_retries: if message_count >= 12 { 4 } else { 2 },
            transport_retry_count: 0,
            focused_edit_timeout_retry_count: 0,
            tool_call_format_retry_count: 0,
        }
    }
}

impl Agent {
    /// Issue #646: build the active `TaskWorkspaceScope` for the current
    /// task. Pure projection of `work_root` + the active user request; no
    /// filesystem mutation. Called from `task_contract_artifact_states` and
    /// `task_contract_recovery_target`; not cached because the bounded
    /// shallow read of `work_root` is cheap and the scope is recomputed
    /// only a handful of times per turn.
    pub(super) fn current_workspace_scope(
        &self,
    ) -> super::task_workspace_scope::TaskWorkspaceScope {
        let request = self.active_request_text().unwrap_or_default();
        super::task_workspace_scope::TaskWorkspaceScope::detect(&self.work_root, &request)
    }

    pub(super) fn workspace_appears_empty(&self) -> bool {
        workspace_appears_empty(&self.work_root)
    }

    pub(super) fn active_task_expects_repo_change(&self) -> bool {
        self.session.mode_state.mode == ExecutionMode::Act
            && self.session.mode_state.policy().repo_edit_required
            && self.active_request_text().as_deref().is_some_and(|task| {
                recovery::classify_action_expectation(task, self.session.mode_state.mode)
                    == recovery::ActionExpectation::RepoChange
            })
    }

    pub(super) fn active_request_text(&self) -> Option<String> {
        repo_change_request_text(
            self.session.working_memory.active_task.as_deref(),
            &self.session.messages,
        )
    }

    pub(super) fn push_system_note(&mut self, note: String) {
        if prompting::should_skip_system_note(&self.session.messages, &note) {
            return;
        }
        self.session
            .messages
            .push(ConversationMessage::system(note));
    }

    pub(super) fn push_user_message(&mut self, content: String) {
        self.session
            .working_memory
            .set_active_task(Some(content.clone()));
        self.session
            .messages
            .push(ConversationMessage::user(content));
    }
}
