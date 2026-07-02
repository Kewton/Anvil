//! Small helpers + shared types extracted from `turn.rs` (parent
//! #680).
//!
//! Hosts the free functions and shared types that lived at the
//! module level (NOT `impl Agent`) of `turn.rs`:
//!
//! - `extract_filename_with_suffix` — pull a bare token ending in the
//!   given suffix out of free-form text (tool-call argument /
//!   feedback recovery heuristics).
//! - `write_stdout_rendered` — `io::stdout().lock()` + `raw_mode_safe_text`
//!   write helper used by iteration status / format-iteration-status
//!   surfaces.
//! - `tool_result_failed` — predicate: `Error:` prefix or
//!   `interrupted=true` marker.
//! - `quality_confirm_cache_key` — Issue #580 SSoT memoization hash
//!   key for the Quality-gate second-pass adapter.
//! - `RetrievalInjection` — Issue #555 photon retrieval message +
//!   selected-id pair (used by `anti_pattern_flow` / `case_record_flow`
//!   / `build_request_messages`).
//! - `WrittenScaffoldArtifacts` — type alias for the deterministic
//!   scaffold installer return value.
//!
//! `pub(super)` limited / no facade re-export (DR3-001).

use std::io::{self, Write};
use std::path::PathBuf;

use super::small_helpers::raw_mode_safe_text;
use crate::session::store::{ConversationMessage, ScaffoldArtifactFileSnapshot};

pub(super) fn extract_filename_with_suffix(text: &str, suffix: &str) -> Option<String> {
    text.split(|ch: char| {
        ch.is_whitespace()
            || matches!(
                ch,
                '`' | '"'
                    | '\''
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '、'
                    | '。'
                    | '，'
                    | '：'
                    | ':'
                    | ';'
            )
    })
    .map(|token| token.trim_matches([',', '.', '。', '、']))
    .find(|token| {
        token.ends_with(suffix)
            && token.len() <= 80
            && !token.contains('/')
            && !token.contains('\\')
            && !token.starts_with('.')
            && token
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    })
    .map(ToString::to_string)
}

pub(super) fn write_stdout_rendered(text: &str, trailing_newline: bool) {
    let mut out = io::stdout().lock();
    let rendered = raw_mode_safe_text(text);
    let _ = out.write_all(rendered.as_bytes());
    if trailing_newline {
        let _ = out.write_all(b"\r\n");
    }
    let _ = out.flush();
}

pub(super) fn tool_result_failed(result: &str) -> bool {
    result.starts_with("Error:") || result.contains("\ninterrupted=true\n")
}

/// Issue #555: carries a retrieval message and the IDs of the
/// selected records so the photon mapper can include them without
/// re-parsing the rendered prompt text.
pub(super) struct RetrievalInjection {
    pub message: ConversationMessage,
    pub selected_ids: Vec<String>,
}

pub(super) type WrittenScaffoldArtifacts = (Vec<PathBuf>, Vec<ScaffoldArtifactFileSnapshot>);

/// Issue #580: SSoT memoization key for the Quality-gate second-pass
/// adapter. Hashes `(request, full_content)` with `DefaultHasher` (per
/// design judgement #5: full_content avoids stale reuse when only the
/// middle of a large file changes — the LLM still sees only the
/// head+tail excerpt).
pub(super) fn quality_confirm_cache_key(request: &str, content: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    request.hash(&mut hasher);
    content.hash(&mut hasher);
    hasher.finish()
}
