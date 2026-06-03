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
                        reason: SafeStopReason::VerifierMissing | SafeStopReason::VerifierWeak
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
