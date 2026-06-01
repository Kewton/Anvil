# Issue 856 Implementation Summary

## Changes

- Docs-only completion now suppresses coding verifier and test-execution gates at the `CompletionPolicy` layer, so README/manual-only edits can reach `Done` after the documentation obligation is satisfied.
- Structured verifier execution now performs generated-test preflight before running the full verifier:
  - Python bound tests run `python -B -m py_compile`.
  - Rust top-level integration tests run `cargo test --no-run --test <name>`.
- Verifier repair prompts now carry `previous_failure_signature` and a bounded `repeated_failure_invariant` when the same signature recurs, with prompt text instructing the repair model to keep edits constrained to that invariant and selected target.
- Eval terminal diagnostics now include:
  - `satisfied_obligations`
  - `missing_obligations`
  - `verifier_status`
  - `last_failure_signature`
- `repair_exhausted` diagnostics now distinguish likely `model_output_failure`, `verification_environment_failure`, and `control_loop_failure` from available terminal evidence.

## Tests Added

- Docs-only "check README" completion reaches `Done` without a coding verifier.
- Structured Python verifier preflights invalid generated-test syntax before pytest.
- Repeated verifier failure signatures appear as a narrow repair invariant in the repair prompt.
- `repair_exhausted` terminal diagnostics classify model-output, verifier-environment, and control-loop domains.
