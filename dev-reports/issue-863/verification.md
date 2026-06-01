## Verification

- `cargo fmt -- --check` passed.
- `cargo test task_contract::tests:: --lib` passed: 85 tests.
- `cargo clippy --all-targets -- -D warnings` passed.
- `cargo test --lib` passed under escalation: 3156 tests.
- `cargo test` passed under escalation, including integration tests and
  doctests.

Note: the sandboxed `cargo test --lib` run failed before escalation because
mockito-backed tests could not bind local sockets (`Operation not permitted`).
The same suite passed with local socket permissions.
