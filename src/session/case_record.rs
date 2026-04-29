//! CaseRecord — extract a short procedural memory from a successful session
//! and persist it under `state_root/cases/<case_id>.json` (Issue #462).
//!
//! Scope (DR-1, Issue body Out of Scope):
//! - extract + scrub + persist only.
//! - search / similarity / re-injection / CLI are deferred to follow-up Issues.
//!
//! Layering (DR3-002 / agent → session is one-way):
//! - This module is in the session layer. It does not import from
//!   `crate::agent::*`. The agent-side adapter (`turn.rs::build_case_record_inputs`)
//!   gathers `verify_commands` and `language_stack` from agent-layer types and
//!   passes them in via `CaseRecordInputs`.
//!
//! SSOT reuse:
//! - `mask_secrets` / `normalize_path_to_workspace` from `session::feedback` (#461).
//! - `truncate_entry` from `session::store` (DR-003).
//! - allowlist + symlink-reject + size-cap pattern from `session::discovery::iter_session_dirs`.
//! - `is_test_file` / `is_setup_file` / `is_implementation_file` from `util::file_classify`.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::session::anvil_score::AnvilScore;
use crate::session::feedback::{FeedbackKind, mask_secrets, normalize_path_to_workspace};
use crate::session::precaution::{Precaution, PrecautionSource, PrecautionStatus, Severity};
use crate::session::store::truncate_entry;
use crate::util::file_classify::{is_implementation_file, is_setup_file, is_test_file};
use crate::util::git_hardened::run_git;

// ---------------------------------------------------------------------------
// Constants (DR-005 / DR-007 / DR-008 / DR-010)
// ---------------------------------------------------------------------------

/// Allowlist prefix for case_id (mirrors `tmp_tests::TMP_TEST_ID_PREFIX`).
const CASE_ID_PREFIX: &str = "case_";
const CASE_ID_BODY_MIN: usize = 16;
const CASE_ID_BODY_MAX: usize = 64;

/// Per-file size cap for a case JSON. 16 KiB.
pub const MAX_CASE_RECORD_BYTES: u64 = 16 * 1024;

/// Total cap on the number of case files in `state_root/cases/`.
pub const MAX_CASE_RECORDS: usize = 256;

/// task_signature length cap, aligned with `WorkingMemory::MAX_PRECAUTION_TEXT`
/// (DR-010). Defensive — `set_active_task` already truncates upstream.
pub const TASK_SIGNATURE_SAFETY_CAP: usize = 240;

/// Per-kind cap for individual file names in `changed_files_summary` (DR-005).
const MAX_CHANGED_FILE_NAMES: usize = 8;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Stable, scrubbed identifier for the repository this CaseRecord belongs to.
/// Never contains absolute paths; credential-bearing remote URLs are scrubbed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoFingerprint {
    pub workspace_key: String,
    pub git_remote: Option<String>,
    pub git_head_branch: Option<String>,
    pub language_stack_hash: String,
}

/// Short-form snapshot of a `Precaution` that preserves severity / source so
/// downstream CBR re-injection can apply the same prompt-budget ordering
/// (DR-002: field is `text` to match `Precaution::text`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrecautionSnapshot {
    pub id: String,
    pub text: String,
    pub severity: Severity,
    pub source: PrecautionSource,
}

impl From<&Precaution> for PrecautionSnapshot {
    fn from(p: &Precaution) -> Self {
        Self {
            id: p.id.clone(),
            text: p.text.clone(),
            severity: p.severity,
            source: p.source,
        }
    }
}

/// A short procedural memory of a successful session, suitable for re-use in
/// future sessions on the same repository / task signature.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseRecord {
    pub case_id: String,
    pub created_at: u64, // Unix epoch seconds, mirroring `tmp_tests::TmpTest::created_at`.
    pub repo_fingerprint: RepoFingerprint,
    pub task_signature: String,
    pub language_stack: Vec<String>,
    pub initial_feedback: Vec<FeedbackKind>,
    pub successful_precautions: Vec<PrecautionSnapshot>,
    pub changed_files_summary: Vec<String>,
    pub verify_commands: Vec<String>,
    pub outcome_score: AnvilScore,
}

/// Result of `iter_case_files`. Mirrors `discovery::SessionDirEntry` in spirit.
#[derive(Debug, Clone)]
pub struct CaseFileEntry {
    pub case_id: String,
    pub path: PathBuf,
    pub created_at: u64,
    pub size: u64,
}

/// Inputs the agent layer (turn.rs adapter) gathers for `extract`. Borrowed
/// view to avoid clones; `extract` produces an owned `CaseRecord`.
pub struct CaseRecordInputs<'a> {
    pub workspace_key: &'a str,
    pub work_root: &'a Path,
    pub active_task: Option<&'a str>,
    pub language_stack: &'a [String],
    pub initial_feedback: &'a [FeedbackKind],
    pub active_precautions: &'a [Precaution],
    pub changed_files: &'a [String],
    pub verify_commands: &'a [String],
    pub anvil_score: &'a AnvilScore,
    pub repo_edit_succeeded_this_turn: bool,
    pub unsafe_blocks_this_turn: usize,
    pub auto_test_active: bool,
}

/// Persistence error.
#[derive(Debug)]
pub enum PersistError {
    Serde(serde_json::Error),
    Io(io::Error),
    TooLarge { bytes: usize },
}

impl std::fmt::Display for PersistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serde(e) => write!(f, "case record serde error: {e}"),
            Self::Io(e) => write!(f, "case record io error: {e}"),
            Self::TooLarge { bytes } => write!(
                f,
                "case record size {bytes} exceeds MAX_CASE_RECORD_BYTES ({MAX_CASE_RECORD_BYTES})"
            ),
        }
    }
}

impl std::error::Error for PersistError {}

impl From<serde_json::Error> for PersistError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serde(e)
    }
}
impl From<io::Error> for PersistError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

// ---------------------------------------------------------------------------
// Pure helpers (M2 / M3)
// ---------------------------------------------------------------------------

/// Validate a `case_id` against the `case_[a-z0-9_-]{16,64}` allowlist.
///
/// DR-007: rejects bodies starting with `-` so future CLI use of `case_id`
/// cannot be mistaken for a flag.
pub fn validate_case_id(case_id: &str) -> bool {
    let Some(body) = case_id.strip_prefix(CASE_ID_PREFIX) else {
        return false;
    };
    if body.is_empty() {
        return false;
    }
    if body.starts_with('-') {
        return false;
    }
    if body.len() < CASE_ID_BODY_MIN || body.len() > CASE_ID_BODY_MAX {
        return false;
    }
    body.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Deterministic case_id from the workspace_key + task_signature + creation
/// timestamp. Same inputs always produce the same id (mirrors
/// `tmp_tests::derive_test_id`).
pub fn derive_case_id(workspace_key: &str, task_signature: &str, created_at: u64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(workspace_key.as_bytes());
    hasher.update(b"\x00");
    hasher.update(task_signature.as_bytes());
    hasher.update(b"\x00");
    hasher.update(created_at.to_be_bytes());
    let digest = hasher.finalize();
    let hex = format!("{digest:x}");
    format!("{CASE_ID_PREFIX}{}", &hex[..24])
}

/// Build the task signature: mask secrets, normalize absolute paths to
/// workspace-relative, and truncate to `TASK_SIGNATURE_SAFETY_CAP` chars
/// (DR-003 reuses `store::truncate_entry`).
pub fn build_task_signature(active_task: Option<&str>, work_root: &Path) -> String {
    let raw = active_task.unwrap_or("").trim();
    if raw.is_empty() {
        return String::new();
    }
    let masked = mask_secrets(raw);
    let normalized = normalize_inline_paths(&masked, work_root);
    truncate_entry(normalized, TASK_SIGNATURE_SAFETY_CAP)
}

/// Replace any absolute path embedded in the input with a workspace-relative
/// `PathBuf` (via `feedback::normalize_path_to_workspace`). Workspace-external
/// paths are replaced with `<external>` so absolute filesystem layout never
/// leaks into the persisted record.
fn normalize_inline_paths(input: &str, work_root: &Path) -> String {
    let mut out = String::with_capacity(input.len());
    for token in input.split_inclusive(|c: char| c.is_whitespace()) {
        // Strip trailing whitespace for path extraction; keep it for the output.
        let (path_part, trailer) = split_trailing_ws(token);
        if !path_part.starts_with('/') {
            out.push_str(token);
            continue;
        }
        let p = Path::new(path_part);
        match normalize_path_to_workspace(p, work_root) {
            Some(rel) => {
                out.push_str(&rel.to_string_lossy());
                out.push_str(trailer);
            }
            None => {
                out.push_str("<external>");
                out.push_str(trailer);
            }
        }
    }
    out
}

fn split_trailing_ws(s: &str) -> (&str, &str) {
    let trim_end = s.trim_end();
    (trim_end, &s[trim_end.len()..])
}

/// Sanitize a git remote URL: redact `userinfo@host` patterns. Handles
/// IPv6 hosts, multi-`@` strings, and scheme-less ssh shorthand (DR-006).
pub fn sanitize_git_remote(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(scheme_end) = trimmed.find("://") {
        let scheme = &trimmed[..scheme_end];
        let after = &trimmed[scheme_end + 3..];
        if let Some(at) = after.rfind('@') {
            let host_and_path = &after[at + 1..];
            return Some(format!("{scheme}://***@{host_and_path}"));
        }
        return Some(trimmed.to_string());
    }

    // ssh shorthand: `user@host:owner/repo.git`
    if let Some(at) = trimmed.rfind('@') {
        let after_at = &trimmed[at + 1..];
        return Some(format!("***@{after_at}"));
    }
    Some(trimmed.to_string())
}

/// Hash a language stack into a 16-hex stable identifier
/// (sha256, lowercase + sort + dedup, no salt).
pub fn hash_language_stack(stack: &[String]) -> String {
    let mut v: Vec<String> = stack.iter().map(|s| s.to_ascii_lowercase()).collect();
    v.sort();
    v.dedup();
    let joined = v.join("\n");
    let digest = Sha256::digest(joined.as_bytes());
    let hex = format!("{digest:x}");
    hex[..16].to_string()
}

// ---------------------------------------------------------------------------
// Git subprocess (M4)
// ---------------------------------------------------------------------------

/// Capture a `RepoFingerprint`. `git_remote` / `git_head_branch` fall back to
/// `None` on any failure (no git, not a repo, detached HEAD, timeout).
pub fn capture_repo_fingerprint(
    workspace_key: &str,
    work_root: &Path,
    language_stack: &[String],
) -> RepoFingerprint {
    RepoFingerprint {
        workspace_key: workspace_key.to_string(),
        git_remote: capture_git_remote(work_root),
        git_head_branch: capture_git_head_branch(work_root),
        language_stack_hash: hash_language_stack(language_stack),
    }
}

fn capture_git_remote(work_root: &Path) -> Option<String> {
    let raw = run_git(work_root, &["config", "--get", "remote.origin.url"])?;
    let masked = mask_secrets(&raw);
    sanitize_git_remote(&masked)
}

fn capture_git_head_branch(work_root: &Path) -> Option<String> {
    run_git(work_root, &["symbolic-ref", "--short", "HEAD"])
}

// ---------------------------------------------------------------------------
// Extract (M5)
// ---------------------------------------------------------------------------

/// Pure transform: `CaseRecordInputs` → `CaseRecord`. Does not perform I/O.
///
/// Filters precautions to `Active` only as a defensive re-filter (S5-001
/// pattern). Groups changed files via `util::file_classify`. Verify commands
/// must already be deduplicated by the agent-layer adapter; this function
/// does not re-check.
pub fn extract(inputs: &CaseRecordInputs<'_>) -> Option<CaseRecord> {
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let task_signature = build_task_signature(inputs.active_task, inputs.work_root);
    let repo_fingerprint = capture_repo_fingerprint(
        inputs.workspace_key,
        inputs.work_root,
        inputs.language_stack,
    );
    let case_id = derive_case_id(inputs.workspace_key, &task_signature, created_at);
    if !validate_case_id(&case_id) {
        return None;
    }

    let successful_precautions: Vec<PrecautionSnapshot> = inputs
        .active_precautions
        .iter()
        .filter(|p| p.status == PrecautionStatus::Active)
        .map(PrecautionSnapshot::from)
        .collect();

    let changed_files_summary = group_by_file_classify(inputs.changed_files);

    let verify_commands = inputs.verify_commands.to_vec();

    Some(CaseRecord {
        case_id,
        created_at,
        repo_fingerprint,
        task_signature,
        language_stack: inputs.language_stack.to_vec(),
        initial_feedback: inputs.initial_feedback.to_vec(),
        successful_precautions,
        changed_files_summary,
        verify_commands,
        outcome_score: inputs.anvil_score.clone(),
    })
}

/// Group `changed_files` by `util::file_classify`. Output form:
///   first line: `"<test:N impl:M setup:K other:O>"`
///   followed by up to `MAX_CHANGED_FILE_NAMES` per kind in `name (kind)` form.
pub fn group_by_file_classify(changed_files: &[String]) -> Vec<String> {
    let mut tests = Vec::new();
    let mut impls = Vec::new();
    let mut setups = Vec::new();
    let mut others = Vec::new();
    for f in changed_files {
        let p = Path::new(f);
        if is_test_file(p) {
            tests.push(f.clone());
        } else if is_setup_file(p) {
            setups.push(f.clone());
        } else if is_implementation_file(p) {
            impls.push(f.clone());
        } else {
            others.push(f.clone());
        }
    }
    let mut out = Vec::new();
    out.push(format!(
        "<test:{} impl:{} setup:{} other:{}>",
        tests.len(),
        impls.len(),
        setups.len(),
        others.len()
    ));
    for n in tests.into_iter().take(MAX_CHANGED_FILE_NAMES) {
        out.push(format!("{n} (test)"));
    }
    for n in impls.into_iter().take(MAX_CHANGED_FILE_NAMES) {
        out.push(format!("{n} (impl)"));
    }
    for n in setups.into_iter().take(MAX_CHANGED_FILE_NAMES) {
        out.push(format!("{n} (setup)"));
    }
    out
}

// ---------------------------------------------------------------------------
// Persist / iter / lazy_evict (M6)
// ---------------------------------------------------------------------------

/// Persist a CaseRecord to `state_root/cases/<case_id>.json`. Returns the
/// number of bytes written on success.
///
/// Performs lazy LRU eviction (DR-005): if the cases directory is at the cap,
/// the oldest entries are removed before the new file is created.
pub fn persist(state_root: &Path, case: &CaseRecord) -> Result<usize, PersistError> {
    let json = serde_json::to_vec_pretty(case)?;
    if json.len() as u64 > MAX_CASE_RECORD_BYTES {
        return Err(PersistError::TooLarge { bytes: json.len() });
    }
    let dir = state_root.join("cases");
    std::fs::create_dir_all(&dir)?;
    lazy_evict(state_root)?;
    let path = dir.join(format!("{}.json", case.case_id));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(&json)?;
    Ok(json.len())
}

/// Enumerate case files under `state_root/cases/`, applying allowlist on
/// file stem (`validate_case_id`), rejecting symlinks (`symlink_metadata`),
/// and rejecting files larger than `MAX_CASE_RECORD_BYTES`. Result is sorted
/// by `created_at` ascending (oldest first).
pub fn iter_case_files(state_root: &Path) -> Vec<CaseFileEntry> {
    let dir = state_root.join("cases");
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if !meta.is_file() {
            continue;
        }
        if meta.len() > MAX_CASE_RECORD_BYTES {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if !validate_case_id(stem) {
            continue;
        }
        let created_at = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        out.push(CaseFileEntry {
            case_id: stem.to_string(),
            path,
            created_at,
            size: meta.len(),
        });
    }
    out.sort_by_key(|e| e.created_at);
    out
}

/// Lazy LRU eviction: while at or above `MAX_CASE_RECORDS`, remove the oldest
/// entry. Best-effort; failures to remove are silently swallowed.
fn lazy_evict(state_root: &Path) -> io::Result<()> {
    let mut entries = iter_case_files(state_root);
    while entries.len() >= MAX_CASE_RECORDS {
        let victim = entries.remove(0);
        let _ = std::fs::remove_file(&victim.path);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Env DI helpers (M7)
// ---------------------------------------------------------------------------

/// `ANVIL_NO_CASE_RECORD=<non-empty>` disables CaseRecord extraction.
/// Closure DI mirrors `tester::tester_disabled` for testability.
pub fn case_record_disabled<F>(get_env: F) -> bool
where
    F: Fn(&str) -> Result<String, std::env::VarError>,
{
    matches!(get_env("ANVIL_NO_CASE_RECORD"), Ok(v) if !v.is_empty())
}

/// `ANVIL_CASE_RECORD_DRY_RUN=<non-empty>` runs `extract` but skips `persist`.
pub fn case_record_dry_run<F>(get_env: F) -> bool
where
    F: Fn(&str) -> Result<String, std::env::VarError>,
{
    matches!(get_env("ANVIL_CASE_RECORD_DRY_RUN"), Ok(v) if !v.is_empty())
}

// ---------------------------------------------------------------------------
// Tests (M2 / M3 / M5 / M6 / M7)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::precaution::{PrecautionSource, PrecautionStatus, Severity};
    use std::time::Duration;

    fn fake_precaution(id: &str, text: &str) -> Precaution {
        Precaution {
            id: id.to_string(),
            source: PrecautionSource::BuildFailure,
            severity: Severity::High,
            text: text.to_string(),
            applies_to: vec![],
            status: PrecautionStatus::Active,
            retired_reason: None,
        }
    }

    fn fake_score() -> AnvilScore {
        AnvilScore {
            build_passed: Some(true),
            tests_passed: Some(true),
            compile_errors_delta: Some(0),
            test_failures_delta: Some(0),
            compile_error_count: Some(0),
            test_failure_count: Some(0),
            implementation_files_changed: Some(1),
            test_files_changed: Some(1),
            setup_files_changed: Some(0),
            unsafe_actions_blocked: 0,
            consecutive_no_progress_turns: 0,
            user_visible_artifact: true,
        }
    }

    // --- M1 / DR-002 ------------------------------------------------------

    #[test]
    fn from_precaution_uses_text_field() {
        let p = fake_precaution("p1", "do not delete .git");
        let snap: PrecautionSnapshot = (&p).into();
        assert_eq!(snap.id, "p1");
        assert_eq!(snap.text, "do not delete .git");
        assert_eq!(snap.severity, Severity::High);
        assert_eq!(snap.source, PrecautionSource::BuildFailure);
    }

    // --- M2 ---------------------------------------------------------------

    #[test]
    fn validate_case_id_accepts_normal_id() {
        assert!(validate_case_id("case_abcdef0123456789abcd"));
    }

    #[test]
    fn validate_case_id_rejects_dash_prefix() {
        assert!(!validate_case_id("case_-rfdeadbeefdeadbeef"));
    }

    #[test]
    fn validate_case_id_rejects_too_short() {
        assert!(!validate_case_id("case_abc"));
    }

    #[test]
    fn validate_case_id_rejects_too_long() {
        let body: String = "a".repeat(65);
        assert!(!validate_case_id(&format!("case_{body}")));
    }

    #[test]
    fn validate_case_id_rejects_uppercase() {
        assert!(!validate_case_id("case_ABCDEF0123456789ABCD"));
    }

    #[test]
    fn validate_case_id_rejects_missing_prefix() {
        assert!(!validate_case_id("xxxxabcdef0123456789abcd"));
    }

    #[test]
    fn derive_case_id_is_deterministic() {
        let a = derive_case_id("ws", "sig", 1234);
        let b = derive_case_id("ws", "sig", 1234);
        assert_eq!(a, b);
        assert!(validate_case_id(&a));
    }

    #[test]
    fn derive_case_id_changes_with_inputs() {
        let a = derive_case_id("ws1", "sig", 1234);
        let b = derive_case_id("ws2", "sig", 1234);
        let c = derive_case_id("ws1", "sig", 1235);
        assert_ne!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn task_signature_truncates_at_240() {
        let long: String = "a".repeat(500);
        let tmp = tempfile::tempdir().unwrap();
        let sig = build_task_signature(Some(&long), tmp.path());
        // truncate_entry adds an ellipsis "..." after `max_chars` chars.
        assert!(sig.chars().count() <= TASK_SIGNATURE_SAFETY_CAP + 3);
        assert!(sig.contains("aaa"));
    }

    #[test]
    fn task_signature_masks_secrets() {
        let tmp = tempfile::tempdir().unwrap();
        let sig = build_task_signature(
            Some("debug api_key=sk_live_supersecret in handler"),
            tmp.path(),
        );
        assert!(!sig.contains("sk_live_supersecret"));
    }

    #[test]
    fn task_signature_handles_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(build_task_signature(None, tmp.path()), "");
        assert_eq!(build_task_signature(Some(""), tmp.path()), "");
    }

    // --- M3 ---------------------------------------------------------------

    #[test]
    fn sanitize_git_remote_handles_https_creds() {
        let out = sanitize_git_remote("https://user:ghp_xxx@github.com/x/y.git").unwrap();
        assert_eq!(out, "https://***@github.com/x/y.git");
    }

    #[test]
    fn sanitize_git_remote_handles_multi_at() {
        let out = sanitize_git_remote("https://a@b:tok@github.com/x.git").unwrap();
        // rfind('@') retains the rightmost `@host` boundary.
        assert_eq!(out, "https://***@github.com/x.git");
    }

    #[test]
    fn sanitize_git_remote_handles_ipv6_host() {
        let out = sanitize_git_remote("https://u:t@[::1]:8080/r.git").unwrap();
        assert_eq!(out, "https://***@[::1]:8080/r.git");
    }

    #[test]
    fn sanitize_git_remote_handles_ssh_shorthand() {
        let out = sanitize_git_remote("git@github.com:owner/repo.git").unwrap();
        assert_eq!(out, "***@github.com:owner/repo.git");
    }

    #[test]
    fn sanitize_git_remote_passes_through_no_creds() {
        assert_eq!(
            sanitize_git_remote("https://github.com/x/y.git").unwrap(),
            "https://github.com/x/y.git"
        );
    }

    #[test]
    fn sanitize_git_remote_returns_none_on_empty() {
        assert!(sanitize_git_remote("").is_none());
        assert!(sanitize_git_remote("   ").is_none());
    }

    #[test]
    fn hash_language_stack_is_case_insensitive_and_dedup() {
        let a = hash_language_stack(&["Rust".into(), "Node".into()]);
        let b = hash_language_stack(&["node".into(), "rust".into(), "Node".into()]);
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn hash_language_stack_changes_with_content() {
        let a = hash_language_stack(&["rust".into()]);
        let b = hash_language_stack(&["python".into()]);
        assert_ne!(a, b);
    }

    // --- M5 ---------------------------------------------------------------

    fn fake_inputs<'a>(
        work_root: &'a Path,
        precautions: &'a [Precaution],
        changed_files: &'a [String],
        verify: &'a [String],
        score: &'a AnvilScore,
    ) -> CaseRecordInputs<'a> {
        static LANG: &[&str] = &["rust"];
        let lang_owned: Vec<String> = LANG.iter().map(|s| s.to_string()).collect();
        // Box the Vec so the &'a slice survives.
        let lang_static: &'static [String] = Box::leak(lang_owned.into_boxed_slice());
        CaseRecordInputs {
            workspace_key: "ws-key",
            work_root,
            active_task: Some("fix bug"),
            language_stack: lang_static,
            initial_feedback: &[],
            active_precautions: precautions,
            changed_files,
            verify_commands: verify,
            anvil_score: score,
            repo_edit_succeeded_this_turn: true,
            unsafe_blocks_this_turn: 0,
            auto_test_active: true,
        }
    }

    #[test]
    fn extract_filters_to_active_precautions_only() {
        let mut retired = fake_precaution("p2", "old rule");
        retired.status = PrecautionStatus::Retired;
        let active = fake_precaution("p1", "active rule");
        let pres = vec![active, retired];
        let score = fake_score();
        let tmp = tempfile::tempdir().unwrap();
        let cf: Vec<String> = vec!["src/a.rs".into()];
        let v: Vec<String> = vec!["cargo test".into()];
        let inputs = fake_inputs(tmp.path(), &pres, &cf, &v, &score);
        let case = extract(&inputs).expect("extract returns Some");
        assert_eq!(case.successful_precautions.len(), 1);
        assert_eq!(case.successful_precautions[0].id, "p1");
    }

    #[test]
    fn extract_groups_changed_files_by_classify() {
        let pres: Vec<Precaution> = vec![];
        let score = fake_score();
        let tmp = tempfile::tempdir().unwrap();
        let cf: Vec<String> = vec![
            "src/case_record.rs".into(),
            "tests/case_record_extraction.rs".into(),
            "Cargo.toml".into(),
        ];
        let v: Vec<String> = vec![];
        let inputs = fake_inputs(tmp.path(), &pres, &cf, &v, &score);
        let case = extract(&inputs).unwrap();
        // first line is the count summary
        assert!(case.changed_files_summary[0].starts_with('<'));
        // at least one entry tagged with (test|impl|setup)
        let body = case.changed_files_summary[1..].join(" ");
        assert!(body.contains("(test)") || body.contains("(impl)") || body.contains("(setup)"));
    }

    #[test]
    fn extract_clones_anvil_score() {
        let pres: Vec<Precaution> = vec![];
        let score = fake_score();
        let tmp = tempfile::tempdir().unwrap();
        let cf: Vec<String> = vec![];
        let v: Vec<String> = vec![];
        let inputs = fake_inputs(tmp.path(), &pres, &cf, &v, &score);
        let case = extract(&inputs).unwrap();
        assert_eq!(case.outcome_score.build_passed, Some(true));
        assert_eq!(case.outcome_score.tests_passed, Some(true));
    }

    // --- M6 ---------------------------------------------------------------

    fn fake_record(case_id: &str) -> CaseRecord {
        CaseRecord {
            case_id: case_id.to_string(),
            created_at: 1_000_000,
            repo_fingerprint: RepoFingerprint {
                workspace_key: "ws".into(),
                git_remote: None,
                git_head_branch: None,
                language_stack_hash: "0000000000000000".into(),
            },
            task_signature: "fix bug".into(),
            language_stack: vec!["rust".into()],
            initial_feedback: vec![],
            successful_precautions: vec![],
            changed_files_summary: vec![],
            verify_commands: vec![],
            outcome_score: fake_score(),
        }
    }

    #[test]
    fn persist_writes_pretty_json_under_state_root_cases() {
        let tmp = tempfile::tempdir().unwrap();
        let case = fake_record("case_aaaaaaaaaaaaaaaaaaaaaaaa");
        let bytes = persist(tmp.path(), &case).unwrap();
        let path = tmp
            .path()
            .join("cases")
            .join("case_aaaaaaaaaaaaaaaaaaaaaaaa.json");
        assert!(path.exists());
        let written = std::fs::read(&path).unwrap();
        assert_eq!(bytes, written.len());
        let restored: CaseRecord = serde_json::from_slice(&written).unwrap();
        assert_eq!(restored.case_id, case.case_id);
    }

    #[test]
    fn persist_returns_too_large_err_on_oversize() {
        let tmp = tempfile::tempdir().unwrap();
        let mut case = fake_record("case_bbbbbbbbbbbbbbbbbbbbbbbb");
        // Fill with a large payload to exceed 16 KiB
        case.changed_files_summary = (0..2000).map(|i| format!("file_{i}.rs")).collect();
        let err = persist(tmp.path(), &case).unwrap_err();
        assert!(matches!(err, PersistError::TooLarge { .. }));
    }

    #[test]
    fn iter_case_files_rejects_invalid_id_filenames() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("cases");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("not_case.json"), b"{}").unwrap();
        std::fs::write(dir.join("case_short.json"), b"{}").unwrap();
        std::fs::write(dir.join("case_aaaaaaaaaaaaaaaaaaaaaaaa.json"), b"{}").unwrap();
        let entries = iter_case_files(tmp.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].case_id, "case_aaaaaaaaaaaaaaaaaaaaaaaa");
    }

    #[test]
    fn iter_case_files_skips_files_over_size_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("cases");
        std::fs::create_dir_all(&dir).unwrap();
        let big = vec![b'a'; (MAX_CASE_RECORD_BYTES + 1) as usize];
        std::fs::write(dir.join("case_aaaaaaaaaaaaaaaaaaaaaaaa.json"), &big).unwrap();
        let entries = iter_case_files(tmp.path());
        assert!(entries.is_empty());
    }

    #[test]
    fn iter_case_files_returns_sorted_by_created_at() {
        let tmp = tempfile::tempdir().unwrap();
        let mut a = fake_record("case_aaaaaaaaaaaaaaaaaaaaaaaa");
        a.created_at = 100;
        let mut b = fake_record("case_bbbbbbbbbbbbbbbbbbbbbbbb");
        b.created_at = 200;
        persist(tmp.path(), &a).unwrap();
        // Sleep a moment so mtime differs between the writes.
        std::thread::sleep(Duration::from_millis(20));
        persist(tmp.path(), &b).unwrap();
        let entries = iter_case_files(tmp.path());
        assert_eq!(entries.len(), 2);
        assert!(entries[0].created_at <= entries[1].created_at);
    }

    #[cfg(unix)]
    #[test]
    fn iter_case_files_rejects_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("cases");
        std::fs::create_dir_all(&dir).unwrap();
        let real_target = tmp.path().join("real.json");
        std::fs::write(&real_target, b"{}").unwrap();
        let link = dir.join("case_aaaaaaaaaaaaaaaaaaaaaaaa.json");
        std::os::unix::fs::symlink(&real_target, &link).unwrap();
        let entries = iter_case_files(tmp.path());
        assert!(entries.is_empty(), "symlinks must be rejected");
    }

    #[test]
    fn lazy_evict_removes_oldest_when_over_cap() {
        let tmp = tempfile::tempdir().unwrap();
        // Create MAX_CASE_RECORDS files; one more triggers eviction.
        let dir = tmp.path().join("cases");
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..MAX_CASE_RECORDS {
            let id = format!("case_{:0>24}", format!("{i:x}"));
            let case = fake_record(&id);
            persist(tmp.path(), &case).unwrap();
            // Stagger mtime so eviction has a deterministic oldest.
            std::thread::sleep(Duration::from_millis(2));
        }
        let entries_before = iter_case_files(tmp.path());
        assert_eq!(entries_before.len(), MAX_CASE_RECORDS);
        let oldest = entries_before[0].case_id.clone();

        // One more persist triggers eviction (lazy eviction reduces to MAX-1
        // BEFORE writing the new file, so total remains MAX).
        let extra_id = format!("case_{:0>24}", "fffffffe");
        let extra = fake_record(&extra_id);
        persist(tmp.path(), &extra).unwrap();
        let entries_after = iter_case_files(tmp.path());
        assert_eq!(entries_after.len(), MAX_CASE_RECORDS);
        assert!(
            !entries_after.iter().any(|e| e.case_id == oldest),
            "oldest case should be evicted"
        );
    }

    // --- M7 ---------------------------------------------------------------

    #[test]
    fn case_record_disabled_returns_true_for_non_empty() {
        assert!(case_record_disabled(|_| Ok("1".into())));
    }

    #[test]
    fn case_record_disabled_returns_false_for_empty_or_unset() {
        assert!(!case_record_disabled(|_| Ok(String::new())));
        assert!(!case_record_disabled(|_| Err(
            std::env::VarError::NotPresent
        )));
    }

    #[test]
    fn case_record_dry_run_returns_true_for_non_empty() {
        assert!(case_record_dry_run(|_| Ok("yes".into())));
        assert!(!case_record_dry_run(|_| Err(
            std::env::VarError::NotPresent
        )));
    }
}
