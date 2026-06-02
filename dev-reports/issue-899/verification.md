# Verification

- `cargo test negated_code_and_tests_request_does_not_require_test_artifact --lib -q` — pass
- `cargo test request_explicitly_requires_tests_respects_negated_test_artifacts --lib -q` — pass
- `cargo test task_contract --lib -q` — pass outside sandbox
- `cargo clippy --all-targets -- -D warnings` — pass

Blocked items:

- None.

