Issue #866 Implementation Summary

Changed completion authority:
- Act-mode final prose no longer becomes `Done` when a task contract is active unless `ArtifactRecoveryAction::Done` was already reached.
- Non-done contract actions now fail closed at the final completion gate; safe-stop actions preserve their specific exit reason.

Changed contract policy:
- Code-shaped project requests (`Cli`, `Library`, `Api`, `WebApp`) now require verifier evidence by default.
- Documentation-shaped and setup-only requests remain artifact-only, preserving docs-only README completion without verifier availability.

Changed eval logging:
- Added bounded `completion_reason` to `EvalRecord`.
- Successful turns classify the reason as verifier evidence, artifact obligations, or answer/plan completion.
- Non-success turns mirror the terminal outcome label.

Tests added/updated:
- Python CLI `main.py` only reaches `Verify`, not `Done`.
- Node CLI `package.json` only still misses implementation.
- Docs-only README with usage/setup/verification surface reaches `Done` without verifier.
- Actor-loop final completion gate allows only contract `Done`.
- Eval-log smoke tests assert `completion_reason`.
