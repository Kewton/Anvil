//! Issue #919 (P2 / Decision #7 + #8): in-crate `#[cfg(test)]` E2E suite that
//! pins the per-kind prose-verifier-free matrix on the **production decision
//! functions** that feed `ExitReason`.
//!
//! `ExitReason::{MissingVerification, SafeStopVerifierMissing,
//! SafeStopVerifierWeak}` and the `agent.safe_stop.report` `verifier_missing` /
//! `verifier_weak` `stop_reason`s all originate from the completion gate
//! (`CompletionPolicy::{verification_required, test_execution_required}` →
//! `TaskContract::evaluate_inner`'s `Verify` / `SafeStop` branches) and the
//! post-loop dispatch (`success.rs`). For a verifier-free prose deliverable
//! those branches are unreachable, because:
//!   * `capability_for(kind)` clamps both policy fields to `false` for
//!     non-coding kinds (Authoring / Docs) and for the AnswerOnly project
//!     intent, so `evaluate_inner` never reaches `Verify` nor the
//!     `test_execution_required`-gated `SafeStop`; and
//!   * `post_loop_verifier_free_for_prose` collapses the three post-loop
//!     verifier inputs to `false` (Decision #8), so no `SafeStopVerifier*`
//!     ExitReason is produced post-loop.
//!
//! This module asserts those invariants directly against the production
//! functions (no LLM loop), which is deterministic and Ollama-free. Per
//! DR3-001 / CB-001 it is `#[cfg(test)]`-only and excluded from the production
//! binary; it is not facade re-exported. Precedent: `safe_stop_e2e_tests.rs`.

#[cfg(test)]
mod tests {
    use super::super::completion_evidence::{CompletionEvidence, EvidenceSet, RepoEditCategory};
    use super::super::success::post_loop_verifier_free_for_prose;
    use super::super::task_contract::{
        ArtifactRole, CompletionDecision, SafeStopReason, TaskContract, TaskKind,
    };

    /// The per-kind matrix (Decision #7): AnswerOnly / Authoring / Docs prose
    /// requests, plus the non-prose Coding control.
    fn matrix() -> Vec<(&'static str, TaskKind, bool)> {
        vec![
            // (request, expected_task_kind_or_dont_care, is_prose)
            (
                "Explain how the auth flow works",
                TaskKind::Coding, /* AnswerOnly intent */
                true,
            ),
            (
                "Translate README.ja.md into English and write README.md",
                TaskKind::Authoring,
                true,
            ),
            (
                "Update README.md with setup, usage, and test sections",
                TaskKind::Docs,
                true,
            ),
            (
                "Rewrite the intro paragraph in docs/intro.md to be clearer",
                TaskKind::Authoring,
                true,
            ),
            // Non-prose control: must NOT be asserted verifier-free.
            (
                "Implement slugify in Python and add pytest tests",
                TaskKind::Coding,
                false,
            ),
        ]
    }

    #[test]
    fn prose_kinds_are_verifier_free_on_the_completion_policy() {
        for (request, _kind, is_prose) in matrix() {
            let contract = TaskContract::from_request(request);
            if is_prose {
                assert!(
                    !contract.completion_policy.verification_required(),
                    "{request}: prose must not require verification"
                );
                assert!(
                    !contract.completion_policy.test_execution_required(),
                    "{request}: prose must not require test execution"
                );
                assert!(
                    post_loop_verifier_free_for_prose(&contract),
                    "{request}: post-loop prose predicate must fire"
                );
            }
        }
    }

    /// The Coding control must still demand executable verification (regression
    /// guard so the prose suppression is not over-broad).
    #[test]
    fn coding_control_still_demands_verifier() {
        let contract =
            TaskContract::from_request("Implement slugify in Python and add pytest tests");
        assert_eq!(contract.task_kind, TaskKind::Coding);
        assert!(
            contract.completion_policy.test_execution_required()
                || contract.completion_policy.verification_required(),
            "coding control must still demand a verifier"
        );
        assert!(!post_loop_verifier_free_for_prose(&contract));
    }

    /// The `SafeStop{VerifierMissing}` / `{VerifierWeak}` reasons — which map to
    /// `ExitReason::SafeStopVerifier*` — are produced by `evaluate_inner` only
    /// behind `test_execution_required()`. For prose that gate is closed, so
    /// even an empty owned-test-artifact slice never yields a SafeStop.
    #[test]
    fn prose_never_safe_stops_on_missing_verifier() {
        for (request, _kind, is_prose) in matrix() {
            if !is_prose {
                continue;
            }
            let contract = TaskContract::from_request(request);
            let empty_owned: Vec<String> = Vec::new();
            // Empty evidence + empty owned slice: prose must not SafeStop.
            let decision =
                contract.evaluate_with_owned_test_artifacts(&EvidenceSet::new(), &empty_owned);
            assert!(
                !matches!(
                    decision,
                    CompletionDecision::SafeStop {
                        reason: SafeStopReason::VerifierMissing | SafeStopReason::VerifierWeak,
                        ..
                    }
                ),
                "{request}: prose must not SafeStop on missing/weak verifier (got {decision:?})"
            );
        }
    }

    /// Authoring completes once the accept-tier path-matched
    /// `ReportCompletenessPass` is recorded — never via a coding verifier, and
    /// never via a raw `RepoEdit(Docs)` (DR3-002).
    #[test]
    fn authoring_completes_via_accept_tier_not_coding_verifier() {
        let contract =
            TaskContract::from_request("Translate README.ja.md into English and write README.md");
        let paths: Vec<String> = contract
            .required_identities_for_role(ArtifactRole::UsageDocs)
            .iter()
            .map(|id| id.path.clone())
            .collect();
        assert!(!paths.is_empty());

        // Raw RepoEdit(Docs) for every path must NOT complete.
        let mut raw = EvidenceSet::new();
        for p in &paths {
            raw.push(CompletionEvidence::RepoEdit {
                category: RepoEditCategory::Docs,
                count: 1,
                path: Some(p.clone()),
            });
        }
        assert!(
            matches!(
                contract.evaluate_with_owned_test_artifacts(&raw, &[]),
                CompletionDecision::Continue { .. }
            ),
            "raw RepoEdit(Docs) must not complete Authoring"
        );

        // Path-matched accept-tier passes complete (Done, no verifier).
        let mut passes = EvidenceSet::new();
        for p in &paths {
            passes.push(CompletionEvidence::ReportCompletenessPass {
                path: Some(p.clone()),
            });
        }
        assert_eq!(
            contract.evaluate_with_owned_test_artifacts(&passes, &[]),
            CompletionDecision::Done
        );
    }

    /// Security pin: the static authoring diagnostic never interpolates a raw
    /// path / excerpt / section (Security §5). A secret-bearing path/excerpt
    /// must not surface in `VerifierDiagnostic::reason()`.
    #[test]
    fn authoring_diagnostic_reason_has_no_path_or_excerpt() {
        use super::super::verifier::authoring_accept_tier_diagnostic;
        let secret_path = "secrets/sk-ABCDEF1234567890token.md";
        // Short stub (< AUTHORING_MIN_CONTENT_CHARS) so the accept tier rejects
        // it; the secret token is embedded to prove it is not interpolated.
        let secret_excerpt = "sk-ABC";
        let diag =
            authoring_accept_tier_diagnostic(TaskKind::Authoring, secret_path, secret_excerpt, &[])
                .expect("stub must produce a diagnostic");
        let reason = diag.reason();
        assert!(
            !reason.contains("sk-ABCDEF1234567890token"),
            "diagnostic reason must not leak the secret token: {reason}"
        );
        assert!(
            !reason.contains("secrets/"),
            "diagnostic reason must not interpolate the raw path: {reason}"
        );
        // It is the documented static summary.
        assert!(reason.contains("authoring artifact is too short or missing requested sections"));
    }
}

/// Issue #919 (CB-001): integration-style coverage of the **production**
/// Write/Edit observation path (`observe_evidence_from_repo_edit`). The earlier
/// `#[cfg(test)] mod tests` block above proves the DR3-002 gate by *manually
/// injecting* `ReportCompletenessPass` into the evidence set — which masked the
/// real defect: the production observation path never promoted an edited
/// Authoring artifact to that pass, so a genuine "translate README.ja.md →
/// write README.md" stayed `Continue { UsageDocs }` until retry exhaustion.
///
/// These tests drive the real entry point (build a live `Agent` with a temp
/// `work_root`, seed an Authoring request, write a file, call
/// `observe_evidence_from_repo_edit`) and assert completion **without** any
/// manually-pushed pass. Deterministic / Ollama-free (the mockito URL is never
/// hit). Precedent: `artifact_ledger_phase5_tests.rs` / `bash_policy_e2e_tests.rs`.
#[cfg(test)]
mod production_path_tests {
    use std::sync::OnceLock;

    use tempfile::{TempDir, tempdir};

    use super::super::repo_edit_observation::observe_evidence_from_repo_edit;
    use super::super::task_contract::{ArtifactRole, CompletionDecision, TaskContract, TaskKind};
    use crate::agent::Agent;
    use crate::agent::loop_run::FooterHandle;
    use crate::config::Config;
    use crate::model_registry::RuntimeModels;
    use crate::ollama::client::OllamaClient;
    use crate::session::store::{SessionSnapshot, SessionStore};

    static LOG_DIR: OnceLock<TempDir> = OnceLock::new();

    fn ensure_logging() {
        let _ = LOG_DIR.get_or_init(|| {
            let dir = tempdir().expect("tempdir for shared log");
            let log_path = dir.path().join("llm-io.jsonl");
            let _ = crate::logging::init_logging(crate::config::LogLevel::Info, &log_path);
            dir
        });
    }

    fn unique_session_id(prefix: &str) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("919cb001-{prefix}-{nanos}")
    }

    /// Build a live `Agent` whose `work_root` is a fresh temp dir and whose
    /// active task is `request` (so `task_contract_authority` resolves to the
    /// Authoring contract the production observation hook reads).
    fn build_agent(session_id: &str, request: &str) -> (Agent, TempDir) {
        ensure_logging();
        let dir = tempdir().expect("tempdir for agent");
        let state_root = dir.path().join("state");
        std::fs::create_dir_all(state_root.join("sessions").join(session_id)).unwrap();

        let config = Config {
            cwd: dir.path().to_path_buf(),
            requested_model: Some("test-model".to_string()),
            state_dir_override: Some(state_root.clone()),
            yes_mode: true,
            max_iterations: 1,
            ..Config::default()
        };

        let workspace_key = format!("anvil-919cb001-{session_id}");
        let mut session = SessionSnapshot {
            id: session_id.to_string(),
            workspace_key: workspace_key.clone(),
            active_root: Some(dir.path().to_path_buf()),
            ..Default::default()
        };
        session.working_memory.active_task = Some(request.to_string());

        let server = mockito::Server::new();
        let agent = Agent::new(
            config,
            RuntimeModels {
                main: "test-model".to_string(),
                sidecar: None,
            },
            OllamaClient::new(server.url()).unwrap(),
            SessionStore::new(&state_root, session_id, &workspace_key),
            session,
            FooterHandle::disabled(),
        );
        (agent, dir)
    }

    /// The verbatim production request from the bug report. We assert it
    /// classifies as Authoring and exposes at least one UsageDocs obligation so
    /// the rest of the scenario is meaningful.
    const TRANSLATE_REQUEST: &str = "Translate README.ja.md into English and write README.md";

    fn usage_docs_paths(contract: &TaskContract) -> Vec<String> {
        let paths: Vec<String> = contract
            .required_identities_for_role(ArtifactRole::UsageDocs)
            .iter()
            .map(|id| id.path.clone())
            .collect();
        assert!(
            !paths.is_empty(),
            "Authoring request must yield a UsageDocs obligation"
        );
        paths
    }

    /// A sufficient translation body (>= AUTHORING_MIN_CONTENT_CHARS; the
    /// TRANSLATE_REQUEST obligations carry no requested sections).
    const SUFFICIENT_BODY: &str = "# Project\n\nRun the server with `cargo run`. This is the English translation of the original README.\n";

    /// CB-001 fix: sufficient real Writes of the required UsageDocs artifact(s),
    /// observed through the production entry point, promote each artifact to an
    /// accept-tier `ReportCompletenessPass` so the contract evaluates to `Done`
    /// — with NO manually-injected evidence.
    ///
    /// CB2-001 fix: the TRANSLATE_REQUEST models ONLY the deliverable
    /// `README.md` as a required UsageDocs identity — the source `README.ja.md`
    /// is an input and is NOT required. So writing ONLY `README.md` must reach
    /// `Done`; the source is never written nor observed. (The earlier version of
    /// this test wrote every path in `required_identities_for_role`, including
    /// the source, which masked CB2-001.)
    #[test]
    fn authoring_write_promotes_to_accept_tier_pass_in_production() {
        let session_id = unique_session_id("promote");
        let (mut agent, dir) = build_agent(&session_id, TRANSLATE_REQUEST);
        let work_root = dir.path();

        let contract = TaskContract::from_request(TRANSLATE_REQUEST);
        assert_eq!(contract.task_kind, TaskKind::Authoring);
        let paths = usage_docs_paths(&contract);
        // CB2-001: the source `README.ja.md` must NOT be a required identity.
        assert_eq!(
            paths,
            vec!["README.md".to_string()],
            "only the deliverable README.md is a required UsageDocs identity; the source README.ja.md is an input"
        );

        for rel in &paths {
            std::fs::write(work_root.join(rel), SUFFICIENT_BODY).unwrap();
            // Drive the REAL production observation path. No manual evidence push.
            observe_evidence_from_repo_edit(&mut agent, rel);
        }

        // Every required obligation must have a path-matched accept-tier pass
        // pushed by the production hook (not injected).
        for rel in &paths {
            assert!(
                agent.task_contract_evidence_set_this_turn.iter().any(|e| matches!(
                    e,
                    super::super::completion_evidence::CompletionEvidence::ReportCompletenessPass {
                        path: Some(p),
                    } if p == rel
                )),
                "production hook must push an accept-tier pass for {rel}"
            );
        }

        assert_eq!(
            contract.evaluate_with_owned_test_artifacts(
                &agent.task_contract_evidence_set_this_turn,
                &[],
            ),
            CompletionDecision::Done,
            "a sufficient Authoring write of ONLY the deliverable through the production path must complete without manual evidence injection"
        );
    }

    /// CB2-001 regression: for `Translate README.ja.md into English and write
    /// README.md`, the required UsageDocs identities are EXACTLY `["README.md"]`
    /// — the source/input `README.ja.md` is excluded.
    #[test]
    fn translate_models_only_output_as_required_usagedocs() {
        let contract = TaskContract::from_request(TRANSLATE_REQUEST);
        assert_eq!(contract.task_kind, TaskKind::Authoring);
        let paths: Vec<String> = contract
            .required_identities_for_role(ArtifactRole::UsageDocs)
            .iter()
            .map(|id| id.path.clone())
            .collect();
        assert_eq!(
            paths,
            vec!["README.md".to_string()],
            "the translation source README.ja.md must not be a required deliverable"
        );
    }

    /// CB2-001 regression: a translation reaches `Done` after writing ONLY the
    /// deliverable `README.md` through the real observation path — the source
    /// `README.ja.md` is never written nor observed.
    #[test]
    fn translate_completes_after_writing_only_output_in_production() {
        let session_id = unique_session_id("translate-out");
        let (mut agent, dir) = build_agent(&session_id, TRANSLATE_REQUEST);
        let work_root = dir.path();

        let contract = TaskContract::from_request(TRANSLATE_REQUEST);
        assert_eq!(contract.task_kind, TaskKind::Authoring);

        // Write & observe ONLY the deliverable; the source is never touched.
        std::fs::write(work_root.join("README.md"), SUFFICIENT_BODY).unwrap();
        observe_evidence_from_repo_edit(&mut agent, "README.md");

        assert!(
            !agent
                .task_contract_evidence_set_this_turn
                .iter()
                .any(|e| matches!(
                    e,
                    super::super::completion_evidence::CompletionEvidence::ReportCompletenessPass {
                        path: Some(p),
                    } if p == "README.ja.md"
                )),
            "the source README.ja.md must never receive an accept-tier pass (it was never written)"
        );

        assert_eq!(
            contract.evaluate_with_owned_test_artifacts(
                &agent.task_contract_evidence_set_this_turn,
                &[],
            ),
            CompletionDecision::Done,
            "writing only the deliverable README.md must complete the translation"
        );
    }

    /// CB2-001 preservation: an in-place single-path authoring request models its
    /// one path as the required deliverable (no source cue, so it is kept).
    #[test]
    fn in_place_authoring_keeps_single_path_required() {
        let contract = TaskContract::from_request(
            "Rewrite the intro paragraph in docs/intro.md to be clearer",
        );
        let paths: Vec<String> = contract
            .required_identities_for_role(ArtifactRole::UsageDocs)
            .iter()
            .map(|id| id.path.clone())
            .collect();
        assert_eq!(
            paths,
            vec!["docs/intro.md".to_string()],
            "an in-place rewrite keeps its single docs path as the required deliverable"
        );
    }

    /// CB2-001 preservation: a true multi-output authoring request keeps BOTH
    /// docs paths required (neither is a source).
    #[test]
    fn multi_output_authoring_keeps_both_paths_required() {
        let contract = TaskContract::from_request("Write docs/intro.md and docs/faq.md");
        let mut paths: Vec<String> = contract
            .required_identities_for_role(ArtifactRole::UsageDocs)
            .iter()
            .map(|id| id.path.clone())
            .collect();
        paths.sort();
        assert_eq!(
            paths,
            vec!["docs/faq.md".to_string(), "docs/intro.md".to_string()],
            "a true multi-output authoring request keeps both docs paths required"
        );
    }

    /// A stub write ("TODO") fails the accept tier (< AUTHORING_MIN_CONTENT_CHARS),
    /// so the production path pushes NO pass and the contract stays
    /// `Continue { UsageDocs }` — the correct behavior the fix must preserve.
    #[test]
    fn authoring_write_stub_stays_continue_in_production() {
        let session_id = unique_session_id("stub");
        let (mut agent, dir) = build_agent(&session_id, TRANSLATE_REQUEST);
        let work_root = dir.path();

        let contract = TaskContract::from_request(TRANSLATE_REQUEST);
        let paths = usage_docs_paths(&contract);

        for rel in &paths {
            std::fs::write(work_root.join(rel), "TODO").unwrap();
            observe_evidence_from_repo_edit(&mut agent, rel);
        }

        // No accept-tier pass may be pushed for any path.
        assert!(
            !agent.task_contract_evidence_set_this_turn.iter().any(|e| matches!(
                e,
                super::super::completion_evidence::CompletionEvidence::ReportCompletenessPass { .. }
            )),
            "a stub Authoring write must NOT promote to an accept-tier pass"
        );
        assert!(
            matches!(
                contract.evaluate_with_owned_test_artifacts(
                    &agent.task_contract_evidence_set_this_turn,
                    &[],
                ),
                CompletionDecision::Continue {
                    missing,
                } if missing == vec![ArtifactRole::UsageDocs]
            ),
            "a stub Authoring write must keep the contract on Continue {{ UsageDocs }}"
        );
    }

    /// Behavior preservation: a Docs contract (NOT Authoring) writing the same
    /// README.md must NOT get an Authoring accept-tier pass pushed — Docs
    /// completes through its own `RepoEdit(Docs)` authority and the Authoring
    /// hook must be inert for non-Authoring kinds.
    #[test]
    fn docs_write_does_not_get_authoring_pass_in_production() {
        let request = "Update README.md with setup, usage, and test sections";
        let session_id = unique_session_id("docs");
        let (mut agent, dir) = build_agent(&session_id, request);
        let work_root = dir.path();

        let contract = TaskContract::from_request(request);
        assert_eq!(contract.task_kind, TaskKind::Docs);
        let paths = usage_docs_paths(&contract);

        let body =
            "# README\n\n## Setup\nInstall deps.\n\n## Usage\nRun it.\n\n## Test\nRun the tests.\n";
        for rel in &paths {
            std::fs::write(work_root.join(rel), body).unwrap();
            observe_evidence_from_repo_edit(&mut agent, rel);
        }

        // The Authoring hook must NOT have fired: no ReportCompletenessPass.
        assert!(
            !agent.task_contract_evidence_set_this_turn.iter().any(|e| matches!(
                e,
                super::super::completion_evidence::CompletionEvidence::ReportCompletenessPass { .. }
            )),
            "the Authoring accept-tier hook must be inert for a Docs contract"
        );
    }
}
