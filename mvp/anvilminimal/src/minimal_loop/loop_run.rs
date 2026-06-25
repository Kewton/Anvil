use std::collections::BTreeSet;
use std::path::Path;

use anyhow::bail;
use serde_json::json;

use crate::config::Config;
use crate::eval_events;
use crate::mode::ExecutionMode;
use crate::providers::ChatClient;
use crate::state::{ConversationMessage, SessionSnapshot};
use crate::tools::path_guard::{
    resolve_existing, resolve_optional_existing, validate_workspace_relative,
};
use crate::tools::registry::{
    ToolContext, ToolRegistry, missing_arg_name, recoverable_tool_error, tool_error_kind,
};
use crate::tui::status::UiStatus;
use crate::tui::{InteractionUi, NOOP_UI};

use super::compact::compact_if_needed;
use super::prompt::{ToolPromptMode, build_request_messages};

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
    run_session_with_required_paths_with_ui(
        client,
        session,
        user_prompt,
        required_paths,
        config,
        &NOOP_UI,
    )
}

pub fn run_session_with_required_paths_with_ui(
    client: &mut dyn ChatClient,
    session: &mut SessionSnapshot,
    user_prompt: &str,
    required_paths: &[String],
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<String> {
    let registry = ToolRegistry::default();
    let mut native_tools_enabled =
        client.supports_native_tools(&config.model) && !session.native_tools_disabled;
    let required_paths =
        effective_required_paths(&config.workspace_root, required_paths, user_prompt);
    let initially_missing_paths = missing_paths(&config.workspace_root, &required_paths);
    let mut pending_feedback: Option<String> = None;
    let mut write_or_edit_seen = false;
    let mut no_tool_feedbacks = 0usize;
    let mut empty_feedbacks = 0usize;
    session
        .messages
        .push(ConversationMessage::user(user_prompt.to_string()));

    for _ in 0..config.max_iterations {
        if ui.interrupted() {
            bail!("interrupted by user");
        }
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
            if native_tools_enabled {
                ToolPromptMode::Native
            } else {
                ToolPromptMode::XmlFallback
            },
        );
        let label = format!("{} {}", client.label(), config.model);
        let chat_result = {
            let _guard = ui.before_model_call(&label);
            client.chat(
                &config.model,
                &request_messages,
                &request_tools,
                native_tools_enabled,
            )
        };
        let reply = match chat_result {
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
        ui.publish_status(UiStatus::for_model_reply(
            config,
            &config.model,
            client.label(),
            reply.prompt_tokens,
            reply.completion_tokens,
        ));
        if ui.interrupted() {
            bail!("interrupted by user");
        }
        let tool_calls = reply.tool_calls.clone();
        session.messages.push(ConversationMessage::assistant(
            reply.content.clone(),
            tool_calls.clone(),
        ));
        if tool_calls.is_empty() {
            let missing = missing_paths(&config.workspace_root, &required_paths);
            if !missing.is_empty() {
                session.messages.pop();
                pending_feedback = Some(super::feedback::missing_artifacts(&missing));
                continue;
            }
            if reply.content.trim().is_empty() && empty_feedbacks < 1 {
                empty_feedbacks += 1;
                session.messages.pop();
                pending_feedback = Some(super::feedback::empty_response());
                continue;
            }
            if !write_or_edit_seen && looks_like_action_prompt(user_prompt) {
                if no_tool_feedbacks < 1 {
                    no_tool_feedbacks += 1;
                    session.messages.pop();
                    pending_feedback = Some(super::feedback::completion_without_write());
                    continue;
                }
                session.messages.pop();
                bail!("missing tool call for action prompt after feedback");
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
            if ui.interrupted() {
                bail!("interrupted by user");
            }
            if !names_seen.insert(call.name.clone()) {
                // Multiple same-tool calls are fine; this keeps clippy from seeing unused state.
            }
            let shape = eval_events::argument_shape(&call.arguments);
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "tool_call_raw",
                    "name": call.name.as_str(),
                    "arguments": shape,
                }),
            );
            let result = {
                let _guard = ui.before_tool_call(&call.name);
                registry.execute(&call.name, &call.arguments, &context)
            };
            let result = match result {
                Ok(result) => {
                    eval_events::emit(
                        config.eval_events_path.as_deref(),
                        json!({
                            "event": "tool_execute",
                            "name": call.name.as_str(),
                            "status": "ok",
                        }),
                    );
                    if matches!(call.name.as_str(), "Write" | "Edit") {
                        write_or_edit_seen = true;
                    }
                    result
                }
                Err(err) if recoverable_tool_error(&err) => {
                    let kind = tool_error_kind(&err);
                    eval_events::emit(
                        config.eval_events_path.as_deref(),
                        json!({
                            "event": "tool_validation_error",
                            "name": call.name.as_str(),
                            "error_kind": kind,
                            "missing_arg": missing_arg_name(&err),
                        }),
                    );
                    recoverable_tool_feedback(&call.name, &err)
                }
                Err(err) => {
                    eval_events::emit(
                        config.eval_events_path.as_deref(),
                        json!({
                            "event": "tool_execute",
                            "name": call.name.as_str(),
                            "status": "error",
                            "error_kind": tool_error_kind(&err),
                        }),
                    );
                    return Err(err);
                }
            };
            session.messages.push(ConversationMessage::tool_result(
                call.name,
                Some(call.id),
                result,
            ));
        }
        if required_paths_satisfied_after_tool(
            &config.workspace_root,
            &required_paths,
            &initially_missing_paths,
            write_or_edit_seen,
        ) {
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "loop_stop",
                    "reason": "required_artifacts_satisfied_after_tool",
                    "required_paths": required_paths,
                }),
            );
            return Ok(format!(
                "required artifacts satisfied: {}",
                required_paths.join(", ")
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

fn effective_required_paths(root: &Path, explicit: &[String], prompt: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for path in explicit
        .iter()
        .cloned()
        .chain(extract_requested_artifact_paths(root, prompt))
    {
        if seen.insert(path.clone()) {
            out.push(path);
        }
    }
    out
}

pub(crate) fn extract_requested_artifact_paths(root: &Path, prompt: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut seen = BTreeSet::new();
    let mut in_required_block = false;
    for line in prompt.lines() {
        let trimmed = line.trim();
        if trimmed
            .to_ascii_lowercase()
            .starts_with("required final artifacts")
        {
            in_required_block = true;
            continue;
        }
        if in_required_block {
            if trimmed.is_empty() {
                continue;
            }
            if !is_artifact_list_line(trimmed) && looks_like_section_boundary(trimmed) {
                in_required_block = false;
            } else if let Some(candidate) = artifact_candidate_from_line(trimmed)
                && requested_artifact_path_allowed(root, &candidate)
                && seen.insert(candidate.clone())
            {
                paths.push(candidate);
                continue;
            }
        }
        for candidate in backticked_candidates(trimmed) {
            if looks_like_artifact_path(&candidate)
                && requested_artifact_path_allowed(root, &candidate)
                && seen.insert(candidate.clone())
            {
                paths.push(candidate);
            }
        }
    }
    paths
}

fn requested_artifact_path_allowed(root: &Path, raw: &str) -> bool {
    if validate_workspace_relative(raw).is_err() {
        return false;
    }
    let path = Path::new(raw);
    let blocked = [".anvil", ".git", "target", "node_modules", ".next"];
    if path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|part| blocked.contains(&part))
    }) {
        return false;
    }
    resolve_optional_existing(root, raw).is_ok()
}

fn required_paths_satisfied_after_tool(
    root: &Path,
    required_paths: &[String],
    initially_missing_paths: &[String],
    write_or_edit_seen: bool,
) -> bool {
    if required_paths.is_empty() || !missing_paths(root, required_paths).is_empty() {
        return false;
    }
    write_or_edit_seen
        || initially_missing_paths
            .iter()
            .any(|path| resolve_existing(root, path).is_ok())
}

fn is_artifact_list_line(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("- ")
        || line.starts_with("* ")
        || line.chars().next().is_some_and(|ch| ch.is_ascii_digit())
}

fn looks_like_section_boundary(line: &str) -> bool {
    line.ends_with(':') || line.starts_with('#')
}

fn artifact_candidate_from_line(line: &str) -> Option<String> {
    let mut value = line.trim();
    value = value
        .trim_start_matches("- ")
        .trim_start_matches("* ")
        .trim_start();
    if let Some((head, tail)) = value.split_once(". ")
        && head.chars().all(|ch| ch.is_ascii_digit())
    {
        value = tail.trim_start();
    }
    let first = value.split_whitespace().next().unwrap_or_default();
    let candidate = first
        .trim_matches('`')
        .trim_matches('"')
        .trim_matches('\'')
        .trim_end_matches([',', ';']);
    if looks_like_artifact_path(candidate) {
        Some(candidate.to_string())
    } else {
        None
    }
}

fn backticked_candidates(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('`') {
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find('`') else {
            break;
        };
        out.push(after_start[..end].trim().to_string());
        rest = &after_start[end + 1..];
    }
    out
}

fn looks_like_artifact_path(value: &str) -> bool {
    if value.is_empty() || value.starts_with("http://") || value.starts_with("https://") {
        return false;
    }
    if value.contains('/') {
        return true;
    }
    matches!(
        value,
        "Cargo.toml"
            | "README.md"
            | "package.json"
            | "tsconfig.json"
            | "index.html"
            | "pyproject.toml"
    ) || Path::new(value).extension().is_some_and(|ext| {
        matches!(
            ext.to_str().unwrap_or_default(),
            "js" | "jsx"
                | "ts"
                | "tsx"
                | "rs"
                | "py"
                | "md"
                | "txt"
                | "csv"
                | "json"
                | "toml"
                | "yaml"
                | "yml"
                | "html"
                | "css"
        )
    })
}

fn looks_like_progress_without_tool(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    lower.contains("i will")
        || lower.contains("next")
        || lower.contains("作成します")
        || lower.contains("実装します")
        || lower.contains("進めます")
}

fn looks_like_action_prompt(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    lower.contains("create")
        || lower.contains("write")
        || lower.contains("edit")
        || lower.contains("fix")
        || lower.contains("implement")
        || lower.contains("add ")
        || lower.contains("build")
        || lower.contains("作成")
        || lower.contains("実装")
        || lower.contains("修正")
        || lower.contains("追加")
}

fn recoverable_tool_feedback(name: &str, err: &anyhow::Error) -> String {
    format!(
        "Tool call `{name}` was rejected with a recoverable validation error: {err}. Retry with the same tool or another available tool using a valid JSON object that matches the tool schema."
    )
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
            eval_events_path: None,
            resume: None,
            fresh_session: false,
            no_footer: false,
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
        assert_eq!(result, "required artifacts satisfied: a.txt");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "ok"
        );
    }

    #[test]
    fn missing_tool_argument_feedback_allows_retry() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new("Grep", json!({}))],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
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
        assert_eq!(result, "required artifacts satisfied: a.txt");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "ok"
        );
        assert!(
            session
                .messages
                .iter()
                .any(|message| message.content.contains("recoverable validation error"))
        );
    }

    #[test]
    fn prompt_requested_artifact_feedback_then_write() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply::text("done")),
                Ok(AssistantReply {
                    content: String::new(),
                    tool_calls: vec![ToolCall::new(
                        "Write",
                        json!({"path":"a.txt","content":"ok"}),
                    )],
                    prompt_tokens: None,
                    completion_tokens: None,
                }),
            ],
        };
        let mut session = SessionSnapshot::new();
        let result = run_session(
            &mut fake,
            &mut session,
            "Create the file.\n\nRequired final artifacts:\n- a.txt",
            &config(dir.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(result, "required artifacts satisfied: a.txt");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "ok"
        );
        assert!(
            !session
                .messages
                .iter()
                .any(|message| message.role == "assistant" && message.content == "done")
        );
    }

    #[test]
    fn completion_without_write_feedback_then_write_then_complete() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply::text("done")),
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
        let result = run_session(
            &mut fake,
            &mut session,
            "create a.txt",
            &config(dir.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(result, "done");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "ok"
        );
    }

    #[test]
    fn empty_response_gets_one_retry_feedback() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply::text("")),
                Ok(AssistantReply::text("final")),
            ],
        };
        let mut session = SessionSnapshot::new();
        let result = run_session(
            &mut fake,
            &mut session,
            "Summarize this workspace.",
            &config(dir.path().to_path_buf()),
        )
        .unwrap();
        assert_eq!(result, "final");
        assert!(
            !session
                .messages
                .iter()
                .any(|message| message.role == "assistant" && message.content.is_empty())
        );
    }

    #[test]
    fn repeated_planned_action_without_tool_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![
                Ok(AssistantReply::text("I will create it.")),
                Ok(AssistantReply::text("I will create it now.")),
            ],
        };
        let mut session = SessionSnapshot::new();
        let err = run_session(
            &mut fake,
            &mut session,
            "create a.txt",
            &config(dir.path().to_path_buf()),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("missing tool call for action prompt"));
    }

    #[test]
    fn requested_artifact_path_extraction_rejects_escape_and_metadata_paths() {
        let dir = tempfile::tempdir().unwrap();
        let prompt = "\
Required final artifacts:
- ../outside.txt
- /tmp/out.txt
- .anvil/session.json
- target/debug/app
- node_modules/pkg/index.js
- package.json
- src/app/page.tsx
";
        let paths = extract_requested_artifact_paths(dir.path(), prompt);
        assert_eq!(paths, vec!["package.json", "src/app/page.tsx"]);
    }

    #[test]
    fn requested_artifact_path_extraction_rejects_backticked_escape() {
        let dir = tempfile::tempdir().unwrap();
        let prompt = "Create `src/main.rs`, not `../main.rs` or `.anvil/log.json`.";
        let paths = extract_requested_artifact_paths(dir.path(), prompt);
        assert_eq!(paths, vec!["src/main.rs"]);
    }

    #[test]
    #[cfg(unix)]
    fn requested_artifact_path_extraction_rejects_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/tmp", dir.path().join("out")).unwrap();
        let prompt = "\
Required final artifacts:
- out/file.txt
- safe/file.txt
";
        let paths = extract_requested_artifact_paths(dir.path(), prompt);
        assert_eq!(paths, vec!["safe/file.txt"]);
    }

    #[test]
    fn dangerous_command_remains_hard_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut fake = Fake {
            replies: vec![Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new("Bash", json!({"command":"rm -rf /"}))],
                prompt_tokens: None,
                completion_tokens: None,
            })],
        };
        let mut session = SessionSnapshot::new();
        let err = run_session(
            &mut fake,
            &mut session,
            "run command",
            &config(dir.path().to_path_buf()),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("dangerous command blocked"));
    }
}
