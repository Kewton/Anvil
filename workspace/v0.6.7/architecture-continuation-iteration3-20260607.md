# v0.6.7 Architecture Continuation Iteration 3 - 2026-06-07

## Scope

This iteration continued from
`architecture-continuation-iteration2-20260607.md`.

The focus was the hard Rust TDD path where the controller had already improved
from `safe_stop_verifier_missing` into diagnostic repair, but still failed to
converge because correct repair intent was blocked by parser, target admission,
or cheap-check boundaries.

The validation used actual local LLM runs with Ollama:

- main model: `qwen3.5:9b`
- sidecar model: `qwen3.5:2b`
- target: Rust library crate `is_palindrome`
- required behavior: create `Cargo.toml`, `src/lib.rs`, integration tests, run
  `cargo test`, and keep fixing until tests pass

## Implemented Small Steps

### 1. Generic patch proposal normalization boundary

The repair editor repeatedly produced semantically useful JSON with small schema
shape drift:

- missing opening quote around a schema key such as `new_string`
- one surplus array closer after the `edits` array
- both defects in the same response

The fix is intentionally bounded to the patch proposal schema boundary:

- extract a JSON object normally first
- if extraction or parsing fails, apply small normalization over known schema
  keys
- drop only unmatched surplus `]` tokens outside strings
- always pass the normalized result through typed proposal validation and
  target/content safety checks

This is not task-specific repair. It does not rewrite Rust, infer a benchmark,
or special-case palindrome. It only makes the LLM-to-typed-JSON boundary more
tolerant of common local model formatting drift.

### 2. Keep generated tests bound when the manifest is invalid

The previous guard rejected generated tests when the manifest was invalid. That
made the controller lose all bound verifier evidence and fall into
`safe_stop_verifier_missing`.

The new behavior keeps the test artifact admitted for verifier binding while
also recording the invalid manifest diagnostic. A broken manifest is setup
evidence, not proof that the generated test is unsafe.

Direct single-test preflight remains strict; only the batch verifier binding path
keeps the test bound so the setup failure can be routed into diagnostic repair.

### 3. Admit setup syntax targets for diagnostic repair

The diagnostic LLM sometimes classifies a broken `Cargo.toml` as
`compile_or_syntax_error` rather than `invalid_manifest`. That classification is
reasonable for local LLMs because the verifier output is a compiler/toolchain
syntax failure.

`CompileOrSyntaxError` now allows setup targets when the target path is a setup
artifact such as `Cargo.toml`.

This is role-aware admission broadening. It does not decide based on prompt text
or file-name string matching alone; the existing artifact role classification is
still the authority.

### 4. Defer coding cheap-check unavailable to the full verifier

A correct edit to an integration test can be rejected by a single-file Rust
syntax/content cheap check when the file needs crate context. In the previous
state, this produced `cheap_check_unavailable`, recorded a `no_safe_candidate`,
and then banned the correct target.

For coding tasks only, `RepairCandidateContentError::Unavailable` is now allowed
to defer to the full verifier after the edit has passed target and patch safety
checks. Non-coding tasks still fail closed on unavailable cheap checks.

This keeps the full evidence runner as the authority and avoids turning an
incomplete cheap check into a hard target ban.

## Actual LLM Validation

### Run 10: verifier binding failure still present

Workspace: `/private/tmp/anvil-v067-tdd-10`

Result:

- terminal state: `safe_stop_verifier_missing`
- generated `src/lib.rs`, `tests/palindrome.rs`, and `Cargo.toml`
- `tests/palindrome.rs` had missing `fn`
- `Cargo.toml` was an invalid virtual manifest
- direct `cargo test` failed on manifest shape

Key observation:

The controller had a safe verifier command, but invalid manifest preflight
rejected the generated test binding. That made the verifier disappear from the
contract lifecycle.

Change driven:

- keep generated test bound when invalid manifest is setup evidence

### Run 11: setup target was diagnosed but not admitted

Workspace: `/private/tmp/anvil-v067-tdd-11`

Result:

- no longer stopped at missing verifier
- diagnostic and repair loop started
- LLM correctly selected `Cargo.toml` / setup after verifier failure
- diagnostic used `compile_or_syntax_error`
- setup target was not admitted for that failure kind
- terminal state: `verifier_failed` / `diagnostic_unavailable`

Change driven:

- allow setup targets for `CompileOrSyntaxError`

### Run 12: correct test repair was blocked by cheap-check unavailable

Workspace: `/private/tmp/anvil-v067-tdd-12`

Result:

- manifest/setup path converged
- direct failure moved to `tests/palindrome.rs` missing `fn`
- diagnostic LLM selected `tests/palindrome.rs` / test
- repair editor proposed a correct multi-edit patch adding `fn`
- shadow validation accepted the patch
- legacy cheap check returned unavailable
- controller recorded `no_safe_candidate`, banned the correct target, and ended
  `diagnostic_unavailable`

Change driven:

- coding unavailable cheap checks defer to the full verifier

### Run 13: hard Rust TDD passed with actual LLM

Workspace: `/private/tmp/anvil-v067-tdd-13`

Command:

```text
anvil --model qwen3.5:9b --sidecar-model qwen3.5:2b \
  --state-dir /private/tmp/anvil-v067-state \
  --max-iterations 45 --chat-timeout-secs 180 --yes --oneshot --no-footer \
  -p 'Create a Rust library crate that exposes pub fn is_palindrome(input: &str) -> bool. Use TDD: create Cargo.toml, src/lib.rs, and tests/palindrome.rs with integration tests for mixed case, spaces/punctuation, empty string, and a negative case. Run cargo test and keep fixing until all tests pass.'
```

Result:

- completed in 13 iterations
- generated `Cargo.toml`, `src/lib.rs`, `tests/palindrome.rs`, and `Cargo.lock`
- initial verifier failed on missing `fn` in integration tests
- first repair proposal was invalid and retried
- second invalid repair caused diagnostic re-run
- diagnostic selected the test file again
- controller-applied repair edited `tests/palindrome.rs`
- final verifier command was `cargo test`
- final verifier exit code was `0`

Direct confirmation:

```text
cargo test
running 4 tests
test empty_string ... ok
test mixed_case ... ok
test negative_case ... ok
test spaces_and_punctuation ... ok
test result: ok. 4 passed; 0 failed
```

This is the first validation in this sequence where the difficult Rust TDD
task moved from missing verifier / failed repair convergence to a full local
LLM pass.

## Verification

Unit and build checks:

- `cargo test post_apply_candidate_defers_coding_unavailable_to_full_verifier --lib`
- `cargo test post_apply_candidate_keeps_non_coding_unavailable_fail_closed --lib`
- `cargo test patch_proposal --lib`
- `cargo test generated_test_guard --lib`
- `cargo test verifier_repair_targeting --lib`
- `cargo test verifier_orchestration --lib`
- `cargo check --lib`
- `cargo build`

All passed.

Actual LLM checks:

- Rust TDD run 10: failed at verifier binding, drove setup-evidence binding fix
- Rust TDD run 11: failed at setup target admission, drove setup syntax target fix
- Rust TDD run 12: failed at cheap-check unavailable target ban, drove verifier
  authority fix
- Rust TDD run 13: passed with actual local LLM and direct `cargo test`

## Architecture Assessment

The architecture moved closer to the target direction.

The important change is not that one palindrome run passed. The important change
is that each failure was pushed to a typed boundary:

- generated-test evidence binding
- diagnostic target admission
- patch proposal schema normalization
- cheap-check versus full-verifier authority

This is materially different from adding task-specific prompt text or a
palindrome-specific rule. The controller still asks the LLM for semantic
diagnosis and repair intent, then uses typed contracts and bounded validators to
decide whether the proposal can be applied.

## Remaining Concerns

### 1. Logging still records stale repair-exhausted reports

Run 13's session state shows final verifier exit code `0`, but
`job-reports.jsonl` still contains an earlier `repair_exhausted` snapshot. The
runtime behavior is correct, but the reporting surface can mislead analysis.

Next direction:

- final terminal reports should emit a final-state record after successful
  verifier rerun
- stale safe-stop snapshots should be distinguishable from terminal summaries

### 2. Patch proposal normalization must stay a boundary, not become a rule pile

The current normalization is acceptable because it is schema-bound and still
requires typed validation. It should not grow into language-specific string
rewrites.

Next direction:

- if more drift appears, prefer a generic structured-output repair pass or
  sidecar reformatting pass over per-key ad hoc expansion
- keep proposal safety checks and full verifier authority unchanged

### 3. Hard validation needs a broader matrix

This iteration proved the Rust TDD path can now converge in one difficult case,
but that is not enough to exclude chance.

Next hard checks should include:

- existing-code feature improvement
- Node CLI coding with generated tests
- Python CLI coding with generated tests
- docs-only objective
- data transformation objective
- ops command-observation objective

The current architecture direction remains valid only if those tasks improve
without reintroducing false-done or coding-only assumptions.

## Direction Update

Continue the same architectural direction:

1. LLM interprets semantic failures and proposes repair intent.
2. Controller converts that intent into typed contract/target/evidence state.
3. Bounded validators enforce safety and ownership.
4. The full evidence runner, not a weak cheap check, decides coding completion.
5. Non-coding tasks keep fail-closed safety unless an objective-specific evidence
   runner can validate the result.

P0 after this iteration:

- fix stale final reporting so analysis can trust terminal reports
- expand actual LLM validation beyond Rust TDD
- keep contract/evidence responsibilities separated as more task kinds are
  added
