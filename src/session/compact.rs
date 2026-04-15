use crate::session::store::ConversationMessage;

pub fn compact_messages(messages: &mut Vec<ConversationMessage>, keep_tail: usize) -> bool {
    if messages.len() <= keep_tail + 4 {
        return false;
    }

    let split_at = messages.len().saturating_sub(keep_tail);
    let mut head = messages[..split_at].to_vec();
    let tail = messages[split_at..].to_vec();

    let summary = summarize_messages(&head);
    head.retain(|message| message.role == "system");
    head.push(ConversationMessage::system(format!(
        "[compact-summary]\n{summary}"
    )));
    head.extend(tail);
    *messages = head;
    true
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
