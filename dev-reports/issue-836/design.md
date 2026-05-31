# Issue 836 Design

## Context

Issue #836 asks for a `ProjectIntent` SSOT for upstream request classification, with `TaskContract` becoming a projection of that classification. The referenced `workspace/v0.4.27/structural-issues-remediation.md` file is not present in this worktree, so this design follows the GitHub issue body and existing `task_contract.rs` contracts.

## Approach

- Add `ProjectIntent` plus `ProjectLanguage`, `ProjectShape`, and `VerificationRequirement` in `task_contract.rs`, reusing the existing `TaskIntent` enum and `f32` confidence.
- Keep `ProjectIntent` free of artifact role lists. It owns only upstream classification: task intent, language, project shape, verification requirement, and confidence.
- Make `TaskContract::from_request` classify `ProjectIntent` first, then project the existing `TaskContract` fields from that value plus the existing artifact support predicates.
- Avoid adding new `request_*` predicates. Remove the local `request_asks_for_verification` helper and replace it with the `VerificationRequirement` projection so the request predicate count does not increase.
- Avoid duplicating artifact and verification ownership inside the `TaskContract` object by clearing the nested behavior schema's legacy `required_artifacts` and `verification` fields after deriving its modern labels and gates. `TaskContract.required_artifacts` and `TaskContract.verification_required` remain the operational projection.

## Tests

Focused unit tests will pin `ProjectIntent` classification for the known failing prompt shapes:

- Rust CLI word counter.
- Node CLI JSON formatter.

Additional regression tests will pin that `TaskContract::from_request` projects from `ProjectIntent` and does not retain nested artifact / verification duplicates.
