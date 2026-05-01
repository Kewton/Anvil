use crate::modes::plan_act::WorkMode;

use super::summary::LoopStats;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProtocolKind {
    TypeScriptUi,
    Python,
    Docs,
    AnswerOnly,
    GenericCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ExecutionProtocol {
    kind: ProtocolKind,
}

#[derive(Clone, Copy)]
pub(super) struct ProtocolSuccessContext<'a> {
    pub(super) stats: &'a LoopStats,
    pub(super) deterministic_recovery_recorded: bool,
    pub(super) model_repo_edits_this_turn: usize,
}

impl ExecutionProtocol {
    pub(super) fn from_work_mode(mode: WorkMode) -> Self {
        let kind = match mode {
            WorkMode::TypeScriptUi => ProtocolKind::TypeScriptUi,
            WorkMode::Python => ProtocolKind::Python,
            WorkMode::Docs => ProtocolKind::Docs,
            WorkMode::AnswerOnly => ProtocolKind::AnswerOnly,
            WorkMode::Auto | WorkMode::GenericCode | WorkMode::Unknown => ProtocolKind::GenericCode,
        };
        Self { kind }
    }

    #[allow(dead_code)] // Issue #466: 一時的に call site が VerifierSkill 経由になり
    // 直接呼ばれなくなったが API は残置 (将来 Tester / CaseRecord の skill 化で再利用予定)
    pub(super) fn kind(self) -> ProtocolKind {
        self.kind
    }

    #[allow(dead_code)] // Convenience wrapper for legacy call sites and focused tests.
    pub(super) fn success_issue(self, stats: &LoopStats) -> Option<String> {
        self.success_issue_with_context(ProtocolSuccessContext {
            stats,
            deterministic_recovery_recorded: false,
            model_repo_edits_this_turn: 0,
        })
    }

    pub(super) fn success_issue_with_context(
        self,
        context: ProtocolSuccessContext<'_>,
    ) -> Option<String> {
        if context.deterministic_recovery_recorded && context.model_repo_edits_this_turn == 0 {
            return Some(
                "protocol requires model-produced or verified work; deterministic fallback is recovery context, not completion"
                    .to_string(),
            );
        }
        let stats = context.stats;
        match self.kind {
            ProtocolKind::AnswerOnly => {
                if stats.total_changed == 0 {
                    None
                } else {
                    Some(
                        "answer-only protocol completed with repository edits; retry without changing files"
                            .to_string(),
                    )
                }
            }
            ProtocolKind::Docs => require_any_changed(
                stats,
                &[".md", ".mdx", ".txt", ".rst"],
                "docs protocol requires a documentation artifact",
            ),
            ProtocolKind::Python => {
                if stats.changed_impl_count > 0
                    || stats.changed_test_count > 0
                    || stats.changed_files.iter().any(|path| path.ends_with(".py"))
                {
                    None
                } else {
                    Some(
                        "python protocol requires a Python implementation or test artifact"
                            .to_string(),
                    )
                }
            }
            ProtocolKind::TypeScriptUi => {
                if stats.changed_impl_count > 0
                    || stats.changed_files.iter().any(|path| {
                        [
                            ".tsx", ".ts", ".jsx", ".js", ".vue", ".svelte", ".astro", ".css",
                            ".html",
                        ]
                        .iter()
                        .any(|suffix| path.ends_with(suffix))
                    })
                {
                    None
                } else {
                    Some(
                        "typescript-ui protocol requires a real UI implementation artifact"
                            .to_string(),
                    )
                }
            }
            ProtocolKind::GenericCode => {
                if stats.total_changed > 0 {
                    None
                } else {
                    Some("code protocol requires at least one repository edit".to_string())
                }
            }
        }
    }
}

fn require_any_changed(
    stats: &LoopStats,
    suffixes: &[&str],
    message: &'static str,
) -> Option<String> {
    if stats
        .changed_files
        .iter()
        .any(|path| suffixes.iter().any(|suffix| path.ends_with(suffix)))
    {
        None
    } else {
        Some(message.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(files: &[&str], total_changed: usize) -> LoopStats {
        LoopStats {
            iter_used: 1,
            iter_max: 50,
            duration_secs: 0,
            changed_files: files.iter().map(|file| file.to_string()).collect(),
            total_changed,
            changed_impl_count: 0,
            changed_test_count: 0,
            changed_setup_count: 0,
        }
    }

    fn context<'a>(
        stats: &'a LoopStats,
        deterministic_recovery_recorded: bool,
    ) -> ProtocolSuccessContext<'a> {
        ProtocolSuccessContext {
            stats,
            deterministic_recovery_recorded,
            model_repo_edits_this_turn: 0,
        }
    }

    #[test]
    fn answer_only_rejects_file_changes() {
        let protocol = ExecutionProtocol::from_work_mode(WorkMode::AnswerOnly);
        assert!(protocol.success_issue(&stats(&[], 0)).is_none());
        assert!(protocol.success_issue(&stats(&["README.md"], 1)).is_some());
    }

    #[test]
    fn protocol_success_is_mode_specific() {
        assert!(
            ExecutionProtocol::from_work_mode(WorkMode::Python)
                .success_issue(&stats(&["analyze_csv.py"], 1))
                .is_none()
        );
        assert!(
            ExecutionProtocol::from_work_mode(WorkMode::Docs)
                .success_issue(&stats(&["README.md"], 1))
                .is_none()
        );
        assert!(
            ExecutionProtocol::from_work_mode(WorkMode::TypeScriptUi)
                .success_issue(&stats(&["src/routes/+page.svelte"], 1))
                .is_none()
        );
    }

    #[test]
    fn protocol_success_uses_full_category_counts_when_changed_files_are_truncated() {
        let mut ui_stats = stats(&[".next/cache/0001.sst"], 36);
        ui_stats.changed_impl_count = 1;
        assert!(
            ExecutionProtocol::from_work_mode(WorkMode::TypeScriptUi)
                .success_issue(&ui_stats)
                .is_none()
        );

        let mut py_stats = stats(&["__pycache__/tool.pyc"], 2);
        py_stats.changed_test_count = 1;
        assert!(
            ExecutionProtocol::from_work_mode(WorkMode::Python)
                .success_issue(&py_stats)
                .is_none()
        );
    }

    #[test]
    fn deterministic_recovery_is_not_protocol_completion() {
        let ui_stats = stats(&["src/app/page.tsx"], 1);
        let protocol = ExecutionProtocol::from_work_mode(WorkMode::TypeScriptUi);

        assert!(
            protocol
                .success_issue_with_context(context(&ui_stats, true))
                .is_some()
        );
        assert!(
            protocol
                .success_issue_with_context(context(&ui_stats, false))
                .is_none()
        );
    }

    #[test]
    fn deterministic_recovery_allows_later_model_repo_edit() {
        let ui_stats = stats(&["src/app/page.tsx"], 1);
        let protocol = ExecutionProtocol::from_work_mode(WorkMode::TypeScriptUi);

        assert!(
            protocol
                .success_issue_with_context(ProtocolSuccessContext {
                    stats: &ui_stats,
                    deterministic_recovery_recorded: true,
                    model_repo_edits_this_turn: 1,
                })
                .is_none()
        );
    }
}
