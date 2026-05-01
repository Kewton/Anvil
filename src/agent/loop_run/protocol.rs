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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProtocolSuccessEvidence {
    pub(super) changed_relevant_artifact: bool,
    pub(super) relevant_artifact_reason: Option<&'static str>,
    pub(super) deterministic_only: bool,
    pub(super) total_changed: usize,
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
        let evidence = self.success_evidence(context);
        if evidence.deterministic_only {
            return Some(
                "protocol requires model-produced or verified work; deterministic fallback is recovery context, not completion"
                    .to_string(),
            );
        }
        match self.kind {
            ProtocolKind::AnswerOnly => {
                if evidence.total_changed == 0 {
                    None
                } else {
                    Some(
                        "answer-only protocol completed with repository edits; retry without changing files"
                            .to_string(),
                    )
                }
            }
            ProtocolKind::Docs => require_evidence_artifact(
                &evidence,
                "docs protocol requires a documentation artifact",
            ),
            ProtocolKind::Python => {
                if evidence.changed_relevant_artifact {
                    None
                } else {
                    Some(
                        "python protocol requires a Python implementation or test artifact"
                            .to_string(),
                    )
                }
            }
            ProtocolKind::TypeScriptUi => {
                if evidence.changed_relevant_artifact {
                    None
                } else {
                    Some(
                        "typescript-ui protocol requires a real UI implementation artifact"
                            .to_string(),
                    )
                }
            }
            ProtocolKind::GenericCode => {
                if evidence.total_changed > 0 {
                    None
                } else {
                    Some("code protocol requires at least one repository edit".to_string())
                }
            }
        }
    }

    pub(super) fn success_evidence(
        self,
        context: ProtocolSuccessContext<'_>,
    ) -> ProtocolSuccessEvidence {
        let stats = context.stats;
        let deterministic_only =
            context.deterministic_recovery_recorded && context.model_repo_edits_this_turn == 0;
        let (changed_relevant_artifact, relevant_artifact_reason) = match self.kind {
            ProtocolKind::AnswerOnly => (stats.total_changed == 0, Some("no_repo_edits")),
            ProtocolKind::Docs => relevant_suffix(stats, &[".md", ".mdx", ".txt", ".rst"])
                .map(|reason| (true, Some(reason)))
                .unwrap_or((false, None)),
            ProtocolKind::Python => {
                if stats.changed_impl_count > 0 {
                    (true, Some("impl_file_category"))
                } else if stats.changed_test_count > 0 {
                    (true, Some("test_file_category"))
                } else {
                    relevant_suffix(stats, &[".py"])
                        .map(|reason| (true, Some(reason)))
                        .unwrap_or((false, None))
                }
            }
            ProtocolKind::TypeScriptUi => {
                if stats.changed_impl_count > 0 {
                    (true, Some("impl_file_category"))
                } else {
                    relevant_suffix(
                        stats,
                        &[
                            ".tsx", ".ts", ".jsx", ".js", ".vue", ".svelte", ".astro", ".css",
                            ".html",
                        ],
                    )
                    .map(|reason| (true, Some(reason)))
                    .unwrap_or((false, None))
                }
            }
            ProtocolKind::GenericCode => (stats.total_changed > 0, Some("repo_edit")),
        };

        ProtocolSuccessEvidence {
            changed_relevant_artifact,
            relevant_artifact_reason,
            deterministic_only,
            total_changed: stats.total_changed,
        }
    }
}

fn require_evidence_artifact(
    evidence: &ProtocolSuccessEvidence,
    message: &'static str,
) -> Option<String> {
    if evidence.changed_relevant_artifact {
        None
    } else {
        Some(message.to_string())
    }
}

fn relevant_suffix(stats: &LoopStats, suffixes: &[&str]) -> Option<&'static str> {
    stats
        .changed_files
        .iter()
        .any(|path| suffixes.iter().any(|suffix| path.ends_with(suffix)))
        .then_some("file_suffix")
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
    fn protocol_success_exposes_evidence_before_decision() {
        let mut ui_stats = stats(&[".next/cache/0001.sst"], 36);
        ui_stats.changed_impl_count = 1;
        let protocol = ExecutionProtocol::from_work_mode(WorkMode::TypeScriptUi);
        let evidence = protocol.success_evidence(ProtocolSuccessContext {
            stats: &ui_stats,
            deterministic_recovery_recorded: false,
            model_repo_edits_this_turn: 1,
        });

        assert!(evidence.changed_relevant_artifact);
        assert_eq!(
            evidence.relevant_artifact_reason,
            Some("impl_file_category")
        );
        assert!(!evidence.deterministic_only);
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
