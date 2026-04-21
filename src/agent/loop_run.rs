use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

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

pub mod commands;
mod lifecycle;
mod spinner;
mod summary;
mod turn;

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
    tool_registry: ToolRegistry,
}

impl Agent {
    pub fn new(
        config: Config,
        models: RuntimeModels,
        client: OllamaClient,
        session_store: SessionStore,
        session: SessionSnapshot,
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
            tool_registry: ToolRegistry::default(),
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
        let output = "Success! Created space-invaders at /tmp/work/space-invaders";
        assert_eq!(
            detect_created_project_root(output),
            Some(PathBuf::from("/tmp/work/space-invaders"))
        );
    }

    #[test]
    fn detects_created_next_app_root_from_create_line() {
        let output = "Creating a new Next.js app in /tmp/work/space-invaders.";
        assert_eq!(
            detect_created_project_root(output),
            Some(PathBuf::from("/tmp/work/space-invaders"))
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
