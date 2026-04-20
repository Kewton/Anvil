#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExitReason {
    Done,
    MaxIterations,
    EmptyResponses,
    NoToolCalls,
    MissingRepoEdits,
    TransportError,
}

impl ExitReason {
    pub(super) fn is_success(self) -> bool {
        matches!(self, ExitReason::Done)
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            ExitReason::Done => "done",
            ExitReason::MaxIterations => "max_iterations",
            ExitReason::EmptyResponses => "empty_responses",
            ExitReason::NoToolCalls => "no_tool_calls",
            ExitReason::MissingRepoEdits => "missing_repo_edits",
            ExitReason::TransportError => "transport_error",
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
            ExitReason::TransportError => "transport error: request failed after retries",
        }
    }
}

pub(super) struct LoopStats {
    pub iter_used: usize,
    pub iter_max: usize,
    pub duration_secs: u64,
    pub changed_files: Vec<String>,
    pub total_changed: usize,
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
            changed_files: files.into_iter().map(str::to_string).collect(),
            total_changed,
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
            ExitReason::TransportError,
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
        assert!(!ExitReason::TransportError.is_success());
    }
}
