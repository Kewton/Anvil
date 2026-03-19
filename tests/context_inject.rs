//! Integration tests for Issue #76: Context Injection (@file).
//!
//! Verifies the acceptance criteria for @file reference expansion,
//! session serialization behaviour, and effective_content DRY accessor.

use anvil::session::{MessageRole, SessionMessage, SessionRecord};

/// SessionMessage with expanded_content serializes WITHOUT it (#[serde(skip)]).
#[test]
fn session_message_serde_skip_expanded_content() {
    let mut msg = SessionMessage::new(MessageRole::User, "you", "@src/main.rs");
    msg.expanded_content = Some("@src/main.rs\n```\nfn main() {}\n```".to_string());

    let json = serde_json::to_string(&msg).expect("serialize");
    assert!(
        !json.contains("expanded_content"),
        "expanded_content must NOT appear in serialized JSON: {json}"
    );
    assert!(
        !json.contains("fn main()"),
        "expanded file content must NOT appear in serialized JSON"
    );

    // Deserialize back: expanded_content should be None
    let restored: SessionMessage = serde_json::from_str(&json).expect("deserialize");
    assert!(
        restored.expanded_content.is_none(),
        "expanded_content must be None after deserialization"
    );
    assert_eq!(restored.content, "@src/main.rs");
}

/// effective_content() returns expanded_content when present, content otherwise.
#[test]
fn effective_content_returns_expanded_when_present() {
    let mut msg = SessionMessage::new(MessageRole::User, "you", "raw input");
    assert_eq!(msg.effective_content(), "raw input");

    msg.expanded_content = Some("expanded input".to_string());
    assert_eq!(msg.effective_content(), "expanded input");
}

/// push_message uses effective_content for token estimation.
/// When expanded_content is set, the token count should reflect the
/// expanded (larger) content, not the raw input.
#[test]
fn push_message_uses_effective_content_for_token_count() {
    let cwd = std::env::current_dir().unwrap();
    let mut session = SessionRecord::new(cwd);

    // Push a message WITHOUT expanded_content
    let msg_raw = SessionMessage::new(MessageRole::User, "you", "short");
    session.push_message(msg_raw);
    let count_raw = session.estimated_token_count();

    // Push a message WITH expanded_content (much larger)
    let mut msg_expanded = SessionMessage::new(MessageRole::User, "you", "@src/main.rs");
    msg_expanded.expanded_content = Some("x ".repeat(5000)); // ~5000 tokens worth of content
    session.push_message(msg_expanded);
    let count_after = session.estimated_token_count();

    // The difference should be much larger than what "@src/main.rs" alone would add
    let delta = count_after - count_raw;
    assert!(
        delta > 100,
        "token count delta should reflect expanded_content size, got delta={delta}"
    );
}

/// SessionRecord round-trip: expanded_content is lost on save/load,
/// which is the intended design (raw input only in session file).
#[test]
fn session_record_serde_drops_expanded_content() {
    let cwd = std::env::current_dir().unwrap();
    let mut session = SessionRecord::new(cwd);

    let mut msg = SessionMessage::new(MessageRole::User, "you", "@src/lib.rs");
    msg.expanded_content = Some("pub mod app;".to_string());
    session.push_message(msg);

    let json = serde_json::to_string_pretty(&session).expect("serialize session");
    assert!(
        !json.contains("pub mod app;"),
        "expanded content must not be in serialized session"
    );

    let restored: SessionRecord = serde_json::from_str(&json).expect("deserialize session");
    assert_eq!(restored.messages.len(), 1);
    assert_eq!(restored.messages[0].content, "@src/lib.rs");
    assert!(restored.messages[0].expanded_content.is_none());
}
