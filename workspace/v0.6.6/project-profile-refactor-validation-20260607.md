# Project Profile Refactor Validation - 2026-06-07

## Scope

- Extracted project language / shape inference out of `task_contract.rs` into `project_profile.rs`.
- Added a bounded LLM project-profile confirmation parser/prompt boundary so future controller wiring can consume semantic profile JSON without growing `TaskContract` string rules.
- Split README `setup` section wording from environment setup/bootstrap intent.
- Guarded `SetupBootstrap` projection fallback so Docs / Authoring / Data / Research artifacts are not forced into Bash-only setup mode.
- Extended implementation non-goal detection to cover `source code` phrasing.

## Static Validation

- `cargo fmt --all -- --check`: pass
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo test --lib`: pass, 3846 tests

## Local LLM Validation

Model:

- `qwen3.6:27b-coding-mxfp8`

### Docs-Only Case

Prompt:

```text
Write README.md with setup, usage, and troubleshooting sections for a small local backup CLI. Do not create source code.
```

Observed before the fix:

- `setup` section text was interpreted as `setup_bootstrap`.
- The model was restricted to Bash-only setup policy and failed.
- After the first fix, the model wrote `README.md` but TaskContract incorrectly requested `implementation`, causing unwanted `backup_cli.py` and tests.

Observed after the fix:

- Completed in 1 iteration.
- Wrote only `README.md`.
- No source files or tests were created.

Workspace:

- `/private/tmp/anvil-profile-docs-B694gE`

### Rust/TDD Case

Prompt:

```text
Create a Rust library in src/lib.rs implementing pub fn slugify(input: &str) -> String. Use TDD by adding tests, and verify with cargo test.
```

Observed:

- The model created `src/lib.rs`, `tests/lib.rs`, and `Cargo.toml`.
- Anvil stopped with `safe_stop_verifier_missing`.
- Manual `cargo test` failed because the generated implementation called `.dedup()` on an iterator without a supporting trait/import.

Interpretation:

- The project-profile refactor did not regress coding artifact creation.
- The remaining failure is in code generation / repair-verifier convergence, not in docs/setup misclassification.
