Issue #905 Implementation Summary

Implemented the smallest coherent guard for PAM advisory-only completion:

- Added `is_deterministic_completion_authority_evidence` in `task_contract.rs`.
- Routed `CompletionPolicy::accepts_evidence`, `observed_artifacts`, and identity-specific evidence checks through that explicit deterministic authority predicate.
- Added `issue905_pam_completion_tests.rs`, an in-crate regression that records live PAM advice on a real `Agent` and verifies:
  - PAM eval remains `advisory_only=true`.
  - PAM eval keeps `completion_judgement_override=false`.
  - No completion evidence is recorded by PAM.
  - `TaskContract` and its recovery planner continue missing implementation evidence instead of producing `Done`.
- Added a TaskContract unit pin that all current `CompletionEvidence` variants are deterministic only because they are explicitly listed.

No provider abstraction, release workflow, or old architecture paths were changed.
