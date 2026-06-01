# Issue 865 Implementation Summary

## Changes

- Added `src/agent/loop_run/verifier.rs` with a local `Verifier` trait and `CodingVerifier`, `DocsVerifier`, and `DataVerifier` adapters.
- Added docs required-section completion evidence via `CompletionEvidence::RequiredSectionsPass`.
- Wired docs completion policy, protocol satisfaction, artifact observation, and required artifact identity checks to accept required-section pass evidence.
- Updated structured coding verifier adapters:
  - Rust `VerifierCommand::from_cargo_test` now runs full `cargo test` while retaining owned test artifact metadata for validation.
  - Python stdlib pytest command now uses `python3 -m pytest -q -p no:cacheprovider`.
  - Node package-script verification now uses full `npm test`.
- Added `RunnerKind::Npm` so `npm test` structured invocations can be represented in verifier-invoked snapshots.
- Updated README verifier documentation.

## Tests

- Added verifier adapter tests for docs section evidence, generic failure packet shape, and coding pass evidence.
- Added/updated task-contract and protocol tests for `RequiredSectionsPass`.
- Updated auto-test tests for full `cargo test`, canonical Python pytest, and `npm test` behavior.
