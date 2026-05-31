## Verification

Focused checks:

- `cargo test task_contract::tests::completion_policy --lib` - passed
- `cargo test protocol::tests::completion_policy --lib` - passed
- `cargo test protocol::tests::evidence_set_satisfies_env_setup_context_matrix --lib` - passed
- `cargo test task_contract::tests::evaluate_required_test_with_unbound_verifier_exit_zero_does_not_done --lib` - passed
- `cargo test task_contract::tests --lib` - passed
- `cargo test protocol::tests --lib` - passed

Required checks:

- `cargo fmt --check` - passed
- `cargo clippy --all-targets --all-features` - passed
- `cargo test` - passed after rerun with permission for local mock HTTP servers
- `cargo build --release` - passed

Note: the first sandboxed `cargo test` attempt failed because existing
`mockito` tests could not start local HTTP servers:
`Operation not permitted (os error 1)`. The same command passed under the
approved escalated run.
