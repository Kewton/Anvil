# RWP-9 20-run Regression Guard

Date: 2026-06-10

## Purpose

RWP-9 checks whether the RWP-1 through RWP-8 changes are stable beyond local
smoke tests. This is a regression guard, not a final improvement claim.

## Run

Command:

```text
python3 workspace/v0.6.11/wp_eval_matrix.py --suite wp10 --variant no_pam --run-id rwp9-20run-regression-guard-20260610 --anvil-bin target/debug/anvil --timeout-secs 600 --chat-timeout-secs 240
```

Result file:

```text
workspace/v0.6.11/eval-runs/rwp9-20run-regression-guard-20260610/results.csv
```

## Summary

| Metric | Result |
| --- | ---: |
| total | 20 |
| pass | 19/20 |
| high_quality | 18/20 |
| verification_pass | 18/20 |
| false_done | 0 |
| false_missing | 0 |
| repair_exhausted | 1 |
| max_iterations | 1 |
| shadow_conflict | 0 |

Baseline comparison against the earlier `wp10-20run-20260610` run:

| Metric | Earlier 20-run | RWP-9 |
| --- | ---: | ---: |
| pass | 14/20 | 19/20 |
| high_quality | 14/20 | 18/20 |
| false_done | 1 | 0 |
| false_missing | 2 | 0 |
| repair_exhausted | 2 | 1 |

## Task Coverage

RWP-9 included:

- coding: Python, TOML, Rust, Node, FastAPI
- feature-style existing project change: covered by the current suite history but not in this exact 20-run sequence
- TDD-like tasks: Python Markdown, TOML, Rust, Node, FastAPI
- data: CSV
- docs: runbook
- research: cache strategy report
- ops: command observation report
- API: FastAPI

## Non High-Quality Rows

| Seq | Case | Terminal | Notes |
| ---: | --- | --- | --- |
| 7 | node_csv | missing_repo_edits | Node test failed. The deliverable files existed, but test evidence failed. This appears to be implementation/test mismatch rather than false terminal success. |
| 13 | python_sales | repair_exhausted | Generated tests expected empty-argv failure, but CLI returned success. Repair loop did not converge. |

Additional observation:

- `ops_health` passed and was high quality, but exited with `max_iterations`.
  This is not a false success, but it is a remaining terminal/progress
  cleanliness issue.

## Assessment

RWP-9 passes as a regression guard:

- high_quality did not drop against the earlier 20-run baseline
- false_done and false_missing did not increase
- repair_exhausted decreased
- non-coding cases passed
- API cases both passed after RWP-4

The result supports proceeding to RWP-10, with one caveat: success-rate
improvement should not be claimed solely from this 20-run because one
max-iteration success and one repair-exhausted coding case remain.
