use super::small_helpers::raw_mode_safe_text;
use super::tool_history::{is_preferred_read_edit_target, latest_user_turn_slice};
use super::workspace_walk::workspace_appears_empty;
use super::*;
use crate::session::store::ScaffoldArtifactFileSnapshot;
use std::path::{Path, PathBuf};

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

pub(super) fn extract_filename_with_suffix(text: &str, suffix: &str) -> Option<String> {
    text.split(|ch: char| {
        ch.is_whitespace()
            || matches!(
                ch,
                '`' | '"'
                    | '\''
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '、'
                    | '。'
                    | '，'
                    | '：'
                    | ':'
                    | ';'
            )
    })
    .map(|token| token.trim_matches([',', '.', '。', '、']))
    .find(|token| {
        token.ends_with(suffix)
            && token.len() <= 80
            && !token.contains('/')
            && !token.contains('\\')
            && !token.starts_with('.')
            && token
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    })
    .map(ToString::to_string)
}

pub(super) fn write_stdout_rendered(text: &str, trailing_newline: bool) {
    let mut out = io::stdout().lock();
    let rendered = raw_mode_safe_text(text);
    let _ = out.write_all(rendered.as_bytes());
    if trailing_newline {
        let _ = out.write_all(b"\r\n");
    }
    let _ = out.flush();
}

pub(super) fn tool_result_failed(result: &str) -> bool {
    result.starts_with("Error:") || result.contains("\ninterrupted=true\n")
}

/// Issue #555: carries a retrieval message and the IDs of the selected
/// records so the photon mapper can include them without re-parsing the
/// rendered prompt text.
pub(super) struct RetrievalInjection {
    pub message: ConversationMessage,
    pub selected_ids: Vec<String>,
}

pub(super) type WrittenScaffoldArtifacts = (Vec<PathBuf>, Vec<ScaffoldArtifactFileSnapshot>);

/// Issue #580: SSoT memoization key for the Quality-gate second-pass adapter.
/// Hashes `(request, full_content)` with `DefaultHasher` (per design judgement
/// #5: full_content avoids stale reuse when only the middle of a large file
/// changes — the LLM still sees only the head+tail excerpt).
pub(super) fn quality_confirm_cache_key(request: &str, content: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    request.hash(&mut hasher);
    content.hash(&mut hasher);
    hasher.finish()
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

pub(super) fn last_read_tool_path(messages: &[ConversationMessage]) -> Option<String> {
    messages.iter().rev().find_map(|message| {
        if message.role != "assistant" {
            return None;
        }
        message.tool_calls.iter().rev().find_map(|tool_call| {
            if tool_call.name != "Read" {
                return None;
            }
            tool_call
                .arguments
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
        })
    })
}

pub(super) fn latest_turn_preferred_read_edit_target(
    messages: &[ConversationMessage],
    work_root: &Path,
) -> Option<PathBuf> {
    let mut latest_existing = None;
    for message in latest_user_turn_slice(messages).iter().rev() {
        if message.role != "assistant" {
            continue;
        }
        for tool_call in message.tool_calls.iter().rev() {
            if tool_call.name != "Read" {
                continue;
            }
            let Some(path) = tool_call
                .arguments
                .get("path")
                .and_then(serde_json::Value::as_str)
            else {
                continue;
            };
            let Ok(candidate) = resolve_user_path(work_root, path) else {
                continue;
            };
            if !candidate.is_file() {
                continue;
            }
            latest_existing.get_or_insert_with(|| candidate.clone());
            if is_preferred_read_edit_target(&candidate) {
                return Some(candidate);
            }
        }
    }
    latest_existing
}
