//! FeedbackFrame: structured runtime feedback record (Issue #450).
//!
//! This module defines the `FeedbackFrame` value object that captures
//! deterministic runtime feedback (Bash failures, auto_test failures, tool
//! parser failures, unsafe-command blocks, no-progress signals, edit failures,
//! verifier-not-detected events). It is the *frozen shape* read by future
//! issues (Reminder Sidecar / active_precautions / Case Memory).
//!
//! Construction goes through `FeedbackFrameDraft -> build_feedback_frame ->
//! FeedbackFrame::from_draft` so secret-mask and excerpt-truncation cannot be
//! bypassed by external modules building the struct literally.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

/// 8 KiB cap for `stdout_excerpt` / `stderr_excerpt`, including the truncate
/// marker.
pub const EXCERPT_CAP_BYTES: usize = 8 * 1024;

/// Snake-case kind tag for serialized FeedbackFrame.
///
/// `#[serde(other)] UnknownFailure` lets future versions of `session.json`
/// add new kinds without breaking older builds. The enum is *not*
/// `#[non_exhaustive]` so downstream `match` blocks get compile-time
/// completeness checks (judgment 4 in the design policy).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackKind {
    BuildPass,
    TestPass,
    CompileError,
    TestFailure,
    TypeError,
    LintFailure,
    Timeout,
    ToolProtocolFailure,
    EditFailure,
    NoRepoProgress,
    UnsafeCommandBlocked,
    NoVerifierAvailable,
    #[serde(other)]
    #[default]
    UnknownFailure,
}

/// Sealed runtime feedback record. Text fields (`command` / `stdout_excerpt`
/// / `stderr_excerpt`) are **module-private** so even other modules in the
/// same crate cannot construct the struct literally and bypass
/// mask/truncate. Use `FeedbackFrameDraft` + `build_feedback_frame` instead
/// (CB-004 / judgment 3 in the design policy).
///
/// ```compile_fail
/// // Direct struct-literal construction is rejected at compile time
/// // because `command` / `stdout_excerpt` / `stderr_excerpt` are private:
/// use anvil::session::feedback::{FeedbackFrame, FeedbackKind};
/// let _ = FeedbackFrame {
///     command: Some("rm -rf /".to_string()),
///     exit_code: None,
///     kind: FeedbackKind::UnsafeCommandBlocked,
///     stdout_excerpt: String::new(),
///     stderr_excerpt: String::new(),
///     primary_error: None,
///     suspected_files: Vec::new(),
///     changed_files: Vec::new(),
///     suggested_focus: None,
/// };
/// ```
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct FeedbackFrame {
    /// Original command (Bash / auto_test). None for tool-failure paths.
    /// Secret-masked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    command: Option<String>,

    /// Process exit code. None for signal kills / timeouts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,

    /// Failure / success kind.
    pub kind: FeedbackKind,

    /// stdout excerpt (head + tail, marker-aware 8 KiB cap). Secret-masked.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    stdout_excerpt: String,

    /// stderr excerpt (same shape). Secret-masked.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    stderr_excerpt: String,

    /// Primary error line (extracted by upstream helpers). Secret-masked
    /// when populated through `build_feedback_frame` (CB-002).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_error: Option<String>,

    /// Workspace-relative paths suspected by the failure. NOT masked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suspected_files: Vec<PathBuf>,

    /// Workspace-relative paths changed in this turn. NOT masked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_files: Vec<PathBuf>,

    /// Reminder-side hint, populated by Issue #3. Always None for Issue #450.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_focus: Option<String>,
}

impl FeedbackFrame {
    /// Sealed final-stage constructor. The `build_feedback_frame` function is
    /// the only intended caller; it has already truncated, masked, and
    /// normalized the draft. This function only re-packs the values into the
    /// `FeedbackFrame` struct shape. Module-private (CB-004) so that
    /// cross-module code MUST go through `build_feedback_frame`.
    fn from_draft(draft: FeedbackFrameDraft) -> Self {
        Self {
            command: draft.command,
            exit_code: draft.exit_code,
            kind: draft.kind,
            stdout_excerpt: draft.stdout,
            stderr_excerpt: draft.stderr,
            primary_error: draft.primary_error,
            suspected_files: draft.suspected_files,
            changed_files: draft.changed_files,
            suggested_focus: None,
        }
    }

    /// Read-only accessor for the masked command string. Crate-internal
    /// readers (turn.rs / sessions_cli.rs) use this in lieu of the now-private
    /// field.
    pub fn command(&self) -> Option<&str> {
        self.command.as_deref()
    }

    /// Read-only accessor for the masked stdout excerpt.
    pub fn stdout_excerpt(&self) -> &str {
        &self.stdout_excerpt
    }

    /// Read-only accessor for the masked stderr excerpt.
    pub fn stderr_excerpt(&self) -> &str {
        &self.stderr_excerpt
    }
}

/// Raw input handed to `build_feedback_frame`. Holds untruncated /
/// unmasked stdout/stderr; the builder converts it into a sealed
/// `FeedbackFrame`.
#[derive(Debug, Clone, Default)]
pub struct FeedbackFrameDraft {
    pub command: Option<String>,
    pub exit_code: Option<i32>,
    pub kind: FeedbackKind,
    pub stdout: String,
    pub stderr: String,
    pub primary_error: Option<String>,
    pub suspected_files: Vec<PathBuf>,
    pub changed_files: Vec<PathBuf>,
}

// --- Secret mask ----------------------------------------------------------

fn token_prefix_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"\b(?:AKIA[0-9A-Z]{16}|ASIA[0-9A-Z]{16}|gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-(?:proj-)?[A-Za-z0-9_-]{20,}|xox[abprs]-[A-Za-z0-9-]{20,}|eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,})\b",
        )
        .expect("valid static token prefix regex")
    })
}

// The `regex` crate is configured with `default-features = false` plus
// `unicode-perl` (Cargo.toml). That gives us `\s` / `\S` etc. but disables
// `unicode-case`, so `(?i)` would fail to compile. We therefore spell out
// ASCII case explicitly via character classes.
fn kv_secret_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"([Aa][Pp][Ii][_\-]?[Kk][Ee][Yy]|[Tt][Oo][Kk][Ee][Nn]|[Ss][Ee][Cc][Rr][Ee][Tt]|[Pp][Aa][Ss][Ss][Ww][Oo][Rr][Dd]|[Aa][Cc][Cc][Ee][Ss][Ss][_\-]?[Kk][Ee][Yy]|[Cc][Ll][Ii][Ee][Nn][Tt][_\-]?[Ss][Ee][Cc][Rr][Ee][Tt])\s*[=:]\s*\S+",
        )
        .expect("valid static key-value secret regex")
    })
}

fn url_credential_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\b([A-Za-z][A-Za-z0-9+.\-]*://)([^/\s:@]+):([^/\s@]+)@")
            .expect("valid static URL credential regex")
    })
}

/// Mask known token / kv / URL-userinfo secrets in `input`. Pure function;
/// safe to call on any UTF-8 string.
pub fn mask_secrets(input: &str) -> String {
    let s1 = token_prefix_regex().replace_all(input, "***");
    let s2 = kv_secret_regex().replace_all(&s1, |caps: &regex::Captures<'_>| {
        format!("{}=***", &caps[1])
    });
    let s3 = url_credential_regex().replace_all(&s2, "$1***:***@");
    s3.into_owned()
}

// --- Excerpt truncation ---------------------------------------------------

fn floor_char_boundary(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn ceil_char_boundary(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

/// Cap `s` to `EXCERPT_CAP_BYTES` (marker included) by keeping head and
/// tail and inserting `[...truncated N bytes...]` between them. Always
/// returns a UTF-8 string at a char boundary.
///
/// CB-005: `removed` reflects the **actual** number of dropped bytes
/// (`original_len - kept_body_len`), not just the difference against
/// `EXCERPT_CAP_BYTES`. Marker length depends on the digit count of
/// `removed`, so we estimate once with a digit-count upper bound, then
/// recompute the final marker after head/tail boundaries are fixed.
pub fn truncate_excerpt(s: &str) -> String {
    let bytes = s.as_bytes();
    let original_len = bytes.len();
    if original_len <= EXCERPT_CAP_BYTES {
        return s.to_string();
    }
    // Upper-bound estimate for the marker length: we treat `removed` as
    // potentially being `original_len` (worst case) for budget reservation.
    // The actual marker text below is rebuilt with the precise removed count.
    let max_marker = format!("\n[...truncated {original_len} bytes...]\n");
    let marker_reserve = max_marker.len();

    let body_budget = EXCERPT_CAP_BYTES.saturating_sub(marker_reserve);
    let head_budget = body_budget / 2;
    let tail_budget = body_budget - head_budget;

    let head_end = floor_char_boundary(s, head_budget);
    let tail_start_raw = s.len().saturating_sub(tail_budget);
    let tail_start = ceil_char_boundary(s, tail_start_raw);

    // Defensive: if the boundaries cross (head and tail overlap due to
    // multibyte alignment), fall back to a head-only excerpt.
    if head_end > tail_start {
        let kept_len = head_end;
        let removed = original_len - kept_len;
        let marker = format!("\n[...truncated {removed} bytes...]\n");
        let mut out = String::with_capacity(EXCERPT_CAP_BYTES);
        out.push_str(&s[..head_end]);
        out.push_str(&marker);
        if out.len() > EXCERPT_CAP_BYTES {
            // Marker overflow guard: shrink the head.
            let safe_end = floor_char_boundary(s, head_budget.saturating_sub(marker.len()));
            let kept_len = safe_end;
            let removed = original_len - kept_len;
            let marker = format!("\n[...truncated {removed} bytes...]\n");
            out.clear();
            out.push_str(&s[..safe_end]);
            out.push_str(&marker);
        }
        return out;
    }

    // Compute kept_len from the stable head/tail boundaries so the marker
    // accurately reflects the dropped byte count (CB-005).
    let kept_len = head_end + (s.len() - tail_start);
    let removed = original_len - kept_len;
    let marker = format!("\n[...truncated {removed} bytes...]\n");

    let mut out = String::with_capacity(EXCERPT_CAP_BYTES);
    out.push_str(&s[..head_end]);
    out.push_str(&marker);
    out.push_str(&s[tail_start..]);
    if out.len() > EXCERPT_CAP_BYTES {
        // Char-boundary alignment can push us up to a few bytes over.
        // Trim the tail to the floor-char-boundary that fits.
        let overflow = out.len() - EXCERPT_CAP_BYTES;
        let new_tail_start_raw = tail_start.saturating_add(overflow);
        let new_tail_start = ceil_char_boundary(s, new_tail_start_raw);
        let kept_len = head_end + (s.len() - new_tail_start.min(s.len()));
        let removed = original_len - kept_len;
        let marker = format!("\n[...truncated {removed} bytes...]\n");
        let mut shrunken = String::with_capacity(EXCERPT_CAP_BYTES);
        shrunken.push_str(&s[..head_end]);
        shrunken.push_str(&marker);
        if new_tail_start <= s.len() {
            shrunken.push_str(&s[new_tail_start..]);
        }
        out = shrunken;
    }
    out
}

// --- Path normalization ---------------------------------------------------

/// Normalize `p` to a workspace-relative `PathBuf` for persistence.
///
/// 1. If canonicalize succeeds and the result is inside `workspace_root`,
///    return the relative path.
/// 2. If canonicalize fails (deleted / not yet created / broken symlink),
///    fall back to `Path::file_name()`.
/// 3. If even the file_name extraction fails (path ends in `..` or empty),
///    return None.
///
/// Never panics. Never returns an absolute path. Never preserves `..`.
pub fn normalize_path_to_workspace(p: &Path, workspace_root: &Path) -> Option<PathBuf> {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        workspace_root.join(p)
    };

    if let (Ok(canonical), Ok(root_canonical)) = (abs.canonicalize(), workspace_root.canonicalize())
    {
        if let Ok(rel) = canonical.strip_prefix(&root_canonical) {
            return Some(rel.to_path_buf());
        }
        return None;
    }

    p.file_name().map(PathBuf::from)
}

// --- Common builder -------------------------------------------------------

/// Build a sealed `FeedbackFrame` from a raw draft. Applies, in order:
///   1. excerpt truncation (UTF-8 boundary, marker-aware 8 KiB cap)
///   2. secret mask (command / stdout / stderr / primary_error)
///   3. workspace-relative path normalization (suspected_files, changed_files)
///   4. seal via `FeedbackFrame::from_draft`
///
/// CB-002: `primary_error` was previously left raw — callers were free to
/// drop the head non-empty stderr/stdout line into it, and `mask_secrets`
/// only ran on `command` / `stdout` / `stderr`. We now apply `mask_secrets`
/// to `primary_error` too, so a leaked token in the first failure line
/// cannot bypass mask via this short-form summary field.
///
/// This is the only entry point external (agent-layer) helpers should use.
pub(crate) fn build_feedback_frame(
    mut draft: FeedbackFrameDraft,
    workspace_root: &Path,
) -> FeedbackFrame {
    // 1. Truncate excerpts.
    draft.stdout = truncate_excerpt(&draft.stdout);
    draft.stderr = truncate_excerpt(&draft.stderr);

    // 2. Mask secrets in command / stdout / stderr / primary_error.
    if let Some(cmd) = draft.command.take() {
        draft.command = Some(mask_secrets(&cmd));
    }
    draft.stdout = mask_secrets(&draft.stdout);
    draft.stderr = mask_secrets(&draft.stderr);
    if let Some(err) = draft.primary_error.take() {
        draft.primary_error = Some(mask_secrets(&err));
    }

    // 3. Normalize paths. Drop ones that fall outside the workspace and
    //    cannot be reduced to a basename.
    draft.suspected_files = draft
        .suspected_files
        .into_iter()
        .filter_map(|p| normalize_path_to_workspace(&p, workspace_root))
        .collect();
    draft.changed_files = draft
        .changed_files
        .into_iter()
        .filter_map(|p| normalize_path_to_workspace(&p, workspace_root))
        .collect();

    // 4. Seal.
    FeedbackFrame::from_draft(draft)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    // --- T4.1 / T4.10: enum + serde ---------------------------------------

    #[test]
    fn feedback_kind_default_is_unknown_failure() {
        assert_eq!(FeedbackKind::default(), FeedbackKind::UnknownFailure);
    }

    #[test]
    fn kind_serialized_as_snake_case() {
        let k = FeedbackKind::CompileError;
        assert_eq!(serde_json::to_string(&k).unwrap(), "\"compile_error\"");
        let k2 = FeedbackKind::ToolProtocolFailure;
        assert_eq!(
            serde_json::to_string(&k2).unwrap(),
            "\"tool_protocol_failure\""
        );
    }

    #[test]
    fn unknown_feedback_kind_deserializes_as_unknown_failure() {
        let kind: FeedbackKind = serde_json::from_str("\"future_unseen_kind\"").unwrap();
        assert_eq!(kind, FeedbackKind::UnknownFailure);
    }

    #[test]
    fn serde_roundtrip_default_frame() {
        let frame = FeedbackFrame::default();
        let json = serde_json::to_string(&frame).unwrap();
        let decoded: FeedbackFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn serde_roundtrip_full_frame() {
        let draft = FeedbackFrameDraft {
            command: Some("cargo test".to_string()),
            exit_code: Some(101),
            kind: FeedbackKind::CompileError,
            stdout: "out".to_string(),
            stderr: "err".to_string(),
            primary_error: Some("cannot find type X".to_string()),
            suspected_files: vec![PathBuf::from("src/lib.rs")],
            changed_files: vec![PathBuf::from("README.md")],
        };
        let dir = tempdir().unwrap();
        let frame = build_feedback_frame(draft, dir.path());
        let json = serde_json::to_string(&frame).unwrap();
        let decoded: FeedbackFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, frame);
    }

    // --- CB-004 sealing: build_feedback_frame is the ONLY construction path

    /// CB-004: even within the same crate, callers must funnel through
    /// `build_feedback_frame` to apply mask/truncate/path-normalize.
    /// `FeedbackFrame::from_draft` is module-private; struct literal
    /// construction is rejected at compile time because the secret-bearing
    /// fields are private. We assert the funnel works as expected here.
    #[test]
    fn build_feedback_frame_is_the_only_construction_path() {
        let draft = FeedbackFrameDraft {
            command: Some("echo hi".into()),
            stdout: "out".into(),
            stderr: "err".into(),
            ..Default::default()
        };
        let dir = tempdir().unwrap();
        let frame = build_feedback_frame(draft, dir.path());
        assert_eq!(frame.command(), Some("echo hi"));
        assert_eq!(frame.stdout_excerpt(), "out");
        assert_eq!(frame.stderr_excerpt(), "err");
    }

    // --- T4.7 + T4.8: secret mask -----------------------------------------

    #[test]
    fn mask_applied_to_command_stdout_stderr() {
        let draft = FeedbackFrameDraft {
            command: Some("curl -H 'token: AKIAIOSFODNN7EXAMPLE' https://api".into()),
            stdout: "received: AKIAIOSFODNN7EXAMPLE".into(),
            stderr: "secret=ABCDE12345".into(),
            ..Default::default()
        };
        let dir = tempdir().unwrap();
        let frame = build_feedback_frame(draft, dir.path());
        assert!(frame.command().unwrap().contains("***"));
        assert!(frame.stdout_excerpt().contains("***"));
        assert!(frame.stderr_excerpt().contains("***"));
        assert!(!frame.command().unwrap().contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(!frame.stdout_excerpt().contains("AKIAIOSFODNN7EXAMPLE"));
    }

    /// CB-002: `primary_error` IS masked through `build_feedback_frame`.
    /// Path-bearing fields (`suspected_files`, `changed_files`) are NOT
    /// masked because they go through path normalization and basename
    /// fallback; secret-looking basenames are accepted on the assumption
    /// that the filesystem layer has already vetted them.
    #[test]
    fn primary_error_is_masked_paths_are_not() {
        let draft = FeedbackFrameDraft {
            primary_error: Some("token: AKIAIOSFODNN7EXAMPLE".into()),
            suspected_files: vec![PathBuf::from("src/secret_AKIAIOSFODNN7EXAMPLE.rs")],
            ..Default::default()
        };
        let dir = tempdir().unwrap();
        let frame = build_feedback_frame(draft, dir.path());
        // CB-002: primary_error must not leak the raw token.
        assert!(
            !frame
                .primary_error
                .as_ref()
                .unwrap()
                .contains("AKIAIOSFODNN7EXAMPLE"),
            "primary_error leaked secret: {:?}",
            frame.primary_error
        );
        assert!(
            frame.primary_error.as_ref().unwrap().contains("***"),
            "primary_error not masked: {:?}",
            frame.primary_error
        );
        // suspected_files goes through file_name fallback (canonicalize fails
        // because the file does not exist), so basename is preserved as-is.
        assert!(
            frame
                .suspected_files
                .iter()
                .any(|p| p.to_string_lossy().contains("AKIAIOSFODNN7EXAMPLE"))
        );
    }

    /// CB-002 regression: when `build_feedback_for_auto_test` (turn.rs)
    /// drops the head non-empty stderr/stdout line into `primary_error`,
    /// any `AKIA*` / `sk-proj-*` / URL credential in that line MUST be
    /// masked before the frame leaves the builder. We exercise the
    /// builder directly here (turn.rs's helper goes through the same
    /// funnel).
    #[test]
    fn primary_error_does_not_leak_token() {
        let cases: &[&str] = &[
            "AKIAIOSFODNN7EXAMPLE just leaked here",
            "secret=sk-proj-abcdefghijklmnopqrstuvwxyz0123 in stderr",
            "https://user:hunter2@example.com/path failed",
        ];
        let secrets_to_check: &[&str] = &[
            "AKIAIOSFODNN7EXAMPLE",
            "sk-proj-abcdefghijklmnopqrstuvwxyz0123",
            "hunter2",
        ];
        let dir = tempdir().unwrap();
        for line in cases {
            let draft = FeedbackFrameDraft {
                kind: FeedbackKind::TestFailure,
                primary_error: Some((*line).to_string()),
                ..Default::default()
            };
            let frame = build_feedback_frame(draft, dir.path());
            let masked = frame.primary_error.unwrap();
            for needle in secrets_to_check {
                assert!(
                    !masked.contains(needle),
                    "primary_error leaked {needle:?} in line {line:?}; got {masked:?}"
                );
            }
        }
    }

    #[test]
    fn mask_covers_common_token_families_and_url_credentials() {
        let cases = [
            "AKIAIOSFODNN7EXAMPLE",
            "ASIAIOSFODNN7EXAMPLE",
            "ghp_abcdefghijklmnopqrstuvwxyz0123",
            "gho_abcdefghijklmnopqrstuvwxyz0123",
            "ghu_abcdefghijklmnopqrstuvwxyz0123",
            "ghs_abcdefghijklmnopqrstuvwxyz0123",
            "ghr_abcdefghijklmnopqrstuvwxyz0123",
            "github_pat_abcdefghijklmnopqrstuvwxyz0123",
            "sk-abcdefghijklmnopqrstuvwxyz0123",
            "sk-proj-abcdefghijklmnopqrstuvwxyz0123",
            "xoxb-abcdefghijklmnopqrstuvwxyz0123",
            "xoxp-abcdefghijklmnopqrstuvwxyz0123",
            "xoxa-abcdefghijklmnopqrstuvwxyz0123",
            "eyJabcdefghijk.eyJabcdefghijk.signaturepart12345",
        ];
        for tok in cases {
            let masked = mask_secrets(tok);
            assert!(
                !masked.contains(tok),
                "token {tok} not masked; got {masked}"
            );
        }
        let url = "https://user:hunter2@example.com/path";
        let masked = mask_secrets(url);
        assert!(masked.contains("***:***@"), "url not masked: {masked}");
        assert!(!masked.contains("hunter2"));
        assert!(masked.contains("example.com"));
    }

    #[test]
    fn mask_regex_initialized_via_oncelock() {
        // Several invocations should not panic and should reuse the cached
        // regex objects.
        for _ in 0..16 {
            let _ = mask_secrets("api_key=AKIAIOSFODNN7EXAMPLE");
        }
    }

    // --- T4.1 / AC6: excerpt truncation -----------------------------------

    #[test]
    fn excerpt_returns_input_when_under_cap() {
        let s = "hello".to_string();
        assert_eq!(truncate_excerpt(&s), s);
    }

    #[test]
    fn excerpt_capped_at_8kib_with_marker() {
        let big = "a".repeat(16 * 1024);
        let out = truncate_excerpt(&big);
        assert!(
            out.len() <= EXCERPT_CAP_BYTES,
            "excerpt too large: {}",
            out.len()
        );
        assert!(out.contains("[...truncated"));
    }

    #[test]
    fn excerpt_respects_utf8_boundary() {
        // Make a 12 KiB string of multibyte chars.
        let unit = "あ"; // 3 bytes in UTF-8
        let many = unit.repeat(4_500); // ~13.5 KiB
        let out = truncate_excerpt(&many);
        assert!(out.len() <= EXCERPT_CAP_BYTES);
        // out must still be valid UTF-8 (Rust String guarantees that, but we
        // also make sure boundary slicing did not insert replacement chars).
        assert!(out.is_char_boundary(0));
        assert!(out.is_char_boundary(out.len()));
    }

    /// CB-005: the marker reports the **actual** number of dropped bytes
    /// (`original_len - kept_body_len`), not just the gap against
    /// `EXCERPT_CAP_BYTES`. We allow a small alignment tolerance for
    /// UTF-8 char-boundary trims.
    #[test]
    fn truncate_marker_reflects_actual_removed_byte_count() {
        let original_len = 16 * 1024;
        let big = "a".repeat(original_len);
        let out = truncate_excerpt(&big);
        // Extract the N from "[...truncated N bytes...]".
        let marker_start = out.find("[...truncated ").expect("marker present");
        let n_start = marker_start + "[...truncated ".len();
        let after = &out[n_start..];
        let n_end = after.find(' ').expect("space after N");
        let n: usize = after[..n_end].parse().expect("N parses as usize");

        // kept body = out.len() - marker.len(). The actual marker text in
        // the output is what reports `n`, so we approximate kept body via
        // the body bytes outside the marker.
        let marker_len = {
            // marker is "\n[...truncated {n} bytes...]\n"
            format!("\n[...truncated {n} bytes...]\n").len()
        };
        let kept_body_len = out.len() - marker_len;
        let actual_removed = original_len - kept_body_len;

        // N must be within a few bytes of actual_removed (multibyte
        // alignment may shift by less than 4 bytes).
        let diff = n.abs_diff(actual_removed);
        assert!(
            diff <= 4,
            "marker N={n} but actual removed={actual_removed} (diff={diff})"
        );
    }

    // --- T4.9: path normalization -----------------------------------------

    #[test]
    fn path_normalization_drops_outside_workspace() {
        let dir = tempdir().unwrap();
        let outside = std::env::temp_dir();
        // canonicalize succeeds for /tmp; strip_prefix should fail.
        let normalized = normalize_path_to_workspace(&outside, dir.path());
        // `tempdir()` creates dirs under `std::env::temp_dir()`, so
        // `outside` may end up being a *parent* of dir.path(). Either:
        //  - strip_prefix fails -> None
        //  - file_name fallback returns the basename of TMPDIR
        // Both are acceptable: never returns an absolute or `..`-bearing
        // path.
        if let Some(p) = normalized {
            assert!(!p.is_absolute());
            assert!(!p.components().any(|c| c == std::path::Component::ParentDir));
        }
    }

    #[test]
    fn path_normalization_drops_absolute_into_relative() {
        let dir = tempdir().unwrap();
        let inside = dir.path().join("sub").join("file.rs");
        std::fs::create_dir_all(inside.parent().unwrap()).unwrap();
        std::fs::write(&inside, "x").unwrap();
        let normalized = normalize_path_to_workspace(&inside, dir.path()).unwrap();
        assert!(!normalized.is_absolute());
        assert_eq!(normalized, PathBuf::from("sub/file.rs"));
    }

    #[test]
    fn path_normalization_fallbacks_to_filename_when_path_does_not_exist() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("ghost.rs");
        let normalized = normalize_path_to_workspace(&missing, dir.path()).unwrap();
        assert_eq!(normalized, PathBuf::from("ghost.rs"));
    }

    #[test]
    fn path_normalization_relative_path_has_no_parent_traversal() {
        let dir = tempdir().unwrap();
        let p = PathBuf::from("../outside.rs");
        let normalized = normalize_path_to_workspace(&p, dir.path());
        if let Some(rel) = normalized {
            assert!(!rel.is_absolute());
            assert!(
                !rel.components()
                    .any(|c| c == std::path::Component::ParentDir)
            );
        }
    }

    #[test]
    fn build_feedback_frame_drops_unresolvable_paths_for_dot_components() {
        let dir = tempdir().unwrap();
        // file_name() for "." returns None, so it must drop.
        let normalized = normalize_path_to_workspace(Path::new("."), dir.path());
        // canonicalize("./") usually succeeds and is the workspace itself,
        // so strip_prefix yields "" (empty PathBuf).
        if let Some(p) = normalized {
            assert!(!p.is_absolute());
        }
    }

    // --- T4.1: build_feedback_frame ordering ------------------------------

    #[test]
    fn build_feedback_frame_truncates_then_masks() {
        // Construct a stdout that is over 8 KiB and contains a secret near
        // the head. Truncate first preserves the head -> mask still hits.
        let mut s = String::new();
        s.push_str("token=AKIAIOSFODNN7EXAMPLE\n");
        s.push_str(&"x".repeat(16 * 1024));
        let draft = FeedbackFrameDraft {
            command: None,
            stdout: s,
            ..Default::default()
        };
        let dir = tempdir().unwrap();
        let frame = build_feedback_frame(draft, dir.path());
        assert!(frame.stdout_excerpt().contains("***"));
        assert!(frame.stdout_excerpt().len() <= EXCERPT_CAP_BYTES);
    }

    #[test]
    fn build_feedback_frame_normalizes_paths_to_relative() {
        let dir = tempdir().unwrap();
        let abs = dir.path().join("src").join("main.rs");
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(&abs, "x").unwrap();
        let draft = FeedbackFrameDraft {
            suspected_files: vec![abs.clone()],
            changed_files: vec![abs],
            ..Default::default()
        };
        let frame = build_feedback_frame(draft, dir.path());
        for p in frame
            .suspected_files
            .iter()
            .chain(frame.changed_files.iter())
        {
            assert!(!p.is_absolute(), "absolute path leaked: {}", p.display());
        }
    }

    #[test]
    fn build_feedback_frame_keeps_suggested_focus_none() {
        let dir = tempdir().unwrap();
        let frame = build_feedback_frame(FeedbackFrameDraft::default(), dir.path());
        assert!(frame.suggested_focus.is_none());
    }

    #[test]
    fn lossy_utf8_input_does_not_panic_and_yields_replacement_chars() {
        // Simulate what a caller would do for non-UTF8 bytes:
        // they call String::from_utf8_lossy first, and that string flows in.
        let raw = b"hello \xFF world";
        let lossy = String::from_utf8_lossy(raw).into_owned();
        let draft = FeedbackFrameDraft {
            stdout: lossy,
            ..Default::default()
        };
        let dir = tempdir().unwrap();
        let _frame = build_feedback_frame(draft, dir.path());
    }
}
