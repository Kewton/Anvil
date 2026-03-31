//! Prompt suggestion engine (Issue #221).
//!
//! Deterministic / heuristic-based suggestion generation.
//! No LLM calls — fixed template strings only (security: no raw user input embedding).

use crate::contracts::{RuntimeState, ToolLogView};
use crate::session::{SessionMessage, WorkingMemory};

/// Context required by the suggestion engine.
pub struct SuggestionContext<'a> {
    pub state: &'a RuntimeState,
    pub tool_logs: &'a [ToolLogView],
    pub working_memory: &'a WorkingMemory,
    pub recent_messages: Vec<&'a SessionMessage>,
    pub last_slash_command: Option<&'a str>,
    pub message_count: usize,
}

/// Generate a prompt suggestion based on heuristic rules.
///
/// Returns `None` when no suggestion applies or the state is not `Done`.
/// The caller is responsible for config guards (`suggestion_enabled`, `interactive`).
pub fn suggest(ctx: &SuggestionContext) -> Option<String> {
    // Only suggest in Done state
    if *ctx.state != RuntimeState::Done {
        return None;
    }

    // Rule 1: unresolved errors present
    if !ctx.working_memory.unresolved_errors.is_empty() {
        return Some("前回のエラーを修正してください".to_string());
    }

    // Rule 2: file.write / file.edit / file.edit_anchor completed in this turn
    if has_recent_file_edit_completed(ctx.tool_logs) {
        return Some("cargo test".to_string());
    }

    // Rule 3: after /plan add
    if ctx
        .last_slash_command
        .is_some_and(|cmd| cmd.starts_with("/plan add"))
    {
        return Some("/plan add で次のステップを追加、または作業を開始".to_string());
    }

    // Rule 4: consecutive same-tool pattern detection (placeholder for future expansion)
    // Currently no-op: raw message content is never echoed into suggestions.

    // Rule 5: empty session
    if ctx.message_count == 0 {
        return Some("/help でコマンド一覧を確認".to_string());
    }

    None
}

/// Check whether any file mutation tool completed in the current turn's tool logs.
fn has_recent_file_edit_completed(tool_logs: &[ToolLogView]) -> bool {
    tool_logs.iter().any(|log| {
        matches!(
            log.tool_name.as_str(),
            "file.write" | "file.edit" | "file.edit_anchor"
        ) && log.is_completed()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::ToolLogView;
    use crate::session::{MessageRole, SessionMessage, WorkingMemory};

    fn empty_working_memory() -> WorkingMemory {
        WorkingMemory::default()
    }

    fn done_state() -> RuntimeState {
        RuntimeState::Done
    }

    fn make_message(role: MessageRole, content: &str) -> SessionMessage {
        SessionMessage::new(role, "test", content)
    }

    #[test]
    fn test_suggest_non_done_state() {
        let wm = empty_working_memory();
        let ctx = SuggestionContext {
            state: &RuntimeState::Thinking,
            tool_logs: &[],
            working_memory: &wm,
            recent_messages: vec![],
            last_slash_command: None,
            message_count: 5,
        };
        assert_eq!(suggest(&ctx), None);
    }

    #[test]
    fn test_suggest_done_with_unresolved_errors() {
        let mut wm = empty_working_memory();
        wm.add_error("compilation failed");
        let state = done_state();
        let ctx = SuggestionContext {
            state: &state,
            tool_logs: &[],
            working_memory: &wm,
            recent_messages: vec![],
            last_slash_command: None,
            message_count: 3,
        };
        let suggestion = suggest(&ctx);
        assert_eq!(
            suggestion,
            Some("前回のエラーを修正してください".to_string())
        );
    }

    #[test]
    fn test_suggest_done_after_file_edit() {
        let wm = empty_working_memory();
        let state = done_state();
        let logs = vec![ToolLogView {
            tool_name: "file.edit".to_string(),
            action: "completed".to_string(),
            target: "src/main.rs".to_string(),
            elapsed_ms: None,
        }];
        let ctx = SuggestionContext {
            state: &state,
            tool_logs: &logs,
            working_memory: &wm,
            recent_messages: vec![],
            last_slash_command: None,
            message_count: 3,
        };
        assert_eq!(suggest(&ctx), Some("cargo test".to_string()));
    }

    #[test]
    fn test_suggest_done_after_file_write() {
        let wm = empty_working_memory();
        let state = done_state();
        let logs = vec![ToolLogView {
            tool_name: "file.write".to_string(),
            action: "completed".to_string(),
            target: "src/lib.rs".to_string(),
            elapsed_ms: None,
        }];
        let ctx = SuggestionContext {
            state: &state,
            tool_logs: &logs,
            working_memory: &wm,
            recent_messages: vec![],
            last_slash_command: None,
            message_count: 3,
        };
        assert_eq!(suggest(&ctx), Some("cargo test".to_string()));
    }

    #[test]
    fn test_suggest_done_after_file_edit_failed_no_suggestion() {
        let wm = empty_working_memory();
        let state = done_state();
        let logs = vec![ToolLogView {
            tool_name: "file.edit".to_string(),
            action: "failed".to_string(),
            target: "src/main.rs".to_string(),
            elapsed_ms: None,
        }];
        let ctx = SuggestionContext {
            state: &state,
            tool_logs: &logs,
            working_memory: &wm,
            recent_messages: vec![],
            last_slash_command: None,
            message_count: 3,
        };
        // failed file.edit should not trigger "cargo test"
        assert_eq!(suggest(&ctx), None);
    }

    #[test]
    fn test_suggest_after_plan_add() {
        let wm = empty_working_memory();
        let state = done_state();
        let ctx = SuggestionContext {
            state: &state,
            tool_logs: &[],
            working_memory: &wm,
            recent_messages: vec![],
            last_slash_command: Some("/plan add implement feature"),
            message_count: 3,
        };
        let suggestion = suggest(&ctx);
        assert!(suggestion.is_some());
        assert!(suggestion.unwrap().contains("/plan add"));
    }

    #[test]
    fn test_suggest_empty_session() {
        let wm = empty_working_memory();
        let state = done_state();
        let ctx = SuggestionContext {
            state: &state,
            tool_logs: &[],
            working_memory: &wm,
            recent_messages: vec![],
            last_slash_command: None,
            message_count: 0,
        };
        let suggestion = suggest(&ctx);
        assert_eq!(suggestion, Some("/help でコマンド一覧を確認".to_string()));
    }

    #[test]
    fn test_suggest_rule_priority_errors_over_file_edit() {
        let mut wm = empty_working_memory();
        wm.add_error("test failure");
        let state = done_state();
        let logs = vec![ToolLogView {
            tool_name: "file.edit".to_string(),
            action: "completed".to_string(),
            target: "src/main.rs".to_string(),
            elapsed_ms: None,
        }];
        let ctx = SuggestionContext {
            state: &state,
            tool_logs: &logs,
            working_memory: &wm,
            recent_messages: vec![],
            last_slash_command: None,
            message_count: 3,
        };
        // Rule 1 (errors) should take priority over Rule 2 (file edit)
        assert_eq!(
            suggest(&ctx),
            Some("前回のエラーを修正してください".to_string())
        );
    }

    #[test]
    fn test_suggest_rule4_never_echoes_raw_message_content() {
        let wm = empty_working_memory();
        let state = done_state();
        let secret_msg = make_message(MessageRole::User, "SECRET_CONTENT_12345");
        let ctx = SuggestionContext {
            state: &state,
            tool_logs: &[],
            working_memory: &wm,
            recent_messages: vec![&secret_msg],
            last_slash_command: None,
            message_count: 5,
        };
        let suggestion = suggest(&ctx);
        // Whether Some or None, the suggestion must not contain raw message content
        if let Some(ref s) = suggestion {
            assert!(
                !s.contains("SECRET_CONTENT_12345"),
                "suggestion must not echo raw message content"
            );
        }
    }

    #[test]
    fn test_suggest_done_no_applicable_rule() {
        let wm = empty_working_memory();
        let state = done_state();
        let logs = vec![ToolLogView {
            tool_name: "file.read".to_string(),
            action: "completed".to_string(),
            target: "src/main.rs".to_string(),
            elapsed_ms: None,
        }];
        let ctx = SuggestionContext {
            state: &state,
            tool_logs: &logs,
            working_memory: &wm,
            recent_messages: vec![],
            last_slash_command: None,
            message_count: 3,
        };
        assert_eq!(suggest(&ctx), None);
    }
}
