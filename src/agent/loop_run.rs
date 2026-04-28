use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::thread;

use crate::agent::prompting;
use crate::agent::recovery;
use crate::config::Config;
use crate::format_model_banner;
use crate::model_registry::RuntimeModels;
use crate::modes::plan_act::ExecutionMode;
use crate::ollama::client::{AssistantReply, OllamaClient, should_use_native_tool_calls};
use crate::ollama::xml_fallback::ToolCall;
use crate::safety::path_guard::resolve_user_path;
use crate::session::compact::{
    approximate_token_count, compact_messages, compact_messages_with_strategy,
};
use crate::session::store::{ConversationMessage, SessionSnapshot, SessionStore};
use crate::stdin_prompt;
use crate::system_prompt::build_system_prompt;
use crate::tools::registry::{ToolContext, ToolRegistry};

mod auto_test;
pub mod commands;
mod deterministic;
mod footer;
mod interrupt;
mod lifecycle;
mod protocol;
mod quality;
mod reminder;
pub mod slash_commands;
mod spinner;
mod summary;
mod turn;

// Public re-exports so `lib.rs::run_cli` can hand a `FooterHandle` into
// `Agent::new` and own the matching `FooterLease` for its scope (issue #430).
pub use footer::{FooterHandle, FooterLease};

// Re-export env helpers from `turn` so `src/tui/markdown.rs` can reuse the
// existing POSIX-compliant NO_COLOR and UTF-8 locale logic without duplicating
// it. `mod turn;` itself stays private; only these two fns leak out (issue #431).
pub(crate) use turn::{no_color_requested, unicode_supported};

// Issue #453: expose the precaution prompt selector so integration tests in
// `tests/` (and any future callers) can validate the Act-mode prompt
// selection pipeline without requiring a live Ollama call.
pub use turn::select_precautions_for_prompt;

const DEFAULT_KEEP_TAIL: usize = 24;
const LATE_TURN_KEEP_TAIL: usize = 12;

pub enum AgentEvent {
    Continue(Option<String>),
    Exit,
}

pub struct Agent {
    config: Config,
    models: RuntimeModels,
    client: OllamaClient,
    session_store: SessionStore,
    session: SessionSnapshot,
    work_root: PathBuf,
    native_tools_enabled: bool,
    plan_model_override: Option<String>,
    tool_registry: ToolRegistry,
    repo_context_cache: Option<RepoContextCache>,
    /// Fixed footer handle. Phase A: always disabled (no-op); the handle
    /// shape is plumbed now so Phase B-D can attach `publish_*` calls
    /// without re-touching `Agent::new` callers (issue #430).
    #[allow(dead_code)]
    footer: FooterHandle,
    /// Per-turn cap for the Reminder Sidecar (#452). Reset at the top of every
    /// `handle_user_message`, set to `true` only when an actual sidecar call
    /// was attempted (Completed/Failed); Skipped does not consume the cap.
    reminder_called_this_turn: bool,
    /// Issue #456: tracks whether `compute_anvil_score` has already run for
    /// the current turn. Reset at the top of every `handle_user_message`,
    /// flipped to `true` after the post-loop compute writes
    /// `session.last_anvil_score`. Used by `maybe_invoke_reminder` to pick
    /// between `AnvilScoreSnapshot::PreviousTurn` (iteration-internal hook,
    /// score is the previous turn's persisted value) and
    /// `AnvilScoreSnapshot::CurrentTurn` (post-loop hook, score is the value
    /// just computed for this turn).
    pub(super) anvil_score_computed_this_turn: bool,
}

#[derive(Clone)]
struct RepoContextCache {
    task: String,
    work_root: PathBuf,
    message: Option<ConversationMessage>,
}

impl Agent {
    pub fn new(
        config: Config,
        models: RuntimeModels,
        client: OllamaClient,
        session_store: SessionStore,
        session: SessionSnapshot,
        footer: FooterHandle,
    ) -> Self {
        let work_root = session
            .active_root
            .clone()
            .unwrap_or_else(|| config.cwd.clone());
        let native_tools_enabled =
            should_use_native_tool_calls(&models.main) && !session.native_tools_disabled;
        Self {
            config,
            models,
            client,
            session_store,
            session,
            work_root,
            native_tools_enabled,
            plan_model_override: None,
            tool_registry: ToolRegistry::default(),
            repo_context_cache: None,
            footer,
            reminder_called_this_turn: false,
            anvil_score_computed_this_turn: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::lifecycle::{format_tool_error, should_compact_late_turn};
    use crate::agent::prompting::{
        compact_tool_result, detect_created_project_root, should_skip_system_note,
    };
    use crate::session::store::ConversationMessage;
    use std::path::PathBuf;

    #[test]
    fn detects_created_next_app_root_from_success_line() {
        let output = "Success! Created sample-app at /tmp/work/sample-app";
        assert_eq!(
            detect_created_project_root(output),
            Some(PathBuf::from("/tmp/work/sample-app"))
        );
    }

    #[test]
    fn detects_created_next_app_root_from_create_line() {
        let output = "Creating a new Next.js app in /tmp/work/sample-app.";
        assert_eq!(
            detect_created_project_root(output),
            Some(PathBuf::from("/tmp/work/sample-app"))
        );
    }

    #[test]
    fn tool_errors_are_formatted_for_model_recovery() {
        assert_eq!(
            format_tool_error("failed to stat /tmp/missing: nope"),
            "Error: failed to stat /tmp/missing: nope"
        );
        assert_eq!(
            format_tool_error("Error: file not found"),
            "Error: file not found"
        );
    }

    #[test]
    fn read_results_are_compacted_for_transcript() {
        let long = "a".repeat(20_000);
        let compacted = compact_tool_result("Read", long);
        assert!(compacted.contains("[truncated"));
        assert!(compacted.len() < 13_000);
    }

    #[test]
    fn skips_duplicate_consecutive_system_note() {
        let messages = vec![ConversationMessage::system("same note".to_string())];
        assert!(should_skip_system_note(&messages, "same note"));
        assert!(!should_skip_system_note(&messages, "other note"));
    }

    #[test]
    fn skips_duplicate_system_note_with_tool_results_between() {
        let messages = vec![
            ConversationMessage::system("same note".to_string()),
            ConversationMessage::assistant(String::new(), Vec::new()),
            ConversationMessage::tool("Read".to_string(), "file contents".to_string()),
        ];
        assert!(should_skip_system_note(&messages, "same note"));
    }

    #[test]
    fn late_turn_compaction_triggers_for_edit_heavy_turns() {
        let messages = (0..18)
            .map(|index| ConversationMessage::user(format!("message {index}")))
            .collect::<Vec<_>>();
        assert!(should_compact_late_turn(&messages, 24_000, 4, 1));
        assert!(!should_compact_late_turn(&messages[..8], 24_000, 1, 0));
    }
}
