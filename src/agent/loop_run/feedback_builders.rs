//! `FeedbackFrame` builder helpers + path-extraction utilities used
//! by the bash / edit failure pipelines extracted from `turn.rs`
//! (parent #680).
//!
//! Hosts:
//!
//! * `build_feedback_for_bash` — `BashExecutionOutcome` →
//!   `Option<FeedbackFrame>` (None for successful non-test commands).
//! * `bash_outcome_primary_error` — picks the most informative single
//!   line for `primary_error` (blocked_reason / timeout / interrupt /
//!   first non-empty stderr/stdout line).
//! * `build_feedback_for_unsafe_block_reason` — typed
//!   `UnsafeCommandBlocked` frame (Issue #461 / DR4-004: rendered
//!   block reason in `primary_error`, raw command in `command` only).
//! * `build_feedback_for_edit_failure` — Edit tool Err branch (CB-001).
//! * `extract_suspected_files_from_text` — best-effort heuristic to
//!   pull file paths out of compiler / test output (cap 8).
//! * `extract_path_tokens_from_text` — workspace-relative path
//!   projection for prompt rendering.
//! * `extract_current_request_paths` — caps to
//!   `prompting::MAX_CURRENT_REQUEST_PATHS`.
//!
//! `pub(super)` limited / no facade re-export (DR3-001).
//! `turn.rs` is the only in-crate consumer.

use std::path::{Path, PathBuf};

use super::Agent;
use super::auto_test::{AutoTestPlan, AutoTestResult, classify_auto_test};
use super::verifier_repair_targeting::extract_path_like_tokens;
use crate::agent::prompting;
use crate::safety::path_guard::resolve_user_path;
use crate::session::feedback::{
    FeedbackFrame, FeedbackFrameDraft, FeedbackKind, build_feedback_frame,
};
use crate::tools::bash::{BashExecutionOutcome, classify_bash_outcome};

/// Issue #450: build a FeedbackFrame from an `AutoTestPlan` /
/// `AutoTestResult` pair. Calls `classify_auto_test` to pick the
/// `FeedbackKind`, picks the first non-empty stderr/stdout line for
/// `primary_error` on failure, and runs the resulting draft through
/// `build_feedback_frame` for truncation / secret masking / path
/// normalization (per design 5.4: no helper performs those steps
/// itself). Used by `verifier_skill.rs` / `verifier_orchestration.rs`
/// callers in addition to turn-internal sites.
pub(super) fn build_feedback_for_auto_test(
    plan: &AutoTestPlan,
    result: &AutoTestResult,
    workspace_root: &Path,
    changed_files: &[String],
) -> FeedbackFrame {
    let kind = classify_auto_test(plan, result);
    let primary_error = if !result.passed {
        result
            .stderr
            .lines()
            .chain(result.stdout.lines())
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(|s| s.to_string())
    } else {
        None
    };
    let suspected_files: Vec<PathBuf> = if !result.passed {
        extract_suspected_files_from_text(&result.stdout, &result.stderr)
    } else {
        Vec::new()
    };
    let changed_files: Vec<PathBuf> = changed_files.iter().map(PathBuf::from).collect();
    let draft = FeedbackFrameDraft {
        command: Some(plan.command.clone()),
        exit_code: result.exit_code,
        kind,
        stdout: result.stdout.clone(),
        stderr: result.stderr.clone(),
        primary_error,
        suspected_files,
        changed_files,
    };
    build_feedback_frame(draft, workspace_root)
}

/// CB-001: build a FeedbackFrame from a `BashExecutionOutcome`. Uses the
/// pure `classify_bash_outcome` helper (Timeout / UnsafeCommandBlocked /
/// exit code != 0). Returns None for an outcome that is not a failure
/// case the FeedbackFrame represents (i.e. successful exit_code=0
/// non-test command — we do not want to spam last_feedback for every
/// successful `pwd` / `ls`).
pub(super) fn build_feedback_for_bash(
    outcome: &BashExecutionOutcome,
    workspace_root: &Path,
) -> Option<FeedbackFrame> {
    if !outcome.is_failure() {
        return None;
    }
    let kind = classify_bash_outcome(outcome);
    let primary_error = bash_outcome_primary_error(outcome);
    let draft = FeedbackFrameDraft {
        command: Some(outcome.command.clone()),
        exit_code: outcome.exit_code,
        kind,
        stdout: outcome.stdout.clone(),
        stderr: outcome.stderr.clone(),
        primary_error,
        suspected_files: extract_suspected_files_from_text(&outcome.stdout, &outcome.stderr),
        changed_files: Vec::new(),
    };
    Some(build_feedback_frame(draft, workspace_root))
}

pub(super) fn bash_outcome_primary_error(outcome: &BashExecutionOutcome) -> Option<String> {
    if let Some(reason) = &outcome.blocked_reason {
        return Some(reason.clone());
    }
    if outcome.timed_out {
        return Some("bash command timed out".to_string());
    }
    if outcome.interrupted {
        return Some("bash command interrupted by user".to_string());
    }
    // Failed exit code: take the first non-empty trimmed line.
    outcome
        .stderr
        .lines()
        .chain(outcome.stdout.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|s| s.to_string())
}

/// Issue #461 / DR4-004: build an `UnsafeCommandBlocked` FeedbackFrame
/// from a typed block reason (the new `bash::check_blocked_command`
/// preflight path). The `primary_error` deliberately contains only the
/// rendered block reason — never the raw command — so that the
/// Reminder Sidecar prompt cannot become a vector for prompt injection
/// from blocked-command text. The `command` field still holds the
/// (mask-applied, byte-capped) raw command so the user can see what was
/// rejected, but Sidecar code paths read `primary_error` rather than
/// `command`.
pub(super) fn build_feedback_for_unsafe_block_reason(
    command: &str,
    rendered_reason: &str,
    workspace_root: &Path,
) -> FeedbackFrame {
    let draft = FeedbackFrameDraft {
        kind: FeedbackKind::UnsafeCommandBlocked,
        command: Some(command.to_string()),
        primary_error: Some(rendered_reason.to_string()),
        ..Default::default()
    };
    build_feedback_frame(draft, workspace_root)
}

/// CB-001: build a FeedbackFrame for an Edit tool Err return. The
/// command isn't a shell command so we use the raw error message as
/// `primary_error` and stash the path token as `suspected_files`.
pub(super) fn build_feedback_for_edit_failure(
    path: Option<&str>,
    err: &str,
    workspace_root: &Path,
) -> FeedbackFrame {
    let suspected = path.map(|p| vec![PathBuf::from(p)]).unwrap_or_default();
    let draft = FeedbackFrameDraft {
        kind: FeedbackKind::EditFailure,
        primary_error: Some(err.to_string()),
        suspected_files: suspected,
        ..Default::default()
    };
    build_feedback_frame(draft, workspace_root)
}

/// Heuristic: pull file paths out of compiler / test output. Not exhaustive
/// — we only need a best-effort `suspected_files` list, and the path
/// normalizer drops anything that does not look real.
pub(super) fn extract_suspected_files_from_text(stdout: &str, stderr: &str) -> Vec<PathBuf> {
    let mut out = Vec::<PathBuf>::new();
    for line in stdout.lines().chain(stderr.lines()) {
        for trimmed in extract_path_like_tokens(line) {
            if !out.iter().any(|p| p.to_string_lossy() == trimmed) {
                out.push(PathBuf::from(trimmed));
            }
            if out.len() >= 8 {
                return out;
            }
        }
    }
    out
}

pub(super) fn extract_path_tokens_from_text(text: &str, work_root: &Path) -> Vec<String> {
    let canonical_root = work_root.canonicalize().ok();
    let mut out = Vec::new();
    for token in extract_path_like_tokens(text) {
        if let Ok(resolved) = resolve_user_path(work_root, token) {
            let rel = if let Some(root) = &canonical_root {
                resolved
                    .strip_prefix(root)
                    .ok()
                    .map(|r| r.to_string_lossy().replace('\\', "/"))
            } else {
                resolved
                    .strip_prefix(work_root)
                    .ok()
                    .map(|r| r.to_string_lossy().replace('\\', "/"))
            };
            if let Some(rel_str) = rel
                && !out.contains(&rel_str)
            {
                out.push(rel_str);
            }
        }
    }
    out
}

pub(super) fn extract_current_request_paths(agent: &Agent, work_root: &Path) -> Vec<String> {
    let mut out = Vec::new();

    if let Some(text) = agent.active_request_text() {
        for p in extract_path_tokens_from_text(&text, work_root) {
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    if let Some(target) = agent.focused_edit_recovery_target() {
        let canonical_root = work_root.canonicalize().ok();
        let rel = if let Some(root) = &canonical_root {
            target
                .strip_prefix(root)
                .ok()
                .map(|r| r.to_string_lossy().replace('\\', "/"))
        } else {
            target
                .strip_prefix(work_root)
                .ok()
                .map(|r| r.to_string_lossy().replace('\\', "/"))
        };
        if let Some(rel_str) = rel
            && !out.contains(&rel_str)
        {
            out.push(rel_str);
        }
    }
    out.truncate(prompting::MAX_CURRENT_REQUEST_PATHS);
    out
}
