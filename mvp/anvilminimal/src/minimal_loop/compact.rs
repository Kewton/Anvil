use crate::state::ConversationMessage;

pub fn compact_if_needed(messages: &mut Vec<ConversationMessage>, context_budget: usize) {
    let approx_chars = context_budget.saturating_mul(4);
    let mut total: usize = messages.iter().map(|m| m.content.len()).sum();
    while total > approx_chars && messages.len() > 4 {
        let protected = protected_indices(messages);
        let remove_at = (1..messages.len())
            .find(|index| !protected.contains(index))
            .unwrap_or(1);
        let removed = messages.remove(remove_at);
        total = total.saturating_sub(removed.content.len());
    }
}

fn protected_indices(messages: &[ConversationMessage]) -> Vec<usize> {
    let mut indices = Vec::new();
    if messages
        .first()
        .is_some_and(|message| message.role == "system")
    {
        indices.push(0);
    }
    if let Some(index) = messages.iter().rposition(|message| message.role == "user") {
        indices.push(index);
    }
    for (index, message) in messages.iter().enumerate().rev() {
        if is_evidence_message(message) {
            indices.push(index);
            if indices.len() >= 6 {
                break;
            }
        }
    }
    indices.sort_unstable();
    indices.dedup();
    indices
}

fn is_evidence_message(message: &ConversationMessage) -> bool {
    if message.role != "tool" {
        return false;
    }
    match message.name.as_deref() {
        Some("Read") => true,
        Some("Edit") => {
            let lower = message.content.to_ascii_lowercase();
            lower.contains("error")
                || lower.contains("edit_")
                || lower.contains("recoverable")
                || lower.contains("anchor")
        }
        Some("Bash") => {
            let lower = message.content.to_ascii_lowercase();
            lower.contains("outcome: commandfailed")
                || lower.contains("outcome: timeout")
                || lower.contains("outcome: cancelled")
                || lower.contains("command failed")
                || lower.contains("error")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compaction_preserves_latest_user_message() {
        let mut messages = vec![
            ConversationMessage::system("system"),
            ConversationMessage::user("old user".repeat(200)),
            ConversationMessage::assistant("old assistant".repeat(200), Vec::new()),
            ConversationMessage::user("latest user"),
            ConversationMessage::assistant("tail".repeat(200), Vec::new()),
        ];
        compact_if_needed(&mut messages, 16);
        assert!(
            messages
                .iter()
                .any(|message| message.content == "latest user")
        );
    }

    #[test]
    fn compaction_preserves_recent_read_evidence() {
        let mut messages = vec![
            ConversationMessage::system("system"),
            ConversationMessage::user("old user".repeat(200)),
            ConversationMessage::assistant("old assistant".repeat(200), Vec::new()),
            ConversationMessage::tool("Read", "important anchor"),
            ConversationMessage::user("latest user"),
            ConversationMessage::assistant("tail".repeat(200), Vec::new()),
        ];
        compact_if_needed(&mut messages, 16);
        assert!(
            messages
                .iter()
                .any(|message| message.name.as_deref() == Some("Read")
                    && message.content == "important anchor")
        );
    }

    #[test]
    fn compaction_preserves_recent_edit_error() {
        let mut messages = vec![
            ConversationMessage::system("system"),
            ConversationMessage::user("old user".repeat(200)),
            ConversationMessage::assistant("old assistant".repeat(200), Vec::new()),
            ConversationMessage::tool("Edit", "edit_anchor_not_found: missing"),
            ConversationMessage::user("latest user"),
            ConversationMessage::assistant("tail".repeat(200), Vec::new()),
        ];
        compact_if_needed(&mut messages, 16);
        assert!(
            messages
                .iter()
                .any(|message| message.name.as_deref() == Some("Edit")
                    && message.content.contains("edit_anchor_not_found"))
        );
    }
}
