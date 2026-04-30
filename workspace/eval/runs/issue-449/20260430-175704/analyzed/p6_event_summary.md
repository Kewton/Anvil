# P6 Event Summary

## Static / Unit Coverage

- `cargo fmt --check` passed.
- `cargo clippy --all-targets -- -D warnings` passed.
- `cargo test` passed.
- `eval_log_smoke` passed 12/12.
- `eval_harness_smoke` passed 7/7.
- `dataset_export_smoke` passed 12/12.
- `tests/test_compare_security.py` passed 9/9.

## Live / Practical Checks

| Scenario | Result | Observation |
| --- | --- | --- |
| P6-01 successful turn | PASS/MIXED | `logs/eval.jsonl` was created for a live qwen3.6 turn. It includes `schema_version`, `tool_calls`, `anvil_score`, `changed_file_classes`, `final_outcome`, and `model`. README edit completed in 3 iterations. |
| P6-01 path scrub | PASS | With `ANVIL_EVAL_SCRUB_PATHS=1`, absolute tool paths in `args_summary` were replaced with `<path>`. |
| P6-03 A/B harness | PASS/MIXED | `bench.sh` accepts two models and feature gates, creates per-model run dirs, `summary.tsv`, and `matrix-report.md`. This was verified with `--dry-run`, not live two-model LLM execution. |
| P6-04 dataset export | FAIL/MIXED | `anvil sessions export` produced valid JSONL and `--success-only`/`--failed-only` behaved correctly. However, a raw `ghp_...`-looking token remained in `feedback_excerpt`, so the live redaction acceptance is not fully met. |

## Key Signals

- Structured eval log exists in real sessions: `state_root/sessions/<id>/logs/eval.jsonl`.
- `AnvilScore` key shape is complete with 12 public fields.
- Path scrubbing for eval log works when `ANVIL_EVAL_SCRUB_PATHS=1`.
- Dataset export stdout/stderr separation works with `--output`: JSONL went to file, summary went to stdout.
- Dataset export filters work in the practical success-only/failed-only check.
- Secret masking is incomplete for at least one realistic raw token shape inside `feedback_excerpt`.

## Residual Risks

- Documentation-only edits still score `user_visible_artifact=false`, matching earlier Issue 446/448 concerns.
- A/B harness was practically checked only in dry-run mode here; full live two-model comparison was not run.
- GitHub tracking state is not fully closed: #449/#471/#472/#473 remain open despite implementation commits being merged.
