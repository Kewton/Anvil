use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::session::compact::{COMPACT_SUMMARY_PREFIX, render_messages_for_summary};
use crate::session::store::ConversationMessage;

const MAX_TOOL_MESSAGE_CHARS: usize = 12_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolProtocol {
    Native,
    TaggedXml,
}

impl ToolProtocol {
    pub(crate) fn from_native_tools_enabled(native_tools_enabled: bool) -> Self {
        if native_tools_enabled {
            Self::Native
        } else {
            Self::TaggedXml
        }
    }

    pub(crate) fn native_tools_enabled(self) -> bool {
        matches!(self, Self::Native)
    }

    pub(crate) fn fallback_instruction(self) -> Option<String> {
        match self {
            Self::Native => None,
            Self::TaggedXml => Some(format!(
                "For this model, do not emit native tool_calls. When you need tools, output only {} with valid JSON arguments.",
                self.fallback_example()
            )),
        }
    }

    pub(crate) fn parser_downgrade_notice(self) -> String {
        match self {
            Self::Native => format!(
                "[Runtime Notice] Native tool calling is disabled for this session because the model/runtime parser rejected a tool-call response. Use only {} with valid JSON arguments.",
                Self::TaggedXml.fallback_example()
            ),
            Self::TaggedXml => format!(
                "[Runtime Notice] Continue using {} with valid JSON arguments.",
                self.fallback_example()
            ),
        }
    }

    pub(crate) fn fallback_example(self) -> &'static str {
        "<anvil_tool_call>{\"name\":\"Tool\",\"arguments\":{...}}</anvil_tool_call>"
    }
}

pub fn detect_language_hint(text: &str) -> &'static str {
    if text.chars().any(|ch| {
        ('\u{3040}'..='\u{30ff}').contains(&ch) || ('\u{4e00}'..='\u{9faf}').contains(&ch)
    }) {
        "Japanese"
    } else {
        "Match the user's language"
    }
}

pub(crate) fn runtime_context_messages(
    cwd: &Path,
    work_root: &Path,
    protocol: ToolProtocol,
) -> Vec<ConversationMessage> {
    let mut messages = Vec::new();
    if let Some(instruction) = protocol.fallback_instruction() {
        messages.push(ConversationMessage::system(instruction));
    }
    if work_root != cwd {
        messages.push(ConversationMessage::system(format!(
            "Current project root is {}. Use relative paths from this directory unless an absolute path is easier for file tools.",
            work_root.display()
        )));
    }
    messages
}

pub(crate) fn detect_scaffold_root(name: &str, arguments: &Value, result: &str) -> Option<PathBuf> {
    if name != "Bash" {
        return None;
    }
    arguments.get("command").and_then(Value::as_str)?;
    detect_created_project_root(result)
}

pub(crate) fn reset_messages_after_scaffold(
    root: &Path,
    messages: &[ConversationMessage],
) -> Vec<ConversationMessage> {
    let summary = build_scaffold_phase_summary(root, messages);
    let latest_user = messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .cloned();

    let mut reset = vec![ConversationMessage::system(format!(
        "{COMPACT_SUMMARY_PREFIX}\n{summary}"
    ))];
    if let Some(user) = latest_user {
        reset.push(user);
    }
    reset
}

pub(crate) fn should_skip_system_note(messages: &[ConversationMessage], note: &str) -> bool {
    for message in messages.iter().rev() {
        if message.role == "user" {
            break;
        }
        if message.role == "system" && message.content == note {
            return true;
        }
    }
    false
}

pub(crate) fn compact_tool_result(name: &str, result: String) -> String {
    if result.chars().count() <= MAX_TOOL_MESSAGE_CHARS {
        return result;
    }

    let chars = result.chars().collect::<Vec<_>>();
    let total = chars.len();
    let head_len = if name == "Read" { 8_000 } else { 10_000 }.min(total);
    let tail_len = if name == "Read" { 2_000 } else { 1_000 }.min(total.saturating_sub(head_len));
    let head = chars[..head_len].iter().collect::<String>();
    let tail = if tail_len == 0 {
        String::new()
    } else {
        chars[total - tail_len..].iter().collect::<String>()
    };

    if tail.is_empty() {
        format!("{head}\n...[truncated {} chars]", total - head_len)
    } else {
        format!(
            "{head}\n...[truncated {} chars]...\n{tail}",
            total.saturating_sub(head_len + tail_len)
        )
    }
}

pub(crate) fn detect_created_project_root(tool_output: &str) -> Option<PathBuf> {
    for line in tool_output.lines() {
        let trimmed = line.trim();
        if let Some(path) = trimmed.strip_prefix("Success! Created ") {
            let (_, path) = path.rsplit_once(" at ")?;
            return Some(PathBuf::from(path.trim()));
        }
        if let Some(rest) = trimmed.strip_prefix("Creating a new ")
            && let Some((_, path)) = rest.rsplit_once(" in ")
        {
            return Some(PathBuf::from(path.trim_end_matches('.').trim()));
        }
    }
    None
}

fn build_scaffold_phase_summary(root: &Path, messages: &[ConversationMessage]) -> String {
    let compact_history = render_messages_for_summary(messages, 2_000);
    let cwd_line = format!("Project scaffold is complete at {}.", root.display());
    format!(
        "{cwd_line}\nContinue implementation in this project root without restarting setup.\nRecent context:\n{compact_history}"
    )
}
