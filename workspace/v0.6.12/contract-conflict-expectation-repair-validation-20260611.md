# Contract-Conflict Expectation Repair Validation - 2026-06-11

## Context

After typed `assert_not_equal_failed` support, the focused 10-run validation
improved to:

- pass: 10/10
- high_quality: 9/10
- repair_exhausted: 1
- max_iterations: 0
- shadow_conflict: 0

The remaining non-HQ row was a generated-test inconsistency case. The verifier
repair eventually proposed changing a generated test expectation to align with
an implementation/usage-docs contract conflict decision, but legacy weakening
validation still rejected the subject-preserving assertion update because it
was not tied to a single observed/expected pair.

## Implemented Slice

`repair_test_weakening_filter` now permits a narrow additional case:

- the active semantic plan is `test_bug`,
- the preferred repair role is `test`,
- the spec authority is not `llm_generated_test`,
- `ContractConflictJob` can classify a real inter-artifact conflict from the
  semantic report,
- assertion count is preserved,
- each changed assertion keeps the same assertion subject.

This keeps the change generic: the filter does not know about Markdown,
headings, or return codes. It only uses typed semantic authority and assertion
structure.

## Deterministic Verification

Passed:

- `cargo fmt`
- `cargo test repair_test_weakening_filter --lib`
- `cargo test failure_packet --lib`
- `cargo test repair_assertion_analysis --lib`
- `cargo test verifier_diagnostic --lib`
- `cargo build`

## Real LLM Validation

Command:

```bash
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --case-sequence python_markdown,python_markdown,python_markdown,python_markdown,python_markdown,python_markdown,python_sales,rust_word,docs_runbook,data_json \
  --variant no_pam \
  --run-id contract-conflict-expectation-repair-20260611 \
  --anvil-bin target/debug/anvil \
  --timeout-secs 600 \
  --chat-timeout-secs 240
```

Result:

- pass: 10/10
- high_quality: 9/10
- verification_pass: 9/10
- repair_exhausted: 0
- max_iterations: 0
- shadow_conflict: 1

Compared with the previous run, this removes the remaining
`repair_exhausted`, but does not improve high quality beyond 9/10.

## Remaining Issue

The new non-HQ row is `python_markdown` with `verifier_failed` and
`shadow_conflict=true`.

Observed final artifact shape:

- implementation still has a behavior bug: heading level is calculated from
  `len(m.group(0).lstrip("#"))`, which returns the length of the whitespace
  after the hashes instead of the hash count;
- tests also expect diagnostics to be printed to stderr, which the task did not
  explicitly require;
- after several repair attempts, diagnostic target selection failed with
  `diagnostic_target_missing`.

This is no longer a weakening-filter problem. It is now a repair-target
selection / diagnostic recovery problem after mixed implementation and
generated-test defects remain.

## Assessment

The slice is directionally useful but should be treated as a convergence
improvement, not a headline success-rate improvement:

- it reduces repair lifecycle exhaustion;
- it keeps the rule bounded by typed semantic authority;
- it does not introduce task-specific pattern matching;
- it leaves the next bottleneck visible: after partial repair, diagnostic
  target admission can still fail to reselect a safe implementation target for
  obvious implementation bugs.

Next work should focus on diagnostic target selection after partial repair:
when verifier output names implementation-facing behavior failures and safe
implementation candidates are already changed, the controller should not end in
`diagnostic_target_missing` without either selecting the changed implementation
target or emitting a stronger typed reason why no safe target exists.
