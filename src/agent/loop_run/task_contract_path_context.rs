//! Shared request path/context scanning primitives for task contract inference.
//!
//! This module owns the generic string/path mechanics used by multiple
//! objective surfaces. Domain-specific aggregation, polarity, and obligation
//! decisions stay in their own modules.

/// Issue #937 (DS1-006): the single tokenizer SSOT. Splits on any character that
/// is NOT a path-construction char (`[alnum _ - . / \]`) and yields each token's
/// byte offset in `s`. Both `mask_path_tokens` and the path extractors share
/// this so the split boundary can never drift between mask and extraction.
fn split_path_tokens(s: &str) -> impl Iterator<Item = (usize, &str)> {
    s.split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | '\\')))
        .scan(0usize, move |cursor, token| {
            // Reconstruct the byte offset: tokens come back in order, and the
            // delimiters between them are single non-path chars. `find` from the
            // cursor recovers the precise start (tokens may repeat).
            let start = if token.is_empty() {
                *cursor
            } else {
                // SAFETY of indices: token is a sub-slice produced by split, so a
                // forward `find` from the cursor lands on this exact occurrence.
                let rel = s[*cursor..].find(token).map(|r| *cursor + r);
                let start = rel.unwrap_or(*cursor);
                *cursor = start + token.len();
                start
            };
            Some((start, token))
        })
}

/// Issue #937 (DS1-005 案A): is `token` a recognized artifact path? Mask + extraction
/// share this exact predicate so the "what is a path" set cannot drift. A token
/// counts when it normalizes to an explicit artifact path (recognized-extension
/// allowlist, identical to `normalize_explicit_artifact_path`) OR contains a
/// path separator. The trailing-`.` run is trimmed before the extension test so a
/// sentence-final `output_data.csv.` still recognizes (M6).
pub(super) fn path_token_is_maskable(token: &str) -> bool {
    let trimmed = token.trim_end_matches('.');
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return true;
    }
    normalize_explicit_artifact_path(trimmed).is_some()
}

/// Issue #937 (DS1-005 / 判断#1): blank every recognized path token to an
/// EQUAL-LENGTH run of spaces, preserving every byte index so callers can locate
/// occurrences via the original `lower` and read context windows on the masked
/// copy. Non-path tokens (real verbs/nouns, `v1.2.3`, `3.14`, `e.g`, JP) are kept
/// verbatim — only authentic path tokens are erased (no over-masking). The
/// masked string is judgement-only and never persisted.
pub(super) fn mask_path_tokens(lower: &str) -> String {
    // Collect the byte spans of maskable path tokens; every such span is
    // ASCII-only (alnum/_-./\\), so blanking each byte to a space is index- and
    // UTF-8-stable. No `unsafe`: rebuild the string byte-wise, substituting
    // spaces inside a span and copying every other byte verbatim.
    let spans: Vec<(usize, usize)> = split_path_tokens(lower)
        .filter(|(_, token)| path_token_is_maskable(token))
        .map(|(start, token)| (start, start + token.len()))
        .collect();
    if spans.is_empty() {
        return lower.to_string();
    }
    let bytes = lower.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut span_iter = spans.iter().peekable();
    for (idx, &b) in bytes.iter().enumerate() {
        while span_iter.peek().is_some_and(|(_, end)| idx >= *end) {
            span_iter.next();
        }
        let in_span = span_iter
            .peek()
            .is_some_and(|(start, end)| idx >= *start && idx < *end);
        out.push(if in_span { b' ' } else { b });
    }
    // SAFETY-free: spans cover ASCII path chars only, so the length and all
    // char boundaries are preserved; the result is valid UTF-8.
    String::from_utf8(out).unwrap_or_else(|_| lower.to_string())
}

// --- shared cue vocabulary (SSOT) -----------------------------------------
// ASCII verb STEMS only; the matcher (`contains_output_verb`) absorbs an
// optional trailing plural `s`. JP markers are substring (no word boundary).
pub(super) const OUTPUT_VERB_STEMS_ASCII: &[&str] = &[
    "write", "produce", "generate", "create", "export", "save", "output", "compile", "draft",
    "prepare", "emit",
];
pub(super) const OUTPUT_PREP_ASCII: &[&str] = &["to", "into"];
pub(super) const OUTPUT_AFTER_ASCII: &[&str] = &[" output", " deliverable", " artifact"];
pub(super) const INPUT_VERBS_ASCII: &[&str] =
    &["input", "source", "from", "read", "reads", "load", "loads"];
/// Shared JP output markers (substring). **bare `書` is intentionally excluded**
/// (DS1-003): it lives only in research's `RESEARCH_OUTPUT_AFTER_JP` so `文書`
/// (document) never fabricates a data output cue.
pub(super) const JP_OUTPUT_MARKERS: &[&str] = &["生成", "出力", "作成", "書き出", "まとめ"];
/// `mentions_output_shape` noun part only (DS1-002/004); verbs are NOT replaced.
pub(super) const DATA_SHAPE_NOUNS: &[&str] = &["schema", "column", "columns", "出力", "列"];
/// research-only **directional after-window** JP output markers (DS1-001). Bare
/// `書` is isolated here (DS1-003) and substring-covers `書き出`/`書いて`. Issue
/// #937 CB-001: this is the directional after-window set — it is **no longer
/// byte-identical** to the pre-#937 `:2654` whole-request set `[作成,出力,書き出,
/// まとめ,書いて]`, by design. The whole-request JP scan (which leaked a later
/// `出力`/`作成` backward onto an earlier neutral input path) was removed from
/// `report_path_in_output_context_with_scan`; its `作成` capability was folded
/// into this after-window so the full original JP output vocabulary is preserved
/// directionally (the rest — `出力`/`まとめ`/`書き出`/`書いて` — was already here).
pub(super) const RESEARCH_OUTPUT_AFTER_JP: &[&str] = &["まとめ", "出力", "書", "作成"];
/// data-only extra input cues, appended to `INPUT_VERBS_ASCII`.
pub(super) const DATA_INPUT_EXTRA: &[&str] = &["sample", "example", "fixture", "ingest"];
/// docs-only output after-window JP markers (判断#5, polarity-preserving).
pub(super) const DOCS_OUTPUT_AFTER_JP: &[&str] = &["に書いて", "に出力", "として保存"];
/// Issue #937 (Codex High): shared input-reference (reading/comparison) verbs.
/// A docs/report path governed by one of these in its (masked) before-window is
/// being CONSUMED — read or compared — not produced, so it is obligation-free
/// for the Research AND Authoring entry points (`Compare findings in
/// draft_report.md`, `Review draft_report.md`). `read`/`reads` already live in
/// `INPUT_VERBS_ASCII`; this set adds the reading/comparison verbs that the
/// authoring gate previously ignored. Word-boundary matched over masked text.
pub(super) const INPUT_REFERENCE_VERBS_ASCII: &[&str] = &[
    "compare",
    "compares",
    "compared",
    "comparing",
    "review",
    "reviews",
    "reviewed",
    "reviewing",
    "summarize",
    "summarise",
    "summarizes",
    "summarises",
    "analyze",
    "analyse",
    "analyzes",
    "analyses",
];
/// Issue #919 / #937 (Codex High): ASCII authoring-verb needles (SSOT). Used by
/// `request_matches_authoring_keyword` (substring over masked text) and by the
/// directional nearest-cue scan (`docs_path_is_input_reference_with_scan`, token
/// `starts_with`) so an authoring verb like `rewrite`/`proofread` that governs a
/// neutral in-place docs target overrides an earlier `review`/`compare` cue.
pub(super) const AUTHORING_KEYWORD_NEEDLES_ASCII: &[&str] = &[
    "translate",
    "translation",
    "rewrite",
    "reword",
    "paraphrase",
    "proofread",
    "copyedit",
    "draft",
];

/// Issue #937 (DS1-002): match an ASCII output verb STEM at a word boundary,
/// absorbing an optional trailing plural `s` (so `generates`/`writes` match the
/// `generate`/`write` stem). Runs over filename-stripped text supplied by caller.
pub(super) fn contains_output_verb(text: &str, stems: &[&str]) -> bool {
    stems.iter().any(|stem| {
        contains_ascii_token(text, stem) || {
            let mut plural = String::with_capacity(stem.len() + 1);
            plural.push_str(stem);
            plural.push('s');
            contains_ascii_token(text, &plural)
        }
    })
}

/// Issue #937 (DS3-001): the per-`from_request` output-context scan. Built once;
/// threaded by `&str` into every surface so the (expensive) mask allocation
/// happens exactly once per top-level request. Stack-only, never stored.
pub(super) struct OutputContextScan {
    pub(super) lower: String,
    pub(super) lower_masked: String,
}

impl OutputContextScan {
    pub(super) fn new(request: &str) -> Self {
        let lower = request.to_ascii_lowercase();
        let lower_masked = mask_path_tokens(&lower);
        Self {
            lower,
            lower_masked,
        }
    }
}

pub(super) fn normalize_explicit_user_artifact_path(token: &str) -> Option<String> {
    let path = normalize_explicit_artifact_path(token)?;
    crate::util::workspace_paths::WorkspacePolicy::default()
        .admits_artifact_display_path(&path)
        .then_some(path)
}

/// Issue #918 (P1): hard cap (bytes) on a stored obligation path.
pub(super) const MAX_OBLIGATION_PATH_BYTES: usize = 4096;

/// Issue #918 (P1): SSOT path validator that every artifact obligation
/// constructor routes its `path` through, so a raw unvalidated traversal /
/// oversized path can never be stored.
///
/// The validation *predicate* is [`normalize_explicit_user_artifact_path`]
/// (normalize + `WorkspacePolicy` admit). It differs from parse-time admission
/// only in failure action: parse-time rejects; construction is infallible and
/// falls back to a sanitized, non-traversing display string.
pub(super) fn validated_obligation_path(raw: String) -> String {
    match normalize_explicit_user_artifact_path(&raw) {
        Some(normalized) => truncate_obligation_path(normalized),
        None => sanitize_rejected_obligation_path(&raw),
    }
}

/// Fail-closed sanitizer for a path that did not pass the validation predicate.
/// Strips traversal (`..`/`.`/leading-`/`), neutralizes control characters,
/// masks secrets, and caps length.
fn sanitize_rejected_obligation_path(raw: &str) -> String {
    let normalized_sep = raw.replace('\\', "/");
    let mut out = String::new();
    for segment in normalized_sep.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            continue;
        }
        if !out.is_empty() {
            out.push('/');
        }
        for ch in segment.chars() {
            out.push(if ch.is_control() { '_' } else { ch });
        }
    }
    let masked = crate::session::feedback::mask_secrets(&out);
    truncate_obligation_path(masked)
}

/// Char-boundary-safe truncation to [`MAX_OBLIGATION_PATH_BYTES`].
fn truncate_obligation_path(mut path: String) -> String {
    if path.len() <= MAX_OBLIGATION_PATH_BYTES {
        return path;
    }
    let mut end = MAX_OBLIGATION_PATH_BYTES;
    while end > 0 && !path.is_char_boundary(end) {
        end -= 1;
    }
    path.truncate(end);
    path
}

pub(super) fn normalize_explicit_artifact_path(token: &str) -> Option<String> {
    let trimmed = token.trim_matches(|ch: char| {
        ch.is_ascii_whitespace()
            || matches!(
                ch,
                '`' | '\''
                    | '"'
                    | ','
                    | '.'
                    | ':'
                    | ';'
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '<'
                    | '>'
                    | '、'
                    | '。'
                    | '，'
                    | '．'
            )
    });
    if !trimmed.contains('.') {
        return None;
    }
    let path = trimmed.replace('\\', "/");
    if matches!(
        path.to_ascii_lowercase().as_str(),
        "node.js" | "next.js" | "vue.js"
    ) {
        return None;
    }
    if path.is_empty()
        || path.starts_with('/')
        || path.starts_with("./.")
        || path.contains("://")
        || path.bytes().any(|b| b.is_ascii_control())
    {
        return None;
    }
    let segments = path.split('/').collect::<Vec<_>>();
    if segments.iter().any(|segment| {
        segment.is_empty()
            || *segment == "."
            || *segment == ".."
            || !segment
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    }) {
        return None;
    }
    let ext = std::path::Path::new(&path)
        .extension()
        .and_then(|ext| ext.to_str())?
        .to_ascii_lowercase();
    let recognized = matches!(
        ext.as_str(),
        "py" | "rs"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "csv"
            | "tsv"
            | "jsonl"
            | "md"
            | "mdx"
            | "txt"
            | "rst"
            | "toml"
            | "json"
            | "yaml"
            | "yml"
            | "lock"
            | "ndjson"
            | "parquet"
    );
    recognized.then_some(path)
}

pub(super) fn bounded_context_before(text: &str, end: usize, max_bytes: usize) -> &str {
    let mut start = end.saturating_sub(max_bytes);
    while start < end && !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..end]
}

pub(super) fn bounded_context_after(text: &str, start: usize, max_bytes: usize) -> &str {
    let mut end = (start + max_bytes).min(text.len());
    while end > start && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[start..end]
}

pub(super) fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

pub(super) fn contains_ascii_token(haystack: &str, needle: &str) -> bool {
    haystack.match_indices(needle).any(|(idx, _)| {
        let before = haystack[..idx]
            .chars()
            .next_back()
            .is_none_or(|ch| !is_ascii_word_char(ch));
        let after_idx = idx + needle.len();
        let after = haystack[after_idx..]
            .chars()
            .next()
            .is_none_or(|ch| !is_ascii_word_char(ch));
        before && after
    })
}

pub(super) fn is_ascii_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_path_tokens_preserves_length_m1() {
        for s in [
            "compare findings in draft_report.md and summary.md",
            "idとtotalの列を持つoutput.csvを生成してください",
            "what columns are in output_data.csv, a csv file?",
            "generate data/results.jsonl with columns id from input.jsonl",
            "v1.2.3 and 3.14 and e.g and i.e are not paths",
            "",
            "no paths here at all",
        ] {
            assert_eq!(
                mask_path_tokens(s).len(),
                s.len(),
                "mask must be byte-length preserving: {s:?}"
            );
        }
    }

    #[test]
    fn mask_path_tokens_keeps_japanese_verbatim_m2() {
        let masked = mask_path_tokens("idとtotalの列を持つoutput.csvを生成してください");
        assert!(masked.contains("生成"), "JP verb must survive: {masked:?}");
        assert!(masked.contains('列'), "JP noun must survive: {masked:?}");
        assert!(
            !masked.contains("output.csv"),
            "path must be blanked: {masked:?}"
        );
    }

    #[test]
    fn mask_path_tokens_blanks_all_paths_m3() {
        let masked = mask_path_tokens("a report.md and b data.csv");
        assert!(
            !masked.contains("report.md"),
            "first path blanked: {masked:?}"
        );
        assert!(
            !masked.contains("data.csv"),
            "second path blanked: {masked:?}"
        );
        assert!(masked.contains(" and "), "connective stays: {masked:?}");
    }

    #[test]
    fn mask_path_tokens_filename_internal_vs_standalone_m4() {
        let masked = mask_path_tokens("draft a report into draft_report.md now");
        assert!(
            contains_ascii_token(&masked, "draft"),
            "standalone draft must survive: {masked:?}"
        );
        assert!(
            !masked.contains("draft_report.md"),
            "filename blanked: {masked:?}"
        );
    }

    #[test]
    fn mask_path_tokens_recognized_extension_only_m5() {
        let masked = mask_path_tokens("v1.2.3 release, see e.g output_data.csv and readme.ja.md");
        assert!(masked.contains("v1.2.3"), "version verbatim: {masked:?}");
        assert!(masked.contains("e.g"), "abbreviation verbatim: {masked:?}");
        assert!(
            !masked.contains("output_data.csv"),
            "csv path blanked: {masked:?}"
        );
        assert!(
            !masked.contains("readme.ja.md"),
            "md path blanked: {masked:?}"
        );
        let with_sep = mask_path_tokens("write data/results.jsonl now");
        assert!(
            !with_sep.contains("data/results.jsonl"),
            "separator path blanked: {with_sep:?}"
        );
    }

    #[test]
    fn mask_path_tokens_trailing_dot_edge_m6() {
        let masked = mask_path_tokens("summarize the trends in output_data.csv.");
        assert!(
            !masked.contains("output_data.csv"),
            "trailing-dot path blanked: {masked:?}"
        );
        assert_eq!(
            masked.len(),
            "summarize the trends in output_data.csv.".len()
        );
    }

    #[test]
    fn mask_path_tokens_allowlist_equals_normalize_m7() {
        let recognized = [
            "py", "rs", "ts", "tsx", "js", "jsx", "csv", "tsv", "jsonl", "md", "mdx", "txt", "rst",
            "toml", "json", "yaml", "yml", "lock", "ndjson", "parquet",
        ];
        for ext in recognized {
            let token = format!("file.{ext}");
            assert!(
                normalize_explicit_artifact_path(&token).is_some(),
                "normalize must accept recognized .{ext}"
            );
            assert!(
                path_token_is_maskable(&token),
                "mask allowlist must accept recognized .{ext}"
            );
        }
        for token in ["file.exe", "file.bin", "file.markdown", "3.14", "e.g"] {
            assert_eq!(
                normalize_explicit_artifact_path(token).is_some(),
                path_token_is_maskable(token),
                "mask allowlist must agree with normalize for {token:?}"
            );
        }
        assert!(path_token_is_maskable("dir/sub/file.csv"));
    }
}
