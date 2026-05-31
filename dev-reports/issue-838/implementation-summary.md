## Implementation Summary

- Added `ProjectIntent` and `CompletionPolicy` in `task_contract.rs`.
- `TaskContract::from_request` now derives a single completion policy from the
  request-backed required artifacts, verification requirement, and
  `RequiredBehaviorContract.test_execution_required`.
- `TaskContract::evaluate_inner` now reads required artifacts, verifier
  requirement, and test-execution requirement through `CompletionPolicy`.
- Protocol evidence satisfaction now consumes the active request's
  `CompletionPolicy`, so docs-only, artifact-only, impl+test, and
  impl-without-test shapes do not rely on scattered `ProtocolKind` category
  checks alone.
- Updated direct `TaskContract` test fixtures to include the derived
  `completion_policy` field.

## Tests Added

- Task-contract policy classification for:
  - docs-only README
  - artifact-only pytest request
  - implementation with tests
  - implementation without tests
- Protocol regressions for:
  - docs-only README evidence satisfying GenericCode completion
  - pytest-pass artifact-only verifier evidence avoiding `missing_repo_edits`
  - documentation evidence not satisfying an implementation request
