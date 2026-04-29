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

    pub(super) fn success_issue(self, stats: &LoopStats) -> Option<String> {
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
            ProtocolKind::Python => require_any_changed(
                stats,
                &[".py"],
                "python protocol requires a Python implementation or test artifact",
            ),
            ProtocolKind::TypeScriptUi => require_any_changed(
                stats,
                &[
                    ".tsx", ".ts", ".jsx", ".js", ".vue", ".svelte", ".astro", ".css", ".html",
                ],
                "typescript-ui protocol requires a real UI implementation artifact",
            ),
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
}
