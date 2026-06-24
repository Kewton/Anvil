use std::path::Path;

use crate::state::ConversationMessage;
use crate::tools::registry::ToolSpec;

pub fn build_request_messages(
    history: &[ConversationMessage],
    tools: &[ToolSpec],
    root: &Path,
    pending_feedback: Option<&str>,
) -> Vec<ConversationMessage> {
    let mut messages = Vec::new();
    messages.push(ConversationMessage::system(system_prompt(tools, root)));
    messages.extend_from_slice(history);
    if let Some(feedback) = pending_feedback {
        messages.push(ConversationMessage::user(feedback.to_string()));
    }
    messages
}

fn system_prompt(tools: &[ToolSpec], root: &Path) -> String {
    let tools = tools
        .iter()
        .map(|tool| format!("- {}: {}", tool.function.name, tool.function.description))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "You are anvilminimal, a local coding agent. Work only inside workspace `{}`.\nUse tools for file changes. Do not claim completion until requested files and evidence are present.\nTools:\n{}",
        root.display(),
        tools
    )
}
