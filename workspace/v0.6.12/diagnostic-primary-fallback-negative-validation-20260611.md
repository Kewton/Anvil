# Diagnostic Primary Fallback Negative Validation

Date: 2026-06-11

## Hypothesis

The remaining Python markdown `diagnostic_target_missing` row might be fixed by allowing the
diagnostic primary repair target to survive no-progress fallback even when secondary or changed
candidates exist.

This was intentionally tested as a generic target-admission change, not a Markdown-specific rule.

## Trial Change

Temporarily changed `model_assessment_to_verifier_repair_assessment` so
`no_progress_fallback_repair_plan` ran whenever both primary diagnostic lists were empty:

```text
repair_plan.is_empty() && repair_candidates.is_empty()
```

instead of requiring primary, secondary, and changed candidate lists to all be empty.

## Deterministic Checks

Passed under the temporary change:

- `cargo fmt`
- `cargo test verifier_diagnostic --lib`
- `cargo test repair_test_weakening_filter --lib`

## Real LLM Validation

Command:

```text
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --case-sequence python_markdown,python_markdown,python_markdown,python_markdown,python_markdown,python_markdown,python_sales,rust_word,docs_runbook,data_json \
  --variant no_pam \
  --run-id diagnostic-primary-fallback-20260611 \
  --anvil-bin target/debug/anvil \
  --timeout-secs 600 \
  --chat-timeout-secs 240
```

Result:

| Metric | Result |
| --- | ---: |
| total | 10 |
| pass | 10/10 |
| high_quality | 9/10 |
| verification_pass | 9/10 |
| repair_exhausted | 1 |
| max_iterations | 0 |
| shadow_conflict | 0 |

Non-HQ row:

- `python_markdown` row 1: `repair_exhausted`, `shadow_terminal_class=evidence_repair_exhausted`

## Interpretation

The temporary change did not improve the 10-run outcome over the prior committed state.

It converted one residual shape from `verifier_failed` / shadow conflict into
`repair_exhausted`. That is not a better architecture outcome: it hides target-missing but
re-enters unsafe patch rejection and repair convergence failure.

The failed row shows the deeper problem:

- the controller can identify or route to a plausible target,
- but generated tests and implementation behavior still drift,
- then patch validation rejects repeated test/implementation weakening attempts.

Therefore, broadening no-progress primary fallback is not the right next step.

## Decision

Do not keep the temporary target-admission relaxation.

Keep the existing stricter no-progress fallback until a better typed design exists.

The next improvement should follow the existing P0 direction:

1. API/state expectation obligation for route/request/response/state drift.
2. EvidenceObservation and terminal projection alignment for externally passing work.
3. Repair action quality that prevents unsafe test/implementation weakening loops without
   treating no-progress bans as hard dead ends.

