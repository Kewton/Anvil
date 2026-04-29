//! AntiPatternRecord — record repeated failure patterns and inject them as
//! avoid-style precautions on similar future tasks (Issue #464 / Epic C).
//!
//! Mirror of `case_record.rs` + `case_retrieval.rs`, specialized for failure:
//!   * `extract_or_increment`: post-loop, on eligible failure FeedbackKind,
//!     upsert a record keyed by (workspace_key, task_signature, feedback_kind).
//!     Repeat count grows monotonically; only `repeat_count >= REPEAT_THRESHOLD`
//!     entries are surfaced by retrieval (= the "repeated" semantics).
//!   * `retrieve_relevant_anti_patterns`: pure-function lexical match against
//!     the persisted set; returns up to `MAX_SELECTED_ANTI_PATTERNS`.
//!   * `format_for_prompt`: renders the `Avoid Patterns (from prior failures):`
//!     section, capped at `MAX_ANTI_PATTERN_RENDERED_CHARS_TOTAL` chars.
//!
//! Layering (DR3-002 / agent → session is one-way):
//!   This module is in the session layer. It does not import from
//!   `crate::agent::*`. The agent-side adapter (`turn.rs::maybe_extract_anti_pattern`
//!   / `try_inject_anti_pattern_message`) gathers `language_stack` and the
//!   per-turn caps and passes them in via `AntiPatternRecordInputs` /
//!   `AntiPatternRetrievalInputs`.
//!
//! SSOT reuse:
//!   * `mask_secrets` from `session::feedback`.
//!   * `RepoFingerprint` / `capture_repo_fingerprint` / `build_task_signature`
//!     from `session::case_record`.
//!   * `truncate_entry` from `session::store`.

use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::session::case_record::RepoFingerprint;
use crate::session::feedback::{FeedbackKind, mask_secrets};
use crate::session::store::truncate_entry;

// ---------------------------------------------------------------------------
// Public constants
// ---------------------------------------------------------------------------

/// Allowlist prefix for `anti_pattern_id`.
const ANTI_PATTERN_ID_PREFIX: &str = "anti_";
const ANTI_PATTERN_ID_BODY_MIN: usize = 16;
const ANTI_PATTERN_ID_BODY_MAX: usize = 64;

/// Per-file size cap. 16 KiB.
pub const MAX_ANTI_PATTERN_RECORD_BYTES: u64 = 16 * 1024;

/// Total cap on number of files in `state_root/anti_patterns/`.
pub const MAX_ANTI_PATTERN_RECORDS: usize = 256;

/// Per-record render cap (chars).
pub const MAX_ANTI_PATTERN_RENDERED_CHARS_PER_RECORD: usize = 200;

/// Section render cap (chars). SSOT — callers must not re-truncate.
pub const MAX_ANTI_PATTERN_RENDERED_CHARS_TOTAL: usize = 512;

/// Maximum number of anti-patterns injected per turn.
pub const MAX_SELECTED_ANTI_PATTERNS: usize = 2;

/// Inclusion threshold (lexical similarity).
pub(crate) const ANTI_PATTERN_RETRIEVAL_SCORE_THRESHOLD: f32 = 0.40;

/// Failure must repeat at least this many times before retrieval surfaces it.
pub const REPEAT_THRESHOLD: usize = 2;

/// `failed_action_summary` length cap.
pub const FAILED_ACTION_SUMMARY_CAP: usize = 200;

/// `avoid_precaution` length cap (aligned with `MAX_PRECAUTION_TEXT`).
pub const AVOID_PRECAUTION_CAP: usize = 240;

// Score weights — heavier on task + kind for failure patterns.
pub(crate) const W_TASK: f32 = 0.40;
pub(crate) const W_KIND: f32 = 0.25;
pub(crate) const W_FILES: f32 = 0.20;
pub(crate) const W_REPO: f32 = 0.15;

// Env-gate keys.
pub(crate) const ENV_DISABLE: &str = "ANVIL_NO_ANTI_PATTERN";
pub(crate) const ENV_DRY_RUN: &str = "ANVIL_ANTI_PATTERN_DRY_RUN";

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Persisted record of a repeated failure pattern.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AntiPatternRecord {
    pub anti_pattern_id: String,
    pub created_at: u64,
    pub last_seen_at: u64,
    pub repo_fingerprint: RepoFingerprint,
    pub task_signature: String,
    pub language_stack: Vec<String>,
    pub failed_action_summary: String,
    pub feedback_kind: FeedbackKind,
    pub avoid_precaution: String,
    pub repeat_count: usize,
    pub touched_files_summary: Vec<String>,
    pub status: AntiPatternStatus,
}

/// Lifecycle state of an anti-pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AntiPatternStatus {
    #[default]
    Active,
    Retired,
    /// Forward-compat fallback for unknown variants in old anti_patterns/*.json.
    #[serde(other)]
    Unknown,
}

/// Inputs gathered by the agent layer to drive `extract_or_increment`.
pub struct AntiPatternRecordInputs<'a> {
    pub workspace_key: &'a str,
    pub work_root: &'a Path,
    pub active_task: Option<&'a str>,
    pub language_stack: &'a [String],
    pub touched_files: &'a [String],
    pub feedback_kind: FeedbackKind,
    pub failed_action_summary: &'a str,
}

/// Inputs gathered by the agent layer to drive `retrieve_relevant_anti_patterns`.
pub struct AntiPatternRetrievalInputs<'a> {
    pub current_task_signature: &'a str,
    pub current_language_stack: &'a [String],
    pub current_repo_fingerprint: &'a RepoFingerprint,
    pub current_touched_files: &'a [String],
    pub current_feedback_kind: Option<FeedbackKind>,
}

/// Per-record score breakdown.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AntiPatternScoreBreakdown {
    pub anti_pattern_id: String,
    pub task: f32,
    pub kind: f32,
    pub files: f32,
    pub repo: f32,
    pub total: f32,
}

/// A record that survived the threshold + cap pipeline.
#[derive(Debug, Clone)]
pub struct SelectedAntiPattern {
    pub record: AntiPatternRecord,
    pub breakdown: AntiPatternScoreBreakdown,
}

#[derive(Debug, Clone)]
pub enum RetrievalOutcome {
    Completed {
        candidate_count: usize,
        selected: Vec<SelectedAntiPattern>,
        skipped_corrupt_count: u32,
        compute_ms: f64,
    },
    Skipped {
        reason: SkipReason,
        candidate_count: usize,
        skipped_corrupt_count: u32,
        compute_ms: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    NoCandidates,
    BelowThreshold,
    DryRun,
    BelowRepeatThreshold,
}

impl SkipReason {
    pub fn as_log_str(self) -> &'static str {
        match self {
            Self::NoCandidates => "no_candidates",
            Self::BelowThreshold => "below_threshold",
            Self::DryRun => "dry_run",
            Self::BelowRepeatThreshold => "below_repeat_threshold",
        }
    }
}

/// Outcome of a single `extract_or_increment` invocation. Adapter maps each
/// variant to `agent.anti_pattern.{extracted, skipped, failed}`.
#[derive(Debug, Clone)]
pub enum ExtractOutcome {
    /// New record created. `repeat_count == 1`.
    Created(AntiPatternRecord),
    /// Existing record incremented. `repeat_count` is the post-increment value.
    Incremented(AntiPatternRecord),
    /// Skipped because the FeedbackKind is not eligible for anti-pattern tracking.
    SkippedIneligibleKind,
    /// Skipped because no `active_task` was set (cannot derive task_signature).
    SkippedNoActiveTask,
}

#[derive(Debug)]
pub enum PersistError {
    Serde(serde_json::Error),
    Io(io::Error),
    TooLarge { bytes: usize },
}

impl std::fmt::Display for PersistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serde(e) => write!(f, "anti_pattern serde error: {e}"),
            Self::Io(e) => write!(f, "anti_pattern io error: {e}"),
            Self::TooLarge { bytes } => write!(
                f,
                "anti_pattern size {bytes} exceeds MAX_ANTI_PATTERN_RECORD_BYTES ({MAX_ANTI_PATTERN_RECORD_BYTES})"
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

#[derive(Debug, Clone)]
pub struct AntiPatternFileEntry {
    pub anti_pattern_id: String,
    pub path: PathBuf,
    pub last_seen_at: u64,
    pub size: u64,
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// FeedbackKinds that warrant an anti-pattern record. Mirrors
/// `FeedbackKind::is_eligible_for_reminder` but explicitly enumerated here
/// so that the anti-pattern policy can drift independently if needed.
pub fn is_repeat_eligible_kind(k: &FeedbackKind) -> bool {
    use FeedbackKind::*;
    matches!(
        k,
        EditFailure
            | CompileError
            | TestFailure
            | TypeError
            | LintFailure
            | Timeout
            | ToolProtocolFailure
            | NoRepoProgress
            | UnsafeCommandBlocked
            | NoToolCall
    )
}

/// Validate an `anti_pattern_id` against `anti_[a-z0-9_-]{16,64}`.
pub fn validate_anti_pattern_id(id: &str) -> bool {
    let Some(body) = id.strip_prefix(ANTI_PATTERN_ID_PREFIX) else {
        return false;
    };
    if body.is_empty() {
        return false;
    }
    if body.starts_with('-') {
        return false;
    }
    if body.len() < ANTI_PATTERN_ID_BODY_MIN || body.len() > ANTI_PATTERN_ID_BODY_MAX {
        return false;
    }
    body.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Stable id over (workspace_key, task_signature, feedback_kind). The same
/// triple deterministically maps to the same `anti_pattern_id`, enabling the
/// upsert (repeat-count increment) flow.
pub fn derive_anti_pattern_id(
    workspace_key: &str,
    task_signature: &str,
    feedback_kind: &FeedbackKind,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(workspace_key.as_bytes());
    hasher.update(b"\x00");
    hasher.update(task_signature.as_bytes());
    hasher.update(b"\x00");
    let kind_label = serde_json::to_string(feedback_kind).unwrap_or_else(|_| "\"unknown\"".into());
    hasher.update(kind_label.as_bytes());
    let digest = hasher.finalize();
    let hex = format!("{digest:x}");
    format!("{ANTI_PATTERN_ID_PREFIX}{}", &hex[..24])
}

/// Build the failed action summary: secret-mask + truncate.
pub fn build_failed_action_summary(raw: &str) -> String {
    let masked = mask_secrets(raw.trim());
    truncate_entry(masked, FAILED_ACTION_SUMMARY_CAP)
}

/// Synthesize the `avoid_precaution` text from a feedback kind + summary.
/// Stable wording — used directly in the `Avoid Patterns:` section.
pub fn build_avoid_precaution(kind: &FeedbackKind, summary: &str) -> String {
    let head = match kind {
        FeedbackKind::EditFailure => "Avoid retrying the same edit shape that failed:",
        FeedbackKind::CompileError => "Avoid the change that produced this compile error:",
        FeedbackKind::TestFailure => "Avoid re-introducing the test failure:",
        FeedbackKind::TypeError => "Avoid the change that produced this type error:",
        FeedbackKind::LintFailure => "Avoid re-introducing this lint failure:",
        FeedbackKind::Timeout => "Avoid the command/path that timed out:",
        FeedbackKind::ToolProtocolFailure => "Avoid the malformed tool call shape:",
        FeedbackKind::NoRepoProgress => "Avoid the no-progress loop:",
        FeedbackKind::UnsafeCommandBlocked => "Avoid the unsafe command:",
        FeedbackKind::NoToolCall => "Avoid finishing the turn without any tool call:",
        _ => "Avoid repeating this failed action:",
    };
    let summary = summary.trim();
    let combined = if summary.is_empty() {
        head.to_string()
    } else {
        format!("{head} {summary}")
    };
    truncate_entry(combined, AVOID_PRECAUTION_CAP)
}

// ---------------------------------------------------------------------------
// Extract / upsert
// ---------------------------------------------------------------------------

/// Build a fresh `AntiPatternRecord` for inputs (no I/O). Returns `None` if
/// the FeedbackKind is not eligible or the active_task is missing.
pub fn build_record_from_inputs(inputs: &AntiPatternRecordInputs<'_>) -> Option<AntiPatternRecord> {
    if !is_repeat_eligible_kind(&inputs.feedback_kind) {
        return None;
    }
    let raw_task = inputs.active_task?.trim();
    if raw_task.is_empty() {
        return None;
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let task_signature =
        crate::session::case_record::build_task_signature(Some(raw_task), inputs.work_root);
    if task_signature.is_empty() {
        return None;
    }

    let repo_fingerprint = crate::session::case_record::capture_repo_fingerprint(
        inputs.workspace_key,
        inputs.work_root,
        inputs.language_stack,
    );

    let id = derive_anti_pattern_id(inputs.workspace_key, &task_signature, &inputs.feedback_kind);
    if !validate_anti_pattern_id(&id) {
        return None;
    }

    let summary = build_failed_action_summary(inputs.failed_action_summary);
    let avoid = build_avoid_precaution(&inputs.feedback_kind, &summary);

    // touched_files: cap to ~8 entries, basename only, secret-masked.
    let touched_files_summary: Vec<String> = inputs
        .touched_files
        .iter()
        .filter_map(|p| {
            Path::new(p)
                .file_name()
                .and_then(|os| os.to_str())
                .map(|s| s.to_string())
        })
        .take(8)
        .map(|s| mask_secrets(&s))
        .collect();

    Some(AntiPatternRecord {
        anti_pattern_id: id,
        created_at: now,
        last_seen_at: now,
        repo_fingerprint,
        task_signature,
        language_stack: inputs.language_stack.to_vec(),
        failed_action_summary: summary,
        feedback_kind: inputs.feedback_kind.clone(),
        avoid_precaution: avoid,
        repeat_count: 1,
        touched_files_summary,
        status: AntiPatternStatus::Active,
    })
}

/// Upsert: if a record with the same `anti_pattern_id` already exists on
/// disk, increment its `repeat_count` and refresh `last_seen_at`. Otherwise
/// persist the freshly built record. Returns the post-write outcome.
pub fn extract_or_increment(
    state_root: &Path,
    inputs: &AntiPatternRecordInputs<'_>,
) -> Result<ExtractOutcome, PersistError> {
    if !is_repeat_eligible_kind(&inputs.feedback_kind) {
        return Ok(ExtractOutcome::SkippedIneligibleKind);
    }
    let Some(raw_task) = inputs.active_task else {
        return Ok(ExtractOutcome::SkippedNoActiveTask);
    };
    if raw_task.trim().is_empty() {
        return Ok(ExtractOutcome::SkippedNoActiveTask);
    }

    let mut fresh = match build_record_from_inputs(inputs) {
        Some(r) => r,
        None => return Ok(ExtractOutcome::SkippedNoActiveTask),
    };

    let dir = state_root.join("anti_patterns");
    let path = dir.join(format!("{}.json", fresh.anti_pattern_id));

    if path.exists()
        && let Ok(bytes) = std::fs::read(&path)
        && let Ok(mut existing) = serde_json::from_slice::<AntiPatternRecord>(&bytes)
    {
        existing.repeat_count = existing.repeat_count.saturating_add(1);
        existing.last_seen_at = fresh.last_seen_at;
        // Keep the more recent action summary so the "avoid" text stays fresh.
        existing.failed_action_summary = fresh.failed_action_summary.clone();
        existing.avoid_precaution = fresh.avoid_precaution.clone();
        existing.touched_files_summary = fresh.touched_files_summary.clone();
        existing.language_stack = fresh.language_stack.clone();
        // Re-Activate if a manual retire was wiped by repeated failure.
        if existing.status == AntiPatternStatus::Retired {
            existing.status = AntiPatternStatus::Active;
        }
        write_record(&path, &existing)?;
        return Ok(ExtractOutcome::Incremented(existing));
    }

    // Fresh path: ensure dir exists, evict if at cap, then create_new.
    std::fs::create_dir_all(&dir)?;
    lazy_evict(state_root)?;
    fresh.repeat_count = 1;
    write_record(&path, &fresh)?;
    Ok(ExtractOutcome::Created(fresh))
}

/// Internal: serialize and write a record, applying the size cap.
fn write_record(path: &Path, record: &AntiPatternRecord) -> Result<(), PersistError> {
    let json = serde_json::to_vec_pretty(record)?;
    if json.len() as u64 > MAX_ANTI_PATTERN_RECORD_BYTES {
        return Err(PersistError::TooLarge { bytes: json.len() });
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    file.write_all(&json)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Iter / persist / evict
// ---------------------------------------------------------------------------

/// Enumerate anti_pattern files under `state_root/anti_patterns/`. Symlinks
/// and oversized files are rejected. Sorted by `last_seen_at` ascending
/// (oldest first) for LRU eviction.
pub fn iter_anti_pattern_files(state_root: &Path) -> Vec<AntiPatternFileEntry> {
    let dir = state_root.join("anti_patterns");
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
        if meta.len() > MAX_ANTI_PATTERN_RECORD_BYTES {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if !validate_anti_pattern_id(stem) {
            continue;
        }
        let last_seen_at = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        out.push(AntiPatternFileEntry {
            anti_pattern_id: stem.to_string(),
            path,
            last_seen_at,
            size: meta.len(),
        });
    }
    out.sort_by_key(|e| e.last_seen_at);
    out
}

/// Lazy LRU eviction: remove the oldest entry while at-or-above
/// `MAX_ANTI_PATTERN_RECORDS`.
fn lazy_evict(state_root: &Path) -> io::Result<()> {
    let mut entries = iter_anti_pattern_files(state_root);
    while entries.len() >= MAX_ANTI_PATTERN_RECORDS {
        let victim = entries.remove(0);
        let _ = std::fs::remove_file(&victim.path);
    }
    Ok(())
}

/// Manually retire a record. Returns true if the record was found and updated.
pub fn retire_anti_pattern(state_root: &Path, anti_pattern_id: &str) -> bool {
    if !validate_anti_pattern_id(anti_pattern_id) {
        return false;
    }
    let path = state_root
        .join("anti_patterns")
        .join(format!("{anti_pattern_id}.json"));
    let Ok(bytes) = std::fs::read(&path) else {
        return false;
    };
    let Ok(mut record) = serde_json::from_slice::<AntiPatternRecord>(&bytes) else {
        return false;
    };
    record.status = AntiPatternStatus::Retired;
    write_record(&path, &record).is_ok()
}

// ---------------------------------------------------------------------------
// Retrieval (lexical similarity, mirrors case_retrieval)
// ---------------------------------------------------------------------------

/// Retrieve anti-patterns relevant to the current turn. Wrapper around the
/// on-disk `iter_anti_pattern_files`.
pub fn retrieve_relevant_anti_patterns(
    state_root: &Path,
    inputs: &AntiPatternRetrievalInputs<'_>,
    dry_run: bool,
) -> Result<RetrievalOutcome, String> {
    let owned = state_root.to_path_buf();
    retrieve_with_iter(inputs, dry_run, move || Ok(iter_anti_pattern_files(&owned)))
}

/// Pluggable I/O seam used by integration tests that need to inject a fake
/// candidate list (or simulate iter failure).
pub fn retrieve_with_iter<F>(
    inputs: &AntiPatternRetrievalInputs<'_>,
    dry_run: bool,
    iter_provider: F,
) -> Result<RetrievalOutcome, String>
where
    F: FnOnce() -> Result<Vec<AntiPatternFileEntry>, String>,
{
    let started = Instant::now();
    let entries = iter_provider()?;
    let candidate_count = entries.len();

    if entries.is_empty() {
        return Ok(RetrievalOutcome::Skipped {
            reason: SkipReason::NoCandidates,
            candidate_count: 0,
            skipped_corrupt_count: 0,
            compute_ms: elapsed_ms(started),
        });
    }

    let mut scored: Vec<(AntiPatternRecord, AntiPatternScoreBreakdown)> =
        Vec::with_capacity(entries.len());
    let mut skipped_corrupt: u32 = 0;
    for entry in &entries {
        match read_and_score_one(entry, inputs) {
            Ok(Some(pair)) => scored.push(pair),
            Ok(None) => {}
            Err(_) => {
                skipped_corrupt = skipped_corrupt.saturating_add(1);
            }
        }
    }

    // Drop Retired and below-repeat-threshold records.
    let pre_repeat_count = scored.len();
    scored.retain(|(r, _)| {
        r.status == AntiPatternStatus::Active && r.repeat_count >= REPEAT_THRESHOLD
    });
    if scored.is_empty() && pre_repeat_count > 0 {
        return Ok(RetrievalOutcome::Skipped {
            reason: SkipReason::BelowRepeatThreshold,
            candidate_count,
            skipped_corrupt_count: skipped_corrupt,
            compute_ms: elapsed_ms(started),
        });
    }

    scored.retain(|(_, b)| b.total >= ANTI_PATTERN_RETRIEVAL_SCORE_THRESHOLD);
    if scored.is_empty() {
        return Ok(RetrievalOutcome::Skipped {
            reason: SkipReason::BelowThreshold,
            candidate_count,
            skipped_corrupt_count: skipped_corrupt,
            compute_ms: elapsed_ms(started),
        });
    }

    // Sort: total desc, then repeat_count desc, then last_seen_at desc.
    scored.sort_by(|a, b| {
        b.1.total
            .partial_cmp(&a.1.total)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.0.repeat_count.cmp(&a.0.repeat_count))
            .then(b.0.last_seen_at.cmp(&a.0.last_seen_at))
    });
    scored.truncate(MAX_SELECTED_ANTI_PATTERNS);

    if dry_run {
        return Ok(RetrievalOutcome::Skipped {
            reason: SkipReason::DryRun,
            candidate_count,
            skipped_corrupt_count: skipped_corrupt,
            compute_ms: elapsed_ms(started),
        });
    }

    let selected: Vec<SelectedAntiPattern> = scored
        .into_iter()
        .map(|(record, breakdown)| SelectedAntiPattern { record, breakdown })
        .collect();

    Ok(RetrievalOutcome::Completed {
        candidate_count,
        selected,
        skipped_corrupt_count: skipped_corrupt,
        compute_ms: elapsed_ms(started),
    })
}

/// Render the `Avoid Patterns (from prior failures):` section. Returns
/// `None` when there is nothing to inject. SSOT for cap application.
pub fn format_for_prompt(selected: &[SelectedAntiPattern]) -> Option<String> {
    if selected.is_empty() {
        return None;
    }
    let header = "Avoid Patterns (from prior failures):\n";
    let mut out = String::with_capacity(MAX_ANTI_PATTERN_RENDERED_CHARS_TOTAL);
    out.push_str(header);
    let mut budget = MAX_ANTI_PATTERN_RENDERED_CHARS_TOTAL.saturating_sub(header.chars().count());
    for s in selected {
        let line = render_one(s, MAX_ANTI_PATTERN_RENDERED_CHARS_PER_RECORD);
        let line_chars = line.chars().count() + 1;
        if line_chars > budget {
            break;
        }
        out.push_str(&line);
        out.push('\n');
        budget -= line_chars;
    }
    if out == header {
        return None;
    }
    Some(mask_secrets(&out))
}

fn render_one(s: &SelectedAntiPattern, budget: usize) -> String {
    let mut out = String::new();
    out.push_str("- ");
    out.push_str(&s.record.avoid_precaution);
    out.push_str(&format!(" (seen x{})", s.record.repeat_count));
    truncate_chars(&out, budget)
}

// ---------------------------------------------------------------------------
// Env gates (closure DI)
// ---------------------------------------------------------------------------

pub fn anti_pattern_disabled<F>(get_env: F) -> bool
where
    F: Fn(&str) -> Result<String, std::env::VarError>,
{
    matches!(get_env(ENV_DISABLE), Ok(v) if !v.is_empty())
}

pub fn anti_pattern_dry_run<F>(get_env: F) -> bool
where
    F: Fn(&str) -> Result<String, std::env::VarError>,
{
    matches!(get_env(ENV_DRY_RUN), Ok(v) if !v.is_empty())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

fn read_and_score_one(
    entry: &AntiPatternFileEntry,
    current: &AntiPatternRetrievalInputs<'_>,
) -> Result<Option<(AntiPatternRecord, AntiPatternScoreBreakdown)>, String> {
    let bytes = std::fs::read(&entry.path).map_err(|e| format!("read: {e}"))?;
    let record: AntiPatternRecord =
        serde_json::from_slice(&bytes).map_err(|e| format!("deserialize: {e}"))?;
    let breakdown = score_record(current, &record);
    Ok(Some((record, breakdown)))
}

fn tokenize_lower(s: &str) -> HashSet<String> {
    s.split_whitespace()
        .map(|t| t.to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}

fn jaccard<'a, I, J>(left: I, right: J) -> f32
where
    I: IntoIterator<Item = &'a str>,
    J: IntoIterator<Item = &'a str>,
{
    let l: HashSet<&str> = left.into_iter().collect();
    let r: HashSet<&str> = right.into_iter().collect();
    if l.is_empty() && r.is_empty() {
        return 0.0;
    }
    let inter = l.intersection(&r).count() as f32;
    let union = l.union(&r).count() as f32;
    if union == 0.0 { 0.0 } else { inter / union }
}

fn score_record(
    current: &AntiPatternRetrievalInputs<'_>,
    candidate: &AntiPatternRecord,
) -> AntiPatternScoreBreakdown {
    debug_assert!(
        (W_TASK + W_KIND + W_FILES + W_REPO - 1.0).abs() < 1e-6,
        "anti_pattern weights must sum to 1.0"
    );

    let cur_task = tokenize_lower(current.current_task_signature);
    let cand_task = tokenize_lower(&candidate.task_signature);
    let task = jaccard(
        cur_task.iter().map(String::as_str),
        cand_task.iter().map(String::as_str),
    );

    let kind = match &current.current_feedback_kind {
        Some(ck) if ck == &candidate.feedback_kind => 1.0,
        Some(ck)
            if ck.is_eligible_for_reminder()
                == candidate.feedback_kind.is_eligible_for_reminder() =>
        {
            0.5
        }
        Some(_) | None => 0.0,
    };

    let cur_files: HashSet<String> = current
        .current_touched_files
        .iter()
        .filter_map(|p| Path::new(p).file_name().and_then(|s| s.to_str()))
        .map(|s| s.to_ascii_lowercase())
        .collect();
    let cand_files: HashSet<String> = candidate
        .touched_files_summary
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    let files = jaccard(
        cur_files.iter().map(String::as_str),
        cand_files.iter().map(String::as_str),
    );

    let repo = if current.current_repo_fingerprint.workspace_key
        == candidate.repo_fingerprint.workspace_key
    {
        1.0
    } else if current.current_repo_fingerprint.language_stack_hash
        == candidate.repo_fingerprint.language_stack_hash
    {
        0.5
    } else {
        0.0
    };

    let total = W_TASK * task + W_KIND * kind + W_FILES * files + W_REPO * repo;

    AntiPatternScoreBreakdown {
        anti_pattern_id: candidate.anti_pattern_id.clone(),
        task,
        kind,
        files,
        repo,
        total,
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_repo_fp(ws: &str, hash: &str) -> RepoFingerprint {
        RepoFingerprint {
            workspace_key: ws.to_string(),
            git_remote: None,
            git_head_branch: None,
            language_stack_hash: hash.to_string(),
        }
    }

    fn fake_record(id: &str, task: &str, kind: FeedbackKind, repeat: usize) -> AntiPatternRecord {
        AntiPatternRecord {
            anti_pattern_id: id.to_string(),
            created_at: 1_000_000,
            last_seen_at: 1_000_000,
            repo_fingerprint: fake_repo_fp("ws-A", "h"),
            task_signature: task.to_string(),
            language_stack: vec!["rust".into()],
            failed_action_summary: "edit src/foo.rs failed".into(),
            feedback_kind: kind,
            avoid_precaution: "Avoid retrying the same edit".into(),
            repeat_count: repeat,
            touched_files_summary: vec!["foo.rs".into()],
            status: AntiPatternStatus::Active,
        }
    }

    // --- pure helpers ---------------------------------------------------

    #[test]
    fn validate_anti_pattern_id_accepts_normal() {
        assert!(validate_anti_pattern_id("anti_abcdef0123456789abcd"));
    }

    #[test]
    fn validate_anti_pattern_id_rejects_dash_prefix_body() {
        assert!(!validate_anti_pattern_id("anti_-rfdeadbeefdeadbeef"));
    }

    #[test]
    fn validate_anti_pattern_id_rejects_too_short() {
        assert!(!validate_anti_pattern_id("anti_abc"));
    }

    #[test]
    fn validate_anti_pattern_id_rejects_uppercase() {
        assert!(!validate_anti_pattern_id("anti_ABCDEFGHIJKLMNOPQRST"));
    }

    #[test]
    fn validate_anti_pattern_id_rejects_missing_prefix() {
        assert!(!validate_anti_pattern_id("xxxx_abcdef0123456789abcd"));
    }

    #[test]
    fn derive_id_is_deterministic() {
        let a = derive_anti_pattern_id("ws", "fix bug", &FeedbackKind::EditFailure);
        let b = derive_anti_pattern_id("ws", "fix bug", &FeedbackKind::EditFailure);
        assert_eq!(a, b);
        assert!(validate_anti_pattern_id(&a));
    }

    #[test]
    fn derive_id_changes_with_kind() {
        let a = derive_anti_pattern_id("ws", "fix bug", &FeedbackKind::EditFailure);
        let b = derive_anti_pattern_id("ws", "fix bug", &FeedbackKind::CompileError);
        assert_ne!(a, b);
    }

    #[test]
    fn is_repeat_eligible_kind_truth_table() {
        assert!(is_repeat_eligible_kind(&FeedbackKind::EditFailure));
        assert!(is_repeat_eligible_kind(&FeedbackKind::CompileError));
        assert!(is_repeat_eligible_kind(&FeedbackKind::TestFailure));
        assert!(is_repeat_eligible_kind(&FeedbackKind::NoRepoProgress));
        assert!(is_repeat_eligible_kind(&FeedbackKind::UnsafeCommandBlocked));
        assert!(is_repeat_eligible_kind(&FeedbackKind::ToolProtocolFailure));
        assert!(is_repeat_eligible_kind(&FeedbackKind::NoToolCall));
        assert!(is_repeat_eligible_kind(&FeedbackKind::TypeError));
        assert!(is_repeat_eligible_kind(&FeedbackKind::LintFailure));
        assert!(is_repeat_eligible_kind(&FeedbackKind::Timeout));
        // Not eligible:
        assert!(!is_repeat_eligible_kind(&FeedbackKind::BuildPass));
        assert!(!is_repeat_eligible_kind(&FeedbackKind::TestPass));
        assert!(!is_repeat_eligible_kind(&FeedbackKind::NoVerifierAvailable));
        assert!(!is_repeat_eligible_kind(&FeedbackKind::UnknownFailure));
    }

    #[test]
    fn build_failed_action_summary_masks_and_truncates() {
        let raw = format!("api_key=sk_live_supersecret on attempt {}", "x".repeat(500));
        let s = build_failed_action_summary(&raw);
        assert!(!s.contains("sk_live_supersecret"));
        assert!(s.chars().count() <= FAILED_ACTION_SUMMARY_CAP + 3);
    }

    #[test]
    fn build_avoid_precaution_picks_kind_specific_head() {
        let s = build_avoid_precaution(&FeedbackKind::EditFailure, "src/foo.rs");
        assert!(s.starts_with("Avoid retrying the same edit"));
        assert!(s.contains("src/foo.rs"));
    }

    // --- build_record_from_inputs --------------------------------------

    #[test]
    fn build_record_returns_none_for_ineligible_kind() {
        let tmp = tempfile::tempdir().unwrap();
        let lang: Vec<String> = vec!["rust".into()];
        let touched: Vec<String> = vec![];
        let inputs = AntiPatternRecordInputs {
            workspace_key: "ws-A",
            work_root: tmp.path(),
            active_task: Some("fix bug"),
            language_stack: &lang,
            touched_files: &touched,
            feedback_kind: FeedbackKind::BuildPass,
            failed_action_summary: "irrelevant",
        };
        assert!(build_record_from_inputs(&inputs).is_none());
    }

    #[test]
    fn build_record_returns_none_for_missing_task() {
        let tmp = tempfile::tempdir().unwrap();
        let lang: Vec<String> = vec!["rust".into()];
        let touched: Vec<String> = vec![];
        let inputs = AntiPatternRecordInputs {
            workspace_key: "ws-A",
            work_root: tmp.path(),
            active_task: None,
            language_stack: &lang,
            touched_files: &touched,
            feedback_kind: FeedbackKind::EditFailure,
            failed_action_summary: "x",
        };
        assert!(build_record_from_inputs(&inputs).is_none());
    }

    #[test]
    fn build_record_populates_avoid_and_summary() {
        let tmp = tempfile::tempdir().unwrap();
        let lang: Vec<String> = vec!["rust".into()];
        let touched: Vec<String> = vec!["src/foo.rs".into()];
        let inputs = AntiPatternRecordInputs {
            workspace_key: "ws-A",
            work_root: tmp.path(),
            active_task: Some("fix off-by-one"),
            language_stack: &lang,
            touched_files: &touched,
            feedback_kind: FeedbackKind::EditFailure,
            failed_action_summary: "exact-match Edit failed for src/foo.rs",
        };
        let r = build_record_from_inputs(&inputs).expect("Some");
        assert_eq!(r.feedback_kind, FeedbackKind::EditFailure);
        assert_eq!(r.repeat_count, 1);
        assert!(r.avoid_precaution.starts_with("Avoid retrying"));
        assert!(r.failed_action_summary.contains("Edit failed"));
        assert_eq!(r.touched_files_summary, vec!["foo.rs".to_string()]);
    }

    // --- extract_or_increment ------------------------------------------

    #[test]
    fn extract_or_increment_creates_then_increments() {
        let tmp = tempfile::tempdir().unwrap();
        let lang: Vec<String> = vec!["rust".into()];
        let touched: Vec<String> = vec!["src/foo.rs".into()];
        let inputs = AntiPatternRecordInputs {
            workspace_key: "ws-A",
            work_root: tmp.path(),
            active_task: Some("fix bug"),
            language_stack: &lang,
            touched_files: &touched,
            feedback_kind: FeedbackKind::EditFailure,
            failed_action_summary: "edit failed",
        };
        match extract_or_increment(tmp.path(), &inputs).unwrap() {
            ExtractOutcome::Created(r) => assert_eq!(r.repeat_count, 1),
            other => panic!("expected Created, got {other:?}"),
        }
        match extract_or_increment(tmp.path(), &inputs).unwrap() {
            ExtractOutcome::Incremented(r) => assert_eq!(r.repeat_count, 2),
            other => panic!("expected Incremented, got {other:?}"),
        }
        match extract_or_increment(tmp.path(), &inputs).unwrap() {
            ExtractOutcome::Incremented(r) => assert_eq!(r.repeat_count, 3),
            other => panic!("expected Incremented, got {other:?}"),
        }
    }

    #[test]
    fn extract_or_increment_skips_ineligible() {
        let tmp = tempfile::tempdir().unwrap();
        let lang: Vec<String> = vec!["rust".into()];
        let touched: Vec<String> = vec![];
        let inputs = AntiPatternRecordInputs {
            workspace_key: "ws-A",
            work_root: tmp.path(),
            active_task: Some("fix bug"),
            language_stack: &lang,
            touched_files: &touched,
            feedback_kind: FeedbackKind::TestPass,
            failed_action_summary: "x",
        };
        assert!(matches!(
            extract_or_increment(tmp.path(), &inputs).unwrap(),
            ExtractOutcome::SkippedIneligibleKind
        ));
    }

    #[test]
    fn extract_or_increment_skips_no_active_task() {
        let tmp = tempfile::tempdir().unwrap();
        let lang: Vec<String> = vec!["rust".into()];
        let touched: Vec<String> = vec![];
        let inputs = AntiPatternRecordInputs {
            workspace_key: "ws-A",
            work_root: tmp.path(),
            active_task: None,
            language_stack: &lang,
            touched_files: &touched,
            feedback_kind: FeedbackKind::EditFailure,
            failed_action_summary: "x",
        };
        assert!(matches!(
            extract_or_increment(tmp.path(), &inputs).unwrap(),
            ExtractOutcome::SkippedNoActiveTask
        ));
    }

    // --- iter / retire -------------------------------------------------

    #[test]
    fn iter_anti_pattern_files_rejects_invalid_id() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("anti_patterns");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("not_anti.json"), b"{}").unwrap();
        std::fs::write(dir.join("anti_short.json"), b"{}").unwrap();
        std::fs::write(dir.join("anti_aaaaaaaaaaaaaaaaaaaaaaaa.json"), b"{}").unwrap();
        let entries = iter_anti_pattern_files(tmp.path());
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn retire_anti_pattern_marks_status_retired() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("anti_patterns");
        std::fs::create_dir_all(&dir).unwrap();
        let r = fake_record(
            "anti_aaaaaaaaaaaaaaaaaaaaaaaa",
            "fix bug",
            FeedbackKind::EditFailure,
            5,
        );
        write_record(&dir.join(format!("{}.json", r.anti_pattern_id)), &r).unwrap();
        assert!(retire_anti_pattern(tmp.path(), &r.anti_pattern_id));
        let bytes = std::fs::read(dir.join(format!("{}.json", r.anti_pattern_id))).unwrap();
        let restored: AntiPatternRecord = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(restored.status, AntiPatternStatus::Retired);
    }

    #[test]
    fn retire_anti_pattern_returns_false_for_missing_or_invalid() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!retire_anti_pattern(tmp.path(), "anti_does_not_exist_xxxx"));
        assert!(!retire_anti_pattern(tmp.path(), "invalid"));
    }

    // --- retrieve_with_iter --------------------------------------------

    fn write_record_at(dir: &Path, r: &AntiPatternRecord) -> AntiPatternFileEntry {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(format!("{}.json", r.anti_pattern_id));
        std::fs::write(&path, serde_json::to_vec_pretty(r).unwrap()).unwrap();
        AntiPatternFileEntry {
            anti_pattern_id: r.anti_pattern_id.clone(),
            path,
            last_seen_at: r.last_seen_at,
            size: 0,
        }
    }

    #[test]
    fn weights_sum_to_one() {
        assert!((W_TASK + W_KIND + W_FILES + W_REPO - 1.0).abs() < 1e-6);
    }

    #[test]
    fn retrieve_no_candidates_returns_skipped() {
        let stack = vec!["rust".to_string()];
        let fp = fake_repo_fp("ws-A", "h");
        let inputs = AntiPatternRetrievalInputs {
            current_task_signature: "x",
            current_language_stack: &stack,
            current_repo_fingerprint: &fp,
            current_touched_files: &[],
            current_feedback_kind: None,
        };
        let out = retrieve_with_iter(&inputs, false, || Ok(Vec::new())).unwrap();
        match out {
            RetrievalOutcome::Skipped { reason, .. } => {
                assert_eq!(reason, SkipReason::NoCandidates);
            }
            _ => panic!("expected Skipped(NoCandidates)"),
        }
    }

    #[test]
    fn retrieve_iter_failure_returns_err() {
        let stack = vec!["rust".to_string()];
        let fp = fake_repo_fp("ws-A", "h");
        let inputs = AntiPatternRetrievalInputs {
            current_task_signature: "x",
            current_language_stack: &stack,
            current_repo_fingerprint: &fp,
            current_touched_files: &[],
            current_feedback_kind: None,
        };
        assert!(retrieve_with_iter(&inputs, false, || Err("inj".into())).is_err());
    }

    #[test]
    fn retrieve_below_repeat_threshold_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("anti_patterns");
        let r = fake_record(
            "anti_aaaaaaaaaaaaaaaaaaaaaaaa",
            "fix bug now",
            FeedbackKind::EditFailure,
            1, // below REPEAT_THRESHOLD
        );
        let entry = write_record_at(&dir, &r);
        let stack = vec!["rust".to_string()];
        let fp = fake_repo_fp("ws-A", "h");
        let inputs = AntiPatternRetrievalInputs {
            current_task_signature: "fix bug now",
            current_language_stack: &stack,
            current_repo_fingerprint: &fp,
            current_touched_files: &[],
            current_feedback_kind: Some(FeedbackKind::EditFailure),
        };
        let out = retrieve_with_iter(&inputs, false, || Ok(vec![entry])).unwrap();
        match out {
            RetrievalOutcome::Skipped { reason, .. } => {
                assert_eq!(reason, SkipReason::BelowRepeatThreshold);
            }
            other => panic!("expected BelowRepeatThreshold, got {other:?}"),
        }
    }

    #[test]
    fn retrieve_retired_records_filtered_out() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("anti_patterns");
        let mut r = fake_record(
            "anti_aaaaaaaaaaaaaaaaaaaaaaaa",
            "fix bug now",
            FeedbackKind::EditFailure,
            5,
        );
        r.status = AntiPatternStatus::Retired;
        let entry = write_record_at(&dir, &r);
        let stack = vec!["rust".to_string()];
        let fp = fake_repo_fp("ws-A", "h");
        let inputs = AntiPatternRetrievalInputs {
            current_task_signature: "fix bug now",
            current_language_stack: &stack,
            current_repo_fingerprint: &fp,
            current_touched_files: &[],
            current_feedback_kind: Some(FeedbackKind::EditFailure),
        };
        let out = retrieve_with_iter(&inputs, false, || Ok(vec![entry])).unwrap();
        // Retired-only candidates should result in BelowRepeatThreshold (since
        // retain drops them in the same pre-repeat pass).
        match out {
            RetrievalOutcome::Skipped { reason, .. } => {
                assert!(
                    reason == SkipReason::BelowRepeatThreshold
                        || reason == SkipReason::BelowThreshold
                );
            }
            other => panic!("expected Skipped, got {other:?}"),
        }
    }

    #[test]
    fn retrieve_above_threshold_returns_completed_capped() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("anti_patterns");
        let mut entries = Vec::new();
        for i in 0..(MAX_SELECTED_ANTI_PATTERNS + 2) {
            let id = format!("anti_{:a<24}", i);
            let mut r = fake_record(
                &id,
                "fix bug now",
                FeedbackKind::EditFailure,
                REPEAT_THRESHOLD,
            );
            r.last_seen_at = 1000 + i as u64;
            let mut e = write_record_at(&dir, &r);
            e.last_seen_at = r.last_seen_at;
            entries.push(e);
        }
        let stack = vec!["rust".to_string()];
        let fp = fake_repo_fp("ws-A", "h");
        let inputs = AntiPatternRetrievalInputs {
            current_task_signature: "fix bug now",
            current_language_stack: &stack,
            current_repo_fingerprint: &fp,
            current_touched_files: &[],
            current_feedback_kind: Some(FeedbackKind::EditFailure),
        };
        let out = retrieve_with_iter(&inputs, false, || Ok(entries)).unwrap();
        match out {
            RetrievalOutcome::Completed { selected, .. } => {
                assert_eq!(selected.len(), MAX_SELECTED_ANTI_PATTERNS);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    #[test]
    fn retrieve_dry_run_skipped_dry_run() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("anti_patterns");
        let r = fake_record(
            "anti_aaaaaaaaaaaaaaaaaaaaaaaa",
            "fix bug now",
            FeedbackKind::EditFailure,
            REPEAT_THRESHOLD,
        );
        let entry = write_record_at(&dir, &r);
        let stack = vec!["rust".to_string()];
        let fp = fake_repo_fp("ws-A", "h");
        let inputs = AntiPatternRetrievalInputs {
            current_task_signature: "fix bug now",
            current_language_stack: &stack,
            current_repo_fingerprint: &fp,
            current_touched_files: &[],
            current_feedback_kind: Some(FeedbackKind::EditFailure),
        };
        let out = retrieve_with_iter(&inputs, true, || Ok(vec![entry])).unwrap();
        match out {
            RetrievalOutcome::Skipped { reason, .. } => {
                assert_eq!(reason, SkipReason::DryRun);
            }
            _ => panic!("expected DryRun"),
        }
    }

    #[test]
    fn retrieve_corrupt_file_increments_skipped_corrupt_count() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("anti_patterns");
        std::fs::create_dir_all(&dir).unwrap();
        let valid = fake_record(
            "anti_aaaaaaaaaaaaaaaaaaaaaaaa",
            "fix bug now",
            FeedbackKind::EditFailure,
            REPEAT_THRESHOLD,
        );
        let valid_entry = write_record_at(&dir, &valid);
        let bad_path = dir.join("anti_bbbbbbbbbbbbbbbbbbbbbbbb.json");
        std::fs::write(&bad_path, b"{ not json").unwrap();
        let bad_entry = AntiPatternFileEntry {
            anti_pattern_id: "anti_bbbbbbbbbbbbbbbbbbbbbbbb".into(),
            path: bad_path,
            last_seen_at: 0,
            size: 10,
        };
        let stack = vec!["rust".to_string()];
        let fp = fake_repo_fp("ws-A", "h");
        let inputs = AntiPatternRetrievalInputs {
            current_task_signature: "fix bug now",
            current_language_stack: &stack,
            current_repo_fingerprint: &fp,
            current_touched_files: &[],
            current_feedback_kind: Some(FeedbackKind::EditFailure),
        };
        let out = retrieve_with_iter(&inputs, false, || Ok(vec![valid_entry, bad_entry])).unwrap();
        match out {
            RetrievalOutcome::Completed {
                skipped_corrupt_count,
                selected,
                ..
            } => {
                assert_eq!(skipped_corrupt_count, 1);
                assert_eq!(selected.len(), 1);
            }
            other => panic!("expected Completed with 1 corrupt, got {other:?}"),
        }
    }

    // --- format_for_prompt ---------------------------------------------

    #[test]
    fn format_for_prompt_empty_returns_none() {
        assert!(format_for_prompt(&[]).is_none());
    }

    #[test]
    fn format_for_prompt_single_includes_header_and_count() {
        let r = fake_record(
            "anti_aaaaaaaaaaaaaaaaaaaaaaaa",
            "fix bug",
            FeedbackKind::EditFailure,
            3,
        );
        let sel = SelectedAntiPattern {
            record: r,
            breakdown: AntiPatternScoreBreakdown {
                anti_pattern_id: "anti_aaaaaaaaaaaaaaaaaaaaaaaa".into(),
                task: 1.0,
                kind: 1.0,
                files: 0.0,
                repo: 1.0,
                total: 0.7,
            },
        };
        let out = format_for_prompt(std::slice::from_ref(&sel)).expect("section");
        assert!(out.starts_with("Avoid Patterns (from prior failures):\n"));
        assert!(out.contains("seen x3"));
    }

    #[test]
    fn format_for_prompt_total_chars_within_cap() {
        let r1 = {
            let mut r = fake_record(
                "anti_aaaaaaaaaaaaaaaaaaaaaaaa",
                "fix bug",
                FeedbackKind::EditFailure,
                3,
            );
            r.avoid_precaution = "x".repeat(500);
            r
        };
        let sel = SelectedAntiPattern {
            record: r1,
            breakdown: AntiPatternScoreBreakdown {
                anti_pattern_id: "anti_aaaaaaaaaaaaaaaaaaaaaaaa".into(),
                task: 1.0,
                kind: 1.0,
                files: 0.0,
                repo: 1.0,
                total: 0.7,
            },
        };
        let out = format_for_prompt(std::slice::from_ref(&sel)).expect("section");
        assert!(out.chars().count() <= MAX_ANTI_PATTERN_RENDERED_CHARS_TOTAL);
    }

    // --- env gates -----------------------------------------------------

    #[test]
    fn env_disable_true_when_set() {
        assert!(anti_pattern_disabled(|_| Ok("1".to_string())));
    }
    #[test]
    fn env_disable_false_when_unset() {
        assert!(!anti_pattern_disabled(|_| Err(
            std::env::VarError::NotPresent
        )));
    }
    #[test]
    fn env_disable_false_when_empty() {
        assert!(!anti_pattern_disabled(|_| Ok(String::new())));
    }
    #[test]
    fn env_dry_run_true_when_set() {
        assert!(anti_pattern_dry_run(|_| Ok("yes".into())));
    }
}
