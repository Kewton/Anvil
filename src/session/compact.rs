use crate::session::store::ConversationMessage;

pub const COMPACT_SUMMARY_PREFIX: &str = "[compact-summary]";

pub fn compact_messages(messages: &mut Vec<ConversationMessage>, keep_tail: usize) -> bool {
    compact_messages_with_strategy(messages, keep_tail, |head| Ok(summarize_messages(head)))
        .unwrap_or(false)
}

pub fn compact_messages_with_strategy<F>(
    messages: &mut Vec<ConversationMessage>,
    keep_tail: usize,
    summarizer: F,
) -> Result<bool, String>
where
    F: FnOnce(&[ConversationMessage]) -> Result<String, String>,
{
    if messages.len() <= keep_tail + 4 {
        return Ok(false);
    }

    let split_at = messages.len().saturating_sub(keep_tail);
    let mut head = messages[..split_at].to_vec();
    let tail = messages[split_at..].to_vec();

    let mut summary = summarizer(&head)?.trim().to_string();
    if summary.is_empty() {
        summary = summarize_messages(&head);
    }
    head.retain(|message| message.role == "system" && !is_compact_summary(message));
    head.push(ConversationMessage::system(format!(
        "{COMPACT_SUMMARY_PREFIX}\n{}",
        summary.chars().take(4_000).collect::<String>()
    )));
    head.extend(tail);
    *messages = head;
    Ok(true)
}

pub fn approximate_token_count(messages: &[ConversationMessage]) -> usize {
    messages
        .iter()
        .map(|message| {
            let content_tokens = message.content.chars().count() / 4;
            let tool_overhead = message.tool_calls.len() * 32;
            content_tokens + tool_overhead + 12
        })
        .sum()
}

pub fn render_messages_for_summary(messages: &[ConversationMessage], max_chars: usize) -> String {
    let mut rendered = String::new();
    for message in messages {
        let label = match message.role.as_str() {
            "tool" => match &message.name {
                Some(name) => format!("tool:{name}"),
                None => "tool".to_string(),
            },
            other => other.to_string(),
        };
        let line = format!("[{label}]\n{}\n\n", message.content.trim());
        let current_len = rendered.chars().count();
        let line_len = line.chars().count();
        if current_len + line_len > max_chars {
            let remaining = max_chars.saturating_sub(current_len);
            rendered.push_str(&line.chars().take(remaining).collect::<String>());
            rendered.push_str("\n...[truncated]");
            break;
        }
        rendered.push_str(&line);
    }
    rendered
}

pub fn is_compact_summary(message: &ConversationMessage) -> bool {
    message.role == "system" && message.content.starts_with(COMPACT_SUMMARY_PREFIX)
}

fn summarize_messages(messages: &[ConversationMessage]) -> String {
    let mut lines = Vec::new();
    let recent = messages
        .iter()
        .filter(|message| message.role != "system")
        .cloned()
        .collect::<Vec<_>>();
    for message in recent
        .iter()
        .rev()
        .take(12)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        let preview = message.content.replace('\n', " ");
        let preview = preview.chars().take(120).collect::<String>();
        lines.push(format!("{}: {}", message.role, preview));
    }
    if lines.is_empty() {
        "No notable earlier conversation.".to_string()
    } else {
        lines.join("\n")
    }
}
