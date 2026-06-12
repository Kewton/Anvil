# Minimal Loop T2-4 Task15 Feedback Rerun

- Date: 2026-06-12 JST
- Benchmark: `minimal-loop-expanded`
- Model: `qwen3.6:27b-coding-nvfp4`
- Engine: `minimal`
- Mechanism: completion-without-write feedback from #1036
- Source root: `.anvil/benchmarks/20260612T174200-32077`
- Baseline report: `docs/eval/minimal-loop-t2-4-recheck-20260612.md`

## Build And Flag Verification

| item | value |
|---|---|
| source commit | `b12f33b2578d6a81e0fd4c8a7d4e12a272d3fe00` |
| git dirty | `false` |
| binary path | `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/task15-rerun/target/release/anvil` |
| binary build time | `2026-06-12T08:40:27Z` |
| off flag | `ANVIL_NO_MINIMAL_COMPLETION_WITHOUT_WRITE_FEEDBACK=1` absent from `active_flags` |

Representative `meta.json` active flags:

```text
BENCH_DEBUG=0
ANVIL_BIN=/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/task15-rerun/target/release/anvil
ENGINE=minimal
MAX_ITERATIONS=12
CHAT_RETRIES=2
ANVIL_NO_REMINDER=1
ANVIL_NO_CASE_RETRIEVAL=1
ANVIL_NO_CASE_RECORD=1
ANVIL_NO_AUTO_TEST=1
```

## Run Command

```sh
bash scripts/bench.sh minimal-loop-expanded \
  --model qwen3.6:27b-coding-nvfp4 \
  --runs 5 \
  --engine minimal \
  --max-iterations 12 \
  --no-auto-test \
  --no-precautions \
  --no-case-memory \
  --bench-no-debug
```

## Aggregate Result

| row | value |
|---|---:|
| runs | 125 |
| `meta.json` files | 125 |
| `rc=0` | 122/125 |
| success_check | 80/125 |
| elapsed total | 4812 sec |
| elapsed mean | 38.5 sec |

Compared with the Task14 recheck baseline:

| engine / run | success | delta vs previous minimal | elapsed mean sec |
|---|---:|---:|---:|
| legacy-lite recheck reference | 66/125 | n/a | 66.6 |
| minimal Task14 recheck baseline | 69/125 | n/a | 28.9 |
| minimal Task15 feedback rerun | 80/125 | +11 | 38.5 |

Task15 improves minimal from 69/125 to 80/125 and puts it +14 runs above the
legacy-lite reference. Runtime regresses from 28.9s to 38.5s mean, but remains
faster than legacy-lite.

## Admission Target Scenarios

| scenario | Task14 baseline | Task15 rerun | delta |
|---|---:|---:|---:|
| `new-python-csv-small` | 2/5 | 3/5 | +1 |
| `fix-rust-parser-error` | 1/5 | 4/5 | +3 |
| `fix-readme-command` | 2/5 | 5/5 | +3 |
| `fix-python-slugify` | 2/5 | 5/5 | +3 |
| total | 7/20 | 17/20 | +10 |

The mechanism meets the primary admission target: all four targeted
no-edit-loop scenarios improved, with +10 runs across the target set.

## Non-Regression Watchlist

| scenario | Task14 baseline | Task15 rerun | delta | note |
|---|---:|---:|---:|---|
| `multi-file-node-package` | 5/5 | 5/5 | +0 | stable |
| `multi-file-rust-library` | 5/5 | 4/5 | -1 | one missing `tests/math_tests.rs` |
| `scaffold-fastapi-service` | 5/5 | 1/5 | -4 | missing FastAPI files in 4 runs |
| `new-typescript-formatter` | 4/5 | 5/5 | +1 | improved |
| `new-markdown-release-notes` | 5/5 | 5/5 | +0 | stable |
| `non-coding-research-brief` | 5/5 | 5/5 | +0 | stable |

The watchlist is mixed. The serious regression is `scaffold-fastapi-service`,
but the feedback did not fire in that scenario's five runs, so this is not
direct evidence that the new feedback caused the regression. Treat it as
run-to-run variance or a separate scaffold instability until ablation says
otherwise. `multi-file-rust-library` did fire feedback in all five runs and
lost one run, so it should be included in ablation review.

## Scenario Summary

| scenario | legacy recheck | minimal Task14 | minimal Task15 | delta vs Task14 | delta vs legacy |
|---|---:|---:|---:|---:|---:|
| fix-css-token-doc | 5/5 | 3/5 | 4/5 | +1 | -1 |
| fix-js-date-helper | 5/5 | 1/5 | 0/5 | -1 | -5 |
| fix-json-normalizer | 5/5 | 5/5 | 5/5 | +0 | +0 |
| fix-python-retry-policy | 0/5 | 5/5 | 3/5 | -2 | +3 |
| fix-python-slugify | 4/5 | 2/5 | 5/5 | +3 | +1 |
| fix-readme-command | 5/5 | 2/5 | 5/5 | +3 | +0 |
| fix-rust-parser-error | 5/5 | 1/5 | 4/5 | +3 | -1 |
| fix-shell-safe-clean | 2/5 | 5/5 | 5/5 | +0 | +3 |
| long-session-data-report | 0/5 | 2/5 | 1/5 | -1 | +1 |
| long-session-large-component | 0/5 | 1/5 | 5/5 | +4 | +5 |
| long-session-read-edit | 0/5 | 1/5 | 1/5 | +0 | +1 |
| multi-file-docs-and-examples | 0/5 | 2/5 | 4/5 | +2 | +4 |
| multi-file-node-package | 0/5 | 5/5 | 5/5 | +0 | +5 |
| multi-file-python-package | 0/5 | 2/5 | 1/5 | -1 | +1 |
| multi-file-rust-library | 5/5 | 5/5 | 4/5 | -1 | -1 |
| new-large-react-kanban | 0/5 | 2/5 | 1/5 | -1 | +1 |
| new-markdown-release-notes | 5/5 | 5/5 | 5/5 | +0 | +0 |
| new-python-csv-small | 5/5 | 2/5 | 3/5 | +1 | -2 |
| new-rust-cli-small | 5/5 | 0/5 | 4/5 | +4 | -1 |
| new-typescript-formatter | 5/5 | 4/5 | 5/5 | +1 | +0 |
| non-coding-research-brief | 5/5 | 5/5 | 5/5 | +0 | +0 |
| non-coding-runbook | 5/5 | 3/5 | 3/5 | +0 | -2 |
| scaffold-fastapi-service | 0/5 | 5/5 | 1/5 | -4 | +1 |
| scaffold-next-dashboard | 0/5 | 1/5 | 1/5 | +0 | +1 |
| scaffold-rust-cli | 0/5 | 0/5 | 0/5 | +0 | +0 |

## Feedback Firing

- Unique runs with completion-without-write feedback: 48/125
- Success among feedback-fired runs: 23/48
- Success among non-feedback runs: 57/77

This does not imply the feedback is harmful. The trigger selects sessions that
were already about to finish without any file changes, so feedback-fired runs
are a harder subset.

Selected scenario-level firing outcomes:

| scenario | feedback-fired success | non-feedback success |
|---|---:|---:|
| `new-python-csv-small` | 1/3 | 2/2 |
| `fix-rust-parser-error` | 1/2 | 3/3 |
| `fix-readme-command` | 2/2 | 3/3 |
| `fix-python-slugify` | 1/1 | 4/4 |
| `multi-file-rust-library` | 4/5 | n/a |
| `scaffold-fastapi-service` | n/a | 1/5 |

## Direct Failure Notes

- `scaffold-fastapi-service` failed 4/5 with missing `app/main.py`,
  `app/routes/health.py`, and/or `tests/test_health.py`. Feedback did not fire
  in any of those five runs.
- `multi-file-rust-library` failed 1/5 with missing `tests/math_tests.rs`.
- `fix-python-retry-policy` regressed to 3/5. Failures were one line-count false
  negative (`src/retry_policy.py:24<25`) and one missing target file.
- `fix-js-date-helper` remains 0/5; failures are line-count false negatives or
  missing `src/dateRange.js`.
- `scaffold-rust-cli` remains a true dead scenario at 0/5.

## Assessment

Task15 is a net positive and satisfies the primary admission target:

- Overall success improved by +11 runs.
- Target no-edit-loop scenarios improved by +10 runs.
- Minimal now beats the legacy-lite recheck reference by +14 runs.

However, non-regression is not fully clean. `scaffold-fastapi-service` regressed
sharply, even though the new feedback did not fire there; `multi-file-rust-library`
lost one run where feedback did fire. The right next step is an ablation run with
`--no-minimal-completion-without-write-feedback` or a narrower targeted rerun of
the watchlist scenarios before treating the regressions as causal.

Recommended follow-up:

1. Record this report in a docs PR.
2. Run ablation with `--no-minimal-completion-without-write-feedback` if GPU time
   allows, at least for the watchlist scenarios.
3. Triage remaining failures in `fix-js-date-helper`, `scaffold-rust-cli`, and
   scaffold instability separately from the no-edit-loop mechanism.
