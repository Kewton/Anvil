# HEAD c500d89 Residual Reproduction Check

Date: 2026-06-11

## Purpose

Before implementing the remaining fixes, this check re-evaluates the current HEAD to confirm whether the known residual issues are stable enough to act on.

Target HEAD:

```text
c500d89
```

## Command

```text
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --case-sequence fastapi_notes,fastapi_notes,fastapi_notes,fastapi_notes,fastapi_notes,fastapi_notes,fastapi_notes,fastapi_notes,fastapi_notes,fastapi_notes,python_markdown,python_markdown,python_markdown,python_markdown,python_markdown,python_markdown,ops_health,ops_health,ops_health,ops_health,ops_health,ops_health,python_sales,python_sales,rust_word,rust_word,toml_merge,toml_merge,docs_runbook,data_json \
  --variant no_pam \
  --run-id head-c500d89-residual-repro-20260611 \
  --anvil-bin target/debug/anvil \
  --timeout-secs 600 \
  --chat-timeout-secs 240
```

## Summary

| Metric | Result |
| --- | ---: |
| total | 30 |
| pass | 25/30 |
| high_quality | 22/30 |
| verification_pass | 22/30 |
| false_done | 0 |
| false_missing | 0 |
| repair_exhausted | 8 |
| max_iterations | 4 |
| shadow_conflict | 0 |

By case:

| Case | Runs | Pass | HQ | Read |
| --- | ---: | ---: | ---: | --- |
| fastapi_notes | 10 | 5 | 5 | residual API/state drift reproduced |
| python_markdown | 6 | 6 | 3 | external pass often diverges from active terminal |
| ops_health | 6 | 6 | 6 | external pass, but active terminal often `max_iterations` |
| python_sales | 2 | 2 | 2 | regression guard clean |
| rust_word | 2 | 2 | 2 | weak verifier stop did not recur |
| toml_merge | 2 | 2 | 2 | regression guard clean |
| docs_runbook | 1 | 1 | 1 | regression guard clean |
| data_json | 1 | 1 | 1 | regression guard clean |

## Reproduced Residuals

### 1. FastAPI API/state expectation drift

Result:

- 5/10 pass and HQ.
- 5/10 failed.
- All failures exited `repair_exhausted`.

Observed failure shape:

- tests expect a posted note to appear in `GET /notes`
- tests also infer list length/order/state behavior not explicitly stabilized
- implementation returns `id=1` for every created note and appends notes
- tests then assert the first item is the latest note or a specific second note, creating contract drift

Example failed assertion:

```text
Expected: Another Note
Actual:   Test Note
```

Interpretation:

- This is stable enough to implement.
- The fix should be generic API contract/state expectation binding.
- Do not add a FastAPI-specific branch.

### 2. Python markdown terminal/evidence convergence

Result:

- 6/6 external pass.
- 3/6 high_quality.
- 3/6 exited `repair_exhausted`.

Observed failure shape:

- external grader considered the deliverable acceptable
- active terminal stayed in `evidence_repair_exhausted`
- failed internal tests were often over-specific or implementation/test drift, while task-level behavior was still acceptable

Example failure:

```text
AssertionError: assert 'heading level jumps from 1 to 4' in 'Line 2: heading level jumps from 2 to 5'
```

Interpretation:

- This is stable enough to implement.
- The fix should focus on EvidenceObservation / terminal projection alignment.
- Avoid weakening false-done safety.

### 3. ops_health command-observation terminal mismatch

Result:

- 6/6 external pass and HQ.
- 4/6 exited `max_iterations`.

Observed failure shape:

- report content was acceptable
- terminal diagnostics said typed evidence obligations were not observed
- logs showed `classified_task_kind=docs` for the ops request in at least one max-iteration row
- command-observation evidence was not bound even though the required report existed

Interpretation:

- This is stable enough to implement.
- The fix should align ops command-observation evidence with terminal projection.
- It may also require checking why an ops request is projected/classified as docs in the terminal record.

## Regression Guard Read

The broader controller recovery remains intact:

- Python sales: 2/2 HQ
- Rust word: 2/2 HQ
- TOML: 2/2 HQ
- docs/data: stable
- false-done: 0
- false-missing: 0

This supports proceeding with focused fixes instead of repeating broad architecture evaluation immediately.

## Recommendation

Proceed with implementation, but only in two narrow tracks:

1. `API Contract State Obligation`
   - target FastAPI/API drift
   - framework-generic
   - public route/request/response/state semantics only

2. `EvidenceObservation Terminal Alignment`
   - target Python markdown and ops_health terminal mismatch
   - reconcile satisfied external evidence with active terminal projection
   - preserve false-done 0

PAM should not be part of the next implementation track. This check was no-PAM only, and the prior WP10 run already showed PAM availability failures.
