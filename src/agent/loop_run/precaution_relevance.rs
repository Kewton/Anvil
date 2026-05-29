//! Issue #453: per-prompt precaution selection pipeline extracted
//! from `turn.rs`.
//!
//! Hosts:
//!
//! * `select_precautions_for_prompt` — the public entry point
//!   re-exported via `loop_run.rs::pub use precaution_relevance::...`
//!   so integration tests + `commands.rs` keep their existing import
//!   paths.
//! * `normalize_relevance_key` / `relevance_keyset_from_touched` /
//!   `relevance_keyset_from_suspected` — key normalization +
//!   construction helpers (Issue #453 DR1-001).
//! * `relevance_score` — Codex CB-001 fix: suspected > touched >
//!   global > unrelated within a severity bucket.
//! * `sort_precautions_for_prompt` — stable sort by `severity_order`
//!   then `relevance_score` (descending).
//! * `apply_budget_caps` — hard cap on count
//!   (`MAX_ACTIVE_PRECAUTIONS_PROMPT`) + soft cap on cumulative
//!   `chars()` (`MAX_ACTIVE_PRECAUTIONS_CHARS`), bypassed once for
//!   the first oversized item (DR1-002).
//!
//! All helpers are intentionally free functions (not `Agent` methods)
//! so they unit-test on plain slices and values (DR2-002). No facade
//! re-export of the internal helpers (DR3-001) — only the public
//! `select_precautions_for_prompt` is re-exported, matching the
//! pre-extraction surface.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::modes::plan_act::ExecutionMode;
use crate::session::precaution::{Precaution, PrecautionStatus, severity_order};
use crate::session::store::WorkingMemory;

/// Normalize one path-like string (an `applies_to` entry, suspected file, or
/// already-normalized `WorkingMemory.touched_files` entry) into the canonical
/// key form used by the relevance set lookup. Idempotent for already
/// forward-slash-only paths (Issue #453 DR1-001).
#[must_use]
pub(super) fn normalize_relevance_key(s: &str) -> String {
    s.replace('\\', "/")
}

/// Build a `HashSet<String>` of normalized keys from `WorkingMemory.touched_files`
/// (already produced by `normalize_memory_path`). Used by
/// `select_precautions_for_prompt` for O(1) relevance lookup (Issue #453).
#[must_use]
pub(super) fn relevance_keyset_from_touched(items: &[String]) -> HashSet<String> {
    items.iter().map(|s| normalize_relevance_key(s)).collect()
}

/// Build a `HashSet<String>` of normalized keys from
/// `FeedbackFrame.suspected_files`. Projects each `PathBuf` via
/// `to_string_lossy()` + slash normalization so the result matches the same
/// key format as `relevance_keyset_from_touched` (Issue #453 DR1-001).
#[must_use]
pub(super) fn relevance_keyset_from_suspected(paths: &[PathBuf]) -> HashSet<String> {
    paths
        .iter()
        .map(|p| normalize_relevance_key(&p.to_string_lossy()))
        .collect()
}

/// Compute a relevance score for a single precaution against the per-turn
/// touched / suspected keysets (Issue #453 DR1-001).
///
/// Order (Codex CB-001 fix): suspected > touched > global > unrelated, so a
/// path-scoped precaution that matches the current turn always outranks a
/// broad global one within the same severity bucket.
///
/// * `3`: any `applies_to` entry hits `suspected_files` (highest priority).
/// * `2`: any `applies_to` entry hits only `touched_files`.
/// * `1`: `applies_to` is empty (treated as a global precaution; sorted ahead
///   of unrelated path-scoped ones to keep the user's broad guidance visible).
/// * `0`: path-scoped but unrelated to current turn.
#[must_use]
pub(super) fn relevance_score(
    p: &Precaution,
    touched: &HashSet<String>,
    suspected: &HashSet<String>,
) -> u8 {
    if p.applies_to.is_empty() {
        return 1;
    }
    let mut best = 0u8;
    for path in &p.applies_to {
        let key = normalize_relevance_key(&path.to_string_lossy());
        if suspected.contains(&key) {
            return 3;
        }
        if touched.contains(&key) {
            best = best.max(2);
        }
    }
    best
}

/// Apply the per-prompt budget caps (Issue #453):
/// * hard cap: at most `MAX_ACTIVE_PRECAUTIONS_PROMPT` items;
/// * soft cap: cumulative `chars().count()` of bullet lines must not exceed
///   `MAX_ACTIVE_PRECAUTIONS_CHARS`. The soft cap is bypassed for the first
///   item so a single oversized precaution is still emitted (DR1-002).
///
/// Per-line length is computed as the exact bullet `format!("- [{label}] {text}")`
/// `chars().count()`, where `label` is `Severity::as_label()`.
#[must_use]
pub(super) fn apply_budget_caps(sorted: Vec<&Precaution>) -> Vec<Precaution> {
    let mut chosen: Vec<Precaution> =
        Vec::with_capacity(WorkingMemory::MAX_ACTIVE_PRECAUTIONS_PROMPT);
    let mut chars_total: usize = 0;
    for p in sorted {
        if chosen.len() >= WorkingMemory::MAX_ACTIVE_PRECAUTIONS_PROMPT {
            break;
        }
        let line_len = "- [".chars().count()
            + p.severity.as_label().chars().count()
            + "] ".chars().count()
            + p.text.chars().count();
        if chars_total + line_len > WorkingMemory::MAX_ACTIVE_PRECAUTIONS_CHARS
            && !chosen.is_empty()
        {
            break;
        }
        chars_total += line_len;
        chosen.push(p.clone());
    }
    chosen
}

/// Select precautions to inject into the Act-mode prompt (Issue #453).
///
/// Pipeline:
///   1. Plan-mode short-circuit -> `Vec::new()` (design judgment #2).
///   2. Active-only filter (defense-in-depth; the renderer re-applies it).
///   3. Stable sort by `severity_order` ascending, then `relevance_score`
///      descending. Stable sort preserves insertion order within ties.
///   4. Budget caps via `apply_budget_caps` (N = 8, M = 1024 chars).
///
/// The function is intentionally a free function (rather than an `Agent`
/// method) so it can be unit-tested with plain slices and values, with no
/// `Agent` fixture (DR2-002).
///
/// # Invariant (CB-002)
///
/// Callers MUST pass `Precaution`s that already went through
/// [`crate::session::store::WorkingMemory::add_precaution`] (or the load-time
/// [`crate::session::store::WorkingMemory::sanitize_active_precautions_after_load`]
/// pass). Those entry points apply secret masking, text truncation, and
/// workspace-relative `applies_to` canonicalization. Passing raw `Precaution`
/// values built outside that pipeline can leak unmasked secrets into prompts
/// and `llm-io.jsonl` and bypass the size/path bounds the renderer assumes.
#[must_use]
pub fn select_precautions_for_prompt(
    active_precautions: &[Precaution],
    mode: ExecutionMode,
    touched_files: &[String],
    suspected_files: Option<&[PathBuf]>,
) -> Vec<Precaution> {
    if mode == ExecutionMode::Plan {
        return Vec::new();
    }

    let active: Vec<&Precaution> = active_precautions
        .iter()
        .filter(|p| p.status == PrecautionStatus::Active)
        .collect();
    if active.is_empty() {
        return Vec::new();
    }

    let touched_set = relevance_keyset_from_touched(touched_files);
    let suspected_set = suspected_files
        .map(relevance_keyset_from_suspected)
        .unwrap_or_default();

    let sorted = sort_precautions_for_prompt(active, &touched_set, &suspected_set);
    apply_budget_caps(sorted)
}

/// Stable sort: primary key is `severity_order` ascending (High first),
/// secondary key is `relevance_score` descending so suspected > touched >
/// global > unrelated within the same severity bucket. Stable sort preserves
/// the original insertion order within identical (severity, relevance) ties
/// (Issue #453 AC: severity 同点時は applies_to 関連度優先 → 残りは insertion order).
#[must_use]
pub(super) fn sort_precautions_for_prompt<'a>(
    mut active: Vec<&'a Precaution>,
    touched: &HashSet<String>,
    suspected: &HashSet<String>,
) -> Vec<&'a Precaution> {
    active.sort_by(|a, b| {
        let by_severity = severity_order(a.severity).cmp(&severity_order(b.severity));
        if by_severity != std::cmp::Ordering::Equal {
            return by_severity;
        }
        relevance_score(b, touched, suspected).cmp(&relevance_score(a, touched, suspected))
    });
    active
}
