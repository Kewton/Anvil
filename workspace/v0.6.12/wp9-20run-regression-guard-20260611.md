# WP9 20-Run Regression Guard

Date: 2026-06-11

## Intent

WP9 validates that WP1-WP8 did not merely improve a few targeted cases by chance. This is a regression guard, not a final improvement claim.

## Run Configuration

Command:

```text
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --case-sequence python_sales,python_sales,python_markdown,python_markdown,node_csv,node_json,rust_word,rust_word,rust_ndjson,rust_ndjson,fastapi_notes,toml_merge,toml_merge,docs_runbook,data_csv,data_json,research_cache,ops_health,feature_discount,feature_discount \
  --variant no_pam \
  --run-id wp9-20run-regression-guard-20260611 \
  --anvil-bin target/debug/anvil \
  --timeout-secs 600 \
  --chat-timeout-secs 240
```

The sequence covers:

- Python coding
- Python TDD-like coding
- Node coding
- Rust coding
- API coding
- parser coding
- docs
- data
- research
- ops
- feature improvement

## Result

Overall:

| Metric | Result |
| --- | ---: |
| pass | 20/20 |
| high_quality | 20/20 |
| verification_pass | 20/20 |
| false_done | 0 |
| false_missing | 0 |
| repair_exhausted | 0 |
| max_iterations | 1 |
| shadow_conflict | 0 |

By case:

| Case | Runs | Pass | HQ |
| --- | ---: | ---: | ---: |
| python_sales | 2 | 2 | 2 |
| python_markdown | 2 | 2 | 2 |
| node_csv | 1 | 1 | 1 |
| node_json | 1 | 1 | 1 |
| rust_word | 2 | 2 | 2 |
| rust_ndjson | 2 | 2 | 2 |
| fastapi_notes | 1 | 1 | 1 |
| toml_merge | 2 | 2 | 2 |
| docs_runbook | 1 | 1 | 1 |
| data_csv | 1 | 1 | 1 |
| data_json | 1 | 1 | 1 |
| research_cache | 1 | 1 | 1 |
| ops_health | 1 | 1 | 1 |
| feature_discount | 2 | 2 | 2 |

## Read

This is a strong regression-guard pass:

- `safe_stop_verifier_weak` did not recur in Rust word / NDJSON.
- feature improvement did not early-stop as `missing_repo_edits`.
- Python sales and Python markdown both passed in this 20-run slice.
- TOML passed 2/2 in this guard, which is meaningfully better than the previous wall but still needs 50-run confirmation.
- docs/data/research/ops stayed stable.

Important caveat:

- `ops_health` externally passed and was high-quality, but Anvil terminal was `max_iterations`.
- This means terminal/evidence convergence still has a residual mismatch for ops command-observation tasks.

## Completion Against Gate

| Gate | Status |
| --- | --- |
| v0.6.12 baselineより悪化しない | passed |
| false-done 0 | passed |
| false-missing 0 | passed |
| weak verifier stops reduced in hard cases | passed in this sample |
| feature task does not early-stop | passed |
| non-coding stability | passed |

## Next

WP10 can proceed because:

- 20-run did not regress.
- targeted failures improved in this sample.
- false-done remained 0.

WP10 must not over-claim from WP9 alone. A 50-run candidate is needed, and terminal mismatch for ops must be tracked separately.
