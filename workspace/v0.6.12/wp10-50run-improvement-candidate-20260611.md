# WP10 50-Run Improvement Candidate

Date: 2026-06-11

## Intent

WP10 checks whether the WP1-WP9 changes are strong enough to be considered an improvement candidate. This is still not a final claim; one 50-run can be high variance.

Baseline:

- v0.6.12: 14/50 pass, 28.0%
- false-done: 0
- false-missing: 1
- notable failures: Rust `safe_stop_verifier_weak`, TOML 0/4, Python markdown 0/4, Node CSV 0/4

## Run Configuration

Command:

```text
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --run-id wp10-50run-improvement-candidate-20260611 \
  --anvil-bin target/debug/anvil \
  --timeout-secs 600 \
  --chat-timeout-secs 240
```

This suite runs:

- 25 no-PAM cases
- 25 PAM cases

## Result

Overall:

| Metric | Result |
| --- | ---: |
| pass | 47/50 |
| high_quality | 46/50 |
| verification_pass | 46/50 |
| false_done | 0 |
| false_missing | 0 |
| repair_exhausted | 3 |
| max_iterations | 3 |
| shadow_conflict | 1 |

By variant:

| Variant | Pass | HQ | Total |
| --- | ---: | ---: | ---: |
| no-PAM | 23 | 22 | 25 |
| PAM | 24 | 24 | 25 |

By PAM availability:

| Availability | Pass | HQ | Total |
| --- | ---: | ---: | ---: |
| no_pam:disabled | 23 | 22 | 25 |
| pam:failed | 24 | 24 | 25 |

PAM interpretation:

- PAM did not inject successfully in this run.
- All PAM rows reported `context_pack_failed:sidecar_call`.
- Therefore, this run cannot be used as evidence that PAM improved success rate.

By case:

| Case | Pass | HQ | Total |
| --- | ---: | ---: | ---: |
| python_sales | 8 | 8 | 8 |
| python_markdown | 2 | 1 | 2 |
| toml_merge | 6 | 6 | 6 |
| rust_word | 4 | 4 | 4 |
| rust_ndjson | 4 | 4 | 4 |
| node_json | 6 | 6 | 6 |
| node_csv | 4 | 4 | 4 |
| fastapi_notes | 3 | 3 | 6 |
| docs_runbook | 4 | 4 | 4 |
| data_csv | 2 | 2 | 2 |
| research_cache | 2 | 2 | 2 |
| ops_health | 2 | 2 | 2 |

## Non-HQ / Failure Rows

| Seq | Variant | Case | Pass | HQ | Exit | Read |
| ---: | --- | --- | --- | --- | --- | --- |
| 19 | no-PAM | fastapi_notes | false | false | `repair_exhausted` | list/order/state expectation drift |
| 21 | no-PAM | fastapi_notes | false | false | `max_iterations` | generated tests referenced unsupported `app.notes` state |
| 22 | no-PAM | python_markdown | true | false | `repair_exhausted` | external grader pass true; terminal did not converge |
| 46 | PAM | fastapi_notes | false | false | `repair_exhausted` | note state expectation drift |

Additional terminal mismatch:

- `ops_health` passed and was high-quality, but both no-PAM and PAM rows ended `max_iterations`.
- This matches the WP9 caveat: ops command-observation evidence can satisfy the external grader while Anvil terminal evidence remains not observed.

## Comparison Against v0.6.12

| Metric | v0.6.12 | WP10 | Read |
| --- | ---: | ---: | --- |
| pass | 14/50 | 47/50 | strong recovery candidate |
| pass rate | 28.0% | 94.0% | large improvement candidate |
| HQ | not separately available in baseline summary | 46/50 | strong, but not comparable as exact delta |
| false-done | 0 | 0 | safety retained |
| false-missing | 1 | 0 | improved in this run |
| Rust word | 0/6 | 4/4 | weak verifier stop did not recur |
| Rust NDJSON | 0/4 | 4/4 | improved in this run |
| TOML | 0/4 | 6/6 | large improvement candidate; needs repeat confirmation |
| Python markdown | 0/4 | 2/2 pass, 1/2 HQ | pass improved; convergence still imperfect |
| Node CSV | 0/4 | 4/4 | improved in this run |
| Python sales | 3/6 | 8/8 | recovered in this run |

## Assessment

This is a strong improvement candidate:

- false-done stayed 0
- false-missing stayed 0
- `safe_stop_verifier_weak` did not recur in Rust hard cases
- feature early-stop issue was already covered by WP9 and did not reappear there
- TOML, Node CSV, Rust, and Python sales recovered in this sample

But it is not enough for a final claim:

- one 50-run can be high variance
- FastAPI remains unstable at 3/6
- Python markdown still has terminal repair convergence issues
- ops command-observation terminal convergence is not solved
- PAM cannot be credited because PAM sidecar/context pack failed in all PAM rows

## Architecture Read

The current direction is supported:

- typed weak-verifier repair target is safer than terminal-only safe-stop
- current-turn behavior delta helped feature tasks without benchmark-specific branches
- prompt boundary separation did not harm docs/data/research/ops and appears to help Python coding stability
- actor loop no-op refactor did not introduce visible regression

The remaining bottleneck is narrower than v0.6.12:

- API contract/state expectation drift
- terminal/evidence convergence when external grader passes
- PAM availability/reporting, not PAM effectiveness

## Next

WP11 should update the architecture direction with these priorities:

1. Treat WP10 as a recovery candidate, not a final claim.
2. Keep false-done protection as non-negotiable.
3. Prioritize API contract expectation binding and state-observation evidence.
4. Fix ops command-observation terminal convergence.
5. Keep PAM advisory and report availability separately until sidecar calls succeed.
