# Not-Equal Expectation Repair Validation - 2026-06-11

## Context

The previous `no-progress-fallback-target-20260611` validation showed that
the no-progress fallback target admission was directionally useful but unsafe
as a standalone improvement:

- pass: 9/10
- high_quality: 4/10
- repair_exhausted: 2
- max_iterations: 2
- shadow_conflict: 4

The dominant failure was not target selection itself. The diagnostic pass
selected the generated test artifact, and the repair LLM proposed a bounded
test expectation edit. Legacy weakening validation rejected that edit as
`AssertionDeleted + LiteralOnlyExpectedChange`, even when the verifier output
showed a failed not-equal assertion such as `assert 0 != 0`.

## Implemented Slice

This slice keeps the controller boundary generic and avoids task-specific
markdown rules:

1. `FailurePacket` now records failed not-equal assertions as typed evidence:
   `assertion_shape=assert_not_equal_failed`.
2. `repair_assertion_analysis` can compare changed assertion operators as
   pure assertion structure, independent of task kind.
3. `repair_test_weakening_filter` permits only the narrow case where:
   - semantic authority allows generated test expectation alignment,
   - the verifier output contains a failed `assert value != value`,
   - the edit preserves the assertion subject and literal,
   - the edit changes only `!= value` to `== value`,
   - the assertion count is preserved.
4. The verifier repair prompt describes the same typed evidence boundary.
5. The no-progress fallback target admission remains in place, but the real
   improvement depends on the typed assertion evidence above.

This is not a markdown-specific patch. It is a generic assertion-expectation
repair rule for failed not-equal assertions under an already-admitted
test-bug/contract-authority repair path.

## Deterministic Verification

Passed:

- `cargo fmt`
- `cargo test failure_packet --lib`
- `cargo test repair_assertion_analysis --lib`
- `cargo test repair_test_weakening_filter --lib`
- `cargo test verifier_diagnostic --lib`
- `cargo test verifier_repair_shadow --lib`
- `cargo test repair_action --lib`
- `cargo test repair_authority --lib`
- `cargo build`

## Real LLM Validation

Command:

```bash
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --case-sequence python_markdown,python_markdown,python_markdown,python_markdown,python_markdown,python_markdown,python_sales,rust_word,docs_runbook,data_json \
  --variant no_pam \
  --run-id not-equal-expectation-repair-20260611 \
  --anvil-bin target/debug/anvil \
  --timeout-secs 600 \
  --chat-timeout-secs 240
```

Result:

- pass: 10/10
- high_quality: 9/10
- verification_pass: 9/10
- repair_exhausted: 1
- max_iterations: 0
- shadow_conflict: 0

Compared with the prior fallback-only run, this recovers Python markdown from
1/6 high quality to 5/6 high quality, and removes the repeated max-iteration
failure mode.

## Remaining Issue

The remaining non-HQ row is still `python_markdown`, but it is a different
shape:

- generated tests contained mixed downward-heading semantics:
  - `### H3 -> # H1` expected valid,
  - `#### H4 -> # H1` expected invalid.
- contract arbitration reported an actionable `fix_test` decision.
- repair still exhausted after invalid repair attempts, including candidate
  syntax failures and a later weakening rejection.

This means the current slice fixed failed not-equal expectation admission, but
there is still a residual conflict-resolution problem when generated tests
contain partially inconsistent examples rather than a single observed/expected
literal mismatch.

## Assessment

This change moves the architecture in the intended direction:

- typed evidence is extracted once in `FailurePacket`;
- assertion structure is handled by a pure analysis module;
- weakening exceptions remain bounded and authority-gated;
- no markdown-specific behavior was introduced;
- real LLM validation improved without increasing rule sprawl materially.

The next improvement should not add more task-specific patterns. It should
focus on a typed conflict-resolution path for internally inconsistent generated
tests, probably by carrying contract-arbitration decisions into repair action
admission and weakening validation instead of relying on post-hoc prompt text.
