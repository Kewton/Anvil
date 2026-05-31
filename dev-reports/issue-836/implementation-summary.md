# Issue 836 Implementation Summary

## Changed

- Added `ProjectIntent`, `ProjectLanguage`, `ProjectShape`, and `VerificationRequirement` to `src/agent/loop_run/task_contract.rs`.
- Reused the existing `TaskIntent` enum and `f32` confidence value, per issue constraints.
- Updated `TaskContract::from_request` to build `ProjectIntent` first and derive `intent` / `verification_required` from that projection.
- Kept artifact-role derivation in the existing projection layer and avoided adding new `request_*` predicates.
- Removed the old `request_asks_for_verification` helper; verification is now projected from `ProjectIntent.verification`.
- Cleared `required_behavior.required_artifacts` and `required_behavior.verification` inside `TaskContract::from_request` so the runtime `TaskContract` does not carry a nested duplicate artifact / verification SSOT.

## Tests

- Added focused unit coverage for the known failure prompt shapes:
  - Rust CLI word counter.
  - Node CLI JSON formatter.
- Each test verifies `ProjectIntent` classification, preferred runner projection, `TaskContract` artifact projection, verification projection, and absence of nested behavior artifact / verification duplicates.
