use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::modes::plan_act::ExecutionMode;
use crate::ollama::client::{AssistantReply, OllamaClient, should_use_native_tool_calls};
use crate::session::store::{ConversationMessage, SessionSnapshot};
use crate::tools::registry::{ToolContext, ToolRegistry, ToolSpec};
use crate::util::workspace_paths::WorkspacePolicy;

use super::compact::compact_if_needed;
use super::feedback::FeedbackState;
use super::prompt::{PromptToolMode, build_system_prompt};

pub const NO_COMPLETION_WITHOUT_WRITE_FEEDBACK_FLAG: &str =
    "ANVIL_NO_MINIMAL_COMPLETION_WITHOUT_WRITE_FEEDBACK";

pub trait MinimalChatClient {
    fn chat(
        &mut self,
        model: &str,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        native_tools_enabled: bool,
    ) -> Result<AssistantReply, String>;
}

impl MinimalChatClient for OllamaClient {
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
        should_use_native_tool_calls(model) && !session.native_tools_disabled;
    let workspace_policy = WorkspacePolicy::for_task_request(user_prompt);
    let mut feedback_state = FeedbackState::default();
    let mut pending_feedback: Option<String> = None;
    let mut tool_calls_seen = false;
    let mut write_or_edit_calls_seen = false;
    let mut completion_without_write_feedback_sent = false;

    session
        .messages
        .push(ConversationMessage::user(user_prompt.to_string()));

    let mut iterations = 0usize;
    while iterations < config.max_iterations {
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
                pending_feedback = Some(feedback);
                continue;
            }
            if config.mode == ExecutionMode::Act
                && !tool_calls_seen
                && !completion_without_write_feedback_sent
                && let Some(feedback) = feedback_state.missing_tool_call(user_prompt)
            {
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
            if is_write_or_edit_tool(&call.name) {
                write_or_edit_calls_seen = true;
            }
            let result = match registry.execute(&call.name, &call.arguments, &tool_context) {
                Ok(result) => result,
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
    }

    Err(format!(
        "minimal loop reached max_iterations ({})",
        config.max_iterations
    ))
}

pub fn completion_without_write_feedback_disabled_from_env() -> bool {
    std::env::var_os(NO_COMPLETION_WITHOUT_WRITE_FEEDBACK_FLAG)
        .is_some_and(|value| !value.is_empty())
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
            .cloned(),
    );
    if let Some(feedback) = ephemeral_feedback {
        messages.push(ConversationMessage::user(feedback.to_string()));
    }
    messages
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
            self.feedback_messages
                .extend(messages.iter().filter_map(|m| {
                    m.content
                        .starts_with(MINIMAL_FEEDBACK_PREFIX)
                        .then(|| m.content.clone())
                }));
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
        assert_eq!(client.native_modes[0], true);
        assert_eq!(client.native_modes[1], false);
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
            "create README.md",
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
            "create README.md",
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
