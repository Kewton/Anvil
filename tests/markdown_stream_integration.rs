//! Integration tests for issue #431 — terminal markdown rendering.
//!
//! Boundary conditions covered here:
//! - AC13: `SessionStore` save/load roundtrip with assistant raw content
//!   containing markdown produces a `session.json` whose `messages[*].content`
//!   has no ANSI escapes.
//! - AC15: `find_last_user_prompt` returns raw user text, regardless of
//!   markdown decoration choices elsewhere.
//! - AC18: `<think>` edge cases — single-line complete, cross-chunk, multiple
//!   occurrences on one line.
//! - `MAX_BUFFERED_LINE_CHARS`: newline-free streams do not cause unbounded
//!   memory and are emitted as sanitized plain text.
//! - Sanitization of 8-bit CSI / OSC and bidi controls in streamed markdown.

use anvil::session::compact::find_last_user_prompt;
use anvil::session::store::{ConversationMessage, SessionSnapshot, SessionStore};
use anvil::tui::markdown::MarkdownRenderer;
use tempfile::tempdir;

// -------- AC13 --------

#[test]
fn session_json_has_no_ansi_after_assistant_markdown_roundtrip() {
    let dir = tempdir().unwrap();
    let session_id = "0199fe00-0000-7000-8000-000000000901";
    let workspace_key = "md-ac13";
    std::fs::create_dir_all(dir.path().join("sessions").join(session_id)).unwrap();
    let store = SessionStore::new(dir.path(), session_id, workspace_key);

    // The raw assistant reply intentionally looks heavily markdownish to
    // simulate realistic LLM output. Even though the on-screen display would
    // be decorated, session storage must never see ANSI.
    let raw_assistant = "# Heading\n\nSome **bold** and `code`.\n\n```rust\nfn main() {}\n```\n";
    let snapshot = SessionSnapshot {
        messages: vec![
            ConversationMessage::user("please show me rust".to_string()),
            ConversationMessage::assistant(raw_assistant.to_string(), Vec::new()),
        ],
        ..SessionSnapshot::default()
    };
    store.save(&snapshot).unwrap();

    // Re-read the raw JSON file bytes so we catch any accidental mutation on
    // the write path (not just the in-memory snapshot roundtrip).
    let disk = std::fs::read_to_string(
        dir.path()
            .join("sessions")
            .join(session_id)
            .join("session.json"),
    )
    .unwrap();
    assert!(
        !disk.contains("\x1b["),
        "session.json on disk contains ANSI: {disk:?}"
    );

    let loaded = store.load_or_new(false).unwrap();
    for msg in &loaded.messages {
        assert!(
            !msg.content.contains("\x1b["),
            "loaded message contains ANSI: {msg:?}"
        );
    }
}

// -------- AC15 --------

#[test]
fn find_last_user_prompt_has_no_ansi_after_roundtrip() {
    let dir = tempdir().unwrap();
    let session_id = "0199fe00-0000-7000-8000-000000000902";
    let workspace_key = "md-ac15";
    std::fs::create_dir_all(dir.path().join("sessions").join(session_id)).unwrap();
    let store = SessionStore::new(dir.path(), session_id, workspace_key);

    let snapshot = SessionSnapshot {
        messages: vec![
            ConversationMessage::user("old prompt".to_string()),
            ConversationMessage::assistant("# Header".to_string(), Vec::new()),
            ConversationMessage::user("refreshed prompt with `inline`".to_string()),
            ConversationMessage::assistant("- item\n- another".to_string(), Vec::new()),
        ],
        ..SessionSnapshot::default()
    };
    store.save(&snapshot).unwrap();

    let loaded = store.load_or_new(false).unwrap();
    let last = find_last_user_prompt(&loaded.messages).expect("user prompt present");
    assert!(!last.contains("\x1b["), "user prompt has ANSI: {last:?}");
    assert_eq!(last, "refreshed prompt with `inline`");
}

// -------- AC18 think edge cases via the public renderer API --------

#[test]
fn think_block_single_line_is_stripped() {
    let mut r = MarkdownRenderer::new(true, true);
    let out = r.push_chunk("<think>secret</think>visible\n");
    assert_eq!(out, "visible\n");
}

#[test]
fn think_block_cross_chunk_is_stripped() {
    let mut r = MarkdownRenderer::new(true, true);
    let mut out = r.push_chunk("prefix <think>hide");
    out.push_str(&r.push_chunk("the detail</think> suffix\n"));
    assert_eq!(out, "prefix  suffix\n");
}

#[test]
fn think_block_multi_occurrence_is_stripped() {
    let mut r = MarkdownRenderer::new(true, true);
    let out = r.push_chunk("a<think>x</think>b<think>y</think>c\n");
    assert_eq!(out, "abc\n");
}

// -------- MAX_BUFFERED_LINE_CHARS overflow --------

#[test]
fn newline_free_overflow_is_fail_open() {
    let mut r = MarkdownRenderer::new(true, true);
    // 65K bytes with no newline. Buffer cap is 64K, so the drain path must
    // fire, producing ASCII-only output without panicking.
    let big = "a".repeat(65 * 1024);
    let mut out = r.push_chunk(&big);
    out.push_str(&r.flush());
    let count = out.chars().filter(|&c| c == 'a').count();
    assert_eq!(count, 65 * 1024);
    // No ANSI should have been injected by the fail-open path.
    assert!(!out.contains("\x1b["));
}

// -------- Sanitization of hostile characters in the stream --------

#[test]
fn streamed_8bit_csi_osc_and_bidi_are_replaced_with_question_mark() {
    let mut r = MarkdownRenderer::new(true, true);
    // U+009B (CSI), U+009D (OSC), U+202E (RTL override) all inside a normal
    // non-heading non-list non-think line.
    let hostile = "ok\u{009B}bad\u{009D}worse\u{202E}bidi\n";
    let out = r.push_chunk(hostile);
    // Every hostile codepoint must be replaced by `?`.
    assert!(!out.contains('\u{009B}'));
    assert!(!out.contains('\u{009D}'));
    assert!(!out.contains('\u{202E}'));
    assert!(out.contains('?'));
    // Non-sanitized text must survive.
    assert!(out.contains("ok"));
    assert!(out.contains("bad"));
    assert!(out.contains("bidi"));
}

#[test]
fn streamed_raw_esc_is_replaced() {
    let mut r = MarkdownRenderer::new(true, true);
    // Raw `\x1b[31m` inside a line must be sanitized so the renderer does not
    // forward a foreign SGR escape to stdout.
    let hostile = "hello\x1b[31mred text\n";
    let out = r.push_chunk(hostile);
    // The original escape must NOT appear verbatim. Only renderer-owned SGR
    // escapes (which we aren't emitting for plain prose) could appear; for
    // this line there's no heading/bold/code so there should be no SGR.
    assert!(!out.contains("\x1b[31m"), "foreign SGR leaked: {out:?}");
    assert!(!out.contains("\x1b["), "any ANSI is unexpected: {out:?}");
    // The literal text after the escape survived.
    assert!(out.contains("red text"));
    assert!(out.contains("hello"));
}

// -------- Chunk-split decoration equals non-split decoration --------

#[test]
fn chunk_split_output_matches_non_split() {
    let input = "# Title\n\nuse **bold** and `code`\n- item one\n- item two\n";
    let baseline = {
        let mut r = MarkdownRenderer::new(true, true);
        let mut out = r.push_chunk(input);
        out.push_str(&r.flush());
        out
    };
    // 1-byte chunks.
    let splitted = {
        let mut r = MarkdownRenderer::new(true, true);
        let mut out = String::new();
        for ch in input.chars() {
            let mut buf = [0u8; 4];
            out.push_str(&r.push_chunk(ch.encode_utf8(&mut buf)));
        }
        out.push_str(&r.flush());
        out
    };
    assert_eq!(splitted, baseline);
}
