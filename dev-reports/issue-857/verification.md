Issue #857 Verification

Focused checks:
- `bash -n scripts/bench.sh tests/scripts/test_bench_smoke.sh`
- `bash tests/scripts/test_bench_smoke.sh`
- `cargo test --test eval_log_smoke`
- `cargo test --test photon_eval_log_smoke`
- `cargo test pam_advisory --lib`

Required checks:
- `cargo fmt --check`
- `cargo clippy --all-targets --all-features`
- `cargo test`

Notes:
- The first sandboxed `cargo test` run failed because mockito tests could not start local HTTP servers (`Operation not permitted`). Re-running the same command with elevated sandbox permission passed.
