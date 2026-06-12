use std::collections::BTreeSet;

use crate::session::store::ConversationMessage;

pub const MINIMAL_COMPACT_SUMMARY_PREFIX: &str = "[minimal-compact-summary]";

const SUMMARY_MAX_CHARS: usize = 1_800;

pub fn compact_if_needed(messages: &mut Vec<ConversationMessage>, context_budget: usize) -> bool {
    let threshold = compaction_threshold(context_budget);
    if approximate_token_count(messages) <= threshold {
        return false;
    }

    let protected = protected_message_indices(messages, threshold / 2);
    let mut summary_input = Vec::new();
    let mut kept = Vec::new();

    for (idx, message) in messages.iter().enumerate() {
        if message.role == "system" || is_minimal_compact_summary(message) {
            continue;
        }
        if protected.contains(&idx) {
            kept.push(message.clone());
        } else {
            summary_input.push(message.clone());
        }
    }

    let mut compacted = Vec::new();
    if let Some(summary) = deterministic_summary(&summary_input) {
        compacted.push(ConversationMessage::assistant(summary, Vec::new()));
    }
    compacted.extend(kept);
    *messages = compacted;
    true
}

pub fn approximate_token_count(messages: &[ConversationMessage]) -> usize {
    messages
        .iter()
        .map(|message| {
            let content_tokens = message.content.chars().count() / 4;
            let tool_call_tokens = message.tool_calls.len() * 32;
            content_tokens + tool_call_tokens + 12
        })
        .sum()
}

pub fn is_minimal_compact_summary(message: &ConversationMessage) -> bool {
    message.role == "assistant" && message.content.starts_with(MINIMAL_COMPACT_SUMMARY_PREFIX)
}

fn compaction_threshold(context_budget: usize) -> usize {
    context_budget.saturating_mul(7).saturating_div(10).max(1)
}

fn protected_message_indices(
    messages: &[ConversationMessage],
    recent_budget_tokens: usize,
) -> BTreeSet<usize> {
    let mut protected = BTreeSet::new();

    if let Some((idx, _)) = messages
        .iter()
        .enumerate()
        .rev()
        .find(|(_, message)| message.role == "user")
    {
        protected.insert(idx);
    }

    for tool_name in ["Read", "Edit"] {
        for (idx, _) in messages
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, message)| {
                message.role == "tool" && message.name.as_deref() == Some(tool_name)
            })
            .take(2)
        {
            protected.insert(idx);
        }
    }

    let mut recent_tokens = 0usize;
    for (idx, message) in messages.iter().enumerate().rev() {
        if message.role == "system" || is_minimal_compact_summary(message) {
            continue;
        }
        let next = approximate_token_count(std::slice::from_ref(message));
        if recent_tokens + next > recent_budget_tokens && protected.len() >= 4 {
            break;
        }
        protected.insert(idx);
        recent_tokens += next;
    }

    protected
}

fn deterministic_summary(messages: &[ConversationMessage]) -> Option<String> {
    let mut lines = Vec::new();
    for message in messages
        .iter()
        .filter(|m| m.role != "system")
        .rev()
        .take(12)
    {
        let label = match message.role.as_str() {
            "tool" => message.name.as_deref().unwrap_or("tool"),
            other => other,
        };
        let preview = message.content.replace('\n', " ");
        let preview = preview.chars().take(140).collect::<String>();
        if !preview.trim().is_empty() || !message.tool_calls.is_empty() {
            lines.push(format!("{label}: {preview}"));
        }
    }
    if lines.is_empty() {
        return None;
    }
    lines.reverse();
    let body = lines.join("\n");
    Some(format!(
        "{MINIMAL_COMPACT_SUMMARY_PREFIX}\n{}",
        body.chars().take(SUMMARY_MAX_CHARS).collect::<String>()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_count_alone_does_not_trigger_compaction() {
        let mut messages = (0..80)
            .map(|i| ConversationMessage::user(format!("ok {i}")))
            .collect::<Vec<_>>();

        let compacted = compact_if_needed(&mut messages, 100_000);

        assert!(!compacted);
        assert_eq!(messages.len(), 80);
    }

    #[test]
    fn token_compaction_keeps_recent_read_and_edit_outputs() {
        let mut messages = vec![ConversationMessage::system(
            "legacy recovery note that must not survive".to_string(),
        )];
        for i in 0..24 {
            messages.push(ConversationMessage::assistant(
                format!("old context {i} {}", "x".repeat(160)),
                Vec::new(),
            ));
        }
        messages.push(ConversationMessage::user(
            "Please edit src/lib.rs using the visible anchor".to_string(),
        ));
        messages.push(ConversationMessage::tool(
            "Read".to_string(),
            "src/lib.rs excerpt: KEEP_THIS_ANCHOR".to_string(),
        ));
        messages.push(ConversationMessage::tool(
            "Edit".to_string(),
            "ERROR: target text not found in src/lib.rs".to_string(),
        ));

        let compacted = compact_if_needed(&mut messages, 800);

        assert!(compacted);
        assert!(messages.iter().all(|message| message.role != "system"));
        assert!(
            messages
                .iter()
                .any(|message| message.content.starts_with(MINIMAL_COMPACT_SUMMARY_PREFIX))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.content.contains("KEEP_THIS_ANCHOR"))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.content.contains("target text not found"))
        );
    }

    #[test]
    fn old_minimal_summaries_are_replaced_not_nested() {
        let mut messages = vec![ConversationMessage::assistant(
            format!("{MINIMAL_COMPACT_SUMMARY_PREFIX}\nold summary"),
            Vec::new(),
        )];
        for i in 0..20 {
            messages.push(ConversationMessage::assistant(
                format!("old context {i} {}", "x".repeat(200)),
                Vec::new(),
            ));
        }
        messages.push(ConversationMessage::user("latest request".to_string()));

        assert!(compact_if_needed(&mut messages, 600));

        let summary_count = messages
            .iter()
            .filter(|message| message.content.starts_with(MINIMAL_COMPACT_SUMMARY_PREFIX))
            .count();
        assert_eq!(summary_count, 1);
        assert!(messages.iter().any(|m| m.content == "latest request"));
    }
}
