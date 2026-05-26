# v0.4.25 Evaluation Results

## Verification

Commands run after the first cleanup slice:

- `cargo fmt --check`: pass
- `cargo test loop_control_action_tests --lib -q`: pass, 17 tests
- `cargo test build_safe_stop_payload --lib -q`: pass, 7 tests
- `cargo test repair_rejection_next_action --lib -q`: pass, 1 test
- `cargo test --lib -q`: pass, 2989 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass

Additional verification after the repair-admission extraction:

- `cargo fmt --check`: pass
- `cargo test repair_plan_admission --lib -q`: pass, 3 tests
- `cargo test recovery_owner_gates_lower_level_fallbacks --lib -q`: pass, 1 test
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo test --lib -q`: pass, 2993 tests when run outside the sandbox

Additional verification after patch-admission extraction:

- `cargo fmt --check`: pass
- `cargo test repair_patch_validation --lib -q`: pass, 3 tests
- `cargo test accepted_plan --lib -q`: pass, 8 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 2996 tests when run outside the sandbox

Additional verification after recovery gate + intent-input extraction:

- `cargo fmt --check`: pass
- `cargo test loop_control --lib -q`: pass, 5 tests
- `cargo test recovery_owner_gates_lower_level_fallbacks --lib -q`: pass
- `cargo test repair_patch_validation --lib -q`: pass, 6 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 2987 tests when run outside the sandbox

Additional verification after duplicate-binding guard extraction:

- `cargo fmt --check`: pass
- `cargo test repair_patch_validation --lib -q`: pass, 8 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 2987 tests when run outside the sandbox

Additional verification after filesystem target + cheap-content extraction:

- `cargo fmt --check`: pass
- `cargo test repair_patch_validation --lib -q`: pass, 13 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 2992 tests when run outside the sandbox

Additional verification after candidate-application extraction:

- `cargo fmt --check`: pass
- `cargo test repair_patch_validation --lib -q`: pass, 15 tests
- `cargo test validate_verifier_repair_intents --lib -q`: pass, 24 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `cargo test --lib -q`: pass, 2994 tests when run outside the sandbox

Note:

- The first sandboxed `cargo test --lib -q` run failed because many tests use
  `mockito`, and the sandbox blocks the local test server with
  `Operation not permitted`. The same command passed when rerun with normal
  permissions.

## Smoke: no-PAM-001

Directory:

```text
/private/tmp/anvil-v0425-eval/no-pam-001
```

Prompt:

```text
PythonでCSVファイルを読み込み、列ごとの合計を返すCLIを開発してください。README.mdとpytestで動くテストも実装してください。
```

Observed:

- Generated:
  - `main.py`
  - `tests/test_main.py`
  - `README.md`
- Ran verifier after all required artifacts were present.
- Entered verifier diagnostic and repair.
- Did not produce false-positive `done`.
- Terminated as controlled `repair_safe_stop`:

```text
verifier repair safe stop after rejected patch: verifier_repair_pass_invalid: repair plan rejected: role_mismatch
```

Assessment:

- Good: verifier repair remained controlled; it did not fall through to generic
  `missing_repo_edits`.
- Good: no wrong-stack or no-verifier `done`.
- Gap: the console safe-stop text was not actionable enough. After this smoke,
  the exit message was updated to include `next_action`, and a unit test now
  anchors the role-mismatch guidance.

## Current Quality Read

This slice improves structural control and report quality, but it is not a full
v0.4.25 completion. Repair admission is now a smaller boundary, and lower-level
fallback ownership is pinned in the arbiter. The remaining blocker is repair
convergence and target/role agreement, not false-positive completion.

The next evaluation step should run the same no-PAM case again plus at least:

- Rust library
- Node CLI/package
- missing verifier/setup
- ambiguous assertion

PAM evaluation should wait until the no-PAM 5-case gate has no false-positive
`done` and no generic retry takeover during verifier repair.

## Smoke: no-PAM-002

Directory:

```text
/Users/maenokota/share/work/localwork/anvilv0.4/anvilwork/v0425-continue-001
```

Prompt:

```text
Rustで文字列の単語数を数える小さなライブラリを開発してください。README.mdとテストも実装してください。
```

Observed:

- Generated:
  - `src/lib.rs`
  - `tests/lib.rs`
  - `README.md`
  - `Cargo.toml`
  - `Cargo.lock`
- Initially reached `Verification missing`, then added `Cargo.toml`.
- First verifier run failed; diagnostic malformed once, fallback diagnostic
  was accepted.
- Controller repair edited `tests/lib.rs`.
- Repair-job verifier rerun passed:

```text
Completed requested repository changes and verified them with cargo test --test lib.
```

Assessment:

- Good: non-FastAPI Rust-library task reached verified `done`.
- Good: missing verifier/setup was handled without false-positive `done`.
- Good: verifier repair stayed inside the repair-job path and completed.
- Residual: diagnostic still produced one malformed reply before fallback, so
  diagnostic schema robustness remains a quality issue.
