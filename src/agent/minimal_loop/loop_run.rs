use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use regex::Regex;

use crate::modes::plan_act::ExecutionMode;
use crate::ollama::client::{AssistantReply, OllamaClient, should_use_native_tool_calls};
use crate::ollama::xml_fallback::ToolCall;
use crate::session::store::{ConversationMessage, SessionSnapshot};
use crate::tools::registry::{ToolContext, ToolRegistry, ToolSpec};
use crate::util::workspace_paths::WorkspacePolicy;

use super::compact::compact_if_needed;
use super::feedback::FeedbackState;
use super::prompt::{PromptToolMode, build_system_prompt};

pub const NO_COMPLETION_WITHOUT_WRITE_FEEDBACK_FLAG: &str =
    "ANVIL_NO_MINIMAL_COMPLETION_WITHOUT_WRITE_FEEDBACK";
pub const NO_REQUESTED_ARTIFACT_FEEDBACK_FLAG: &str =
    "ANVIL_NO_MINIMAL_REQUESTED_ARTIFACT_FEEDBACK";
const MAX_PLANNED_ACTION_WITHOUT_TOOL_FEEDBACKS: usize = 3;
const MAX_MISSING_RELATIVE_IMPORT_FEEDBACKS: usize = 3;

pub trait MinimalChatClient {
    fn supports_native_tools(&self, _model: &str) -> bool {
        false
    }

    fn chat(
        &mut self,
        model: &str,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        native_tools_enabled: bool,
    ) -> Result<AssistantReply, String>;
}

impl MinimalChatClient for OllamaClient {
    fn supports_native_tools(&self, model: &str) -> bool {
        should_use_native_tool_calls(model)
    }

    fn chat(
        &mut self,
        model: &str,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        native_tools_enabled: bool,
    ) -> Result<AssistantReply, String> {
        self.chat_with_mode(model, messages, tools, native_tools_enabled)
    }
}

#[derive(Debug, Clone)]
pub struct MinimalLoopConfig {
    pub work_root: PathBuf,
    pub mode: ExecutionMode,
    pub context_budget: usize,
    pub max_iterations: usize,
    pub auto_approve: bool,
    pub offline: bool,
    pub cancel_flag: Option<Arc<AtomicBool>>,
    pub completion_without_write_feedback: bool,
    pub requested_artifact_feedback: bool,
    pub early_success_paths: Vec<String>,
}

pub fn run_session<C: MinimalChatClient>(
    client: &mut C,
    model: &str,
    session: &mut SessionSnapshot,
    user_prompt: &str,
    config: &MinimalLoopConfig,
) -> Result<String, String> {
    let registry = ToolRegistry::default();
    let mut native_tools_enabled =
        client.supports_native_tools(model) && !session.native_tools_disabled;
    let workspace_policy = WorkspacePolicy::for_task_request(user_prompt);
    let mut feedback_state = FeedbackState::default();
    let mut pending_feedback: Option<String> = None;
    let mut tool_calls_seen = false;
    let mut write_or_edit_calls_seen = false;
    let mut completion_without_write_feedback_sent = false;
    let mut planned_action_without_tool_feedbacks = 0usize;
    let mut missing_relative_import_feedbacks = 0usize;
    let mut changed_source_paths = BTreeSet::new();
    let requested_artifact_paths = extract_requested_artifact_paths(user_prompt);

    session
        .messages
        .push(ConversationMessage::user(user_prompt.to_string()));

    let mut iterations = 0usize;
    while iterations < config.max_iterations {
        if crate::signal_interrupt::interrupted() {
            return Err("interrupted".to_string());
        }
        iterations += 1;
        let available_tools = tool_specs_for_mode(&registry, config.mode);
        let request_tools = if native_tools_enabled {
            available_tools.clone()
        } else {
            Vec::new()
        };
        compact_if_needed(&mut session.messages, config.context_budget);
        let request_messages = build_request_messages(
            session,
            &available_tools,
            &config.work_root,
            prompt_tool_mode(native_tools_enabled),
            pending_feedback.as_deref(),
        );
        let reply = match client.chat(
            model,
            &request_messages,
            &request_tools,
            native_tools_enabled,
        ) {
            Ok(reply) => {
                pending_feedback = None;
                reply
            }
            Err(err) if native_tools_enabled && is_native_tool_parser_failure(&err) => {
                downgrade_to_xml_fallback(session, &mut native_tools_enabled);
                pending_feedback = Some(feedback_state.malformed_tool_call_xml_fallback(&err));
                continue;
            }
            Err(err) if is_tool_call_parser_failure(&err) => {
                downgrade_to_xml_fallback(session, &mut native_tools_enabled);
                pending_feedback = Some(feedback_state.malformed_tool_call_xml_fallback(&err));
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
            if reply.content.trim().is_empty()
                && let Some(feedback) = feedback_state.empty_response()
            {
                discard_last_no_tool_assistant_message(session);
                pending_feedback = Some(feedback);
                continue;
            }
            if should_send_completion_without_write_feedback(
                config,
                write_or_edit_calls_seen,
                completion_without_write_feedback_sent,
            ) && let Some(feedback) = feedback_state.completion_without_write()
            {
                completion_without_write_feedback_sent = true;
                discard_last_no_tool_assistant_message(session);
                pending_feedback = Some(feedback);
                continue;
            }
            let missing_requested_artifacts =
                missing_requested_artifact_paths(&config.work_root, &requested_artifact_paths);
            if should_send_requested_artifact_feedback(config, &missing_requested_artifacts)
                && let Some(feedback) =
                    feedback_state.requested_artifacts_missing(&missing_requested_artifacts)
            {
                discard_last_no_tool_assistant_message(session);
                pending_feedback = Some(feedback);
                continue;
            }
            let missing_relative_imports =
                missing_relative_imports(&config.work_root, &changed_source_paths);
            if config.mode == ExecutionMode::Act && !missing_relative_imports.is_empty() {
                if missing_relative_import_feedbacks >= MAX_MISSING_RELATIVE_IMPORT_FEEDBACKS {
                    discard_last_no_tool_assistant_message(session);
                    return Err(format!(
                        "assistant left unresolved relative imports: {}",
                        missing_relative_imports.join("; ")
                    ));
                }
                missing_relative_import_feedbacks += 1;
                discard_last_no_tool_assistant_message(session);
                pending_feedback =
                    Some(feedback_state.missing_relative_imports(&missing_relative_imports));
                continue;
            }
            if config.mode == ExecutionMode::Act
                && let Some(feedback) = feedback_state.planned_action_without_tool(&reply.content)
            {
                if planned_action_without_tool_feedbacks
                    >= MAX_PLANNED_ACTION_WITHOUT_TOOL_FEEDBACKS
                {
                    discard_last_no_tool_assistant_message(session);
                    return Err(
                        "assistant repeatedly described a future tool action without issuing a tool call"
                            .to_string(),
                    );
                }
                planned_action_without_tool_feedbacks += 1;
                discard_last_no_tool_assistant_message(session);
                pending_feedback = Some(feedback);
                continue;
            }
            if config.mode == ExecutionMode::Act
                && !tool_calls_seen
                && !completion_without_write_feedback_sent
                && let Some(feedback) = feedback_state.missing_tool_call(user_prompt)
            {
                discard_last_no_tool_assistant_message(session);
                pending_feedback = Some(feedback);
                continue;
            }
            return Ok(reply.content);
        }
        tool_calls_seen = true;

        let tool_context = ToolContext {
            root: config.work_root.clone(),
            mode: config.mode,
            plan_path: None,
            plan_stage: session.mode_state.plan_stage,
            auto_approve: config.auto_approve,
            interactive_approval: false,
            offline: config.offline,
            cancel_flag: config.cancel_flag.clone(),
            tmp_tests_root: None,
            tester_active: false,
            workspace_policy,
        };

        for call in tool_calls {
            let changed_source_path = changed_source_path(&config.work_root, &call);
            if is_write_or_edit_tool(&call.name) {
                write_or_edit_calls_seen = true;
            }
            let execution = registry.execute(&call.name, &call.arguments, &tool_context);
            let result = match execution {
                Ok(result) => {
                    if let Some(path) = changed_source_path {
                        changed_source_paths.insert(path);
                    }
                    result
                }
                Err(err) => {
                    if call.name == "Edit"
                        && let Some(feedback) = feedback_state.edit_anchor_mismatch(&err)
                    {
                        pending_feedback.get_or_insert(feedback);
                    }
                    format!("ERROR: {err}")
                }
            };
            session
                .messages
                .push(ConversationMessage::tool(call.name, result));
        }
        if early_success_paths_satisfied(&config.work_root, &config.early_success_paths) {
            return Ok("expected paths satisfied".to_string());
        }
    }

    Err(format!(
        "minimal loop reached max_iterations ({})",
        config.max_iterations
    ))
}

fn early_success_paths_satisfied(work_root: &Path, paths: &[String]) -> bool {
    !paths.is_empty() && paths.iter().all(|path| work_root.join(path).exists())
}

pub fn completion_without_write_feedback_disabled_from_env() -> bool {
    std::env::var_os(NO_COMPLETION_WITHOUT_WRITE_FEEDBACK_FLAG)
        .is_some_and(|value| !value.is_empty())
}

pub fn requested_artifact_feedback_disabled_from_env() -> bool {
    std::env::var_os(NO_REQUESTED_ARTIFACT_FEEDBACK_FLAG).is_some_and(|value| !value.is_empty())
}

fn should_send_completion_without_write_feedback(
    config: &MinimalLoopConfig,
    write_or_edit_calls_seen: bool,
    completion_without_write_feedback_sent: bool,
) -> bool {
    config.mode == ExecutionMode::Act
        && config.completion_without_write_feedback
        && !write_or_edit_calls_seen
        && !completion_without_write_feedback_sent
}

fn should_send_requested_artifact_feedback(
    config: &MinimalLoopConfig,
    missing_requested_artifacts: &[String],
) -> bool {
    config.mode == ExecutionMode::Act
        && config.requested_artifact_feedback
        && !missing_requested_artifacts.is_empty()
}

fn missing_requested_artifact_paths(work_root: &Path, paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .filter(|path| !work_root.join(path).is_file())
        .cloned()
        .collect()
}

fn missing_relative_imports(work_root: &Path, source_paths: &BTreeSet<String>) -> Vec<String> {
    let mut missing = BTreeSet::new();
    for source_path in source_paths {
        let source_absolute = work_root.join(source_path);
        let Ok(content) = std::fs::read_to_string(&source_absolute) else {
            continue;
        };
        for specifier in extract_relative_import_specifiers(&content) {
            let Some(candidates) = resolve_relative_module_candidates(source_path, &specifier)
            else {
                continue;
            };
            if candidates
                .iter()
                .any(|candidate| work_root.join(candidate).is_file())
            {
                continue;
            }
            let candidate_list = candidates
                .iter()
                .filter_map(|candidate| path_to_slash_string(candidate))
                .take(4)
                .collect::<Vec<_>>()
                .join(", ");
            missing.insert(format!(
                "{source_path} imports {specifier} (expected one of: {candidate_list})"
            ));
        }
    }
    missing.into_iter().collect()
}

fn changed_source_path(work_root: &Path, call: &ToolCall) -> Option<String> {
    if !is_write_or_edit_tool(&call.name) {
        return None;
    }
    let raw_path = call.arguments.get("path")?.as_str()?;
    let normalized = normalize_tool_path(work_root, raw_path)?;
    if is_frontend_source_path(&normalized) {
        Some(normalized)
    } else {
        None
    }
}

fn normalize_tool_path(work_root: &Path, raw_path: &str) -> Option<String> {
    let path = Path::new(raw_path);
    let relative = if path.is_absolute() {
        path.strip_prefix(work_root).ok()?
    } else {
        path
    };
    let normalized = normalize_relative_path(relative)?;
    path_to_slash_string(&normalized)
}

fn is_frontend_source_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".d.ts") {
        return false;
    }
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| matches!(ext, "js" | "jsx" | "ts" | "tsx"))
}

fn extract_relative_import_specifiers(content: &str) -> BTreeSet<String> {
    let mut specifiers = BTreeSet::new();
    let patterns = [
        r#"\b(?:import|export)\s+(?:[^'"]*?\s+from\s+)?['"]([^'"]+)['"]"#,
        r#"\bimport\s*\(\s*['"]([^'"]+)['"]\s*\)"#,
        r#"\brequire\s*\(\s*['"]([^'"]+)['"]\s*\)"#,
    ];
    for pattern in patterns {
        let regex = Regex::new(pattern).expect("valid relative import regex");
        for captures in regex.captures_iter(content) {
            let specifier = captures[1].to_string();
            if specifier.starts_with("./") || specifier.starts_with("../") {
                specifiers.insert(specifier);
            }
        }
    }
    specifiers
}

fn resolve_relative_module_candidates(source_path: &str, specifier: &str) -> Option<Vec<PathBuf>> {
    let source = Path::new(source_path);
    let base_dir = source.parent().unwrap_or_else(|| Path::new(""));
    let base = normalize_relative_path(&base_dir.join(specifier))?;
    Some(module_candidate_paths(&base))
}

fn module_candidate_paths(base: &Path) -> Vec<PathBuf> {
    if base.extension().is_some() {
        return vec![base.to_path_buf()];
    }

    let mut candidates = Vec::new();
    for ext in ["ts", "tsx", "js", "jsx", "mjs", "cjs", "json"] {
        candidates.push(base.with_extension(ext));
    }
    for ext in ["ts", "tsx", "js", "jsx", "mjs", "cjs", "json"] {
        candidates.push(base.join(format!("index.{ext}")));
    }
    candidates
}

fn normalize_relative_path(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(part) => normalized.push(part),
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => return None,
        }
    }
    Some(normalized)
}

fn path_to_slash_string(path: &Path) -> Option<String> {
    Some(path.to_str()?.replace('\\', "/"))
}

fn extract_requested_artifact_paths(prompt: &str) -> Vec<String> {
    let mut paths = BTreeSet::new();
    for raw in prompt.split_whitespace() {
        if let Some(path) = normalize_requested_path_token(raw)
            && is_safe_requested_artifact_path(&path)
        {
            paths.insert(path);
        }
    }
    paths.into_iter().collect()
}

fn normalize_requested_path_token(raw: &str) -> Option<String> {
    let mut token = raw.trim_matches(|ch: char| {
        ch.is_whitespace()
            || matches!(
                ch,
                '`' | '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | ':'
            )
    });
    while token.ends_with('.') {
        token = &token[..token.len() - 1];
    }
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

fn is_safe_requested_artifact_path(path: &str) -> bool {
    if path.starts_with('/')
        || path.starts_with('-')
        || path.contains('\\')
        || path.contains("://")
        || path.ends_with('/')
        || path.len() > 240
        || !path.contains('.')
        || is_framework_name_token(path)
    {
        return false;
    }

    let candidate = Path::new(path);
    if candidate.components().any(|component| {
        !matches!(
            component,
            std::path::Component::Normal(_) | std::path::Component::CurDir
        )
    }) {
        return false;
    }

    candidate
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(_, ext)| {
            (1..=12).contains(&ext.len())
                && ext
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        })
}

fn is_framework_name_token(path: &str) -> bool {
    if path.contains('/') {
        return false;
    }
    matches!(
        path.to_ascii_lowercase().as_str(),
        "next.js" | "node.js" | "react.js" | "vue.js" | "express.js" | "three.js" | "d3.js"
    )
}

fn discard_last_no_tool_assistant_message(session: &mut SessionSnapshot) {
    if session.messages.last().is_some_and(|message| {
        message.role == "assistant" && message.tool_calls.is_empty() && message.name.is_none()
    }) {
        session.messages.pop();
    }
}

fn is_write_or_edit_tool(name: &str) -> bool {
    matches!(name, "Write" | "Edit")
}

fn build_request_messages(
    session: &SessionSnapshot,
    tools: &[ToolSpec],
    work_root: &std::path::Path,
    prompt_tool_mode: PromptToolMode,
    ephemeral_feedback: Option<&str>,
) -> Vec<ConversationMessage> {
    let mut messages = Vec::with_capacity(session.messages.len() + 2);
    messages.push(ConversationMessage::system(build_system_prompt(
        work_root,
        tools,
        prompt_tool_mode,
    )));
    messages.extend(
        session
            .messages
            .iter()
            .filter(|message| message.role != "system")
            .map(prompt_history_message),
    );
    if let Some(feedback) = ephemeral_feedback {
        messages.push(ConversationMessage::user(feedback.to_string()));
    }
    messages
}

fn prompt_history_message(message: &ConversationMessage) -> ConversationMessage {
    let mut message = message.clone();
    if message.role == "assistant" && !message.tool_calls.is_empty() {
        message.content.clear();
    }
    message
}

fn prompt_tool_mode(native_tools_enabled: bool) -> PromptToolMode {
    if native_tools_enabled {
        PromptToolMode::Native
    } else {
        PromptToolMode::XmlFallback
    }
}

fn downgrade_to_xml_fallback(session: &mut SessionSnapshot, native_tools_enabled: &mut bool) {
    *native_tools_enabled = false;
    session.native_tools_disabled = true;
}

fn tool_specs_for_mode(registry: &ToolRegistry, mode: ExecutionMode) -> Vec<ToolSpec> {
    registry
        .specs()
        .iter()
        .filter(|spec| {
            mode == ExecutionMode::Act
                || matches!(spec.function.name.as_str(), "Read" | "Glob" | "Grep")
        })
        .cloned()
        .collect()
}

fn is_native_tool_parser_failure(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("native tool parser failed")
        || lower.contains("unexpected end element")
        || lower.contains("unexpected eof")
}

fn is_tool_call_parser_failure(error: &str) -> bool {
    error
        .to_ascii_lowercase()
        .contains("tool call parser failed")
}

#[cfg(test)]
mod tests {
    use super::super::feedback::MINIMAL_FEEDBACK_PREFIX;
    use super::*;
    use crate::ollama::xml_fallback::ToolCall;
    use serde_json::json;
    use std::collections::VecDeque;

    enum MockResponse {
        Reply(AssistantReply),
        Err(String),
    }

    #[derive(Default)]
    struct MockClient {
        responses: VecDeque<MockResponse>,
        native_modes: Vec<bool>,
        tool_counts: Vec<usize>,
        system_prompts: Vec<String>,
        feedback_messages: Vec<String>,
        feedback_seen: bool,
    }

    impl MockClient {
        fn push_reply(&mut self, content: &str, tool_calls: Vec<ToolCall>) {
            self.responses
                .push_back(MockResponse::Reply(AssistantReply {
                    content: content.to_string(),
                    tool_calls,
                    prompt_tokens: None,
                    completion_tokens: None,
                }));
        }

        fn push_err(&mut self, err: &str) {
            self.responses.push_back(MockResponse::Err(err.to_string()));
        }
    }

    impl MinimalChatClient for MockClient {
        fn supports_native_tools(&self, model: &str) -> bool {
            crate::ollama::client::should_use_native_tool_calls(model)
        }

        fn chat(
            &mut self,
            _model: &str,
            messages: &[ConversationMessage],
            tools: &[ToolSpec],
            native_tools_enabled: bool,
        ) -> Result<AssistantReply, String> {
            assert_eq!(messages[0].role, "system");
            assert_eq!(messages.iter().filter(|m| m.role == "system").count(), 1);
            self.native_modes.push(native_tools_enabled);
            self.tool_counts.push(tools.len());
            self.system_prompts.push(messages[0].content.clone());
            self.feedback_messages.extend(
                messages
                    .iter()
                    .filter(|m| m.content.starts_with(MINIMAL_FEEDBACK_PREFIX))
                    .map(|m| m.content.clone()),
            );
            self.feedback_seen |= messages
                .iter()
                .any(|m| m.content.starts_with(MINIMAL_FEEDBACK_PREFIX));
            match self.responses.pop_front().expect("mock response") {
                MockResponse::Reply(reply) => Ok(reply),
                MockResponse::Err(err) => Err(err),
            }
        }
    }

    const TYPE_A_CSV_WRONG_CLOSER_PAYLOAD: &str = r#"<anvil_tool_call>{"name":"Write","arguments":{"path":"tools/csv_stats.py","content":"import csv\nimport sys\n\n\ndef main():\n    if len(sys.argv) < 2:\n        print(\"Usage: python csv_stats.py <csv_path>\")\n        sys.exit(1)\n\n    csv_path = sys.argv[1]\n\n    with open(csv_path, newline=\"\") as f:\n        reader = csv.reader(f)\n        rows = list(reader)\n\n    row_count = len(rows)\n    column_count = len(rows[0]) if rows else 0\n\n    print(f\"row_count={row_count}\")\n    print(f\"column_count={column_count}\")\n\n\nif __name__ == \"__main__\":\n    main()\n"}}
</function>
</tool_call>"#;

    const TYPE_A_MARKDOWN_UNTERMINATED_PAYLOAD: &str = r##"<anvil_tool_call>{"name":"Write","arguments":{"path":"reports/local-llm-brief.md","content":"# Local LLM Trade-offs: A Concise Research Brief\n\n| Factor | Impact |\n|---|---|\n| Latency | Local inference depends on GPU memory and model size. |\n| Privacy | Local execution keeps prompts and files on the workstation. |\n"}}"##;

    fn parser_failure_for_type_a_payload(payload: &str) -> String {
        assert!(payload.contains("<anvil_tool_call>"));
        assert!(!payload.contains("</anvil_tool_call>"));
        "tool call parser failed: unterminated <anvil_tool_call> block".to_string()
    }

    fn tool_call(name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: format!("call-{name}"),
            name: name.to_string(),
            arguments,
        }
    }

    fn config(root: PathBuf, max_iterations: usize) -> MinimalLoopConfig {
        MinimalLoopConfig {
            work_root: root,
            mode: ExecutionMode::Act,
            context_budget: 24_000,
            max_iterations,
            auto_approve: true,
            offline: false,
            cancel_flag: None,
            completion_without_write_feedback: true,
            requested_artifact_feedback: true,
            early_success_paths: Vec::new(),
        }
    }

    #[test]
    fn normal_loop_executes_tool_then_returns_final_response() {
        let temp = tempfile::tempdir().unwrap();
        let mut client = MockClient::default();
        client.push_reply(
            "",
            vec![tool_call(
                "Write",
                json!({"path": "hello.txt", "content": "hello"}),
            )],
        );
        client.push_reply("done", Vec::new());
        let mut session = SessionSnapshot::default();

        let reply = run_session(
            &mut client,
            "qwen3:8b",
            &mut session,
            "write hello",
            &config(temp.path().to_path_buf(), 4),
        )
        .unwrap();

        assert_eq!(reply, "done");
        assert_eq!(
            std::fs::read_to_string(temp.path().join("hello.txt")).unwrap(),
            "hello"
        );
        assert!(session.messages.iter().any(|m| m.role == "tool"));
    }

    #[test]
    fn early_success_paths_stop_after_tool_execution() {
        let temp = tempfile::tempdir().unwrap();
        let mut client = MockClient::default();
        client.push_reply(
            "",
            vec![tool_call(
                "Write",
                json!({"path": "package.json", "content": "{}\n"}),
            )],
        );
        let mut session = SessionSnapshot::default();
        let mut config = config(temp.path().to_path_buf(), 8);
        config.early_success_paths = vec!["package.json".to_string()];

        let reply = run_session(
            &mut client,
            "qwen3:8b",
            &mut session,
            "create package.json",
            &config,
        )
        .unwrap();

        assert_eq!(reply, "expected paths satisfied");
        assert_eq!(client.tool_counts.len(), 1);
        assert!(temp.path().join("package.json").is_file());
    }

    #[test]
    fn native_parser_failure_downgrades_to_xml_for_session() {
        let temp = tempfile::tempdir().unwrap();
        let mut client = MockClient::default();
        client.push_err("native tool parser failed: unexpected eof");
        client.push_reply("done after downgrade", Vec::new());
        client.push_reply("done after downgrade", Vec::new());
        let mut session = SessionSnapshot::default();

        let reply = run_session(
            &mut client,
            "qwen3.6:27b-coding-nvfp4",
            &mut session,
            "answer",
            &config(temp.path().to_path_buf(), 4),
        )
        .unwrap();

        assert_eq!(reply, "done after downgrade");
        assert_eq!(client.native_modes, vec![true, false, false]);
        assert_eq!(client.tool_counts[1], 0);
        assert!(!client.system_prompts[0].contains("<anvil_tool_call>"));
        assert!(client.system_prompts[1].contains("<anvil_tool_call>"));
        assert_eq!(client.feedback_messages.len(), 2);
        assert!(client.feedback_messages[0].contains("<anvil_tool_call>"));
        assert!(client.feedback_messages[1].contains("No file changes"));
        assert!(session.native_tools_disabled);
    }

    #[test]
    fn xml_parser_failure_downgrades_to_xml_feedback_and_retries() {
        let temp = tempfile::tempdir().unwrap();
        let mut client = MockClient::default();
        client.push_err("tool call parser failed: unterminated <anvil_tool_call> block");
        client.push_reply(
            "",
            vec![tool_call(
                "Write",
                json!({"path": "fixed.txt", "content": "ok"}),
            )],
        );
        client.push_reply("done", Vec::new());
        let mut session = SessionSnapshot::default();

        let reply = run_session(
            &mut client,
            "qwen3.6:27b-coding-nvfp4",
            &mut session,
            "create fixed.txt",
            &config(temp.path().to_path_buf(), 4),
        )
        .unwrap();

        assert_eq!(reply, "done");
        assert_eq!(
            std::fs::read_to_string(temp.path().join("fixed.txt")).unwrap(),
            "ok"
        );
        assert!(client.feedback_seen);
        assert!(client.native_modes[0]);
        assert!(!client.native_modes[1]);
        assert_eq!(client.tool_counts[1], 0);
        assert!(client.feedback_messages[0].contains("Native tool calls are disabled"));
        assert!(session.native_tools_disabled);
        assert!(
            !session
                .messages
                .iter()
                .any(|m| m.content.starts_with(MINIMAL_FEEDBACK_PREFIX)),
            "ephemeral feedback must not be persisted to the session"
        );
    }

    #[test]
    fn type_a_deepdive_payloads_trigger_session_downgrade() {
        for payload in [
            TYPE_A_CSV_WRONG_CLOSER_PAYLOAD,
            TYPE_A_MARKDOWN_UNTERMINATED_PAYLOAD,
        ] {
            let temp = tempfile::tempdir().unwrap();
            let mut client = MockClient::default();
            let err = parser_failure_for_type_a_payload(payload);
            client.push_err(&err);
            client.push_reply("done after downgrade", Vec::new());
            client.push_reply("done after downgrade", Vec::new());
            let mut session = SessionSnapshot::default();

            let reply = run_session(
                &mut client,
                "qwen3.6:27b-coding-nvfp4",
                &mut session,
                "answer",
                &config(temp.path().to_path_buf(), 3),
            )
            .unwrap();

            assert_eq!(reply, "done after downgrade");
            assert_eq!(client.native_modes, vec![true, false, false]);
            assert_eq!(client.tool_counts[0], 6);
            assert_eq!(client.tool_counts[1], 0);
            assert!(client.feedback_messages[0].contains("<anvil_tool_call>"));
            assert!(client.feedback_messages[1].contains("No file changes"));
            assert!(session.native_tools_disabled);
        }
    }

    #[test]
    fn max_iterations_returns_error_when_tools_never_finish() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("loop.txt"), "loop").unwrap();
        let mut client = MockClient::default();
        client.push_reply("", vec![tool_call("Read", json!({"path": "loop.txt"}))]);
        client.push_reply("", vec![tool_call("Read", json!({"path": "loop.txt"}))]);
        let mut session = SessionSnapshot::default();

        let err = run_session(
            &mut client,
            "qwen3:8b",
            &mut session,
            "keep reading",
            &config(temp.path().to_path_buf(), 2),
        )
        .unwrap_err();

        assert_eq!(err, "minimal loop reached max_iterations (2)");
    }

    #[test]
    fn edit_anchor_feedback_is_ephemeral_and_not_persisted() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("main.rs"), "fn main() {}\n").unwrap();
        let mut client = MockClient::default();
        client.push_reply(
            "",
            vec![tool_call(
                "Edit",
                json!({
                    "path": "main.rs",
                    "old_string": "missing anchor",
                    "new_string": "replacement",
                }),
            )],
        );
        client.push_reply("done", Vec::new());
        let mut session = SessionSnapshot::default();

        let reply = run_session(
            &mut client,
            "qwen3:8b",
            &mut session,
            "fix main.rs",
            &config(temp.path().to_path_buf(), 4),
        )
        .unwrap();

        assert_eq!(reply, "done");
        assert!(client.feedback_seen);
        assert!(
            !session
                .messages
                .iter()
                .any(|m| m.content.starts_with(MINIMAL_FEEDBACK_PREFIX)),
            "ephemeral feedback must not be persisted to the session"
        );
    }

    #[test]
    fn completion_without_write_feedback_then_write_then_complete() {
        let temp = tempfile::tempdir().unwrap();
        let mut client = MockClient::default();
        client.push_reply("I will create README.md.", Vec::new());
        client.push_reply(
            "",
            vec![tool_call(
                "Write",
                json!({"path": "README.md", "content": "# Demo\n"}),
            )],
        );
        client.push_reply("done", Vec::new());
        let mut session = SessionSnapshot::default();

        let reply = run_session(
            &mut client,
            "qwen3:8b",
            &mut session,
            "create a README file",
            &config(temp.path().to_path_buf(), 4),
        )
        .unwrap();

        assert_eq!(reply, "done");
        assert_eq!(
            std::fs::read_to_string(temp.path().join("README.md")).unwrap(),
            "# Demo\n"
        );
        assert_eq!(client.feedback_messages.len(), 1);
        assert!(client.feedback_messages[0].contains("No file changes"));
        assert!(
            !session
                .messages
                .iter()
                .any(|m| m.content.starts_with(MINIMAL_FEEDBACK_PREFIX)),
            "ephemeral feedback must not be persisted to the session"
        );
    }

    #[test]
    fn requested_artifact_feedback_then_missing_write_then_complete() {
        let temp = tempfile::tempdir().unwrap();
        let mut client = MockClient::default();
        client.push_reply(
            "",
            vec![tool_call(
                "Write",
                json!({"path": "data/sample-sales.csv", "content": "product,total\nA,10\n"}),
            )],
        );
        client.push_reply("Now I'll create the sales analysis report.", Vec::new());
        client.push_reply(
            "",
            vec![tool_call(
                "Write",
                json!({"path": "reports/sales-analysis.md", "content": "# Sales Analysis\n\n## Top Products\nA\n"}),
            )],
        );
        client.push_reply("done", Vec::new());
        let mut session = SessionSnapshot::default();

        let reply = run_session(
            &mut client,
            "qwen3:8b",
            &mut session,
            "Create data/sample-sales.csv and reports/sales-analysis.md.",
            &config(temp.path().to_path_buf(), 5),
        )
        .unwrap();

        assert_eq!(reply, "done");
        assert!(temp.path().join("data/sample-sales.csv").is_file());
        assert!(temp.path().join("reports/sales-analysis.md").is_file());
        assert_eq!(client.feedback_messages.len(), 1);
        assert!(
            client.feedback_messages[0].contains("reports/sales-analysis.md"),
            "got: {}",
            client.feedback_messages[0]
        );
        assert!(
            !session
                .messages
                .iter()
                .any(|m| m.content.starts_with(MINIMAL_FEEDBACK_PREFIX)),
            "ephemeral feedback must not be persisted to the session"
        );
    }

    #[test]
    fn requested_artifact_feedback_accepts_blocker_response() {
        let temp = tempfile::tempdir().unwrap();
        let mut client = MockClient::default();
        client.push_reply(
            "",
            vec![tool_call(
                "Write",
                json!({"path": "data/sample-sales.csv", "content": "product,total\nA,10\n"}),
            )],
        );
        client.push_reply("Now I'll create the sales analysis report.", Vec::new());
        client.push_reply(
            "I cannot create reports/sales-analysis.md because the required input is missing.",
            Vec::new(),
        );
        let mut session = SessionSnapshot::default();

        let reply = run_session(
            &mut client,
            "qwen3:8b",
            &mut session,
            "Create data/sample-sales.csv and reports/sales-analysis.md.",
            &config(temp.path().to_path_buf(), 4),
        )
        .unwrap();

        assert_eq!(
            reply,
            "I cannot create reports/sales-analysis.md because the required input is missing."
        );
        assert_eq!(client.feedback_messages.len(), 1);
        assert!(client.feedback_messages[0].contains("reports/sales-analysis.md"));
    }

    #[test]
    fn planned_action_after_feedback_gets_one_more_tool_prompt() {
        let temp = tempfile::tempdir().unwrap();
        let mut client = MockClient::default();
        client.push_reply("Now let me create the Next.js app structure.", Vec::new());
        client.push_reply("Now let me create the Next.js app structure.", Vec::new());
        client.push_reply(
            "",
            vec![tool_call(
                "Write",
                json!({"path": "package.json", "content": "{\"scripts\":{\"dev\":\"next dev -p 3011\"}}\n"}),
            )],
        );
        client.push_reply("Created package.json.", Vec::new());
        let mut session = SessionSnapshot::default();

        let reply = run_session(
            &mut client,
            "qwen3:8b",
            &mut session,
            "create a Next.js app",
            &config(temp.path().to_path_buf(), 6),
        )
        .unwrap();

        assert_eq!(reply, "Created package.json.");
        assert!(temp.path().join("package.json").is_file());
        assert_eq!(client.feedback_messages.len(), 2);
        assert!(client.feedback_messages[0].contains("No file changes"));
        assert!(
            client.feedback_messages[1].contains("described a next action"),
            "got: {}",
            client.feedback_messages[1]
        );
        assert!(
            !session
                .messages
                .iter()
                .any(|message| message.content == "Now let me create the Next.js app structure."),
            "failed no-tool planning replies must not remain in prompt history"
        );
    }

    #[test]
    fn planned_action_after_write_gets_tool_prompt() {
        let temp = tempfile::tempdir().unwrap();
        let mut client = MockClient::default();
        client.push_reply(
            "",
            vec![tool_call(
                "Write",
                json!({"path": "app/page.tsx", "content": "export default function Home(){ return <main /> }\n"}),
            )],
        );
        client.push_reply(
            "Now I'll create the main game component with canvas rendering.",
            Vec::new(),
        );
        client.push_reply(
            "",
            vec![tool_call(
                "Write",
                json!({"path": "app/Game.tsx", "content": "export default function Game(){ return <canvas /> }\n"}),
            )],
        );
        client.push_reply("Created the page and game component.", Vec::new());
        let mut session = SessionSnapshot::default();

        let reply = run_session(
            &mut client,
            "qwen3:8b",
            &mut session,
            "create a Next.js app",
            &config(temp.path().to_path_buf(), 6),
        )
        .unwrap();

        assert_eq!(reply, "Created the page and game component.");
        assert!(temp.path().join("app/page.tsx").is_file());
        assert!(temp.path().join("app/Game.tsx").is_file());
        assert_eq!(client.feedback_messages.len(), 1);
        assert!(
            client.feedback_messages[0].contains("described a next action"),
            "got: {}",
            client.feedback_messages[0]
        );
        assert!(
            !session.messages.iter().any(|message| message.content
                == "Now I'll create the main game component with canvas rendering."),
            "failed no-tool planning replies must not remain in prompt history"
        );
    }

    #[test]
    fn missing_relative_import_gets_repair_prompt_before_final() {
        let temp = tempfile::tempdir().unwrap();
        let mut client = MockClient::default();
        client.push_reply(
            "",
            vec![tool_call(
                "Write",
                json!({"path": "app/page.tsx", "content": "import dynamic from 'next/dynamic';\nconst GameCanvas = dynamic(() => import('../components/GameCanvas'), { ssr: false });\nexport default function Home(){ return <GameCanvas /> }\n"}),
            )],
        );
        client.push_reply("Created the app.", Vec::new());
        client.push_reply(
            "",
            vec![tool_call(
                "Write",
                json!({"path": "components/GameCanvas.tsx", "content": "export default function GameCanvas(){ return <canvas /> }\n"}),
            )],
        );
        client.push_reply("Created the app and GameCanvas component.", Vec::new());
        let mut session = SessionSnapshot::default();

        let reply = run_session(
            &mut client,
            "qwen3:8b",
            &mut session,
            "create a Next.js app",
            &config(temp.path().to_path_buf(), 6),
        )
        .unwrap();

        assert_eq!(reply, "Created the app and GameCanvas component.");
        assert!(temp.path().join("app/page.tsx").is_file());
        assert!(temp.path().join("components/GameCanvas.tsx").is_file());
        assert_eq!(client.feedback_messages.len(), 1);
        assert!(
            client.feedback_messages[0].contains("app/page.tsx imports ../components/GameCanvas"),
            "got: {}",
            client.feedback_messages[0]
        );
        assert!(
            !session
                .messages
                .iter()
                .any(|message| message.content == "Created the app."),
            "failed final response must not remain in prompt history"
        );
    }

    #[test]
    fn repeated_planned_action_without_tool_returns_error() {
        let temp = tempfile::tempdir().unwrap();
        let mut client = MockClient::default();
        client.push_reply("Now let me create the Next.js app structure.", Vec::new());
        client.push_reply("Now let me create the Next.js app structure.", Vec::new());
        client.push_reply("Now let me create the Next.js app structure.", Vec::new());
        client.push_reply("Now let me create the Next.js app structure.", Vec::new());
        client.push_reply("Now let me create the Next.js app structure.", Vec::new());
        let mut session = SessionSnapshot::default();

        let err = run_session(
            &mut client,
            "qwen3:8b",
            &mut session,
            "create a Next.js app",
            &config(temp.path().to_path_buf(), 8),
        )
        .unwrap_err();

        assert!(err.contains("future tool action"));
        assert_eq!(client.feedback_messages.len(), 4);
        assert!(
            !session
                .messages
                .iter()
                .any(|message| message.content == "Now let me create the Next.js app structure."),
            "failed no-tool planning replies must be discarded before retry"
        );
    }

    #[test]
    fn tool_call_assistant_preamble_is_not_reprompted() {
        let temp = tempfile::tempdir().unwrap();
        let mut session = SessionSnapshot::default();
        session.messages.push(ConversationMessage::user(
            "create a Next.js app".to_string(),
        ));
        session.messages.push(ConversationMessage::assistant(
            "I'll create the project step by step.".to_string(),
            vec![tool_call(
                "Write",
                json!({"path": "package.json", "content": "{}\n"}),
            )],
        ));
        session.messages.push(ConversationMessage::tool(
            "Write".to_string(),
            "wrote package.json".to_string(),
        ));

        let messages = build_request_messages(
            &session,
            ToolRegistry::default().specs(),
            temp.path(),
            PromptToolMode::Native,
            None,
        );

        let assistant = messages
            .iter()
            .find(|message| message.role == "assistant" && !message.tool_calls.is_empty())
            .expect("assistant tool call history");
        assert_eq!(assistant.content, "");
        assert_eq!(assistant.tool_calls[0].name, "Write");
        assert!(
            messages
                .iter()
                .any(|message| message.role == "tool" && message.content == "wrote package.json")
        );
    }

    #[test]
    fn requested_artifact_feedback_ignores_existing_paths() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/dateRange.js"), "old").unwrap();
        let mut client = MockClient::default();
        client.push_reply("done", Vec::new());
        let mut session = SessionSnapshot::default();
        let mut cfg = config(temp.path().to_path_buf(), 4);
        cfg.completion_without_write_feedback = false;

        let reply = run_session(
            &mut client,
            "qwen3:8b",
            &mut session,
            "Explain src/dateRange.js.",
            &cfg,
        )
        .unwrap();

        assert_eq!(reply, "done");
        assert!(client.feedback_messages.is_empty());
    }

    #[test]
    fn requested_artifact_feedback_can_be_disabled() {
        let temp = tempfile::tempdir().unwrap();
        let mut client = MockClient::default();
        client.push_reply(
            "",
            vec![tool_call(
                "Write",
                json!({"path": "data/sample-sales.csv", "content": "product,total\nA,10\n"}),
            )],
        );
        client.push_reply("Created data/sample-sales.csv.", Vec::new());
        let mut session = SessionSnapshot::default();
        let mut cfg = config(temp.path().to_path_buf(), 4);
        cfg.requested_artifact_feedback = false;

        let reply = run_session(
            &mut client,
            "qwen3:8b",
            &mut session,
            "Create data/sample-sales.csv and reports/sales-analysis.md.",
            &cfg,
        )
        .unwrap();

        assert_eq!(reply, "Created data/sample-sales.csv.");
        assert!(client.feedback_messages.is_empty());
    }

    #[test]
    fn requested_artifact_path_extraction_is_safe_and_explicit() {
        assert_eq!(
            extract_requested_artifact_paths(
                "Create data/sample-sales.csv and reports/sales-analysis.md."
            ),
            vec![
                "data/sample-sales.csv".to_string(),
                "reports/sales-analysis.md".to_string()
            ]
        );
        assert_eq!(
            extract_requested_artifact_paths("Fix src/dateRange.js and README.md."),
            vec!["README.md".to_string(), "src/dateRange.js".to_string()]
        );
        assert!(extract_requested_artifact_paths("Use http://example.test/a.md").is_empty());
        assert!(extract_requested_artifact_paths("Create ../outside.md").is_empty());
        assert!(extract_requested_artifact_paths("Create a Next.js app.").is_empty());
        assert!(extract_requested_artifact_paths("next.jsアプリを作成").is_empty());
        assert_eq!(
            extract_requested_artifact_paths("Create package.json."),
            vec!["package.json".to_string()]
        );
    }

    #[test]
    fn completion_without_write_feedback_accepts_second_no_tool_response() {
        let temp = tempfile::tempdir().unwrap();
        let mut client = MockClient::default();
        client.push_reply("I will create README.md.", Vec::new());
        client.push_reply("No file change is needed.", Vec::new());
        let mut session = SessionSnapshot::default();

        let reply = run_session(
            &mut client,
            "qwen3:8b",
            &mut session,
            "create a README file",
            &config(temp.path().to_path_buf(), 4),
        )
        .unwrap();

        assert_eq!(reply, "No file change is needed.");
        assert_eq!(client.feedback_messages.len(), 1);
        assert!(client.feedback_messages[0].contains("No file changes"));
        assert!(
            !session
                .messages
                .iter()
                .any(|m| m.content.starts_with(MINIMAL_FEEDBACK_PREFIX)),
            "ephemeral feedback must not be persisted to the session"
        );
    }

    #[test]
    fn completion_without_write_feedback_can_be_disabled() {
        let temp = tempfile::tempdir().unwrap();
        let mut client = MockClient::default();
        client.push_reply("Here is the explanation.", Vec::new());
        let mut session = SessionSnapshot::default();
        let mut cfg = config(temp.path().to_path_buf(), 4);
        cfg.completion_without_write_feedback = false;

        let reply = run_session(
            &mut client,
            "qwen3:8b",
            &mut session,
            "explain this repository",
            &cfg,
        )
        .unwrap();

        assert_eq!(reply, "Here is the explanation.");
        assert!(client.feedback_messages.is_empty());
    }

    #[test]
    fn completion_without_write_feedback_does_not_fire_in_plan_mode() {
        let temp = tempfile::tempdir().unwrap();
        let mut client = MockClient::default();
        client.push_reply("Plan: create README.md after review.", Vec::new());
        let mut session = SessionSnapshot::default();
        let mut cfg = config(temp.path().to_path_buf(), 4);
        cfg.mode = ExecutionMode::Plan;

        let reply = run_session(
            &mut client,
            "qwen3:8b",
            &mut session,
            "create README.md",
            &cfg,
        )
        .unwrap();

        assert_eq!(reply, "Plan: create README.md after review.");
        assert!(client.feedback_messages.is_empty());
    }
}
