# Issue 899 Verification

## Focused Tests

- `cargo test negated_code_and_tests_phrases_do_not_require_code_or_tests`:
  passed.
- `cargo test coding_task_that_requires_tests_requires_verifier_evidence`:
  passed.
- `cargo test negated_docs_only_policy_context_completes_without_tests`:
  passed.

## Required Checks

- `cargo fmt --check`: failed before formatting on rustfmt line wrapping only.
- `cargo fmt`: applied rustfmt.
- `cargo fmt --check`: passed.
- `cargo clippy --all-targets`: passed.
- `cargo test`: failed inside the sandbox because tests that start local
  `mockito` HTTP servers hit `Operation not permitted`.
- `cargo test` with escalated permissions: passed.
