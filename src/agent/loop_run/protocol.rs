use crate::modes::plan_act::WorkMode;
use crate::tools::bash::BashCommandClass;

use super::completion_evidence::{CompletionEvidence, EvidenceSet, RepoEditCategory};
use super::summary::LoopStats;

/// Issue #607: judgment context extracted from the active request text.
/// Bundled in a struct (instead of two bool params) so future flags
/// (`request_is_docs_only`, `request_is_bench_only`, …) can be added with
/// a field append rather than a breaking signature change (DR1-003 OCP).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct RequestContext {
    pub requires_tests: bool,
    pub is_env_setup_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProtocolKind {
    TypeScriptUi,
    Python,
    Docs,
    AnswerOnly,
    GenericCode,
}

impl ProtocolKind {
    /// Issue #606 T-1.9: stable `&'static str` label used in completion
    /// evidence log payloads. Lower-case snake_case matches the enum
    /// rename convention used elsewhere in the codebase.
    pub(super) fn label(self) -> &'static str {
        match self {
            ProtocolKind::Python => "python",
            ProtocolKind::TypeScriptUi => "typescript_ui",
            ProtocolKind::Docs => "docs",
            ProtocolKind::AnswerOnly => "answer_only",
            ProtocolKind::GenericCode => "generic_code",
        }
    }

    /// Issue #606 T-1.4: does this protocol accept the given single
    /// `CompletionEvidence` as proof of forward progress this turn? Pure
    /// function over the (kind, evidence) cross-product:
    ///
    /// | kind          | RepoEdit accepted categories | VerifierExitZero | AnswerOnly |
    /// |---------------|------------------------------|------------------|------------|
    /// | Python        | Impl / Test                  | yes              | no         |
    /// | TypeScriptUi  | Impl                         | yes              | no         |
    /// | GenericCode   | Impl / Test / Setup / Other  | yes              | no         |
    /// | AnswerOnly    | (none)                       | yes              | yes        |
    /// | Docs          | Docs                         | no               | no         |
    ///
    /// `RepoEdit { category: Other }` is accepted only by `GenericCode` —
    /// it is not a strong-enough signal for Python / TypeScriptUi / Docs.
    pub(super) fn accepts(self, evidence: &CompletionEvidence) -> bool {
        match (self, evidence) {
            // Python protocol — implementation OR test edit OR verifier pass.
            (
                ProtocolKind::Python,
                CompletionEvidence::RepoEdit {
                    category: RepoEditCategory::Impl | RepoEditCategory::Test,
                    ..
                },
            ) => true,
            (ProtocolKind::Python, CompletionEvidence::VerifierExitZero { .. }) => true,
            // TypeScript UI protocol — implementation edit OR verifier pass.
            (
                ProtocolKind::TypeScriptUi,
                CompletionEvidence::RepoEdit {
                    category: RepoEditCategory::Impl,
                    ..
                },
            ) => true,
            (ProtocolKind::TypeScriptUi, CompletionEvidence::VerifierExitZero { .. }) => true,
            // GenericCode — any RepoEdit (incl. Setup / Other) OR verifier pass.
            (ProtocolKind::GenericCode, CompletionEvidence::RepoEdit { .. }) => true,
            (ProtocolKind::GenericCode, CompletionEvidence::VerifierExitZero { .. }) => true,
            // AnswerOnly — accepts a verifier pass (e.g. user asked the
            // model to run a read-only sanity script) OR an explicit
            // AnswerOnly marker. RepoEdit is NOT accepted because the
            // protocol forbids file changes.
            (ProtocolKind::AnswerOnly, CompletionEvidence::VerifierExitZero { .. }) => true,
            (ProtocolKind::AnswerOnly, CompletionEvidence::AnswerOnly) => true,
            // Docs protocol — only a Docs RepoEdit counts. Verifier pass
            // alone is not a docs artifact, even though a docs-style build
            // command (e.g. `mkdocs build`) might exit 0.
            (
                ProtocolKind::Docs,
                CompletionEvidence::RepoEdit {
                    category: RepoEditCategory::Docs,
                    ..
                },
            ) => true,
            _ => false,
        }
    }

    /// Issue #606 T-1.4: OR-fold — true when any single observation in the
    /// set is accepted by this protocol. Pure / context-free.
    ///
    /// Production code calls `evidence_set_satisfies_with_context` so the
    /// EnvSetup vs BuildTest distinction is preserved; the context-free
    /// form remains for module-level tests and serves as the underlying
    /// invariant about which evidence kinds the protocol can ever accept.
    #[cfg(test)]
    pub(super) fn evidence_set_satisfies(self, set: &EvidenceSet) -> bool {
        set.iter().any(|ev| self.accepts(ev))
    }

    /// Issue #607: context-aware OR-fold. EnvSetup-only `VerifierExitZero`
    /// only counts when the active request is setup-only (and does not also
    /// ask for tests). When tests / code are requested the protocol still
    /// demands a `BuildTest` verifier (or a code-shaped RepoEdit). AnswerOnly
    /// never accepts EnvSetup alone — running `npm install` is not an
    /// answer-only artifact. Docs is unchanged.
    pub(super) fn evidence_set_satisfies_with_context(
        self,
        set: &EvidenceSet,
        ctx: &RequestContext,
    ) -> bool {
        // Repo-edit / AnswerOnly evidence retain their context-free semantics.
        let has_non_env_setup_evidence = set.iter().any(|ev| match ev {
            CompletionEvidence::VerifierExitZero { class, .. } => {
                *class != BashCommandClass::EnvSetup && self.accepts(ev)
            }
            _ => self.accepts(ev),
        });
        if has_non_env_setup_evidence {
            return true;
        }
        // Only EnvSetup verifier(s) are present. Allow completion only for
        // pure setup-only requests on code-bearing protocols.
        let env_setup_present = set.iter().any(|ev| {
            matches!(
                ev,
                CompletionEvidence::VerifierExitZero {
                    class: BashCommandClass::EnvSetup,
                    ..
                }
            )
        });
        if !env_setup_present {
            return false;
        }
        if ctx.requires_tests {
            return false;
        }
        if !ctx.is_env_setup_only {
            return false;
        }
        matches!(
            self,
            ProtocolKind::Python | ProtocolKind::TypeScriptUi | ProtocolKind::GenericCode
        )
    }

    /// Issue #607: context-aware missing-shapes report. When tests/code are
    /// requested and only EnvSetup evidence is present, surface
    /// `"verifier_exit_zero"` so the model knows a real BuildTest verifier
    /// is still required. When setup-only request grants EnvSetup
    /// satisfaction, the slot is suppressed.
    pub(super) fn evidence_set_missing_shapes_with_context(
        self,
        set: &EvidenceSet,
        ctx: &RequestContext,
    ) -> Vec<&'static str> {
        let mut missing = self.evidence_set_missing_shapes(set);
        // Tests requested + only EnvSetup verifier in the set → the
        // context-free helper considers `verifier_exit_zero` satisfied (any
        // VerifierExitZero counts), but we still need a real test verifier.
        let only_env_setup_verifier = set
            .iter()
            .any(|ev| matches!(ev, CompletionEvidence::VerifierExitZero { .. }))
            && set.iter().all(|ev| match ev {
                CompletionEvidence::VerifierExitZero { class, .. } => {
                    *class == BashCommandClass::EnvSetup
                }
                _ => true,
            });
        if ctx.requires_tests && only_env_setup_verifier {
            // Re-add the slot iff this protocol cares about a verifier and
            // the slot was claimed by the context-free helper.
            let wants_verifier = matches!(
                self,
                ProtocolKind::Python
                    | ProtocolKind::TypeScriptUi
                    | ProtocolKind::GenericCode
                    | ProtocolKind::AnswerOnly
            );
            if wants_verifier && !missing.contains(&"verifier_exit_zero") {
                missing.push("verifier_exit_zero");
            }
        }
        missing
    }

    /// Issue #606 T-1.4: human-friendly list of evidence shapes this
    /// protocol still wants observed in the current turn. Used by
    /// `success.rs` to populate `agent.completion_evidence.unsatisfied`
    /// log payloads when the OR-fold returned false. The returned slice
    /// of `&'static str` keeps allocation low and the names stable for
    /// log analysers (no `format!` interpolation).
    pub(super) fn evidence_set_missing_shapes(self, set: &EvidenceSet) -> Vec<&'static str> {
        // For each shape we *would* accept, mark "missing" when the set
        // does not contain a matching observation. The order here matches
        // the documentation table above so log readers see a consistent
        // ordering.
        let mut missing: Vec<&'static str> = Vec::new();
        match self {
            ProtocolKind::Python => {
                if !set.iter().any(|ev| {
                    matches!(
                        ev,
                        CompletionEvidence::RepoEdit {
                            category: RepoEditCategory::Impl | RepoEditCategory::Test,
                            ..
                        }
                    )
                }) {
                    missing.push("repo_edit_impl_or_test");
                }
                if !set
                    .iter()
                    .any(|ev| matches!(ev, CompletionEvidence::VerifierExitZero { .. }))
                {
                    missing.push("verifier_exit_zero");
                }
            }
            ProtocolKind::TypeScriptUi => {
                if !set.iter().any(|ev| {
                    matches!(
                        ev,
                        CompletionEvidence::RepoEdit {
                            category: RepoEditCategory::Impl,
                            ..
                        }
                    )
                }) {
                    missing.push("repo_edit_impl");
                }
                if !set
                    .iter()
                    .any(|ev| matches!(ev, CompletionEvidence::VerifierExitZero { .. }))
                {
                    missing.push("verifier_exit_zero");
                }
            }
            ProtocolKind::GenericCode => {
                if !set
                    .iter()
                    .any(|ev| matches!(ev, CompletionEvidence::RepoEdit { .. }))
                {
                    missing.push("repo_edit_any");
                }
                if !set
                    .iter()
                    .any(|ev| matches!(ev, CompletionEvidence::VerifierExitZero { .. }))
                {
                    missing.push("verifier_exit_zero");
                }
            }
            ProtocolKind::AnswerOnly => {
                if !set
                    .iter()
                    .any(|ev| matches!(ev, CompletionEvidence::VerifierExitZero { .. }))
                {
                    missing.push("verifier_exit_zero");
                }
                if !set
                    .iter()
                    .any(|ev| matches!(ev, CompletionEvidence::AnswerOnly))
                {
                    missing.push("answer_only");
                }
            }
            ProtocolKind::Docs => {
                if !set.iter().any(|ev| {
                    matches!(
                        ev,
                        CompletionEvidence::RepoEdit {
                            category: RepoEditCategory::Docs,
                            ..
                        }
                    )
                }) {
                    missing.push("repo_edit_docs");
                }
            }
        }
        missing
    }
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
    pub(super) requested_paths: &'a [String],
    pub(super) verifier_passed_after_edit: Option<bool>,
    /// Issue #606 (T-1.4): Stage-2 short-circuit signal. When the agent
    /// post-hoc-observed enough evidence to satisfy the active protocol via
    /// `ProtocolKind::evidence_set_satisfies`, `success.rs` sets this to
    /// `true` so the per-protocol per-kind reject text never fires. Stage 1
    /// (`deterministic_only` / `verifier_passed_after_edit == Some(false)`)
    /// still wins because those are deterministic failure signals, not
    /// missing-evidence signals.
    pub(super) evidence_satisfied: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProtocolSuccessEvidence {
    pub(super) changed_relevant_artifact: bool,
    pub(super) relevant_artifact_reason: Option<&'static str>,
    pub(super) requested_path_count: usize,
    pub(super) requested_path_changed: bool,
    pub(super) unrequested_changed_count: usize,
    pub(super) verifier_passed_after_edit: Option<bool>,
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
            requested_paths: &[],
            verifier_passed_after_edit: None,
            evidence_satisfied: false,
        })
    }

    pub(super) fn success_issue_with_context(
        self,
        context: ProtocolSuccessContext<'_>,
    ) -> Option<String> {
        // Issue #606 T-1.5: Stage-1 deterministic failure signals always win.
        // Stage-2 (`evidence_satisfied`) is honored only after Stage-1 cleared.
        let evidence_satisfied = context.evidence_satisfied;
        let evidence = self.success_evidence(context);
        if evidence.deterministic_only {
            return Some(
                "protocol requires model-produced or verified work; deterministic fallback is recovery context, not completion"
                    .to_string(),
            );
        }
        if evidence.verifier_passed_after_edit == Some(false) {
            return Some("protocol verifier failed after the edit".to_string());
        }
        // Stage-2: post-hoc evidence short-circuit. AnswerOnly is intentionally
        // NOT shortcut here — its reject ("answer-only protocol completed with
        // repository edits") fires when `evidence.total_changed > 0` because
        // it represents a *behavioural violation*, not missing evidence.
        if evidence_satisfied && !matches!(self.kind, ProtocolKind::AnswerOnly) {
            return None;
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
                if let Some(issue) = requested_path_issue(&evidence) {
                    return Some(issue);
                }
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
                if let Some(issue) = requested_path_issue(&evidence) {
                    return Some(issue);
                }
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
                if let Some(issue) = requested_path_issue(&evidence) {
                    return Some(issue);
                }
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
        let requested_path_changed =
            requested_path_changed(context.requested_paths, &stats.all_changed_files);
        let unrequested_changed_count =
            unrequested_changed_count(context.requested_paths, &stats.all_changed_files);
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
            requested_path_count: context.requested_paths.len(),
            requested_path_changed,
            unrequested_changed_count,
            verifier_passed_after_edit: context.verifier_passed_after_edit,
            deterministic_only,
            total_changed: stats.total_changed,
        }
    }
}

pub(super) fn requested_paths_from_text(text: &str) -> Vec<String> {
    const EXTENSIONS: &[&str] = &[
        ".rs", ".py", ".ts", ".tsx", ".js", ".jsx", ".vue", ".svelte", ".astro", ".css", ".html",
        ".md", ".mdx", ".toml", ".json", ".yaml", ".yml", ".txt",
    ];
    let mut out = Vec::new();
    for raw in text.split_whitespace() {
        let token = raw.trim_matches(|c: char| {
            matches!(
                c,
                '`' | '\''
                    | '"'
                    | ','
                    | ':'
                    | ';'
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '<'
                    | '>'
                    | '「'
                    | '」'
                    | '『'
                    | '』'
                    | '（'
                    | '）'
                    | '、'
                    | '。'
            )
        });
        let token = token.strip_prefix("./").unwrap_or(token);
        let lower = token.to_ascii_lowercase();
        if !EXTENSIONS.iter().any(|ext| lower.ends_with(ext)) {
            continue;
        }
        if token.is_empty()
            || token.starts_with('/')
            || token.contains("..")
            || token.contains("://")
            || token.contains('\\')
        {
            continue;
        }
        let normalized = token.to_string();
        if !out.contains(&normalized) {
            out.push(normalized);
        }
        if out.len() >= 8 {
            break;
        }
    }
    out
}

fn requested_path_issue(evidence: &ProtocolSuccessEvidence) -> Option<String> {
    if evidence.requested_path_count > 0 && !evidence.requested_path_changed {
        Some(format!(
            "protocol requires changing the requested path; unrequested_changed_count={}",
            evidence.unrequested_changed_count
        ))
    } else {
        None
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
        .all_changed_files
        .iter()
        .any(|path| suffixes.iter().any(|suffix| path.ends_with(suffix)))
        .then_some("file_suffix")
}

fn requested_path_changed(requested_paths: &[String], changed_files: &[String]) -> bool {
    !requested_paths.is_empty()
        && requested_paths.iter().any(|requested| {
            changed_files
                .iter()
                .any(|changed| path_matches_request(changed, requested))
        })
}

fn unrequested_changed_count(requested_paths: &[String], changed_files: &[String]) -> usize {
    if requested_paths.is_empty() {
        return 0;
    }
    changed_files
        .iter()
        .filter(|changed| {
            !requested_paths
                .iter()
                .any(|requested| path_matches_request(changed, requested))
        })
        .count()
}

fn path_matches_request(changed: &str, requested: &str) -> bool {
    let changed = changed.strip_suffix(" (deleted)").unwrap_or(changed);
    let requested = requested.strip_prefix("./").unwrap_or(requested);
    changed == requested
        || std::path::Path::new(changed)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == requested)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(files: &[&str], total_changed: usize) -> LoopStats {
        LoopStats {
            iter_used: 1,
            iter_max: 50,
            duration_secs: 0,
            changed_files: files
                .iter()
                .map(|file| file.to_string())
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            all_changed_files: files
                .iter()
                .map(|file| file.to_string())
                .collect::<Vec<_>>()
                .into_boxed_slice(),
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
            requested_paths: &[],
            verifier_passed_after_edit: None,
            evidence_satisfied: false,
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
            requested_paths: &[],
            verifier_passed_after_edit: None,
            evidence_satisfied: false,
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
                    requested_paths: &[],
                    verifier_passed_after_edit: None,
                    evidence_satisfied: false,
                })
                .is_none()
        );
    }

    #[test]
    fn requested_path_must_be_changed_when_explicit() {
        let protocol = ExecutionProtocol::from_work_mode(WorkMode::Python);
        let requested = vec!["src/tool.py".to_string()];

        assert!(
            protocol
                .success_issue_with_context(ProtocolSuccessContext {
                    stats: &stats(&["src/other.py"], 1),
                    deterministic_recovery_recorded: false,
                    model_repo_edits_this_turn: 1,
                    requested_paths: &requested,
                    verifier_passed_after_edit: None,
                    evidence_satisfied: false,
                })
                .is_some()
        );
        assert!(
            protocol
                .success_issue_with_context(ProtocolSuccessContext {
                    stats: &stats(&["src/tool.py"], 1),
                    deterministic_recovery_recorded: false,
                    model_repo_edits_this_turn: 1,
                    requested_paths: &requested,
                    verifier_passed_after_edit: None,
                    evidence_satisfied: false,
                })
                .is_none()
        );
    }

    #[test]
    fn evidence_counts_unrequested_changes_against_requested_paths() {
        let protocol = ExecutionProtocol::from_work_mode(WorkMode::GenericCode);
        let requested = vec!["src/app.ts".to_string()];
        let stats = stats(&["src/app.ts", "src/other.ts", "README.md"], 3);
        let evidence = protocol.success_evidence(ProtocolSuccessContext {
            stats: &stats,
            deterministic_recovery_recorded: false,
            model_repo_edits_this_turn: 1,
            requested_paths: &requested,
            verifier_passed_after_edit: None,
            evidence_satisfied: false,
        });

        assert!(evidence.requested_path_changed);
        assert_eq!(evidence.unrequested_changed_count, 2);
    }

    #[test]
    fn verifier_failure_after_edit_blocks_protocol_success() {
        let protocol = ExecutionProtocol::from_work_mode(WorkMode::GenericCode);
        let stats = stats(&["src/app.ts"], 1);

        assert_eq!(
            protocol.success_issue_with_context(ProtocolSuccessContext {
                stats: &stats,
                deterministic_recovery_recorded: false,
                model_repo_edits_this_turn: 1,
                requested_paths: &[],
                verifier_passed_after_edit: Some(false),
                evidence_satisfied: false,
            }),
            Some("protocol verifier failed after the edit".to_string())
        );
        assert!(
            protocol
                .success_issue_with_context(ProtocolSuccessContext {
                    stats: &stats,
                    deterministic_recovery_recorded: false,
                    model_repo_edits_this_turn: 1,
                    requested_paths: &[],
                    verifier_passed_after_edit: Some(true),
                    evidence_satisfied: false,
                })
                .is_none()
        );
    }

    #[test]
    fn requested_paths_from_text_extracts_safe_relative_paths() {
        assert_eq!(
            requested_paths_from_text("src/app/page.tsx と `README.md` を更新してください"),
            vec!["src/app/page.tsx".to_string(), "README.md".to_string()]
        );
        assert!(requested_paths_from_text("../secret.py /tmp/x.py https://x/y.py").is_empty());
    }

    // ----------------------------- Issue #606 T-1.4 -----------------------

    use crate::tools::bash::BashCommandClass;

    fn ev_verifier() -> CompletionEvidence {
        CompletionEvidence::VerifierExitZero {
            class: BashCommandClass::BuildTest,
            command: "cargo test".to_string(),
        }
    }

    fn ev_repo_edit(category: RepoEditCategory) -> CompletionEvidence {
        CompletionEvidence::RepoEdit { category, count: 1 }
    }

    /// U-09 — 5 ProtocolKind × evidence variant matrix smoke.
    #[test]
    fn protocol_kind_accepts_each_completion_evidence_kind() {
        let verifier = ev_verifier();
        let impl_edit = ev_repo_edit(RepoEditCategory::Impl);
        let test_edit = ev_repo_edit(RepoEditCategory::Test);
        let docs_edit = ev_repo_edit(RepoEditCategory::Docs);
        let setup_edit = ev_repo_edit(RepoEditCategory::Setup);
        let other_edit = ev_repo_edit(RepoEditCategory::Other);
        let answer_only = CompletionEvidence::AnswerOnly;

        // Python
        assert!(ProtocolKind::Python.accepts(&verifier));
        assert!(ProtocolKind::Python.accepts(&impl_edit));
        assert!(ProtocolKind::Python.accepts(&test_edit));
        assert!(!ProtocolKind::Python.accepts(&docs_edit));
        assert!(!ProtocolKind::Python.accepts(&setup_edit));
        assert!(!ProtocolKind::Python.accepts(&other_edit));
        assert!(!ProtocolKind::Python.accepts(&answer_only));

        // TypeScriptUi
        assert!(ProtocolKind::TypeScriptUi.accepts(&verifier));
        assert!(ProtocolKind::TypeScriptUi.accepts(&impl_edit));
        assert!(!ProtocolKind::TypeScriptUi.accepts(&test_edit));
        assert!(!ProtocolKind::TypeScriptUi.accepts(&docs_edit));

        // GenericCode — any RepoEdit, plus VerifierExitZero
        assert!(ProtocolKind::GenericCode.accepts(&verifier));
        assert!(ProtocolKind::GenericCode.accepts(&impl_edit));
        assert!(ProtocolKind::GenericCode.accepts(&test_edit));
        assert!(ProtocolKind::GenericCode.accepts(&docs_edit));
        assert!(ProtocolKind::GenericCode.accepts(&setup_edit));
        assert!(ProtocolKind::GenericCode.accepts(&other_edit));
        assert!(!ProtocolKind::GenericCode.accepts(&answer_only));

        // AnswerOnly
        assert!(ProtocolKind::AnswerOnly.accepts(&verifier));
        assert!(ProtocolKind::AnswerOnly.accepts(&answer_only));
        assert!(!ProtocolKind::AnswerOnly.accepts(&impl_edit));
        assert!(!ProtocolKind::AnswerOnly.accepts(&docs_edit));

        // Docs
        assert!(ProtocolKind::Docs.accepts(&docs_edit));
        assert!(!ProtocolKind::Docs.accepts(&verifier));
        assert!(!ProtocolKind::Docs.accepts(&impl_edit));
        assert!(!ProtocolKind::Docs.accepts(&answer_only));
    }

    /// U-10 — AnswerOnly + VerifierExitZero + no repo edits → done. The
    /// success.rs Stage-2 short-circuit is suppressed for AnswerOnly, but
    /// the original AnswerOnly logic still accepts `total_changed == 0`.
    #[test]
    fn answer_only_with_verifier_exit_zero_is_done() {
        let protocol = ExecutionProtocol::from_work_mode(WorkMode::AnswerOnly);
        // total_changed == 0 (verifier ran but no edits)
        assert!(
            protocol
                .success_issue_with_context(ProtocolSuccessContext {
                    stats: &stats(&[], 0),
                    deterministic_recovery_recorded: false,
                    model_repo_edits_this_turn: 0,
                    requested_paths: &[],
                    verifier_passed_after_edit: None,
                    evidence_satisfied: true,
                })
                .is_none()
        );
    }

    /// U-11 — AnswerOnly + RepoEdit only (no verifier) is still rejected
    /// even when `evidence_satisfied = true`. This pins VR-02 regression:
    /// the Stage-2 short-circuit must NOT silence the answer-only edit ban.
    #[test]
    fn answer_only_with_repo_edit_only_is_still_rejected() {
        let protocol = ExecutionProtocol::from_work_mode(WorkMode::AnswerOnly);
        let issue = protocol.success_issue_with_context(ProtocolSuccessContext {
            stats: &stats(&["README.md"], 1),
            deterministic_recovery_recorded: false,
            model_repo_edits_this_turn: 1,
            requested_paths: &[],
            verifier_passed_after_edit: None,
            evidence_satisfied: true, // ← even with evidence flag set
        });
        assert_eq!(
            issue,
            Some(
                "answer-only protocol completed with repository edits; retry without changing files"
                    .to_string()
            )
        );
    }

    /// U-12 — Stage-1 (`verifier_passed_after_edit == Some(false)`) wins
    /// over Stage-2 (`evidence_satisfied == true`). Deterministic verifier
    /// failure must not be silenced by post-hoc evidence.
    #[test]
    fn verifier_passed_after_edit_some_false_overrides_evidence_satisfied() {
        let protocol = ExecutionProtocol::from_work_mode(WorkMode::GenericCode);
        let stats = stats(&["src/app.ts"], 1);
        assert_eq!(
            protocol.success_issue_with_context(ProtocolSuccessContext {
                stats: &stats,
                deterministic_recovery_recorded: false,
                model_repo_edits_this_turn: 1,
                requested_paths: &[],
                verifier_passed_after_edit: Some(false),
                evidence_satisfied: true,
            }),
            Some("protocol verifier failed after the edit".to_string())
        );
    }

    /// U-19 — `evidence_set_missing_shapes` returns the static labels
    /// describing which shape is still wanted. Empty set → all shapes.
    #[test]
    fn evidence_set_missing_shapes_describes_unsatisfied_protocol() {
        let empty = EvidenceSet::new();
        assert_eq!(
            ProtocolKind::Python.evidence_set_missing_shapes(&empty),
            vec!["repo_edit_impl_or_test", "verifier_exit_zero"]
        );
        assert_eq!(
            ProtocolKind::TypeScriptUi.evidence_set_missing_shapes(&empty),
            vec!["repo_edit_impl", "verifier_exit_zero"]
        );
        assert_eq!(
            ProtocolKind::GenericCode.evidence_set_missing_shapes(&empty),
            vec!["repo_edit_any", "verifier_exit_zero"]
        );
        assert_eq!(
            ProtocolKind::AnswerOnly.evidence_set_missing_shapes(&empty),
            vec!["verifier_exit_zero", "answer_only"]
        );
        assert_eq!(
            ProtocolKind::Docs.evidence_set_missing_shapes(&empty),
            vec!["repo_edit_docs"]
        );

        // Partial — a Test edit satisfies Python's repo-edit slot but
        // leaves the verifier slot open.
        let mut set = EvidenceSet::new();
        set.push(ev_repo_edit(RepoEditCategory::Test));
        assert_eq!(
            ProtocolKind::Python.evidence_set_missing_shapes(&set),
            vec!["verifier_exit_zero"]
        );
    }

    /// Integration of T-1.3 evidence_set_satisfies into protocol decisions.
    #[test]
    fn evidence_set_satisfies_python_with_test_edit() {
        let mut set = EvidenceSet::new();
        set.push(ev_repo_edit(RepoEditCategory::Test));
        assert!(ProtocolKind::Python.evidence_set_satisfies(&set));
    }

    #[test]
    fn evidence_set_satisfies_generic_code_with_verifier_exit_zero() {
        let mut set = EvidenceSet::new();
        set.push(ev_verifier());
        assert!(ProtocolKind::GenericCode.evidence_set_satisfies(&set));
    }

    #[test]
    fn evidence_set_unsatisfied_docs_with_only_verifier() {
        let mut set = EvidenceSet::new();
        set.push(ev_verifier());
        assert!(!ProtocolKind::Docs.evidence_set_satisfies(&set));
    }

    // ---------------------------------------------------------------------
    // Issue #607 VR-β-04 (f): EnvSetup verifier matrix.
    // ---------------------------------------------------------------------

    fn ev_env_setup() -> CompletionEvidence {
        CompletionEvidence::VerifierExitZero {
            class: BashCommandClass::EnvSetup,
            command: "npm install".to_string(),
        }
    }

    /// Context-free `accepts()` treats EnvSetup `VerifierExitZero` the same
    /// way it treats BuildTest — every code-bearing protocol receives it,
    /// Docs rejects it. The context-aware satisfaction logic above narrows
    /// this down per request shape.
    #[test]
    fn protocol_kind_accepts_env_setup_verifier_matrix() {
        let env_setup = ev_env_setup();
        assert!(ProtocolKind::Python.accepts(&env_setup));
        assert!(ProtocolKind::TypeScriptUi.accepts(&env_setup));
        assert!(ProtocolKind::GenericCode.accepts(&env_setup));
        assert!(ProtocolKind::AnswerOnly.accepts(&env_setup));
        assert!(!ProtocolKind::Docs.accepts(&env_setup));
    }

    fn setup_only_ctx() -> RequestContext {
        RequestContext {
            requires_tests: false,
            is_env_setup_only: true,
        }
    }

    fn tests_required_ctx() -> RequestContext {
        RequestContext {
            requires_tests: true,
            is_env_setup_only: false,
        }
    }

    #[test]
    fn evidence_set_satisfies_env_setup_context_matrix() {
        // BuildTest alone — every code-bearing protocol + AnswerOnly accept.
        let mut build_only = EvidenceSet::new();
        build_only.push(ev_verifier());
        for kind in [
            ProtocolKind::Python,
            ProtocolKind::TypeScriptUi,
            ProtocolKind::GenericCode,
            ProtocolKind::AnswerOnly,
        ] {
            assert!(
                kind.evidence_set_satisfies_with_context(&build_only, &setup_only_ctx()),
                "BuildTest alone should satisfy {kind:?}"
            );
            assert!(
                kind.evidence_set_satisfies_with_context(&build_only, &tests_required_ctx()),
                "BuildTest alone should satisfy {kind:?} even when tests required"
            );
        }
        assert!(
            !ProtocolKind::Docs.evidence_set_satisfies_with_context(&build_only, &setup_only_ctx())
        );

        // EnvSetup alone (setup-only) — Python / TypeScriptUi / GenericCode ✔.
        let mut env_only = EvidenceSet::new();
        env_only.push(ev_env_setup());
        for kind in [
            ProtocolKind::Python,
            ProtocolKind::TypeScriptUi,
            ProtocolKind::GenericCode,
        ] {
            assert!(
                kind.evidence_set_satisfies_with_context(&env_only, &setup_only_ctx()),
                "EnvSetup alone should satisfy {kind:?} for setup-only"
            );
        }
        // AnswerOnly: EnvSetup alone is not an answer-only artifact.
        assert!(
            !ProtocolKind::AnswerOnly
                .evidence_set_satisfies_with_context(&env_only, &setup_only_ctx())
        );
        // Docs always rejects.
        assert!(
            !ProtocolKind::Docs.evidence_set_satisfies_with_context(&env_only, &setup_only_ctx())
        );

        // EnvSetup alone but tests requested — every protocol rejects.
        for kind in [
            ProtocolKind::Python,
            ProtocolKind::TypeScriptUi,
            ProtocolKind::GenericCode,
            ProtocolKind::AnswerOnly,
            ProtocolKind::Docs,
        ] {
            assert!(
                !kind.evidence_set_satisfies_with_context(&env_only, &tests_required_ctx()),
                "EnvSetup alone must not satisfy {kind:?} when tests required"
            );
        }

        // EnvSetup + BuildTest — BuildTest is the satisfier, valid for all
        // code-bearing protocols + AnswerOnly.
        let mut env_plus_build = EvidenceSet::new();
        env_plus_build.push(ev_env_setup());
        env_plus_build.push(ev_verifier());
        for kind in [
            ProtocolKind::Python,
            ProtocolKind::TypeScriptUi,
            ProtocolKind::GenericCode,
            ProtocolKind::AnswerOnly,
        ] {
            assert!(
                kind.evidence_set_satisfies_with_context(&env_plus_build, &tests_required_ctx()),
                "EnvSetup + BuildTest should satisfy {kind:?} when tests required"
            );
            assert!(
                kind.evidence_set_satisfies_with_context(&env_plus_build, &setup_only_ctx()),
                "EnvSetup + BuildTest should satisfy {kind:?} when setup-only"
            );
        }
        assert!(
            !ProtocolKind::Docs
                .evidence_set_satisfies_with_context(&env_plus_build, &tests_required_ctx())
        );
    }

    /// Missing-shape reporting in context. With EnvSetup-only evidence and
    /// tests requested, `"verifier_exit_zero"` stays in the missing list to
    /// nudge the model to run a real test. With setup-only and EnvSetup
    /// evidence, the slot is satisfied and not reported.
    #[test]
    fn evidence_set_missing_shapes_with_context_reflects_env_setup_satisfaction() {
        let mut env_only = EvidenceSet::new();
        env_only.push(ev_env_setup());

        // setup-only: verifier slot is satisfied → not in missing list.
        let setup_missing = ProtocolKind::GenericCode
            .evidence_set_missing_shapes_with_context(&env_only, &setup_only_ctx());
        assert!(!setup_missing.contains(&"verifier_exit_zero"));

        // tests required: still missing.
        let tests_missing = ProtocolKind::GenericCode
            .evidence_set_missing_shapes_with_context(&env_only, &tests_required_ctx());
        assert!(tests_missing.contains(&"verifier_exit_zero"));
    }
}
