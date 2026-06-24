use std::collections::BTreeSet;

use anyhow::bail;

use crate::config::Config;
use crate::mode::ExecutionMode;
use crate::providers::ChatClient;
use crate::state::{ConversationMessage, SessionSnapshot};
use crate::tools::path_guard::resolve_existing;
use crate::tools::registry::{ToolContext, ToolRegistry};

use super::compact::compact_if_needed;
use super::prompt::build_request_messages;

pub fn run_session(
    client: &mut dyn ChatClient,
    session: &mut SessionSnapshot,
    user_prompt: &str,
    config: &Config,
) -> anyhow::Result<String> {
    run_session_with_required_paths(client, session, user_prompt, &[], config)
}

pub fn run_session_with_required_paths(
    client: &mut dyn ChatClient,
    session: &mut SessionSnapshot,
    user_prompt: &str,
    required_paths: &[String],
    config: &Config,
) -> anyhow::Result<String> {
    let registry = ToolRegistry::default();
    let mut native_tools_enabled =
        client.supports_native_tools(&config.model) && !session.native_tools_disabled;
    let mut pending_feedback: Option<String> = None;
    let mut write_or_edit_seen = false;
    let mut no_tool_feedbacks = 0usize;
    session
        .messages
        .push(ConversationMessage::user(user_prompt.to_string()));

    for _ in 0..config.max_iterations {
        compact_if_needed(&mut session.messages, config.context_budget);
        let specs = registry.specs().to_vec();
        let request_tools = if native_tools_enabled {
            specs.clone()
        } else {
            Vec::new()
        };
        let request_messages = build_request_messages(
            &session.messages,
            &specs,
            &config.workspace_root,
            pending_feedback.as_deref(),
        );
        let reply = match client.chat(
            &config.model,
            &request_messages,
            &request_tools,
            native_tools_enabled,
        ) {
            Ok(reply) => {
                pending_feedback = None;
                reply
            }
            Err(err) if native_tools_enabled && client.allows_xml_fallback() => {
                native_tools_enabled = false;
                session.native_tools_disabled = true;
                pending_feedback = Some(super::feedback::malformed_tool_call(&err.to_string()));
                continue;
            }
            Err(err) => return Err(err),
        };
        let tool_calls = reply.tool_calls.clone();
        session.messages.push(ConversationMessage::assistant(
            reply.content.clone(),
            tool_calls.clone(),
        ));
        if tool_calls.is_empty() {
            let missing = missing_paths(&config.workspace_root, required_paths);
            if !missing.is_empty() {
                session.messages.pop();
                pending_feedback = Some(super::feedback::missing_artifacts(&missing));
                continue;
            }
            if !write_or_edit_seen
                && looks_like_progress_without_tool(&reply.content)
                && no_tool_feedbacks < 3
            {
                no_tool_feedbacks += 1;
                session.messages.pop();
                pending_feedback = Some(super::feedback::no_tool_progress());
                continue;
            }
            return Ok(reply.content);
        }

        let context = ToolContext {
            root: config.workspace_root.clone(),
            mode: ExecutionMode::Act,
            auto_approve: config.yes,
            interactive_approval: false,
            offline: config.offline,
        };
        let mut names_seen = BTreeSet::new();
        for call in tool_calls {
            if !names_seen.insert(call.name.clone()) {
                // Multiple same-tool calls are fine; this keeps clippy from seeing unused state.
            }
            if matches!(call.name.as_str(), "Write" | "Edit") {
                write_or_edit_seen = true;
            }
            let result = registry.execute(&call.name, &call.arguments, &context)?;
            session.messages.push(ConversationMessage::tool_result(
                call.name,
                Some(call.id),
                result,
            ));
        }
    }
    bail!(
        "minimal loop reached max_iterations ({})",
        config.max_iterations
    )
}

fn missing_paths(root: &std::path::Path, required_paths: &[String]) -> Vec<String> {
    required_paths
        .iter()
        .filter(|path| resolve_existing(root, path).is_err())
        .cloned()
        .collect()
}

fn looks_like_progress_without_tool(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    lower.contains("i will")
        || lower.contains("next")
        || lower.contains("作成します")
        || lower.contains("実装します")
        || lower.contains("進めます")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::AssistantReply;
    use crate::state::ToolCall;
    use serde_json::json;

    struct Fake {
        replies: Vec<anyhow::Result<AssistantReply>>,
    }

    impl ChatClient for Fake {
        fn label(&self) -> &str {
            "fake"
        }
        fn supports_native_tools(&self, _model: &str) -> bool {
            true
        }
        fn chat(
            &mut self,
            _model: &str,
            _messages: &[ConversationMessage],
            _tools: &[crate::tools::registry::ToolSpec],
            _native_tools_enabled: bool,
        ) -> anyhow::Result<AssistantReply> {
            self.replies.remove(0)
        }
    }

    fn config(root: std::path::PathBuf) -> Config {
        Config {
            workspace_root: root,
            state_dir: std::path::PathBuf::from("state"),
            yes: true,
            offline: false,
            context_budget: 1000,
            model: "m".to_string(),
            provider: crate::config::Provider::Ollama,
            planner_model: "m".to_string(),
            planner_provider: crate::config::Provider::Ollama,
            ollama_host: "http://localhost:11434".to_string(),
            num_predict: 100,
            max_iterations: 4,
            chat_timeout_secs: 1,
            chat_retries: 1,
            resume: None,
            fresh_session: false,
            profile: "generic".to_string(),
            style: "default".to_string(),
            action: crate::config::Action::Repl,
        }
    }

    #[test]
    fn fake_write_then_final() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"a.txt","content":"ok"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
                Ok(AssistantReply::text("done")),
            ],
        };
        let mut session = SessionSnapshot::new();
        let result = run_session_with_required_paths(
            &mut fake,
            &mut session,
            "create a.txt",
            &["a.txt".to_string()],
            &config(dir.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(result, "done");
    }
}
