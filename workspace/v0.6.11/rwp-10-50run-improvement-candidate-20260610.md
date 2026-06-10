# RWP-10 50-run Improvement Claim Candidate

Date: 2026-06-10

## Purpose

RWP-10 checks whether the residual architecture changes support an improvement
claim beyond local smoke tests and the RWP-9 20-run regression guard.

This is still a candidate claim. It must not treat PAM failures as PAM benefit,
and it must not hide feature-improvement failures behind aggregate success.

## Main 50-run

Command:

```text
python3 workspace/v0.6.11/wp_eval_matrix.py --suite wp11 --run-id rwp10-50run-improvement-candidate-20260610 --anvil-bin target/debug/anvil --timeout-secs 600 --chat-timeout-secs 240
```

Result file:

```text
workspace/v0.6.11/eval-runs/rwp10-50run-improvement-candidate-20260610/results.csv
```

Summary:

| Metric | Result |
| --- | ---: |
| total | 50 |
| pass | 49/50 |
| high_quality | 46/50 |
| verification_pass | 46/50 |
| false_done | 0 |
| false_missing | 0 |
| repair_exhausted | 2 |
| max_iterations | 4 |
| shadow_conflict | 2 |

Baseline comparison against the earlier `wp11-50run-20260610` run:

| Metric | Earlier 50-run | RWP-10 |
| --- | ---: | ---: |
| pass | 38/50 | 49/50 |
| high_quality | 35/50 | 46/50 |
| verification_pass | 37/50 | 46/50 |
| false_done | 2 | 0 |
| false_missing | 5 | 0 |
| repair_exhausted | 8 | 2 |

By variant:

| Variant | PAM availability | Pass | High quality | Total |
| --- | --- | ---: | ---: | ---: |
| no_pam | disabled | 24 | 22 | 25 |
| pam | failed | 25 | 24 | 25 |

PAM assessment:

- `pam` rows were `pam_availability=failed`.
- `pam_unused_reason=context_pack_failed:sidecar_call`.
- `pam_failure_phase=sidecar_call`.
- Therefore the run does not prove PAM-injected improvement. The aggregate
  improvement should be attributed to non-PAM controller/evidence changes and
  LLM variance, not to PAM.

## Case Results

| Case | Pass | High quality | Total |
| --- | ---: | ---: | ---: |
| python_sales | 8 | 8 | 8 |
| docs_runbook | 4 | 4 | 4 |
| toml_merge | 6 | 5 | 6 |
| rust_word | 4 | 4 | 4 |
| rust_ndjson | 4 | 4 | 4 |
| node_json | 6 | 6 | 6 |
| node_csv | 3 | 3 | 4 |
| fastapi_notes | 6 | 6 | 6 |
| python_markdown | 2 | 0 | 2 |
| data_csv | 2 | 2 | 2 |
| research_cache | 2 | 2 | 2 |
| ops_health | 2 | 2 | 2 |

Non high-quality rows:

| Seq | Variant | Case | Terminal | Notes |
| ---: | --- | --- | --- | --- |
| 8 | no_pam | toml_merge | max_iterations | Functional output passed, but active terminal reached max iterations. Shadow terminal marked success, so terminal cleanup still has conflict risk. |
| 17 | no_pam | node_csv | max_iterations | Node test failed on whitespace preservation. Shadow terminal marked success; this is a remaining terminal/evidence conflict. |
| 22 | no_pam | python_markdown | repair_exhausted | Functional pass but verifier/evidence repair exhausted. |
| 47 | pam | python_markdown | repair_exhausted | Same pattern under PAM-failed variant. |

## Feature Improvement Supplement

The built-in `wp11` 50-run does not include `feature_discount`, so an explicit
feature-improvement supplement was run.

Commands:

```text
python3 workspace/v0.6.11/wp_eval_matrix.py --suite wp11 --case-sequence feature_discount,feature_discount --variant no_pam --run-id rwp10-feature-supplement-no-pam-20260610 --anvil-bin target/debug/anvil --timeout-secs 600 --chat-timeout-secs 240
python3 workspace/v0.6.11/wp_eval_matrix.py --suite wp11 --case-sequence feature_discount,feature_discount --variant pam --run-id rwp10-feature-supplement-pam-20260610 --anvil-bin target/debug/anvil --timeout-secs 600 --chat-timeout-secs 240
```

Supplement result:

| Variant | Pass | High quality | Total | Dominant terminal |
| --- | ---: | ---: | ---: | --- |
| no_pam | 0 | 0 | 2 | missing_repo_edits |
| pam | 0 | 0 | 2 | missing_repo_edits |

Feature assessment:

- `feature_discount` exited in about 1.4 seconds with no meaningful edit to
  `discounts.py`.
- The prompt was present in the command, but the loop entered the task-contract
  verifier path and stopped with `missing_repo_edits` before a useful edit.
- This is not primarily a local LLM implementation failure. It points to a
  controller/admission issue for existing-project feature-improvement tasks:
  pre-existing passing tests can make the loop verify too early instead of
  turning the current request into a required behavior delta.

## Improvement Claim

Supported claim:

- For the current `wp11` coding/docs/data/research/ops/API suite excluding
  feature-improvement supplementation, high_quality improved from 35/50 to
  46/50.
- false_done and false_missing dropped to zero in the main 50-run.
- repair_exhausted decreased from 8 to 2.
- FastAPI/API and non-coding tasks improved materially.

Unsupported claim:

- Do not claim general-purpose task success across feature-improvement tasks.
- Do not claim PAM benefit; PAM was unavailable/failed in this run.
- Do not claim terminal projection is fully clean; max-iteration success and
  shadow conflicts remain.

## Next Architecture Hypotheses

1. Feature-improvement tasks need current-request behavior-delta admission
   before verifier-first completion checks.
2. Evidence repair should distinguish "functional pass but evidence repair
   exhausted" from true failure, especially for Python Markdown.
3. Active terminal projection should consume the same success observation as
   shadow terminal so max-iterations does not mask completed work.
4. PAM should fail closed with explicit telemetry unless sidecar context
   packing can complete; failed PAM must not be mixed into injected results.
