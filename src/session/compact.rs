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

/// Walk the message history from the tail toward the head and return the
/// content of the most recent `role=user` message, skipping compaction
/// summaries. Used by `--resume` to replay the last user turn.
///
/// Returns `None` when the tail (or the entire history) has been collapsed
/// into a summary by `compact_messages` — callers should surface a specific
/// error telling the user the last prompt is no longer replayable.
pub fn find_last_user_prompt(messages: &[ConversationMessage]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|m| m.role == "user" && !is_compact_summary(m))
        .map(|m| m.content.clone())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// AC14: `summarize_messages` produces summaries without ANSI escapes. The
    /// input is pre-render raw text, but the output goes back into the LLM so
    /// we defensively assert the lack of `\x1b[`. Uses only ANSI-free content
    /// — summarize_messages is synchronous and does not call the network.
    #[test]
    fn summarize_messages_output_has_no_ansi() {
        let messages = vec![
            ConversationMessage::user("plain user text".to_string()),
            ConversationMessage::assistant(
                "# heading\n\nSome **bold** text and `code`.".to_string(),
                Vec::new(),
            ),
            ConversationMessage::tool("Read".to_string(), "file contents".to_string()),
        ];
        let summary = summarize_messages(&messages);
        assert!(
            !summary.contains("\x1b["),
            "ANSI escape leaked into summary: {summary:?}"
        );
    }

    /// AC14: `render_messages_for_summary` produces ANSI-free output.
    #[test]
    fn render_messages_for_summary_output_has_no_ansi() {
        let messages = vec![
            ConversationMessage::user("plain user text".to_string()),
            ConversationMessage::assistant(
                "# heading\n\n`code here` and **bold**".to_string(),
                Vec::new(),
            ),
        ];
        let rendered = render_messages_for_summary(&messages, 2_000);
        assert!(
            !rendered.contains("\x1b["),
            "ANSI escape leaked into rendered summary: {rendered:?}"
        );
    }

    /// AC7 (Issue #451): `compact_messages` operates only on the message
    /// vector and never observes `WorkingMemory.active_precautions`. We seed
    /// a snapshot with a precaution + 40 messages, run compaction, and assert
    /// the precaution Vec is untouched.
    #[test]
    fn compact_messages_preserves_active_precautions() {
        use crate::session::precaution::{
            Precaution, PrecautionSource, PrecautionStatus, Severity,
        };
        use crate::session::store::{SessionSnapshot, WorkingMemory};
        use std::path::PathBuf;

        let dir = tempfile::tempdir().unwrap();
        let mut wm = WorkingMemory::default();
        wm.add_precaution(
            Precaution {
                id: String::new(),
                source: PrecautionSource::BuildFailure,
                severity: Severity::High,
                text: "Avoid using sed for in-place edits".to_string(),
                applies_to: vec![PathBuf::from("src/lib.rs")],
                status: PrecautionStatus::Active,
                retired_reason: None,
            },
            dir.path(),
        );

        let mut snapshot = SessionSnapshot {
            messages: (0..40)
                .map(|i| ConversationMessage::user(format!("message {i}")))
                .collect(),
            working_memory: wm,
            ..SessionSnapshot::default()
        };

        let before = snapshot.working_memory.active_precautions.clone();
        let compacted = compact_messages(&mut snapshot.messages, 10);
        assert!(compacted, "expected compaction to fire on 40-message log");
        assert_eq!(
            snapshot.working_memory.active_precautions, before,
            "active_precautions must survive compact_messages"
        );
    }

    /// AC10 (Issue #450): `compact_messages` collapses head messages into a
    /// summary but never observes (let alone touches) `SessionSnapshot.last_feedback`.
    /// The contract is "compaction operates on the message vector only,
    /// last_feedback survives by virtue of separation."
    #[test]
    fn compact_messages_preserves_last_feedback() {
        use crate::session::feedback::{
            FeedbackFrame, FeedbackFrameDraft, FeedbackKind, build_feedback_frame,
        };
        use crate::session::store::SessionSnapshot;
        use std::path::PathBuf;

        let workspace = PathBuf::from(".");
        let frame = build_feedback_frame(
            FeedbackFrameDraft {
                kind: FeedbackKind::CompileError,
                primary_error: Some("compile error: type mismatch".to_string()),
                ..Default::default()
            },
            &workspace,
        );

        let mut snapshot = SessionSnapshot {
            messages: (0..40)
                .map(|i| ConversationMessage::user(format!("message {i}")))
                .collect(),
            last_feedback: Some(frame.clone()),
            ..SessionSnapshot::default()
        };

        let before: Option<FeedbackFrame> = snapshot.last_feedback.clone();
        let compacted = compact_messages(&mut snapshot.messages, 10);
        assert!(compacted, "expected compaction to fire on 40-message log");
        assert_eq!(
            snapshot.last_feedback, before,
            "last_feedback must survive compact_messages"
        );
    }
}
