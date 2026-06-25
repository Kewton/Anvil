use std::path::Path;

use crate::state::ConversationMessage;
use crate::tools::registry::ToolSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPromptMode {
    Native,
    XmlFallback,
}

pub fn build_request_messages(
    history: &[ConversationMessage],
    tools: &[ToolSpec],
    root: &Path,
    pending_feedback: Option<&str>,
    profile_guidance: Option<&str>,
    mode: ToolPromptMode,
) -> Vec<ConversationMessage> {
    let mut messages = Vec::new();
    messages.push(ConversationMessage::system(system_prompt(
        tools,
        root,
        profile_guidance,
        mode,
    )));
    messages.extend(history.iter().map(sanitize_history_message_for_request));
    if let Some(feedback) = pending_feedback {
        messages.push(ConversationMessage::user(feedback.to_string()));
    }
    messages
}

fn system_prompt(
    tools: &[ToolSpec],
    root: &Path,
    profile_guidance: Option<&str>,
    mode: ToolPromptMode,
) -> String {
    let tools = tools
        .iter()
        .map(|tool| format!("- {}: {}", tool.function.name, tool.function.description))
        .collect::<Vec<_>>()
        .join("\n");
    let mut prompt = format!(
        "You are anvilminimal, a local coding agent. Work only inside workspace `{}`.\n\
Use tools for file changes, repository facts, file inspection, build/test checks, and any action that changes the workspace.\n\
Do not end with planned future work. Do not claim files, tests, or builds succeeded unless you observed them with tools.\n\
Use workspace-relative paths only, and do not read or write outside the workspace.\n\
Do not claim completion until requested files and evidence are present.\n\
Tools:\n{}",
        root.display(),
        tools
    );
    if let Some(guidance) = profile_guidance {
        prompt.push_str("\n\nProfile guidance:\n");
        prompt.push_str(guidance);
    }
    if mode == ToolPromptMode::XmlFallback {
        prompt.push_str(
            "\n\nNative tool calls are unavailable for this provider turn. Use XML fallback tool calls exactly like:\n\
<anvil_tool_call name=\"Read\">{\"path\":\"src/main.rs\"}</anvil_tool_call>\n\
The XML body must be one JSON object matching the tool schema.",
        );
    }
    prompt
}

fn sanitize_history_message_for_request(message: &ConversationMessage) -> ConversationMessage {
    if message.role == "assistant" && !message.tool_calls.is_empty() {
        let mut cloned = message.clone();
        cloned.content.clear();
        return cloned;
    }
    message.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ToolCall;
    use crate::tools::registry::ToolRegistry;
    use serde_json::json;

    #[test]
    fn system_prompt_contains_source_safety_rules() {
        let messages = build_request_messages(
            &[],
            ToolRegistry::default().specs(),
            Path::new("/tmp/work"),
            None,
            None,
            ToolPromptMode::Native,
        );
        let prompt = &messages[0].content;
        assert!(prompt.contains("Do not end with planned future work"));
        assert!(prompt.contains("Do not claim files, tests, or builds succeeded"));
        assert!(prompt.contains("workspace-relative paths"));
    }

    #[test]
    fn xml_fallback_prompt_contains_tool_call_example() {
        let messages = build_request_messages(
            &[],
            ToolRegistry::default().specs(),
            Path::new("/tmp/work"),
            None,
            None,
            ToolPromptMode::XmlFallback,
        );
        let prompt = &messages[0].content;
        assert!(prompt.contains("<anvil_tool_call"));
        assert!(prompt.contains("one JSON object"));
        assert!(prompt.contains("Do not end with planned future work"));
    }

    #[test]
    fn tool_call_assistant_preamble_is_not_reprompted() {
        let history = vec![ConversationMessage::assistant(
            "I will create it now.",
            vec![ToolCall::new(
                "Write",
                json!({"path":"a.txt","content":"ok"}),
            )],
        )];
        let messages = build_request_messages(
            &history,
            ToolRegistry::default().specs(),
            Path::new("/tmp/work"),
            None,
            Some(
                "For the nextjs profile, create a runnable Next.js app, not only package metadata.",
            ),
            ToolPromptMode::Native,
        );
        assert_eq!(messages[1].role, "assistant");
        assert!(messages[1].content.is_empty());
        assert_eq!(history[0].content, "I will create it now.");
        assert!(messages[0].content.contains("Profile guidance"));
    }
}
