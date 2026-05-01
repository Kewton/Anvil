//! Lexical case retrieval over persisted CaseRecord set (Issue #463 / Epic C).
//!
//! Pure functions, no LLM calls, no I/O outside `iter_case_files` + `read`.
//!
//! Layering (DR3-002 / agent → session is one-way):
//! - This module is in the session layer. It does not import from
//!   `crate::agent::*`. Agent-layer types (e.g. `language_stack`) are received
//!   as `&[String]` plain slices through `CaseRetrievalInputs`.
//! - The exception in `anvil_score.rs` (which imports
//!   `crate::agent::orchestration::RepoVerification`) is intentionally NOT
//!   followed here.
//!
//! SSOT reuse:
//! - `mask_secrets` from `session::feedback`.
//! - `iter_case_files` / `CaseRecord` / `RepoFingerprint` / `PrecautionSnapshot`
//!   from `session::case_record`.

use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use crate::session::case_record::{
    CaseFileEntry, CaseRecord, PrecautionSnapshot, RepoFingerprint, iter_case_files,
};
use crate::session::feedback::{FeedbackKind, mask_secrets};

// ---------------------------------------------------------------------------
// Public constants (DR1-001 / DR1-005 / S3-005 / S3-007)
// ---------------------------------------------------------------------------

/// Per-case render cap. 240 chars (`chars().count()`-based, CJK safe).
pub const MAX_CASE_RENDERED_CHARS_PER_CASE: usize = 240;

/// Section render cap. 1024 chars total. SSOT — callers do not re-truncate.
pub const MAX_CASE_RENDERED_CHARS_TOTAL: usize = 1024;

/// Maximum number of cases injected per turn.
pub const MAX_SELECTED_CASES: usize = 3;

/// Inclusion threshold. Candidates strictly below this are dropped.
pub(crate) const CASE_RETRIEVAL_SCORE_THRESHOLD: f32 = 0.40;

// Score weights (DR1-001). Sum MUST equal 1.0 — debug_assert in `score_case`.
pub(crate) const W_TASK: f32 = 0.10;
pub(crate) const W_SEMANTIC: f32 = 0.20;
pub(crate) const W_STACK: f32 = 0.10;
pub(crate) const W_REPO: f32 = 0.20;
pub(crate) const W_FILES: f32 = 0.20;
pub(crate) const W_KIND: f32 = 0.10;
pub(crate) const W_PRECAUTIONS: f32 = 0.10;

// Env-gate keys.
pub(crate) const ENV_DISABLE: &str = "ANVIL_NO_CASE_RETRIEVAL";
pub(crate) const ENV_DRY_RUN: &str = "ANVIL_CASE_RETRIEVAL_DRY_RUN";

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Per-case score breakdown. Logged inside `selected_reasons` array
/// (DR2-010: structured object array, not free-form string).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CaseScoreBreakdown {
    pub case_id: String,
    pub task: f32,
    pub semantic: f32,
    pub stack: f32,
    pub repo: f32,
    pub files: f32,
    pub kind: f32,
    pub precautions: f32,
    pub total: f32,
}

/// Inputs gathered by the agent-layer adapter. `current_active_precautions` is
/// narrowed to `&[PrecautionSnapshot]` (DR1-002) instead of full `&[Precaution]`
/// so the retrieval signature is stable against future Precaution field
/// additions; the adapter builds the snapshot via `PrecautionSnapshot::from`.
pub struct CaseRetrievalInputs<'a> {
    pub current_task_signature: &'a str,
    pub current_language_stack: &'a [String],
    pub current_repo_fingerprint: &'a RepoFingerprint,
    pub current_touched_files: &'a [String],
    pub current_feedback_kind: Option<FeedbackKind>,
    pub current_active_precautions: &'a [PrecautionSnapshot],
}

/// Outcome of a retrieval invocation. The adapter maps each variant to
/// `agent.case_retrieval.{completed, skipped}` log events.
#[derive(Debug, Clone)]
pub enum RetrievalOutcome {
    Completed {
        candidate_count: usize,
        selected: Vec<SelectedCase>,
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

/// Session-layer-terminal skip reason. Adapter-layer reasons (`plan_mode`,
/// `per_turn_cap_consumed`) are emitted directly by the adapter and are NOT
/// part of this enum (DR1-005 / 設計判断 #6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    NoCandidates,
    BelowThreshold,
    DryRun,
}

impl SkipReason {
    pub fn as_log_str(self) -> &'static str {
        match self {
            Self::NoCandidates => "no_candidates",
            Self::BelowThreshold => "below_threshold",
            Self::DryRun => "dry_run",
        }
    }
}

/// A case that survived the threshold + cap pipeline, with its score breakdown.
#[derive(Debug, Clone)]
pub struct SelectedCase {
    pub record: CaseRecord,
    pub breakdown: CaseScoreBreakdown,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Retrieve cases that score at or above `CASE_RETRIEVAL_SCORE_THRESHOLD`.
/// Wrapper that uses the on-disk `iter_case_files`.
pub fn retrieve_relevant_cases(
    state_root: &Path,
    inputs: &CaseRetrievalInputs<'_>,
    dry_run: bool,
) -> Result<RetrievalOutcome, String> {
    let owned = state_root.to_path_buf();
    retrieve_with_iter(inputs, dry_run, move || Ok(iter_case_files(&owned)))
}

/// DR1-006 / 設計判断 #7: pluggable I/O seam used by the E2E `iter_failure`
/// fixture. `iter_provider` returns the candidate file list; an `Err` return
/// surfaces here as a `failed` outcome to the adapter.
///
/// Visibility: `pub` (rather than `pub(crate)` per the original design judgment)
/// so integration tests in `tests/case_retrieval_smoke.rs` can exercise R6
/// (iter failure → failed event → actor loop continues).
pub fn retrieve_with_iter<F>(
    inputs: &CaseRetrievalInputs<'_>,
    dry_run: bool,
    iter_provider: F,
) -> Result<RetrievalOutcome, String>
where
    F: FnOnce() -> Result<Vec<CaseFileEntry>, String>,
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

    let mut scored: Vec<(CaseRecord, CaseScoreBreakdown)> = Vec::with_capacity(entries.len());
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

    scored.retain(|(_, b)| b.total >= CASE_RETRIEVAL_SCORE_THRESHOLD);
    if scored.is_empty() {
        return Ok(RetrievalOutcome::Skipped {
            reason: SkipReason::BelowThreshold,
            candidate_count,
            skipped_corrupt_count: skipped_corrupt,
            compute_ms: elapsed_ms(started),
        });
    }

    // Sort: total desc, then created_at desc as tie-break.
    scored.sort_by(|a, b| {
        b.1.total
            .partial_cmp(&a.1.total)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.0.created_at.cmp(&a.0.created_at))
    });
    scored.truncate(MAX_SELECTED_CASES);

    if dry_run {
        return Ok(RetrievalOutcome::Skipped {
            reason: SkipReason::DryRun,
            candidate_count,
            skipped_corrupt_count: skipped_corrupt,
            compute_ms: elapsed_ms(started),
        });
    }

    let selected: Vec<SelectedCase> = scored
        .into_iter()
        .map(|(record, breakdown)| SelectedCase { record, breakdown })
        .collect();

    Ok(RetrievalOutcome::Completed {
        candidate_count,
        selected,
        skipped_corrupt_count: skipped_corrupt,
        compute_ms: elapsed_ms(started),
    })
}

/// Render the `Relevant Local Cases:` section. Returns `None` when there is
/// nothing to inject. SSOT for cap application — callers must not re-truncate.
pub fn format_for_prompt(selected: &[SelectedCase]) -> Option<String> {
    if selected.is_empty() {
        return None;
    }
    let header = "Relevant Local Cases:\n";
    let mut out = String::with_capacity(MAX_CASE_RENDERED_CHARS_TOTAL);
    out.push_str(header);
    let mut budget_chars = MAX_CASE_RENDERED_CHARS_TOTAL.saturating_sub(header.chars().count());
    for case in selected {
        let line = render_one_case(case, MAX_CASE_RENDERED_CHARS_PER_CASE);
        let line_chars = line.chars().count() + 1; // include trailing newline
        if line_chars > budget_chars {
            break;
        }
        out.push_str(&line);
        out.push('\n');
        budget_chars -= line_chars;
    }
    if out == header {
        // Nothing fit; treat as no-section.
        return None;
    }
    Some(mask_secrets(&out))
}

// ---------------------------------------------------------------------------
// Env gates (closure DI, DR2-001 signature == case_record_disabled)
// ---------------------------------------------------------------------------

/// `ANVIL_NO_CASE_RETRIEVAL=<non-empty>` disables retrieval entirely.
pub fn case_retrieval_disabled<F>(get_env: F) -> bool
where
    F: Fn(&str) -> Result<String, std::env::VarError>,
{
    matches!(get_env(ENV_DISABLE), Ok(v) if !v.is_empty())
}

/// `ANVIL_CASE_RETRIEVAL_DRY_RUN=<non-empty>` runs scoring + log but skips
/// prompt injection.
pub fn case_retrieval_dry_run<F>(get_env: F) -> bool
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

/// Read one file, deserialize as `CaseRecord`, score it. `Err` indicates
/// per-file corruption (caller increments `skipped_corrupt_count` and continues).
fn read_and_score_one(
    entry: &CaseFileEntry,
    current: &CaseRetrievalInputs<'_>,
) -> Result<Option<(CaseRecord, CaseScoreBreakdown)>, String> {
    let bytes = std::fs::read(&entry.path).map_err(|e| format!("read: {e}"))?;
    let record: CaseRecord =
        serde_json::from_slice(&bytes).map_err(|e| format!("deserialize: {e}"))?;
    let breakdown = score_case(current, &record);
    Ok(Some((record, breakdown)))
}

/// ASCII-lowercase whitespace tokenizer. Empty / whitespace-only input
/// yields an empty set.
fn tokenize_ascii_lowercase(s: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    for tok in s.split_whitespace() {
        let lower = tok.to_ascii_lowercase();
        if !lower.is_empty() {
            out.insert(lower);
        }
    }
    out
}

fn semantic_tokens(s: &str) -> HashSet<String> {
    let lower = s.to_lowercase();
    let mut out = HashSet::new();
    let groups: &[(&str, &[&str])] = &[
        (
            "bugfix",
            &[
                "bug",
                "defect",
                "issue",
                "broken",
                "fix",
                "repair",
                "correct",
                "不具合",
                "バグ",
                "修正",
                "直し",
                "直す",
            ],
        ),
        (
            "failing-test",
            &[
                "failing",
                "failure",
                "failed",
                "assertion",
                "test",
                "pytest",
                "テスト",
                "失敗",
                "落ちる",
                "通す",
            ],
        ),
        (
            "ui-route",
            &[
                "route",
                "page",
                "screen",
                "view",
                "component",
                "sveltekit",
                "svelte",
                "画面",
                "ページ",
                "ルート",
            ],
        ),
        (
            "run-script",
            &[
                "run",
                "execute",
                "script",
                "command",
                "summarize",
                "実行",
                "コマンド",
                "スクリプト",
                "要約",
            ],
        ),
        (
            "verify",
            &[
                "verify",
                "verifier",
                "validate",
                "check",
                "build",
                "テスト",
                "検証",
                "確認",
            ],
        ),
    ];
    for (canonical, terms) in groups {
        if terms.iter().any(|term| lower.contains(term)) {
            out.insert((*canonical).to_string());
        }
    }
    out
}

/// Jaccard over two `&str` iterators. Both empty → 0.0 (no signal).
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

/// Compute the 6-component score for one candidate.
fn score_case(current: &CaseRetrievalInputs<'_>, candidate: &CaseRecord) -> CaseScoreBreakdown {
    debug_assert!(
        (W_TASK + W_SEMANTIC + W_STACK + W_REPO + W_FILES + W_KIND + W_PRECAUTIONS - 1.0).abs()
            < 1e-6,
        "case_retrieval weights must sum to 1.0"
    );

    // Task token overlap (Jaccard, ASCII whitespace tokens).
    let cur_task = tokenize_ascii_lowercase(current.current_task_signature);
    let cand_task = tokenize_ascii_lowercase(&candidate.task_signature);
    let task = jaccard(
        cur_task.iter().map(String::as_str),
        cand_task.iter().map(String::as_str),
    );
    let cur_semantic = semantic_tokens(current.current_task_signature);
    let cand_semantic = semantic_tokens(&candidate.task_signature);
    let semantic = jaccard(
        cur_semantic.iter().map(String::as_str),
        cand_semantic.iter().map(String::as_str),
    );

    // Language stack: Jaccard over &[String].
    let stack = jaccard(
        current.current_language_stack.iter().map(String::as_str),
        candidate.language_stack.iter().map(String::as_str),
    );

    // Repo fingerprint: categorical.
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

    // Same files: Jaccard over basenames of changed files.
    let cur_basenames: HashSet<String> = current
        .current_touched_files
        .iter()
        .filter_map(|p| Path::new(p).file_name().and_then(|s| s.to_str()))
        .map(|s| s.to_ascii_lowercase())
        .collect();
    let cand_basenames: HashSet<String> = candidate
        .changed_files_summary
        .iter()
        .filter_map(|s| {
            // changed_files_summary entries look like "src/foo.rs (impl)" or the
            // header "<test:N impl:M ...>". Extract the file path before " (".
            if s.starts_with('<') {
                return None;
            }
            let path_part = s.split(" (").next().unwrap_or(s);
            Path::new(path_part)
                .file_name()
                .and_then(|os| os.to_str())
                .map(|n| n.to_ascii_lowercase())
        })
        .collect();
    let files = jaccard(
        cur_basenames.iter().map(String::as_str),
        cand_basenames.iter().map(String::as_str),
    );

    // FeedbackKind: categorical.
    let kind = match current.current_feedback_kind {
        Some(ref ck) if candidate.initial_feedback.contains(ck) => 1.0,
        Some(ref ck) => {
            let ck_eligible = ck.is_eligible_for_reminder();
            if candidate
                .initial_feedback
                .iter()
                .any(|cf| cf.is_eligible_for_reminder() == ck_eligible)
            {
                0.5
            } else {
                0.0
            }
        }
        None => 0.0,
    };

    // Precaution overlap: Jaccard over text tokens.
    let cur_prec_tokens: HashSet<String> = current
        .current_active_precautions
        .iter()
        .flat_map(|p| {
            tokenize_ascii_lowercase(&p.text)
                .into_iter()
                .collect::<Vec<_>>()
        })
        .collect();
    let cand_prec_tokens: HashSet<String> = candidate
        .successful_precautions
        .iter()
        .flat_map(|p| {
            tokenize_ascii_lowercase(&p.text)
                .into_iter()
                .collect::<Vec<_>>()
        })
        .collect();
    let precautions = jaccard(
        cur_prec_tokens.iter().map(String::as_str),
        cand_prec_tokens.iter().map(String::as_str),
    );

    let total = W_TASK * task
        + W_SEMANTIC * semantic
        + W_STACK * stack
        + W_REPO * repo
        + W_FILES * files
        + W_KIND * kind
        + W_PRECAUTIONS * precautions;

    CaseScoreBreakdown {
        case_id: candidate.case_id.clone(),
        task,
        semantic,
        stack,
        repo,
        files,
        kind,
        precautions,
        total,
    }
}

/// Render a single case as 1–3 lines with per-case char budget.
fn render_one_case(case: &SelectedCase, char_budget: usize) -> String {
    let task = truncate_chars(&case.record.task_signature, char_budget.saturating_sub(2));
    let mut out = String::new();
    out.push_str("- ");
    out.push_str(&task);

    // Files line (basenames CSV up to 60 chars).
    let basenames: Vec<&str> = case
        .record
        .changed_files_summary
        .iter()
        .filter(|s| !s.starts_with('<'))
        .filter_map(|s| {
            let path_part = s.split(" (").next().unwrap_or(s);
            Path::new(path_part).file_name().and_then(|os| os.to_str())
        })
        .take(4)
        .collect();
    if !basenames.is_empty() {
        out.push_str("\n  Files: ");
        let csv = basenames.join(", ");
        out.push_str(&truncate_chars(&csv, 60));
    }

    // Verify line.
    if let Some(verify) = case.record.verify_commands.first() {
        out.push_str("\n  Verify: ");
        out.push_str(&truncate_chars(verify, 60));
    }

    // Final per-case truncation (defensive, char-based).
    truncate_chars(&out, char_budget)
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
    use crate::session::anvil_score::AnvilScore;
    use crate::session::case_record::{CaseFileEntry, CaseRecord, RepoFingerprint};
    use crate::session::feedback::FeedbackKind;
    use crate::session::precaution::{PrecautionSource, Severity};

    // --- env gate -------------------------------------------------------

    #[test]
    fn env_gate_disabled_true_when_set() {
        assert!(case_retrieval_disabled(|_| Ok("1".to_string())));
    }
    #[test]
    fn env_gate_disabled_false_when_unset() {
        assert!(!case_retrieval_disabled(|_| Err(
            std::env::VarError::NotPresent
        )));
    }
    #[test]
    fn env_gate_disabled_false_when_empty_string() {
        assert!(!case_retrieval_disabled(|_| Ok(String::new())));
    }
    #[test]
    fn env_gate_dry_run_true_when_set() {
        assert!(case_retrieval_dry_run(|_| Ok("1".to_string())));
    }
    #[test]
    fn env_gate_dry_run_false_when_unset() {
        assert!(!case_retrieval_dry_run(|_| Err(
            std::env::VarError::NotPresent
        )));
    }

    // --- weights / jaccard / tokenize ----------------------------------

    #[test]
    fn weights_sum_to_one() {
        let s = W_TASK + W_SEMANTIC + W_STACK + W_REPO + W_FILES + W_KIND + W_PRECAUTIONS;
        assert!((s - 1.0).abs() < 1e-6, "sum was {s}");
    }

    #[test]
    fn jaccard_empty_both_returns_zero() {
        let empty: Vec<&str> = vec![];
        assert_eq!(jaccard(empty.iter().copied(), Vec::<&str>::new()), 0.0);
    }
    #[test]
    fn jaccard_identical_returns_one() {
        let a = ["x", "y"];
        let b = ["y", "x"];
        assert!((jaccard(a.iter().copied(), b.iter().copied()) - 1.0).abs() < 1e-6);
    }
    #[test]
    fn jaccard_partial_returns_intersection_over_union() {
        let a = ["x", "y", "z"];
        let b = ["y", "z", "w"];
        let v = jaccard(a.iter().copied(), b.iter().copied());
        assert!((v - (2.0 / 4.0)).abs() < 1e-6, "got {v}");
    }
    #[test]
    fn jaccard_dedup_via_set() {
        // Sets dedup automatically.
        let a = ["x", "x"];
        let b = ["x"];
        assert!((jaccard(a.iter().copied(), b.iter().copied()) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn tokenize_ascii_lowercase_basic() {
        let s = tokenize_ascii_lowercase("Hello World HELLO");
        assert!(s.contains("hello"));
        assert!(s.contains("world"));
        assert_eq!(s.len(), 2);
    }
    #[test]
    fn tokenize_ascii_lowercase_empty() {
        assert!(tokenize_ascii_lowercase("   ").is_empty());
    }

    #[test]
    fn semantic_tokens_bridge_japanese_and_english_bugfix_terms() {
        let japanese = semantic_tokens("失敗しているテストを通すために不具合を修正");
        let english = semantic_tokens("repair broken failing test bug");
        assert!(japanese.contains("bugfix"));
        assert!(japanese.contains("failing-test"));
        assert!(english.contains("bugfix"));
        assert!(english.contains("failing-test"));
        assert!(
            jaccard(
                japanese.iter().map(String::as_str),
                english.iter().map(String::as_str)
            ) > 0.0
        );
    }

    // --- score_case ----------------------------------------------------

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

    fn fake_record(case_id: &str, task: &str, ws: &str, stack_hash: &str) -> CaseRecord {
        CaseRecord {
            case_id: case_id.to_string(),
            created_at: 1000,
            repo_fingerprint: RepoFingerprint {
                workspace_key: ws.to_string(),
                git_remote: None,
                git_head_branch: None,
                language_stack_hash: stack_hash.to_string(),
            },
            task_signature: task.to_string(),
            language_stack: vec!["rust".to_string()],
            initial_feedback: vec![FeedbackKind::CompileError],
            successful_precautions: vec![],
            changed_files_summary: vec!["src/foo.rs (impl)".to_string()],
            verify_commands: vec!["cargo test".to_string()],
            outcome_score: fake_score(),
        }
    }

    #[test]
    fn score_repo_match_workspace_key_is_one() {
        let stack = vec!["rust".to_string()];
        let rec = fake_record("case_aaaaaaaaaaaaaaaaaaaa", "fix bug", "ws-A", "h1");
        let cur_fp = RepoFingerprint {
            workspace_key: "ws-A".to_string(),
            git_remote: None,
            git_head_branch: None,
            language_stack_hash: "h2".to_string(),
        };
        let inputs = CaseRetrievalInputs {
            current_task_signature: "unrelated",
            current_language_stack: &stack,
            current_repo_fingerprint: &cur_fp,
            current_touched_files: &[],
            current_feedback_kind: None,
            current_active_precautions: &[],
        };
        let b = score_case(&inputs, &rec);
        assert_eq!(b.repo, 1.0);
    }

    #[test]
    fn score_repo_match_language_stack_hash_only_is_half() {
        let stack = vec!["rust".to_string()];
        let rec = fake_record("case_aaaaaaaaaaaaaaaaaaaa", "x", "ws-A", "h-same");
        let cur_fp = RepoFingerprint {
            workspace_key: "ws-B".to_string(),
            git_remote: None,
            git_head_branch: None,
            language_stack_hash: "h-same".to_string(),
        };
        let inputs = CaseRetrievalInputs {
            current_task_signature: "x",
            current_language_stack: &stack,
            current_repo_fingerprint: &cur_fp,
            current_touched_files: &[],
            current_feedback_kind: None,
            current_active_precautions: &[],
        };
        assert_eq!(score_case(&inputs, &rec).repo, 0.5);
    }

    #[test]
    fn score_repo_no_match_is_zero() {
        let stack = vec!["rust".to_string()];
        let rec = fake_record("case_aaaaaaaaaaaaaaaaaaaa", "x", "ws-A", "h1");
        let cur_fp = RepoFingerprint {
            workspace_key: "ws-B".to_string(),
            git_remote: None,
            git_head_branch: None,
            language_stack_hash: "h2".to_string(),
        };
        let inputs = CaseRetrievalInputs {
            current_task_signature: "x",
            current_language_stack: &stack,
            current_repo_fingerprint: &cur_fp,
            current_touched_files: &[],
            current_feedback_kind: None,
            current_active_precautions: &[],
        };
        assert_eq!(score_case(&inputs, &rec).repo, 0.0);
    }

    #[test]
    fn score_kind_exact_match_is_one() {
        let stack = vec!["rust".to_string()];
        let rec = fake_record("case_aaaaaaaaaaaaaaaaaaaa", "x", "ws-A", "h");
        let cur_fp = RepoFingerprint {
            workspace_key: "ws-A".to_string(),
            git_remote: None,
            git_head_branch: None,
            language_stack_hash: "h".to_string(),
        };
        let inputs = CaseRetrievalInputs {
            current_task_signature: "x",
            current_language_stack: &stack,
            current_repo_fingerprint: &cur_fp,
            current_touched_files: &[],
            current_feedback_kind: Some(FeedbackKind::CompileError),
            current_active_precautions: &[],
        };
        assert_eq!(score_case(&inputs, &rec).kind, 1.0);
    }

    #[test]
    fn score_kind_same_family_is_half() {
        // CompileError and TypeError are both eligible-for-reminder.
        let stack = vec!["rust".to_string()];
        let mut rec = fake_record("case_aaaaaaaaaaaaaaaaaaaa", "x", "ws-A", "h");
        rec.initial_feedback = vec![FeedbackKind::TypeError];
        let cur_fp = RepoFingerprint {
            workspace_key: "ws-A".to_string(),
            git_remote: None,
            git_head_branch: None,
            language_stack_hash: "h".to_string(),
        };
        let inputs = CaseRetrievalInputs {
            current_task_signature: "x",
            current_language_stack: &stack,
            current_repo_fingerprint: &cur_fp,
            current_touched_files: &[],
            current_feedback_kind: Some(FeedbackKind::CompileError),
            current_active_precautions: &[],
        };
        assert_eq!(score_case(&inputs, &rec).kind, 0.5);
    }

    #[test]
    fn score_kind_no_signal_when_current_none() {
        let stack = vec!["rust".to_string()];
        let rec = fake_record("case_aaaaaaaaaaaaaaaaaaaa", "x", "ws-A", "h");
        let cur_fp = RepoFingerprint {
            workspace_key: "ws-A".to_string(),
            git_remote: None,
            git_head_branch: None,
            language_stack_hash: "h".to_string(),
        };
        let inputs = CaseRetrievalInputs {
            current_task_signature: "x",
            current_language_stack: &stack,
            current_repo_fingerprint: &cur_fp,
            current_touched_files: &[],
            current_feedback_kind: None,
            current_active_precautions: &[],
        };
        assert_eq!(score_case(&inputs, &rec).kind, 0.0);
    }

    #[test]
    fn score_total_in_unit_range() {
        let stack = vec!["rust".to_string()];
        let rec = fake_record("case_aaaaaaaaaaaaaaaaaaaa", "fix the bug", "ws-A", "h");
        let cur_fp = RepoFingerprint {
            workspace_key: "ws-A".to_string(),
            git_remote: None,
            git_head_branch: None,
            language_stack_hash: "h".to_string(),
        };
        let inputs = CaseRetrievalInputs {
            current_task_signature: "fix the bug",
            current_language_stack: &stack,
            current_repo_fingerprint: &cur_fp,
            current_touched_files: &[],
            current_feedback_kind: Some(FeedbackKind::CompileError),
            current_active_precautions: &[],
        };
        let b = score_case(&inputs, &rec);
        assert!(b.total >= 0.0 && b.total <= 1.0);
    }

    #[test]
    fn score_semantic_match_when_lexical_overlap_is_low() {
        let stack = vec!["rust".to_string()];
        let rec = fake_record(
            "case_aaaaaaaaaaaaaaaaaaaa",
            "repair broken failing test bug",
            "ws-X",
            "h",
        );
        let cur_fp = RepoFingerprint {
            workspace_key: "ws-Y".to_string(),
            git_remote: None,
            git_head_branch: None,
            language_stack_hash: "h".to_string(),
        };
        let inputs = CaseRetrievalInputs {
            current_task_signature: "失敗しているテストを通すために不具合を修正",
            current_language_stack: &stack,
            current_repo_fingerprint: &cur_fp,
            current_touched_files: &[],
            current_feedback_kind: Some(FeedbackKind::CompileError),
            current_active_precautions: &[],
        };
        let b = score_case(&inputs, &rec);
        assert_eq!(b.task, 0.0);
        assert!(
            b.semantic > 0.0,
            "semantic score should bridge wording: {b:?}"
        );
    }

    // --- format_for_prompt ---------------------------------------------

    fn fake_selected(case_id: &str, task: &str, total: f32) -> SelectedCase {
        SelectedCase {
            record: fake_record(case_id, task, "ws-A", "h"),
            breakdown: CaseScoreBreakdown {
                case_id: case_id.to_string(),
                task: 1.0,
                semantic: 1.0,
                stack: 1.0,
                repo: 1.0,
                files: 0.0,
                kind: 0.0,
                precautions: 0.0,
                total,
            },
        }
    }

    #[test]
    fn format_for_prompt_empty_returns_none() {
        assert!(format_for_prompt(&[]).is_none());
    }

    #[test]
    fn format_for_prompt_single_includes_header() {
        let v = vec![fake_selected("case_aaaaaaaaaaaaaaaaaaaa", "fix bug", 0.6)];
        let out = format_for_prompt(&v).expect("section");
        assert!(out.starts_with("Relevant Local Cases:\n"));
        assert!(out.contains("fix bug"));
    }

    #[test]
    fn format_for_prompt_total_chars_within_cap() {
        let v: Vec<SelectedCase> = (0..5)
            .map(|i| fake_selected(&format!("case_{:0<20}", i), &"a".repeat(300), 0.5))
            .collect();
        let out = format_for_prompt(&v).expect("section");
        assert!(out.chars().count() <= MAX_CASE_RENDERED_CHARS_TOTAL);
    }

    #[test]
    fn format_for_prompt_masks_unmasked_secret_in_record() {
        // Inject something that mask_secrets would scrub. mask_secrets is a
        // best-effort mask; this verifies the renderer pipes through it.
        let mut sel = fake_selected("case_aaaaaaaaaaaaaaaaaaaa", "task", 0.5);
        sel.record.task_signature = "AKIAIOSFODNN7EXAMPLE secret".to_string();
        let out = format_for_prompt(std::slice::from_ref(&sel)).expect("section");
        // Verify the literal AWS-key shape is masked away.
        assert!(
            !out.contains("AKIAIOSFODNN7EXAMPLE"),
            "renderer must apply mask_secrets; got: {out}"
        );
    }

    // --- retrieve_with_iter --------------------------------------------

    fn write_record(dir: &Path, rec: &CaseRecord) -> std::io::Result<CaseFileEntry> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join(format!("{}.json", rec.case_id));
        std::fs::write(&path, serde_json::to_vec_pretty(rec).unwrap())?;
        Ok(CaseFileEntry {
            case_id: rec.case_id.clone(),
            path,
            created_at: rec.created_at,
            size: 0,
        })
    }

    fn cur_fp_match() -> RepoFingerprint {
        RepoFingerprint {
            workspace_key: "ws-A".to_string(),
            git_remote: None,
            git_head_branch: None,
            language_stack_hash: "h".to_string(),
        }
    }

    #[test]
    fn retrieve_no_candidates_returns_skipped() {
        let stack = vec!["rust".to_string()];
        let fp = cur_fp_match();
        let inputs = CaseRetrievalInputs {
            current_task_signature: "anything",
            current_language_stack: &stack,
            current_repo_fingerprint: &fp,
            current_touched_files: &[],
            current_feedback_kind: None,
            current_active_precautions: &[],
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
    fn retrieve_iter_failure_propagates_as_err() {
        let stack = vec!["rust".to_string()];
        let fp = cur_fp_match();
        let inputs = CaseRetrievalInputs {
            current_task_signature: "x",
            current_language_stack: &stack,
            current_repo_fingerprint: &fp,
            current_touched_files: &[],
            current_feedback_kind: None,
            current_active_precautions: &[],
        };
        let out = retrieve_with_iter(&inputs, false, || Err("inj".to_string()));
        assert!(out.is_err());
    }

    #[test]
    fn retrieve_below_threshold_returns_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("cases");
        let rec = fake_record(
            "case_aaaaaaaaaaaaaaaaaaaa",
            "completely different things",
            "ws-X",
            "h-other",
        );
        let entry = write_record(&dir, &rec).unwrap();
        let stack = vec!["rust".to_string()];
        let fp = cur_fp_match();
        let inputs = CaseRetrievalInputs {
            current_task_signature: "alpha bravo charlie",
            current_language_stack: &stack,
            current_repo_fingerprint: &fp,
            current_touched_files: &[],
            current_feedback_kind: None,
            current_active_precautions: &[],
        };
        let out = retrieve_with_iter(&inputs, false, || Ok(vec![entry])).unwrap();
        match out {
            RetrievalOutcome::Skipped { reason, .. } => {
                assert_eq!(reason, SkipReason::BelowThreshold);
            }
            _ => panic!("expected Skipped(BelowThreshold)"),
        }
    }

    #[test]
    fn retrieve_above_threshold_returns_completed_capped_at_max() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("cases");
        let mut entries = Vec::new();
        for i in 0..(MAX_SELECTED_CASES + 2) {
            let id = format!("case_{:a<20}", i);
            let rec = fake_record(&id, "fix bug now", "ws-A", "h");
            let mut e = write_record(&dir, &rec).unwrap();
            e.created_at = 1000 + i as u64;
            entries.push(e);
        }
        let stack = vec!["rust".to_string()];
        let fp = cur_fp_match();
        let inputs = CaseRetrievalInputs {
            current_task_signature: "fix bug now",
            current_language_stack: &stack,
            current_repo_fingerprint: &fp,
            current_touched_files: &[],
            current_feedback_kind: None,
            current_active_precautions: &[],
        };
        let out = retrieve_with_iter(&inputs, false, || Ok(entries)).unwrap();
        match out {
            RetrievalOutcome::Completed {
                selected,
                candidate_count,
                ..
            } => {
                assert_eq!(selected.len(), MAX_SELECTED_CASES);
                assert!(candidate_count >= MAX_SELECTED_CASES);
            }
            _ => panic!("expected Completed"),
        }
    }

    #[test]
    fn retrieve_dry_run_returns_skipped_dry_run() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("cases");
        let rec = fake_record("case_aaaaaaaaaaaaaaaaaaaa", "fix bug now", "ws-A", "h");
        let entry = write_record(&dir, &rec).unwrap();
        let stack = vec!["rust".to_string()];
        let fp = cur_fp_match();
        let inputs = CaseRetrievalInputs {
            current_task_signature: "fix bug now",
            current_language_stack: &stack,
            current_repo_fingerprint: &fp,
            current_touched_files: &[],
            current_feedback_kind: None,
            current_active_precautions: &[],
        };
        let out = retrieve_with_iter(&inputs, true, || Ok(vec![entry])).unwrap();
        match out {
            RetrievalOutcome::Skipped { reason, .. } => {
                assert_eq!(reason, SkipReason::DryRun);
            }
            _ => panic!("expected Skipped(DryRun)"),
        }
    }

    #[test]
    fn retrieve_corrupt_file_increments_skipped_corrupt_count() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("cases");
        std::fs::create_dir_all(&dir).unwrap();
        // 1 valid record + 1 corrupt file.
        let rec = fake_record("case_aaaaaaaaaaaaaaaaaaaa", "fix bug now", "ws-A", "h");
        let valid_entry = write_record(&dir, &rec).unwrap();
        let bad_path = dir.join("case_bbbbbbbbbbbbbbbbbbbb.json");
        std::fs::write(&bad_path, b"{ not json").unwrap();
        let bad_entry = CaseFileEntry {
            case_id: "case_bbbbbbbbbbbbbbbbbbbb".to_string(),
            path: bad_path,
            created_at: 999,
            size: 10,
        };
        let stack = vec!["rust".to_string()];
        let fp = cur_fp_match();
        let inputs = CaseRetrievalInputs {
            current_task_signature: "fix bug now",
            current_language_stack: &stack,
            current_repo_fingerprint: &fp,
            current_touched_files: &[],
            current_feedback_kind: None,
            current_active_precautions: &[],
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
            other => panic!("expected Completed with 1 skipped, got {:?}", other),
        }
    }

    #[test]
    fn retrieve_tie_break_prefers_newer_created_at() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("cases");
        let mut older = fake_record("case_oooooooooooooooooooo", "fix bug now", "ws-A", "h");
        older.created_at = 100;
        let mut newer = fake_record("case_nnnnnnnnnnnnnnnnnnnn", "fix bug now", "ws-A", "h");
        newer.created_at = 200;
        let mut e_old = write_record(&dir, &older).unwrap();
        e_old.created_at = older.created_at;
        let mut e_new = write_record(&dir, &newer).unwrap();
        e_new.created_at = newer.created_at;
        let stack = vec!["rust".to_string()];
        let fp = cur_fp_match();
        let inputs = CaseRetrievalInputs {
            current_task_signature: "fix bug now",
            current_language_stack: &stack,
            current_repo_fingerprint: &fp,
            current_touched_files: &[],
            current_feedback_kind: None,
            current_active_precautions: &[],
        };
        let out = retrieve_with_iter(&inputs, false, || Ok(vec![e_old, e_new])).unwrap();
        match out {
            RetrievalOutcome::Completed { selected, .. } => {
                assert_eq!(
                    selected.first().unwrap().record.case_id,
                    "case_nnnnnnnnnnnnnnnnnnnn",
                    "newer should rank first on score tie"
                );
            }
            _ => panic!("expected Completed"),
        }
    }

    #[test]
    fn retrieve_semantic_rerank_prefers_synonym_match_over_misleading_lexical_overlap() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("cases");
        let mut semantic = fake_record(
            "case_semanticmatch00000",
            "repair broken failing test bug",
            "ws-A",
            "h",
        );
        semantic.created_at = 100;
        let mut misleading = fake_record(
            "case_misleading0000000",
            "alpha bravo charlie docs",
            "ws-A",
            "h",
        );
        misleading.created_at = 200;
        let e_semantic = write_record(&dir, &semantic).unwrap();
        let e_misleading = write_record(&dir, &misleading).unwrap();
        let stack = vec!["rust".to_string()];
        let fp = cur_fp_match();
        let inputs = CaseRetrievalInputs {
            current_task_signature: "失敗しているテストを通すために不具合を修正 alpha",
            current_language_stack: &stack,
            current_repo_fingerprint: &fp,
            current_touched_files: &[],
            current_feedback_kind: Some(FeedbackKind::CompileError),
            current_active_precautions: &[],
        };
        let out =
            retrieve_with_iter(&inputs, false, || Ok(vec![e_misleading, e_semantic])).unwrap();
        match out {
            RetrievalOutcome::Completed { selected, .. } => {
                assert_eq!(selected[0].record.case_id, "case_semanticmatch00000");
                assert!(selected[0].breakdown.semantic > selected[1].breakdown.semantic);
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }

    // --- Severity / PrecautionSource use to silence dead-import warnings ----
    #[test]
    fn snapshot_construction_matches_text() {
        let _ = (Severity::High, PrecautionSource::BuildFailure);
    }
}
