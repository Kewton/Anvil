# Issue 449 Evaluation Report

Run ID: `20260430-175704`
Evaluation commit: `be1fae4534a3ac162f8f8de2dd72993a51a2d05a`

## Verdict

Judgement: `Mixed Positive / Observability Foundation Works, Export Redaction Needs Fix`

Issue 449 delivered the main observability foundation. Structured per-turn eval logs exist, A/B harness plumbing exists, token usage parsing has tests, and dataset export can produce JSONL with summary separation and filters.

The main blocker is safety quality in dataset export. A practical export fixture showed that `TOKEN=...` in the task was masked, but a raw `ghp_...`-looking token in `feedback_excerpt` survived. Because Epic F explicitly requires secret-looking values not to be exported, the Epic should not be considered fully high quality yet.

## Static Verification

| Check | Result | Notes |
| --- | --- | --- |
| `cargo fmt --check` | PASS | no formatting drift |
| `cargo clippy --all-targets -- -D warnings` | PASS | no warnings |
| `cargo test` | PASS | full suite passed |
| `cargo test --test eval_log_smoke` | PASS | 12/12 |
| `cargo test --test eval_harness_smoke` | PASS | 7/7 |
| `cargo test --test dataset_export_smoke` | PASS | 12/12 |
| `python3 -m unittest tests/test_compare_security.py` | PASS | 9/9 |

## Practical Results

| Scenario | Result | Key Observation |
| --- | --- | --- |
| P6-01 live successful turn | PASS/MIXED | `eval.jsonl` was written and included turn summary, tool calls, AnvilScore, changed-file classes, and final outcome. Documentation edit still scored `user_visible_artifact=false`. |
| P6-01 eval path scrub | PASS | `ANVIL_EVAL_SCRUB_PATHS=1` replaced absolute paths in eval log tool args with `<path>`. |
| P6-03 A/B harness | PASS/MIXED | Dry-run matrix with two models created `summary.tsv` and `matrix-report.md`; not a live two-model quality comparison. |
| P6-04 dataset export | FAIL/MIXED | JSONL export was valid and filters worked, but `feedback_excerpt` leaked a raw `ghp_abc123def456` token-like value. |

## What Improved

- Runtime now emits a structured `logs/eval.jsonl` beside `llm-io.jsonl`.
- Eval log records have a stable schema and include `schema_version: 1`.
- `AnvilScore` is serialized with the expected 12 public fields.
- `ANVIL_EVAL_SCRUB_PATHS=1` works for absolute paths in eval log JSON.
- `sessions export` can emit training-format JSONL and keeps summary text separate.
- `--success-only`, `--failed-only`, and mutual exclusion behavior are present.
- `bench.sh` supports two-model matrix dry-run and feature gates for precautions, case memory, and auto-test.

## Remaining Gaps

- Dataset export redaction is incomplete. The practical export record still contained `ghp_abc123def456` in `input.feedback_excerpt`.
- Documentation edits are still not treated as user-visible artifacts in AnvilScore, so observability can undercount successful docs work.
- A/B comparison was not validated with live two-model LLM runs in this evaluation; only the harness interface and dry-run matrix were checked.
- Issue tracking is inconsistent: #449 and child issues #471/#472/#473 are still open even though implementation commits are merged.

## Recommendation

Treat Issue 449 as useful and mostly implemented, but not fully complete from a quality/safety standpoint.

1. Fix dataset export to apply secret masking consistently to `feedback_excerpt`, `active_precautions`, and `added_precautions`, not only task-like fields.
2. Add a regression test for bare `ghp_...` token-like values in every exported string field.
3. Reuse or close the existing follow-up about documentation artifacts so `README.md` edits do not score as invisible work.
4. Run a real two-model A/B benchmark once the redaction fix lands, then update the Issue 444-449 tracking summary.
