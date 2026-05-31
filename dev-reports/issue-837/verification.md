# Issue 837 Verification

## Focused

- `cargo test project_skeleton --lib` - passed
- `cargo test materialize_scaffold_writes_rust_cli_skeleton --lib` - passed

## Required

- `cargo fmt --check` - passed
- `cargo clippy --all-targets --all-features` - passed
- `cargo test` - initial sandbox run failed because mockito local server binding was denied with `Operation not permitted`; rerun with escalation passed
- `cargo build --release` - passed
