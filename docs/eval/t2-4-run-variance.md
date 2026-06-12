# T2-4 Run Variance From Non-Fired Task15 Runs

- Date: 2026-06-12 JST
- Fixed-binary baseline root: `.anvil/benchmarks/20260612T145227-40162`
- Task15 root: `.anvil/benchmarks/20260612T174200-32077`
- Model: `qwen3.6:27b-coding-nvfp4`
- Engine: `minimal`

## Method

This analysis uses Task15 runs where the completion-without-write feedback did
not fire. The firing detector reads only each canonical
`run-*/logs/llm-io.jsonl` file and checks for the `[minimal-feedback]` marker.
Duplicated logs under `state/sessions/` are ignored.

For each Task15 non-fired scenario/run pair, the same scenario/run pair is read
from the fixed-binary baseline root. The fixed-binary side uses
`summary.recheck.tsv`; the Task15 side uses the original `summary.tsv` because
Task15 was already run after the Task14 check fixes.

Run numbers are replicate identifiers, not deterministic seeds. The comparison
therefore estimates run-to-run variance, not a strict paired sample.

## Aggregate

| metric | value |
|---|---:|
| Task15 feedback-fired runs | 48/125 |
| Task15 feedback-non-fired runs | 77/125 |
| fixed-binary success in non-fired subset | 42/77 |
| Task15 success in non-fired subset | 57/77 |
| changed outcome pairs | 33/77 |
| fixed failed / Task15 passed | 24 |
| fixed passed / Task15 failed | 9 |

The non-fired subset moved by +15 successes even though the feedback did not
fire. This means a material part of the Task15 aggregate movement is ordinary
run-to-run variance, not mechanism effect.

## Scenario Table

| scenario | non-fired n | fixed-binary success | Task15 success | delta | pair flips |
|---|---:|---:|---:|---:|---|
| `fix-css-token-doc` | 3 | 2/3 | 3/3 | +1 | +1 |
| `fix-js-date-helper` | 2 | 1/2 | 0/2 | -1 | -1 |
| `fix-json-normalizer` | 5 | 5/5 | 5/5 | +0 | 0 |
| `fix-python-retry-policy` | 1 | 1/1 | 1/1 | +0 | 0 |
| `fix-python-slugify` | 4 | 1/4 | 4/4 | +3 | +3 |
| `fix-readme-command` | 3 | 0/3 | 3/3 | +3 | +3 |
| `fix-rust-parser-error` | 3 | 0/3 | 3/3 | +3 | +3 |
| `fix-shell-safe-clean` | 5 | 5/5 | 5/5 | +0 | 0 |
| `long-session-data-report` | 3 | 0/3 | 1/3 | +1 | +1 |
| `long-session-large-component` | 4 | 0/4 | 4/4 | +4 | +4 |
| `long-session-read-edit` | 4 | 0/4 | 1/4 | +1 | +1 |
| `multi-file-docs-and-examples` | 2 | 0/2 | 2/2 | +2 | +2 |
| `multi-file-node-package` | 3 | 3/3 | 3/3 | +0 | 0 |
| `multi-file-python-package` | 4 | 2/4 | 0/4 | -2 | -2 |
| `new-markdown-release-notes` | 5 | 5/5 | 5/5 | +0 | 0 |
| `new-python-csv-small` | 2 | 0/2 | 2/2 | +2 | +2 |
| `new-rust-cli-small` | 1 | 0/1 | 1/1 | +1 | +1 |
| `new-typescript-formatter` | 5 | 4/5 | 5/5 | +1 | +1 |
| `non-coding-research-brief` | 5 | 5/5 | 5/5 | +0 | 0 |
| `non-coding-runbook` | 3 | 2/3 | 2/3 | +0 | +1 / -1 |
| `scaffold-fastapi-service` | 5 | 5/5 | 1/5 | -4 | -4 |
| `scaffold-next-dashboard` | 4 | 1/4 | 1/4 | +0 | +1 / -1 |
| `scaffold-rust-cli` | 1 | 0/1 | 0/1 | +0 | 0 |

Signed scenario deltas in this non-fired subset:

| delta | scenario count |
|---:|---:|
| -4 | 1 |
| -2 | 1 |
| -1 | 1 |
| 0 | 9 |
| +1 | 5 |
| +2 | 2 |
| +3 | 3 |
| +4 | 1 |

Absolute deltas:

| abs(delta) | scenario count |
|---:|---:|
| 0 | 9 |
| 1 | 6 |
| 2 | 3 |
| 3 | 3 |
| 4 | 2 |

## Scaffold FastAPI

`scaffold-fastapi-service` is the key watchlist case because it moved from 5/5
in the fixed-binary baseline to 1/5 in Task15. Feedback did not fire in any of
its five Task15 runs.

| run | fixed-binary | Task15 | Task15 reason |
|---|---|---|---|
| run-1 | pass | fail | `missing_file:app/main.py,command_failed:0` |
| run-2 | pass | fail | `missing_file:app/main.py,missing_file:app/routes/health.py,missing_file:tests/test_health.py,command_failed:0` |
| run-3 | pass | fail | `missing_file:app/main.py,missing_file:app/routes/health.py,missing_file:tests/test_health.py,command_failed:0` |
| run-4 | pass | fail | `missing_file:app/main.py,missing_file:app/routes/health.py,missing_file:tests/test_health.py,command_failed:0` |
| run-5 | pass | pass | `ok` |

Because the feedback never fired, this regression is explainable as
run-to-run variance or scaffold-specific instability. It is not evidence that
the completion-without-write feedback caused the regression.

## Variance Rule

For n=5 scenario-level comparisons, treat +/-2 runs as normal noise unless the
same direction repeats across related scenarios or an llm-io trace shows a
deterministic mechanism path. Treat +/-3 or larger as an investigation trigger,
not as causal proof.

This run provides one extreme calibration point: `scaffold-fastapi-service`
moved by -4 with zero feedback firings. Therefore even a 5/5 to 1/5 movement can
be explained by variance for an unstable scaffold scenario, but it should still
be triaged before admission decisions rely on it.

The Task15 admission result remains useful as an aggregate signal, but this
analysis weakens any claim that the full +11 overall movement is solely caused
by the new mechanism. A narrow on/off ablation is still the cleanest way to
separate mechanism effect from sampling variance for watchlist scenarios.
