#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExitReason {
    Done,
    MaxIterations,
    EmptyResponses,
    NoToolCalls,
    MissingRepoEdits,
    MissingVerification,
    VerifierFailed,
    PlanIncomplete,
    ToolCallFormatError,
    TransportError,
    Interrupted,
    /// Issue #651 Task 4.2: `CompletionDecision::SafeStop` with the
    /// `VerifierWeak` reason — a structurally runnable verifier was
    /// detected, but the current task's owned test artifact could not
    /// be bound to its argv. The agent stops without claiming Done so
    /// we never report false-positive completion. Distinct from
    /// `VerifierFailed` (verifier ran and the suite failed) and from
    /// `MissingVerification` (no verifier evidence was recorded yet).
    SafeStopVerifierWeak,
    /// Issue #651 Task 4.2: `CompletionDecision::SafeStop` with the
    /// `VerifierMissing` reason — either no allowlisted runner was
    /// detected at all, or the request literally asked for tests but
    /// no owned test artifact is staged on the verifier command line.
    SafeStopVerifierMissing,
}

impl ExitReason {
    pub(super) fn is_success(self) -> bool {
        matches!(self, ExitReason::Done)
    }

    pub(super) fn keeps_repl_alive(self) -> bool {
        matches!(
            self,
            ExitReason::MaxIterations
                | ExitReason::EmptyResponses
                | ExitReason::NoToolCalls
                | ExitReason::MissingRepoEdits
                | ExitReason::MissingVerification
                | ExitReason::VerifierFailed
                | ExitReason::PlanIncomplete
                | ExitReason::ToolCallFormatError
                | ExitReason::Interrupted
                | ExitReason::SafeStopVerifierWeak
                | ExitReason::SafeStopVerifierMissing
        )
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            ExitReason::Done => "done",
            ExitReason::MaxIterations => "max_iterations",
            ExitReason::EmptyResponses => "empty_responses",
            ExitReason::NoToolCalls => "no_tool_calls",
            ExitReason::MissingRepoEdits => "missing_repo_edits",
            ExitReason::MissingVerification => "missing_verification",
            ExitReason::VerifierFailed => "verifier_failed",
            ExitReason::PlanIncomplete => "plan_incomplete",
            ExitReason::ToolCallFormatError => "tool_call_format_error",
            ExitReason::TransportError => "transport_error",
            ExitReason::Interrupted => "interrupted",
            ExitReason::SafeStopVerifierWeak => "safe_stop_verifier_weak",
            ExitReason::SafeStopVerifierMissing => "safe_stop_verifier_missing",
        }
    }

    pub(super) fn default_error_text(self) -> &'static str {
        match self {
            ExitReason::Done => "",
            ExitReason::MaxIterations => "assistant did not finish within max iterations",
            ExitReason::EmptyResponses => "assistant returned empty responses repeatedly",
            ExitReason::NoToolCalls => {
                "assistant kept describing actions without using tools to perform them"
            }
            ExitReason::MissingRepoEdits => {
                "assistant kept stopping before making the requested repository edits"
            }
            ExitReason::MissingVerification => {
                "assistant completed repository artifacts but did not obtain required verification"
            }
            ExitReason::VerifierFailed => "required verifier failed after repository edits",
            ExitReason::PlanIncomplete => {
                "assistant did not finish the plan after repeated planning retries"
            }
            ExitReason::ToolCallFormatError => {
                "assistant emitted malformed or truncated tool calls repeatedly"
            }
            ExitReason::TransportError => "transport error: request failed after retries",
            ExitReason::Interrupted => "",
            ExitReason::SafeStopVerifierWeak => {
                "assistant stopped: structured verifier could not bind the task's owned test artifact"
            }
            ExitReason::SafeStopVerifierMissing => {
                "assistant stopped: request asks for test execution but no owned test artifact reached the verifier"
            }
        }
    }
}

pub(super) struct LoopStats {
    pub iter_used: usize,
    pub iter_max: usize,
    pub duration_secs: u64,
    /// Display-capped changed files for summaries.
    pub changed_files: Box<[String]>,
    /// Complete changed file list for protocol-level evidence.
    pub all_changed_files: Box<[String]>,
    pub total_changed: usize,
    /// Issue #471: classification counts from `build_stats`. NOT derived from
    /// `changed_files` (which is truncated to 16) — these come from the full
    /// `RepoVerification` accumulator (DR3-001).
    pub changed_impl_count: usize,
    pub changed_test_count: usize,
    pub changed_setup_count: usize,
}

/// Ok((prose, stats)) on success; Err((reason, error_text, stats)) on failure.
/// Stats are included in both arms so the caller can always emit a summary line.
pub(super) type LoopResult = Result<(String, LoopStats), (ExitReason, String, LoopStats)>;

fn sanitize_filename(name: &str) -> String {
    const MAX_LEN: usize = 120;
    let sanitized: String = name
        .chars()
        .map(|c| if c.is_control() { '?' } else { c })
        .collect();
    if sanitized.chars().count() > MAX_LEN {
        let truncated: String = sanitized.chars().take(MAX_LEN).collect();
        format!("{truncated}...")
    } else {
        sanitized
    }
}

pub(super) fn format_run_summary(reason: ExitReason, stats: &LoopStats) -> String {
    let mark = if reason.is_success() { "✔" } else { "✘" };
    let label = reason.label();
    let iter = format!("iter {}/{}", stats.iter_used, stats.iter_max);
    let duration = format!("duration {}s", stats.duration_secs);

    let total = stats.total_changed;
    let file_part = if total == 0 {
        "edited 0 files".to_string()
    } else {
        let shown: Vec<String> = stats
            .changed_files
            .iter()
            .take(3)
            .map(|f| sanitize_filename(f))
            .collect();
        let names = shown.join(", ");
        if total > shown.len() {
            let extra = total - shown.len();
            format!("edited {total} files ({names}, +{extra} more)")
        } else {
            format!("edited {total} files ({names})")
        }
    };

    format!("{mark} {label}  {iter}  {duration}  {file_part}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(
        iter_used: usize,
        iter_max: usize,
        duration_secs: u64,
        files: Vec<&str>,
        total_changed: usize,
    ) -> LoopStats {
        LoopStats {
            iter_used,
            iter_max,
            duration_secs,
            changed_files: files
                .iter()
                .map(|file| (*file).to_string())
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            all_changed_files: files
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            total_changed,
            changed_impl_count: 0,
            changed_test_count: 0,
            changed_setup_count: 0,
        }
    }

    #[test]
    fn done_shows_checkmark_and_fields() {
        let s = stats(
            18,
            40,
            324,
            vec!["page.tsx", "lib/game.ts", "lib/entities.ts"],
            3,
        );
        let out = format_run_summary(ExitReason::Done, &s);
        assert!(out.starts_with("✔ done"), "got: {out}");
        assert!(out.contains("iter 18/40"), "got: {out}");
        assert!(out.contains("duration 324s"), "got: {out}");
        assert!(out.contains("edited 3 files"), "got: {out}");
    }

    #[test]
    fn max_iterations_shows_cross() {
        let s = stats(40, 40, 680, vec!["lib/game.ts", "tests/game.test.ts"], 2);
        let out = format_run_summary(ExitReason::MaxIterations, &s);
        assert!(out.starts_with("✘ max_iterations"), "got: {out}");
        assert!(out.contains("iter 40/40"), "got: {out}");
    }

    #[test]
    fn zero_files_shows_no_parentheses() {
        let s = stats(5, 40, 10, vec![], 0);
        let out = format_run_summary(ExitReason::Done, &s);
        assert!(out.contains("edited 0 files"), "got: {out}");
        assert!(!out.contains('('), "got: {out}");
    }

    #[test]
    fn more_than_three_files_shows_plus_extra() {
        // changed_files has 3 entries but total_changed is 5
        let s = stats(10, 40, 50, vec!["a.rs", "b.rs", "c.rs"], 5);
        let out = format_run_summary(ExitReason::Done, &s);
        assert!(out.contains("+2 more"), "got: {out}");
        assert!(out.contains("edited 5 files"), "got: {out}");
    }

    #[test]
    fn control_chars_in_filename_are_sanitized() {
        let s = stats(1, 10, 5, vec!["bad\nfile.rs", "ok.rs"], 2);
        let out = format_run_summary(ExitReason::Done, &s);
        assert!(!out.contains('\n'), "got: {out}");
        assert!(out.contains('?'), "got: {out}");
    }

    #[test]
    fn all_exit_reason_labels_are_distinct() {
        let reasons = [
            ExitReason::Done,
            ExitReason::MaxIterations,
            ExitReason::EmptyResponses,
            ExitReason::NoToolCalls,
            ExitReason::MissingRepoEdits,
            ExitReason::MissingVerification,
            ExitReason::VerifierFailed,
            ExitReason::PlanIncomplete,
            ExitReason::ToolCallFormatError,
            ExitReason::TransportError,
            ExitReason::Interrupted,
            ExitReason::SafeStopVerifierWeak,
            ExitReason::SafeStopVerifierMissing,
        ];
        let labels: Vec<_> = reasons.iter().map(|r| r.label()).collect();
        let unique: std::collections::HashSet<_> = labels.iter().collect();
        assert_eq!(labels.len(), unique.len());
    }

    #[test]
    fn success_only_for_done() {
        assert!(ExitReason::Done.is_success());
        assert!(!ExitReason::MaxIterations.is_success());
        assert!(!ExitReason::EmptyResponses.is_success());
        assert!(!ExitReason::NoToolCalls.is_success());
        assert!(!ExitReason::MissingRepoEdits.is_success());
        assert!(!ExitReason::MissingVerification.is_success());
        assert!(!ExitReason::VerifierFailed.is_success());
        assert!(!ExitReason::PlanIncomplete.is_success());
        assert!(!ExitReason::ToolCallFormatError.is_success());
        assert!(!ExitReason::TransportError.is_success());
        assert!(!ExitReason::Interrupted.is_success());
        assert!(!ExitReason::SafeStopVerifierWeak.is_success());
        assert!(!ExitReason::SafeStopVerifierMissing.is_success());
    }

    #[test]
    fn safe_stop_variants_keep_repl_alive() {
        assert!(ExitReason::SafeStopVerifierWeak.keeps_repl_alive());
        assert!(ExitReason::SafeStopVerifierMissing.keeps_repl_alive());
    }

    #[test]
    fn safe_stop_variants_have_distinct_default_error_text() {
        // Issue #651 Task 4.2: each reason must communicate the
        // structured-verifier failure mode to the user so the run
        // summary explains *why* the agent stopped without claiming
        // completion.
        let weak = ExitReason::SafeStopVerifierWeak.default_error_text();
        let missing = ExitReason::SafeStopVerifierMissing.default_error_text();
        assert!(!weak.is_empty());
        assert!(!missing.is_empty());
        assert_ne!(weak, missing);
    }

    #[test]
    fn interrupted_renders_cross_with_label() {
        let s = stats(3, 10, 15, vec!["a.rs"], 1);
        let out = format_run_summary(ExitReason::Interrupted, &s);
        assert!(out.starts_with("✘ interrupted"), "got: {out}");
        assert!(out.contains("iter 3/10"), "got: {out}");
    }

    #[test]
    fn interrupted_has_empty_default_error_text() {
        assert_eq!(ExitReason::Interrupted.default_error_text(), "");
    }

    #[test]
    fn soft_failures_keep_repl_alive() {
        assert!(ExitReason::MaxIterations.keeps_repl_alive());
        assert!(ExitReason::EmptyResponses.keeps_repl_alive());
        assert!(ExitReason::NoToolCalls.keeps_repl_alive());
        assert!(ExitReason::MissingRepoEdits.keeps_repl_alive());
        assert!(ExitReason::MissingVerification.keeps_repl_alive());
        assert!(ExitReason::VerifierFailed.keeps_repl_alive());
        assert!(ExitReason::PlanIncomplete.keeps_repl_alive());
        assert!(ExitReason::ToolCallFormatError.keeps_repl_alive());
        assert!(ExitReason::Interrupted.keeps_repl_alive());
        assert!(!ExitReason::TransportError.keeps_repl_alive());
    }
}
