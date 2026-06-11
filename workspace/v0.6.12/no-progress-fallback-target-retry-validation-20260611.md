# No-progress fallback target retry validation

Date: 2026-06-11

## Context

The fixed-head residual check found a `python_markdown` row where the external
grader passed, but Anvil stopped with `verifier_failed` after the verifier
diagnostic repeatedly selected `markdown_lint.py` and the controller reported
`diagnostic_target_missing`.

The important failure mode was not weak model diagnosis. The diagnostic LLM
selected the implementation target correctly. The controller filtered the
no-progress-banned implementation target, saw an unrelated changed test
candidate, and therefore did not activate the existing no-progress fallback.
That produced `diagnostic did not identify a safe repair target`.

## Change

`model_assessment_to_verifier_repair_assessment` now applies the existing
no-progress fallback when the final target selection is empty, rather than only
when every candidate pool is empty. This preserves the previous behavior when a
fresh alternate is available, but lets an explicit high-confidence diagnostic
target be retried when the only remaining candidates are not actually selectable
for the current diagnostic role.

This is intentionally generic:

- no Markdown-specific logic
- no new failure-string matching
- no relaxation of test weakening gates
- no change to repair admission or patch validation

## Focused Validation

Commands:

```text
cargo fmt
cargo test verifier_diagnostic_no_progress_fallback_ignores_unrelated_changed_candidate --lib
cargo test verifier_diagnostic_rejects_no_progress_banned_target_reselection --lib
cargo test verifier_diagnostic_keeps_only_safe_target_after_no_progress_bans_all_candidates --lib
cargo test verifier_diagnostic_stale_assertion_retargets_test_after_failed_non_test_repair --lib
cargo test verifier_diagnostic_stale_assertion_retargets_test_after_improved_non_test_repair --lib
cargo build
git diff --check
cargo test --lib
```

Result: all passed. The full lib test result was 4259 passed, 0 failed.

## Real LLM Validation

Command:

```text
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --case-sequence python_markdown,python_markdown,python_markdown,python_markdown,python_markdown,python_markdown,python_sales,rust_word,docs_runbook,data_json \
  --variant no_pam \
  --run-id no-progress-fallback-target-retry-20260611 \
  --out-root workspace/v0.6.12/eval-runs \
  --anvil-bin target/debug/anvil \
  --timeout-secs 600 \
  --chat-timeout-secs 240
```

Summary:

- total: 10/10 pass, 10/10 high_quality
- python_markdown: 6/6 pass, 6/6 high_quality
- guard cases: python_sales, rust_word, docs_runbook, data_json all passed high_quality
- verification_pass: 10/10
- false_done: 0
- false_missing: 0
- repair_exhausted: 0
- `diagnostic_target_missing`: 0 log hits
- `max_iterations`: 1 row, but shadow terminal was success and high_quality was true

## Residual Insight

`ambiguous_authority` retry events still appear in several `python_markdown`
runs, but they no longer force the target-selection safe stop in this sample.
The remaining issue is therefore not this no-progress fallback boundary; it is
the upstream diagnostic brief/admission mismatch that sometimes produces an
initial ambiguous repair action before a later diagnostic/repair succeeds.

Next useful improvement should keep the same architecture direction:

- preserve LLM semantic diagnosis
- keep controller admission typed and conservative
- improve the legacy brief adapter so `wrong_semantics` + behavior contract can
produce an implementation repair action without an initial ambiguous rejection
- avoid adding task-specific string patterns
