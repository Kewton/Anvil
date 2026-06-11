# Python Markdown Residual Validation - 2026-06-11

## Scope

This slice validates two small, general-purpose changes aimed at the Python markdown residuals observed from fixed HEAD `c500d89`:

1. Keep generated tests from asserting exact human-readable diagnostic text unless the ObjectiveContract declares it.
2. Let an accepted diagnostic repair plan start fresh instead of being blocked by stale patch rejection history from an earlier target attempt.

The changes are intentionally not specific to Markdown linting. They affect contract-bound generation guidance, test expectation audit guidance, and repair lifecycle state.

## Deterministic Checks

Passed:

- `cargo test contract_generation_expectations --lib`
- `cargo test contract_bound_generation --lib`
- `cargo test test_expectation_audit --lib`
- `cargo test repair_job_ --lib`
- `cargo build`

## Real LLM Checks

### Diagnostic Text Policy

Command:

```text
python3 workspace/v0.6.11/wp_eval_matrix.py --suite wp11 --case-sequence python_markdown,python_markdown,python_markdown,python_markdown,python_markdown,python_markdown,python_sales,rust_word,docs_runbook,data_json --variant no_pam --run-id diagnostic-text-policy-20260611 --anvil-bin target/debug/anvil --timeout-secs 600 --chat-timeout-secs 240
```

Result:

- Total: 10/10 pass, 8/10 high_quality
- Python markdown: 6/6 pass, 4/6 high_quality
- Residual: 2 Python markdown rows still ended as `evidence_repair_exhausted`.

The failed rows showed that diagnostic-text assertion drift was reduced, but not enough by itself. The LLM identified the implementation defect in the repair diagnosis, then the repair lifecycle stopped because earlier patch rejections remained active after a later accepted plan.

### Plan Accepted Repair Reset

Command:

```text
python3 workspace/v0.6.11/wp_eval_matrix.py --suite wp11 --case-sequence python_markdown,python_markdown,python_markdown,python_markdown,python_markdown,python_markdown,python_sales,rust_word,docs_runbook,data_json --variant no_pam --run-id plan-accepted-repair-reset-20260611 --anvil-bin target/debug/anvil --timeout-secs 600 --chat-timeout-secs 240
```

Result:

- Total: 9/10 pass, 9/10 high_quality
- Python markdown: 5/6 pass, 5/6 high_quality
- Guard cases: Python sales, Rust word, docs runbook, and data JSON all passed high_quality.
- `repair_exhausted`: 0
- `max_iterations`: 0

This supports the lifecycle hypothesis: once a diagnostic plan is accepted, stale patch rejection history should not keep blocking a new bounded patch attempt.

## Remaining Issue

The remaining failed Python markdown row is a different failure mode:

- Terminal: `verifier_failed`
- Safe stop reason: `diagnostic_target_missing`
- Missing reason: `all_candidates_unreadable`
- Failure evidence: pytest failures pointed at `tests/test_markdown_lint.py`, while the implementation also contained a real defect.

Observed generated implementation defects:

- `lint(content)` used `text.splitlines()` instead of `content.splitlines()`.
- Heading level extraction used `len(m.group(0).lstrip("#"))`, which measures the whitespace suffix instead of the number of leading `#` characters.

The important architectural insight is that the verifier failure was available, but diagnostic target selection could not authorize a repair target from the generated/written artifacts. The next improvement should not add Markdown-specific rules. It should make target selection treat owned, freshly written artifacts as safe diagnostic candidates, or explicitly schedule a bounded read of the candidate before safe-stopping.

## Conclusion

The current slice improved convergence and did not regress the sampled coding/docs/data guards. It also exposed the next residual boundary more clearly: diagnostic target selection still depends too much on read-history availability, even when the controller owns the generated artifacts and verifier output identifies the failure surface.
