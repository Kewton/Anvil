//! Issue #557: Safe renderer for photon-action-memory context_pack responses.
//!
//! Converts a `ContextPackResponse` into a prompt section string by:
//!   1. Parsing `items[]` (kind/summary/source projection only)
//!   2. Filtering to `kind == "summary"` items only
//!   3. Applying `mask_payload_inplace` at the JSON Object level
//!   4. Extracting and `mask_secrets`-ing the summary text
//!   5. Normalising control characters (CR/LF/TAB/bidi)
//!   6. Rejecting prompt injection and destructive command patterns
//!   7. Enforcing size limits
//!   8. Building the "Photon Context:" section
//!
//! Issue #583: warnings-based seed filtering.
//! `extract_blocked_summary_ids` parses `warnings[]` from the response and returns
//! the set of summary IDs flagged by `summary_quality_gate` with the
//! `premature_termination_risk` reason. `render_context_pack` accepts this set
//! and skips matching items before the prompt-injection / destructive-command
//! filters. The function is pure (no I/O, no side effects); log emission lives
//! in the adapter at `src/agent/loop_run/turn.rs::invoke_photon_context_pack`.

use std::collections::HashSet;

use crate::logging::mask_payload_inplace;
use crate::photon::schema::ContextPackResponse;
use crate::session::feedback::mask_secrets;

// ---------------------------------------------------------------------------
// Public constants (SSOT for all callers)
// ---------------------------------------------------------------------------

pub const MAX_PROMPT_ITEMS: usize = 5;
/// Maximum items scanned before output limit — limits DoS via large arrays.
pub const MAX_PROMPT_SCAN_ITEMS: usize = 50;
pub const MAX_PROMPT_ITEM_CHARS: usize = 200;
pub const MAX_PROMPT_TOTAL_CHARS: usize = 800;

// ---------------------------------------------------------------------------
// Issue #583: warning filter SSOT constants
// ---------------------------------------------------------------------------

/// Maximum number of `warnings[]` entries scanned per response (DoS cap).
pub const MAX_PHOTON_WARNINGS_SCAN: usize = 64;
/// Maximum number of unique blocked summary IDs tracked per response.
pub const MAX_BLOCKED_SUMMARY_IDS: usize = 32;
/// Maximum byte length of a single sanitized summary ID.
pub const MAX_BLOCKED_SUMMARY_ID_BYTES: usize = 256;
/// Maximum byte length of a single warning message accepted before parsing.
pub const MAX_PHOTON_WARNING_MESSAGE_BYTES: usize = 2048;
/// Single reason string treated as "blocking" by the warning filter.
pub const BLOCKED_WARNING_REASON: &str = "premature_termination_risk";

// ---------------------------------------------------------------------------
// Pattern lists — case-insensitive substring match via to_ascii_lowercase()
// ---------------------------------------------------------------------------

const PROMPT_INJECTION_PATTERNS: &[&str] = &[
    "[inst]",
    "###",
    "system:",
    "<|im_start|>",
    "<|endoftext|>",
    "ignore previous instructions",
    "you are now",
    // boundary escape / role spoofing (DR4-001)
    "[end photon external memory]",
    "photon external memory",
    "photon context:",
    "developer:",
    "assistant:",
    "tool:",
    "user:",
    "tool_call",
    "function_call",
    "<tool",
    "</tool",
];

const DESTRUCTIVE_PATTERNS: &[&str] = &[
    "rm -rf",
    "drop table",
    "git reset --hard",
    "git clean -fd",
    ":(){:|:&};:",
    "mkfs",
    "dd if=",
];

// ---------------------------------------------------------------------------
// Issue #583: value objects for warning filter and render stats
// ---------------------------------------------------------------------------

/// Statistics about the blocked-summary-id extraction pass.
///
/// `pub(crate)` because callers outside the photon crate only see the resulting
/// `HashSet<String>` via the adapter; the stats are emitted as a log event.
#[derive(Debug, Default, Clone)]
pub(crate) struct BlockedIdsStats {
    /// Total number of entries in the original `warnings[]` array (cap-unaware).
    pub total_warnings: usize,
    /// `true` when `warnings.len() > MAX_PHOTON_WARNINGS_SCAN`.
    pub truncated_scan: bool,
    /// `true` when the unique-ID cap (`MAX_BLOCKED_SUMMARY_IDS`) was reached.
    pub truncated_unique: bool,
}

/// Per-call statistics produced by `render_context_pack`.
///
/// Invariant (DS1-005 / DR3-002, CB-001 reshape):
/// `items_in_response == scanned + items_scan_capped`
/// where
/// `scanned == items_adopted + items_blocked + items_filtered_kind
///  + items_filtered_missing_text + items_filtered_security + items_over_cap
///  + items_dropped_total_chars`.
/// (`items_in_response` is the length of the original `items[]` array in the
/// response, before `MAX_PROMPT_SCAN_ITEMS` is applied.)
#[derive(Debug, Default, Clone)]
pub struct RenderStats {
    /// Items skipped because their sanitized `id` was in `blocked_ids`.
    pub items_blocked: usize,
    /// Items actually emitted into the rendered section (post-total-cap).
    pub items_adopted: usize,
    /// Items accepted by all filters but before the total-chars cap was applied.
    pub items_accepted_pre_total_cap: usize,
    /// Items dropped because their kind was not a recognised summary kind.
    pub items_filtered_kind: usize,
    /// Items dropped because they had no `text` / `summary` field.
    pub items_filtered_missing_text: usize,
    /// Items dropped by the prompt-injection / destructive-command filter.
    pub items_filtered_security: usize,
    /// Items dropped because `MAX_PROMPT_ITEMS` had already been reached.
    pub items_over_cap: usize,
    /// Items accepted pre-cap but not emitted due to `MAX_PROMPT_TOTAL_CHARS`.
    pub items_dropped_total_chars: usize,
    /// Items in the response array beyond `MAX_PROMPT_SCAN_ITEMS` that were
    /// never inspected (CB-001 conservation closure).
    pub items_scan_capped: usize,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Render a `ContextPackResponse` into a `"Photon Context:\n- …"` section.
/// Returns `None` when no valid items survive the filter pipeline, alongside
/// `RenderStats` describing the pipeline's bookkeeping (Issue #583).
/// Pure function — no side effects, no I/O, no unsafe.
pub fn render_context_pack(
    resp: &ContextPackResponse,
    blocked_ids: &HashSet<String>,
) -> (Option<String>, RenderStats) {
    let (items, total_in_array) = parse_items_with_total(&resp.0);
    // CB-001: account for items beyond MAX_PROMPT_SCAN_ITEMS so the conservation
    // closure `items_in_response = scanned + items_scan_capped` holds even when
    // the scan cap fires on large responses or under DoS.
    let mut stats = RenderStats {
        items_scan_capped: total_in_array.saturating_sub(items.len()),
        ..RenderStats::default()
    };
    let mut filtered: Vec<String> = Vec::new();

    for item in items.into_iter() {
        if !is_summary_kind(&item) {
            stats.items_filtered_kind += 1;
            continue;
        }
        // Issue #583: skip items whose sanitized `id` matches a blocked entry
        // (fail-open: missing id / non-string id passes through).
        let id_blocked = item
            .get("id")
            .and_then(|v| v.as_str())
            .map(|id| blocked_ids.contains(id))
            .unwrap_or(false);
        if id_blocked {
            stats.items_blocked += 1;
            continue;
        }

        let masked = mask_item(item);
        let text = match extract_summary_text(&masked) {
            Some(t) => normalize_summary_text(&t),
            None => {
                stats.items_filtered_missing_text += 1;
                continue;
            }
        };
        if contains_prompt_injection(&text) || contains_destructive_command(&text) {
            stats.items_filtered_security += 1;
            continue;
        }
        if filtered.len() >= MAX_PROMPT_ITEMS {
            stats.items_over_cap += 1;
            continue;
        }
        filtered.push(truncate_chars(text, MAX_PROMPT_ITEM_CHARS));
        stats.items_accepted_pre_total_cap += 1;
    }

    let (section, adopted, dropped) = build_section_with_stats(&filtered);
    stats.items_adopted = adopted;
    stats.items_dropped_total_chars = dropped;
    (section, stats)
}

// ---------------------------------------------------------------------------
// pub(crate) helpers — tested via module unit tests (DR3-001)
// ---------------------------------------------------------------------------

/// Extract the `items` array from the response value.
/// Scans at most `MAX_PROMPT_SCAN_ITEMS` items and projects each to
/// the expected fields only (kind / summary / source).
/// Accepts v0.2 sidecar layout (`context_pack.items`) first,
/// then falls back to legacy top-level `items` for test fixtures.
///
/// Production code calls `parse_items_with_total` directly (CB-001).
/// This thin wrapper is retained for the in-module unit tests (U1, U6)
/// that document the projection contract independently of the total count.
#[cfg(test)]
pub(crate) fn parse_items(value: &serde_json::Value) -> Vec<serde_json::Value> {
    parse_items_with_total(value).0
}

/// Same as `parse_items` but also returns the total length of the source
/// `items[]` array in the response (cap-unaware). Used by
/// `render_context_pack` to populate `RenderStats::items_scan_capped`
/// (CB-001 conservation closure).
pub(crate) fn parse_items_with_total(value: &serde_json::Value) -> (Vec<serde_json::Value>, usize) {
    let arr = match value
        .get("context_pack")
        .and_then(|cp| cp.get("items"))
        .and_then(|v| v.as_array())
        .or_else(|| value.get("items").and_then(|v| v.as_array()))
    {
        Some(a) => a,
        None => return (vec![], 0),
    };
    let total = arr.len();
    let projected = arr
        .iter()
        .take(MAX_PROMPT_SCAN_ITEMS)
        .map(project_item_fields)
        .collect();
    (projected, total)
}

/// Returns `true` when `item["kind"]` is a valid summary kind.
/// Accepts legacy `"summary"` as well as v0.2 `"action_summary"` and `"text"`.
pub(crate) fn is_summary_kind(item: &serde_json::Value) -> bool {
    matches!(
        item.get("kind").and_then(|v| v.as_str()),
        Some("summary") | Some("action_summary") | Some("text")
    )
}

/// Apply `mask_payload_inplace` to the whole JSON object (Value level masking).
pub(crate) fn mask_item(mut item: serde_json::Value) -> serde_json::Value {
    mask_payload_inplace(&mut item);
    item
}

/// Extract the summary text, applying `mask_secrets` to the result.
/// Checks `"text"` first (v0.2 field), then falls back to `"summary"` (legacy).
/// Returns `None` when neither field is present or not a string.
pub(crate) fn extract_summary_text(item: &serde_json::Value) -> Option<String> {
    let s = item
        .get("text")
        .or_else(|| item.get("summary"))
        .and_then(|v| v.as_str())?;
    Some(mask_secrets(s))
}

/// Normalise control characters so they cannot escape the section boundary.
/// CR / LF / TAB / ASCII control (0x00-0x1f) and Unicode bidi control
/// characters are replaced with a space; consecutive spaces are collapsed.
pub(crate) fn normalize_summary_text(text: &str) -> String {
    let replaced: String = text
        .chars()
        .map(|c| {
            if c.is_ascii_control() || is_bidi_control(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    // Collapse consecutive ASCII whitespace runs to a single space.
    let mut out = String::with_capacity(replaced.len());
    let mut prev_space = false;
    for c in replaced.chars() {
        if c == ' ' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

/// Returns `true` when `text` contains a known prompt injection pattern
/// (case-insensitive ASCII substring match).
pub(crate) fn contains_prompt_injection(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    PROMPT_INJECTION_PATTERNS.iter().any(|p| lower.contains(p))
}

/// Returns `true` when `text` contains a known destructive command pattern
/// (case-insensitive ASCII substring match).
pub(crate) fn contains_destructive_command(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    DESTRUCTIVE_PATTERNS.iter().any(|p| lower.contains(p))
}

/// Build the `"Photon Context:\n- …"` section from a list of validated items.
/// Returns `None` when `items` is empty.
/// Total section length is capped at `MAX_PROMPT_TOTAL_CHARS` (UTF-8 chars).
///
/// Issue #583 (DR3-002): this is a thin wrapper over `build_section_with_stats`
/// which also returns the number of items actually emitted vs. dropped by the
/// total-chars cap. The wrapper is retained for internal callers (module tests)
/// that do not need the stats.
#[cfg(test)]
pub(crate) fn build_section(items: &[String]) -> Option<String> {
    let (section, _adopted, _dropped) = build_section_with_stats(items);
    section
}

/// Build the `"Photon Context:\n- …"` section, returning bookkeeping for the
/// total-chars cap (Issue #583, DR3-002).
///
/// Returns `(section, items_adopted, items_dropped_total_chars)`.
/// - `items_adopted` is the count of items that produced a complete bullet
///   line within the cap.
/// - `items_dropped_total_chars` is the number of items present in `items`
///   that could not be emitted as a complete bullet due to the total cap.
///
/// CB-002: when a bullet does not fit within `MAX_PROMPT_TOTAL_CHARS`, the
/// function stops emission immediately rather than writing a partial line.
/// This keeps the rendered prompt content and the `RenderStats` counters in
/// sync — every bullet that appears in the section is counted as adopted, and
/// the dropped tail is counted as `items_dropped_total_chars`.
pub(crate) fn build_section_with_stats(items: &[String]) -> (Option<String>, usize, usize) {
    if items.is_empty() {
        return (None, 0, 0);
    }
    let mut out = String::from("Photon Context:\n");
    let mut char_count = out.chars().count();
    let budget = MAX_PROMPT_TOTAL_CHARS;
    let mut adopted = 0usize;

    for item in items.iter() {
        let line = format!("- {item}\n");
        let line_chars = line.chars().count();
        if char_count + line_chars > budget {
            // CB-002: do not write a partial bullet. Stopping here keeps the
            // emitted content aligned with `items_adopted`; the remainder is
            // counted as `items_dropped_total_chars` by the caller below.
            break;
        }
        out.push_str(&line);
        char_count += line_chars;
        adopted += 1;
    }

    let dropped = items.len().saturating_sub(adopted);
    (Some(out), adopted, dropped)
}

// ---------------------------------------------------------------------------
// Issue #583: warning filter helpers
// ---------------------------------------------------------------------------

/// Single source of truth for normalising a photon-provided summary ID.
///
/// Rules (DR4-002):
/// 1. `mask_secrets(raw) != raw` ⇒ drop (potential secret).
/// 2. Contains a secret-related word (case-insensitive) ⇒ drop.
/// 3. Replace ASCII control + Unicode bidi/format chars with `_`.
/// 4. After cleaning, restrict to ASCII `[A-Za-z0-9._-]`. Anything else ⇒ drop.
/// 5. Reject empty results and IDs longer than `MAX_BLOCKED_SUMMARY_ID_BYTES`.
///
/// Used by both the extraction side (HashSet insert) and the comparison side
/// (`project_item_fields`'s `id` field) to guarantee bit-identical matching.
pub(crate) fn sanitize_summary_id(raw: &str) -> Option<String> {
    let masked = mask_secrets(raw);
    if masked != raw {
        return None;
    }
    if contains_secret_word(raw) {
        return None;
    }
    let cleaned: String = masked
        .chars()
        .map(|c| {
            if is_summary_id_control_or_format(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    if cleaned.is_empty() {
        return None;
    }
    if cleaned.len() > MAX_BLOCKED_SUMMARY_ID_BYTES {
        return None;
    }
    if !cleaned.chars().all(is_safe_summary_id_char) {
        return None;
    }
    Some(cleaned)
}

/// Parse a warning message of the shape `"<summary_id>: <reason>[: <detail>]"`.
/// Returns `Some((id_raw, BLOCKED_WARNING_REASON))` when the reason matches
/// `BLOCKED_WARNING_REASON` either exactly or as the prefix before a `:`.
/// Returns `None` otherwise.
///
/// `msg` is expected to have already been byte-capped by the caller; this
/// helper does not enforce a length limit.
fn parse_warning_message(msg: &str) -> Option<(&str, &str)> {
    let (id_raw, rest) = msg.split_once(": ")?;
    let reason_matches =
        rest == BLOCKED_WARNING_REASON || rest.starts_with(&format!("{BLOCKED_WARNING_REASON}:"));
    if !reason_matches {
        return None;
    }
    Some((id_raw, BLOCKED_WARNING_REASON))
}

/// Extract the set of summary IDs that the photon sidecar has flagged with
/// `summary_quality_gate` / `premature_termination_risk` (Issue #583).
///
/// Fail-open: malformed responses, missing fields, and unexpected types yield
/// an empty set rather than blocking everything. Returns the set together with
/// stats describing which DoS caps fired.
pub(crate) fn extract_blocked_summary_ids(
    resp: &ContextPackResponse,
) -> (HashSet<String>, BlockedIdsStats) {
    let warnings = match resp.0.get("warnings").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return (HashSet::new(), BlockedIdsStats::default()),
    };
    let total = warnings.len();
    let truncated_scan = total > MAX_PHOTON_WARNINGS_SCAN;
    let mut blocked: HashSet<String> = HashSet::new();
    let mut truncated_unique = false;

    for w in warnings.iter().take(MAX_PHOTON_WARNINGS_SCAN) {
        let kind = match w.get("kind").and_then(|v| v.as_str()) {
            Some(k) => k,
            None => continue,
        };
        if kind != "summary_quality_gate" {
            continue;
        }
        let msg = match w.get("message").and_then(|v| v.as_str()) {
            Some(m) => m,
            None => continue,
        };
        // DR4-001: enforce byte cap *before* `split_once` to prevent a giant
        // message from doing unnecessary scanning.
        if msg.len() > MAX_PHOTON_WARNING_MESSAGE_BYTES {
            continue;
        }
        let (id_raw, _reason) = match parse_warning_message(msg) {
            Some(parts) => parts,
            None => continue,
        };
        let id_clean = match sanitize_summary_id(id_raw) {
            Some(s) => s,
            None => continue,
        };
        // Unique-ID cap: duplicates do not consume the cap.
        if blocked.len() >= MAX_BLOCKED_SUMMARY_IDS && !blocked.contains(&id_clean) {
            truncated_unique = true;
            break;
        }
        blocked.insert(id_clean);
    }

    (
        blocked,
        BlockedIdsStats {
            total_warnings: total,
            truncated_scan,
            truncated_unique,
        },
    )
}

fn is_safe_summary_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
}

fn is_summary_id_control_or_format(c: char) -> bool {
    c.is_ascii_control()
        || matches!(
            c,
            '\u{200b}'
                | '\u{200c}'
                | '\u{200d}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'
                | '\u{202b}'
                | '\u{202c}'
                | '\u{202d}'
                | '\u{202e}'
                | '\u{2066}'
                | '\u{2067}'
                | '\u{2068}'
                | '\u{2069}'
                | '\u{feff}'
        )
}

fn contains_secret_word(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    [
        "token",
        "secret",
        "password",
        "api_key",
        "apikey",
        "access_key",
        "accesskey",
        "client_secret",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Project an item JSON object to expected fields only
/// (kind/summary/text/source/id).
/// Handles both legacy (`summary` field) and v0.2 (`text` field) response
/// shapes. Unknown fields are discarded to limit recursive mask surface area
/// (DR4-002).
///
/// Issue #583: the `id` field is sanitized through `sanitize_summary_id` so the
/// comparison side uses the same normalisation as the HashSet insert side.
/// Non-string IDs and IDs rejected by the sanitizer are dropped entirely
/// (fail-open: a missing id field means the warning filter cannot block the
/// item).
fn project_item_fields(item: &serde_json::Value) -> serde_json::Value {
    let mut projected = serde_json::Map::new();
    for key in &["kind", "summary", "text", "source", "id"] {
        if let Some(v) = item.get(key) {
            if *key == "id" {
                match v.as_str() {
                    Some(s) => match sanitize_summary_id(s) {
                        Some(clean) => {
                            projected.insert(key.to_string(), serde_json::Value::String(clean));
                        }
                        None => continue,
                    },
                    None => continue,
                }
            } else {
                projected.insert(key.to_string(), v.clone());
            }
        }
    }
    serde_json::Value::Object(projected)
}

/// Truncate `text` to at most `max_chars` Unicode scalar values.
fn truncate_chars(text: String, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text;
    }
    text.chars().take(max_chars).collect()
}

/// Returns `true` for Unicode bidi control characters that can be used to
/// reorder rendered text and hide injected content.
fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{200b}'  // Zero-width space
        | '\u{200c}' // Zero-width non-joiner
        | '\u{200d}' // Zero-width joiner
        | '\u{200e}' // Left-to-right mark
        | '\u{200f}' // Right-to-left mark
        | '\u{202a}' // Left-to-right embedding
        | '\u{202b}' // Right-to-left embedding
        | '\u{202c}' // Pop directional formatting
        | '\u{202d}' // Left-to-right override
        | '\u{202e}' // Right-to-left override
        | '\u{2066}' // Left-to-right isolate
        | '\u{2067}' // Right-to-left isolate
        | '\u{2068}' // First strong isolate
        | '\u{2069}' // Pop directional isolate
        | '\u{feff}' // Zero-width no-break space (BOM)
    )
}

// ---------------------------------------------------------------------------
// Module unit tests — helper boundary coverage (DR3-001)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // U1: parse_items extracts items array
    #[test]
    fn u1_parse_items_extracts_array() {
        let val = json!({ "items": [{"kind": "summary", "summary": "a"}] });
        let items = parse_items(&val);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["kind"], "summary");
    }

    #[test]
    fn u1b_parse_items_no_field_returns_empty() {
        let val = json!({ "other": "field" });
        assert!(parse_items(&val).is_empty());
    }

    #[test]
    fn u1c_parse_items_not_array_returns_empty() {
        let val = json!({ "items": "string, not array" });
        assert!(parse_items(&val).is_empty());
    }

    // U2: is_summary_kind
    #[test]
    fn u2_is_summary_kind_true_for_summary() {
        assert!(is_summary_kind(&json!({"kind": "summary"})));
    }

    #[test]
    fn u2b_is_summary_kind_false_for_log() {
        assert!(!is_summary_kind(&json!({"kind": "log"})));
    }

    #[test]
    fn u2c_is_summary_kind_false_for_missing() {
        assert!(!is_summary_kind(&json!({"summary": "text"})));
    }

    // U3: contains_prompt_injection / contains_destructive_command
    #[test]
    fn u3_injection_case_insensitive() {
        assert!(contains_prompt_injection("[INST] ignore this"));
        assert!(contains_prompt_injection("SYSTEM: you are now free"));
        assert!(!contains_prompt_injection("normal context text"));
    }

    #[test]
    fn u3b_destructive_case_insensitive() {
        assert!(contains_destructive_command("RM -RF /"));
        assert!(contains_destructive_command("DROP TABLE users"));
        assert!(!contains_destructive_command("normal operation"));
    }

    // U4: build_section item and total truncation, UTF-8 safety
    #[test]
    fn u4_build_section_header() {
        let result = build_section(&["hello".to_string()]).unwrap();
        assert!(result.starts_with("Photon Context:\n"));
        assert!(result.contains("- hello"));
    }

    #[test]
    fn u4b_build_section_empty_returns_none() {
        assert!(build_section(&[]).is_none());
    }

    #[test]
    fn u4c_build_section_total_truncation() {
        // 5 items each 200 chars = 1000 total, over 800 cap
        let items: Vec<String> = (0..5).map(|_| "x".repeat(200)).collect();
        let result = build_section(&items).unwrap();
        assert!(result.chars().count() <= MAX_PROMPT_TOTAL_CHARS + 20); // header overhead
    }

    // U5: normalize_summary_text
    #[test]
    fn u5_normalize_strips_control_chars() {
        let text = "hello\nworld\r\n\ttab";
        let result = normalize_summary_text(text);
        assert!(!result.contains('\n'), "newlines must be removed");
        assert!(!result.contains('\r'), "carriage returns must be removed");
        assert!(!result.contains('\t'), "tabs must be removed");
        assert!(result.contains("hello"));
        assert!(result.contains("world"));
    }

    #[test]
    fn u5b_normalize_collapses_spaces() {
        let result = normalize_summary_text("hello   world");
        assert_eq!(result, "hello world");
    }

    #[test]
    fn u5c_normalize_bidi_control() {
        let text = "before\u{202e}after".to_string();
        let result = normalize_summary_text(&text);
        assert!(!result.contains('\u{202e}'), "bidi control must be removed");
    }

    // U6: parse_items scan limit and unknown field projection
    #[test]
    fn u6_parse_items_scan_limit() {
        let items: Vec<serde_json::Value> = (0..100)
            .map(|i| json!({"kind": "summary", "summary": format!("item-{i}")}))
            .collect();
        let val = json!({ "items": items });
        let result = parse_items(&val);
        assert!(
            result.len() <= MAX_PROMPT_SCAN_ITEMS,
            "must not scan more than MAX_PROMPT_SCAN_ITEMS"
        );
    }

    #[test]
    fn u6b_parse_items_projects_expected_fields_only() {
        let val = json!({
            "items": [{
                "kind": "summary",
                "summary": "text",
                "source": "src",
                "unknown_field": "should be dropped",
                "deep_object": {"nested": "value"},
            }]
        });
        let items = parse_items(&val);
        assert_eq!(items.len(), 1);
        assert!(items[0].get("kind").is_some());
        assert!(items[0].get("summary").is_some());
        assert!(items[0].get("source").is_some());
        assert!(
            items[0].get("unknown_field").is_none(),
            "unknown_field must be projected away"
        );
        assert!(
            items[0].get("deep_object").is_none(),
            "deep_object must be projected away"
        );
    }

    // -----------------------------------------------------------------------
    // U7: Issue #583 warning filter helpers
    // -----------------------------------------------------------------------

    fn mk_resp(value: serde_json::Value) -> ContextPackResponse {
        ContextPackResponse(value)
    }

    // sanitize_summary_id — boundary cases
    #[test]
    fn u7_sanitize_id_accepts_ascii_allowlist() {
        assert_eq!(
            sanitize_summary_id("seed.abc-123_xyz"),
            Some("seed.abc-123_xyz".to_string())
        );
    }

    #[test]
    fn u7_sanitize_id_rejects_colon_and_whitespace() {
        // colon and space are not in the ASCII allowlist
        assert_eq!(sanitize_summary_id("seed:bad"), None);
        assert_eq!(sanitize_summary_id("seed bad"), None);
        // tab is an ASCII control char → flattened to `_`, then re-validates;
        // the result is still ASCII-allowlisted, so it's accepted with the
        // control char replaced. (Hidden control characters cannot survive
        // verbatim; this is the documented behaviour.)
        let flattened = sanitize_summary_id("seed\tbad").unwrap();
        assert_eq!(flattened, "seed_bad");
        assert!(!flattened.contains('\t'));
    }

    #[test]
    fn u7_sanitize_id_rejects_unicode_bidi_zero_width() {
        // Bidi / zero-width chars get flattened to `_`; result keeps `_` so
        // still ASCII-allowlisted, but the underscore replaces the hidden char.
        let cleaned = sanitize_summary_id("seed\u{202e}rev").unwrap();
        assert!(!cleaned.contains('\u{202e}'));
        assert!(cleaned.contains('_'));
    }

    #[test]
    fn u7_sanitize_id_rejects_secret_like_words() {
        assert_eq!(sanitize_summary_id("api_key_seed"), None);
        assert_eq!(sanitize_summary_id("SEED_TOKEN"), None);
        assert_eq!(sanitize_summary_id("ClientSecret"), None);
    }

    #[test]
    fn u7_sanitize_id_rejects_over_byte_cap() {
        let raw = "a".repeat(MAX_BLOCKED_SUMMARY_ID_BYTES + 1);
        assert_eq!(sanitize_summary_id(&raw), None);
    }

    #[test]
    fn u7_sanitize_id_rejects_empty() {
        assert_eq!(sanitize_summary_id(""), None);
    }

    // parse_warning_message — boundary cases
    #[test]
    fn u7_parse_warning_message_exact_reason() {
        let (id, reason) = parse_warning_message("seed_x: premature_termination_risk").unwrap();
        assert_eq!(id, "seed_x");
        assert_eq!(reason, BLOCKED_WARNING_REASON);
    }

    #[test]
    fn u7_parse_warning_message_reason_with_detail() {
        let (id, reason) =
            parse_warning_message("seed_y: premature_termination_risk: ended early").unwrap();
        assert_eq!(id, "seed_y");
        assert_eq!(reason, BLOCKED_WARNING_REASON);
    }

    #[test]
    fn u7_parse_warning_message_rejects_unrelated_reason() {
        assert!(parse_warning_message("seed_z: other_reason").is_none());
    }

    #[test]
    fn u7_parse_warning_message_rejects_no_colon() {
        assert!(parse_warning_message("seed without separator").is_none());
    }

    // WF-01: normal block path through extract + render
    #[test]
    fn wf01_blocked_id_skips_matching_item() {
        let resp = mk_resp(json!({
            "items": [
                { "kind": "summary", "id": "seed_a", "summary": "summary a" },
                { "kind": "summary", "id": "seed_b", "summary": "summary b" },
            ],
            "warnings": [
                { "kind": "summary_quality_gate", "message": "seed_a: premature_termination_risk" }
            ]
        }));
        let (blocked, stats) = extract_blocked_summary_ids(&resp);
        assert_eq!(stats.total_warnings, 1);
        assert!(blocked.contains("seed_a"));
        assert!(!blocked.contains("seed_b"));
        let (section, render_stats) = render_context_pack(&resp, &blocked);
        let section = section.expect("expected at least one surviving item");
        assert!(section.contains("summary b"));
        assert!(!section.contains("summary a"));
        assert_eq!(render_stats.items_blocked, 1);
        assert_eq!(render_stats.items_adopted, 1);
    }

    // WF-02: kind != "summary_quality_gate" is ignored
    #[test]
    fn wf02_other_warning_kind_does_not_block() {
        let resp = mk_resp(json!({
            "items": [{ "kind": "summary", "id": "seed_a", "summary": "summary a" }],
            "warnings": [
                { "kind": "other_kind", "message": "seed_a: premature_termination_risk" }
            ]
        }));
        let (blocked, _stats) = extract_blocked_summary_ids(&resp);
        assert!(blocked.is_empty());
        let (section, render_stats) = render_context_pack(&resp, &blocked);
        let section = section.expect("item must survive");
        assert!(section.contains("summary a"));
        assert_eq!(render_stats.items_blocked, 0);
    }

    // WF-03: unrelated reason is ignored
    #[test]
    fn wf03_unrelated_reason_does_not_block() {
        let resp = mk_resp(json!({
            "items": [{ "kind": "summary", "id": "seed_a", "summary": "summary a" }],
            "warnings": [
                { "kind": "summary_quality_gate", "message": "seed_a: other_reason" }
            ]
        }));
        let (blocked, _stats) = extract_blocked_summary_ids(&resp);
        assert!(blocked.is_empty());
    }

    // WF-04: malformed message (no colon) is dropped
    #[test]
    fn wf04_malformed_message_dropped() {
        let resp = mk_resp(json!({
            "items": [{ "kind": "summary", "id": "seed_a", "summary": "summary a" }],
            "warnings": [
                { "kind": "summary_quality_gate", "message": "no colon here" }
            ]
        }));
        let (blocked, _stats) = extract_blocked_summary_ids(&resp);
        assert!(blocked.is_empty());
    }

    // WF-05: empty warnings array
    #[test]
    fn wf05_empty_warnings_array() {
        let resp = mk_resp(json!({ "items": [], "warnings": [] }));
        let (blocked, stats) = extract_blocked_summary_ids(&resp);
        assert!(blocked.is_empty());
        assert_eq!(stats.total_warnings, 0);
        assert!(!stats.truncated_scan);
        assert!(!stats.truncated_unique);
    }

    // WF-06: warnings scan cap
    #[test]
    fn wf06_warnings_scan_cap() {
        let warnings: Vec<serde_json::Value> = (0..(MAX_PHOTON_WARNINGS_SCAN + 10))
            .map(|i| {
                json!({
                    "kind": "summary_quality_gate",
                    "message": format!("seed_{i}: premature_termination_risk")
                })
            })
            .collect();
        let resp = mk_resp(json!({ "items": [], "warnings": warnings }));
        let (_blocked, stats) = extract_blocked_summary_ids(&resp);
        assert!(stats.truncated_scan);
        assert!(stats.total_warnings > MAX_PHOTON_WARNINGS_SCAN);
    }

    // WF-07: mixed non-object elements are ignored
    #[test]
    fn wf07_mixed_non_object_elements() {
        let resp = mk_resp(json!({
            "items": [],
            "warnings": [42, null, "str", { "kind": "other" }]
        }));
        let (blocked, stats) = extract_blocked_summary_ids(&resp);
        assert!(blocked.is_empty());
        assert_eq!(stats.total_warnings, 4);
    }

    // WF-08: item without id field passes through (fail-open)
    #[test]
    fn wf08_item_without_id_passes() {
        let resp = mk_resp(json!({
            "items": [{ "kind": "summary", "summary": "summary without id" }],
            "warnings": [
                { "kind": "summary_quality_gate", "message": "seed_a: premature_termination_risk" }
            ]
        }));
        let (blocked, _stats) = extract_blocked_summary_ids(&resp);
        assert!(blocked.contains("seed_a"));
        let (section, render_stats) = render_context_pack(&resp, &blocked);
        let section = section.expect("item lacking id must survive (fail-open)");
        assert!(section.contains("summary without id"));
        assert_eq!(render_stats.items_blocked, 0);
        assert_eq!(render_stats.items_adopted, 1);
    }

    // SEC-01: warning message larger than byte cap is rejected before parsing
    #[test]
    fn sec01_oversize_warning_message_dropped() {
        let big = format!(
            "seed_a: premature_termination_risk{}",
            "x".repeat(MAX_PHOTON_WARNING_MESSAGE_BYTES)
        );
        let resp = mk_resp(json!({
            "items": [{ "kind": "summary", "id": "seed_a", "summary": "summary a" }],
            "warnings": [{ "kind": "summary_quality_gate", "message": big }]
        }));
        let (blocked, _stats) = extract_blocked_summary_ids(&resp);
        assert!(blocked.is_empty());
    }

    // SEC-02: control / non-allowlist characters in id are rejected by sanitizer
    #[test]
    fn sec02_id_with_disallowed_chars_dropped() {
        // colon, slash and quote should all fail the allowlist after the bidi
        // / control flattening step.
        let resp = mk_resp(json!({
            "items": [],
            "warnings": [
                { "kind": "summary_quality_gate", "message": "seed/x: premature_termination_risk" },
                { "kind": "summary_quality_gate", "message": "seed\"y: premature_termination_risk" }
            ]
        }));
        let (blocked, _stats) = extract_blocked_summary_ids(&resp);
        assert!(blocked.is_empty());
    }

    // SEC-03: secret-like id is dropped
    #[test]
    fn sec03_secret_like_id_dropped() {
        let resp = mk_resp(json!({
            "items": [],
            "warnings": [
                { "kind": "summary_quality_gate", "message": "seed_TOKEN: premature_termination_risk" }
            ]
        }));
        let (blocked, _stats) = extract_blocked_summary_ids(&resp);
        assert!(blocked.is_empty());
    }

    // Unique-ID cap: duplicates do not consume cap; surplus unique triggers flag
    #[test]
    fn wf_unique_id_cap_triggers_truncation() {
        let warnings: Vec<serde_json::Value> = (0..(MAX_BLOCKED_SUMMARY_IDS + 5))
            .map(|i| {
                json!({
                    "kind": "summary_quality_gate",
                    "message": format!("seed_{i}: premature_termination_risk")
                })
            })
            .collect();
        let resp = mk_resp(json!({ "items": [], "warnings": warnings }));
        let (blocked, stats) = extract_blocked_summary_ids(&resp);
        assert_eq!(blocked.len(), MAX_BLOCKED_SUMMARY_IDS);
        assert!(stats.truncated_unique);
    }

    // RenderStats invariant: scanned counts plus the scan-capped tail equal
    // the total number of items in the response (CB-001 conservation closure).
    #[test]
    fn wf_render_stats_invariant_counts_sum() {
        let resp = mk_resp(json!({
            "items": [
                { "kind": "summary", "id": "seed_a", "summary": "a" },
                { "kind": "summary", "id": "seed_b", "summary": "b" },
                { "kind": "summary", "id": "seed_c", "summary": "c" },
                { "kind": "log",     "id": "seed_d", "summary": "d" },
            ],
            "warnings": [
                { "kind": "summary_quality_gate", "message": "seed_a: premature_termination_risk" }
            ]
        }));
        let (blocked, _stats) = extract_blocked_summary_ids(&resp);
        let (_section, render_stats) = render_context_pack(&resp, &blocked);
        let scanned = render_stats.items_adopted
            + render_stats.items_blocked
            + render_stats.items_filtered_kind
            + render_stats.items_filtered_missing_text
            + render_stats.items_filtered_security
            + render_stats.items_over_cap
            + render_stats.items_dropped_total_chars;
        let total_in = scanned + render_stats.items_scan_capped;
        assert_eq!(total_in, 4);
        assert_eq!(render_stats.items_scan_capped, 0);
        assert_eq!(render_stats.items_blocked, 1);
        assert_eq!(render_stats.items_filtered_kind, 1);
        assert_eq!(render_stats.items_adopted, 2);
    }

    // CB-001: scan cap fires → 51st item onward is accounted in items_scan_capped,
    // and the conservation closure still holds.
    #[test]
    fn wf_render_stats_scan_cap_conservation() {
        // 50 + 10 = 60 items, all valid summaries; only first 50 are scanned.
        let items: Vec<serde_json::Value> = (0..(MAX_PROMPT_SCAN_ITEMS + 10))
            .map(|i| {
                json!({
                    "kind": "summary",
                    "id": format!("seed_{i}"),
                    "summary": format!("summary-{i}"),
                })
            })
            .collect();
        let resp = mk_resp(json!({ "items": items, "warnings": [] }));
        let (blocked, _stats) = extract_blocked_summary_ids(&resp);
        let (_section, render_stats) = render_context_pack(&resp, &blocked);

        let scanned = render_stats.items_adopted
            + render_stats.items_blocked
            + render_stats.items_filtered_kind
            + render_stats.items_filtered_missing_text
            + render_stats.items_filtered_security
            + render_stats.items_over_cap
            + render_stats.items_dropped_total_chars;
        assert_eq!(
            scanned, MAX_PROMPT_SCAN_ITEMS,
            "scanned must equal MAX_PROMPT_SCAN_ITEMS when scan cap fires"
        );
        assert_eq!(
            render_stats.items_scan_capped, 10,
            "items beyond the scan cap must be accounted in items_scan_capped"
        );
        assert_eq!(scanned + render_stats.items_scan_capped, 60);
    }

    // build_section_with_stats: total cap dropped count is exposed
    #[test]
    fn wf_build_section_with_stats_total_cap() {
        let items: Vec<String> = (0..5).map(|_| "x".repeat(200)).collect();
        let (section, adopted, dropped) = build_section_with_stats(&items);
        assert!(section.is_some());
        assert!(adopted + dropped == items.len());
        assert!(dropped > 0, "5 × 200 char items should exceed total cap");
    }

    // CB-002: when the total-chars cap is hit, the section must not contain a
    // partial bullet for the dropped item. Every bullet in the section is
    // complete (terminated by '\n') and matches items_adopted exactly.
    #[test]
    fn wf_build_section_no_partial_bullet_on_total_cap() {
        let items: Vec<String> = (0..5).map(|_| "x".repeat(200)).collect();
        let (section_opt, adopted, dropped) = build_section_with_stats(&items);
        let section = section_opt.expect("section must exist");
        assert!(dropped > 0, "5 × 200 char items should exceed total cap");
        // Section shape: "Photon Context:\n" header + N complete bullets, each
        // formatted as "- <item>\n". Count newlines minus the header newline.
        let bullet_newline_count = section.matches('\n').count().saturating_sub(1);
        assert_eq!(
            bullet_newline_count, adopted,
            "section's bullet-terminating '\\n' count must equal items_adopted"
        );
        // Hard CB-002 guard: section ends with '\n' from the last complete
        // bullet, never a mid-bullet truncation.
        assert!(
            section.ends_with('\n'),
            "section must end with a newline, not a truncated bullet"
        );
        // Belt-and-braces: total chars must not exceed the cap (no overflow
        // from a stray partial line).
        assert!(section.chars().count() <= MAX_PROMPT_TOTAL_CHARS);
        assert_eq!(adopted + dropped, items.len());
    }
}
