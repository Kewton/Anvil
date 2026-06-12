# Minimal Loop Phase 3 Cycle 1 Evaluation

- Date: 2026-06-12 JST
- Benchmark: `minimal-loop-expanded`
- Model: `qwen3.6:27b-coding-nvfp4`
- Mechanism under admission: completion-without-write feedback (#1036)
- Current mechanism ledger entry: [mechanism-ledger.md](mechanism-ledger.md)

## Executive Summary

The first Phase 3 mechanism admission is accepted.

After the Task18 check corrections, the current comparison is:

| series | success | elapsed mean | note |
|---|---:|---:|---|
| legacy-lite | 71/125 | 66.6 sec | existing legacy-lite artifacts, rechecked |
| minimal before mechanism | 68/125 | 28.9 sec | fixed-binary minimal artifacts, rechecked |
| minimal with completion-without-write feedback | 83/125 | 38.5 sec | Task15 artifacts, rechecked |

Minimal with the admitted mechanism is +12 runs over legacy-lite and remains
1.7x faster on elapsed mean. The main admission target set improved from 7/20
to 17/20 in the actual Task15 rerun.

The result is not proof that all +15 runs over the fixed-binary minimal baseline
come from the mechanism. The non-fired-run variance analysis shows substantial
run-to-run movement. It is proof that the mechanism clears the admission bar
under the current benchmark discipline and should remain enabled by default.

## Data Sources

| document | role |
|---|---|
| [minimal-loop-t2-4-recheck-20260612.md](minimal-loop-t2-4-recheck-20260612.md) | Task14 check-only baseline before Task15 |
| [minimal-loop-t2-4-task15-feedback-rerun-20260612.md](minimal-loop-t2-4-task15-feedback-rerun-20260612.md) | GPU/Ollama Task15 rerun with the feedback mechanism enabled |
| [t2-4-run-variance.md](t2-4-run-variance.md) | GPU-free estimate of run-to-run variance from Task15 non-fired runs |
| [minimal-loop-t2-4-task18-recheck-20260612.md](minimal-loop-t2-4-task18-recheck-20260612.md) | Final post-hoc recheck after check false-negative fixes |
| [mechanism-ledger.md](mechanism-ledger.md) | Admission ledger for mechanism M001 |

## What Was Re-run

Task15 was a real GPU/Ollama benchmark run:

- Root: `.anvil/benchmarks/20260612T174200-32077`
- Engine: `minimal`
- Runs: 25 scenarios x 5 runs
- Mechanism: `completion-without-write feedback` enabled
- Off flag absent: `ANVIL_NO_MINIMAL_COMPLETION_WITHOUT_WRITE_FEEDBACK=1`

Task18 was not a model rerun. It was a post-hoc recheck of saved artifacts:

- legacy root: `.anvil/benchmarks/20260612T024532-70925`
- Task14 fixed-binary minimal root: `.anvil/benchmarks/20260612T145227-40162`
- Task15 feedback minimal root: `.anvil/benchmarks/20260612T174200-32077`

This distinction matters: Task18 measures the effect of corrected
`success_check` definitions, not new model behavior.

## Admission Target

The mechanism targeted four no-edit-loop scenarios identified during triage:

| scenario | Task14 baseline | Task15 rerun | delta |
|---|---:|---:|---:|
| `new-python-csv-small` | 2/5 | 3/5 | +1 |
| `fix-rust-parser-error` | 1/5 | 4/5 | +3 |
| `fix-readme-command` | 2/5 | 5/5 | +3 |
| `fix-python-slugify` | 2/5 | 5/5 | +3 |
| total | 7/20 | 17/20 | +10 |

This meets the primary admission criterion.

## Final Scenario Table

The following table uses the Task18 recheck as the current baseline.

| scenario | legacy final | minimal before mechanism | minimal with mechanism | delta vs legacy |
|---|---:|---:|---:|---:|
| `fix-css-token-doc` | 5/5 | 3/5 | 4/5 | -1 |
| `fix-js-date-helper` | 5/5 | 0/5 | 2/5 | -3 |
| `fix-json-normalizer` | 5/5 | 5/5 | 5/5 | +0 |
| `fix-python-retry-policy` | 5/5 | 5/5 | 4/5 | -1 |
| `fix-python-slugify` | 4/5 | 2/5 | 5/5 | +1 |
| `fix-readme-command` | 5/5 | 2/5 | 5/5 | +0 |
| `fix-rust-parser-error` | 5/5 | 1/5 | 4/5 | -1 |
| `fix-shell-safe-clean` | 2/5 | 5/5 | 5/5 | +3 |
| `long-session-data-report` | 0/5 | 2/5 | 1/5 | +1 |
| `long-session-large-component` | 0/5 | 1/5 | 5/5 | +5 |
| `long-session-read-edit` | 0/5 | 1/5 | 1/5 | +1 |
| `multi-file-docs-and-examples` | 0/5 | 2/5 | 4/5 | +4 |
| `multi-file-node-package` | 0/5 | 5/5 | 5/5 | +5 |
| `multi-file-python-package` | 0/5 | 2/5 | 1/5 | +1 |
| `multi-file-rust-library` | 5/5 | 5/5 | 4/5 | -1 |
| `new-large-react-kanban` | 0/5 | 2/5 | 1/5 | +1 |
| `new-markdown-release-notes` | 5/5 | 5/5 | 5/5 | +0 |
| `new-python-csv-small` | 5/5 | 2/5 | 3/5 | -2 |
| `new-rust-cli-small` | 5/5 | 0/5 | 4/5 | -1 |
| `new-typescript-formatter` | 5/5 | 4/5 | 5/5 | +0 |
| `non-coding-research-brief` | 5/5 | 5/5 | 5/5 | +0 |
| `non-coding-runbook` | 5/5 | 3/5 | 3/5 | -2 |
| `scaffold-fastapi-service` | 0/5 | 5/5 | 1/5 | +1 |
| `scaffold-next-dashboard` | 0/5 | 1/5 | 1/5 | +1 |
| `scaffold-rust-cli` | 0/5 | 0/5 | 0/5 | +0 |

## Interpretation

### Confirmed

- The admitted mechanism directly addresses the observed no-edit-loop failure
  class and moves the target set by +10 runs in the real Task15 rerun.
- The current headline comparison is 83/125 minimal vs 71/125 legacy-lite.
- The speed advantage remains material: 38.5 sec vs 66.6 sec elapsed mean.
- The mechanism is small, deterministic, and has an explicit off flag:
  `ANVIL_NO_MINIMAL_COMPLETION_WITHOUT_WRITE_FEEDBACK=1`.

### Not Confirmed

- The full aggregate gain cannot be attributed only to the mechanism. In Task17,
  Task15 runs where feedback did not fire still moved from 42/77 to 57/77.
- Single-scenario n=5 swings are not reliable causal evidence. Treat +/-2 runs
  as ordinary noise and +/-3 or larger as a triage trigger.
- `scaffold-fastapi-service` is not evidence against the mechanism: it regressed
  from 5/5 to 1/5, but feedback did not fire in any of those five runs.

### Watchlist

| item | status | next action |
|---|---|---|
| `multi-file-rust-library` | 5/5 to 4/5 while feedback fired in all five runs | Optional narrow on/off ablation, 10 runs each if GPU time is available |
| `fix-js-date-helper` | still 2/5 vs legacy 5/5 after semantic check | Triage remaining behavior/missing-file failures before adding any mechanism |
| `new-python-csv-small` | target improved but remains 3/5 vs legacy 5/5 | Inspect residual failures; do not assume parser or no-edit-loop cause |
| scaffold scenarios | highly unstable, especially FastAPI | Treat n=5 scaffold swings as variance until reproduced |
| `scaffold-rust-cli` | 0/5 for all current series | Still a true dead scenario; likely requires separate scaffold/loop triage |

## Decision

M001, completion-without-write feedback, is admitted and should remain enabled
by default.

The Phase 3 first-cycle baseline is:

- legacy-lite: 71/125
- minimal before mechanism: 68/125
- minimal with M001: 83/125

The next mechanism should not be added directly from the aggregate table.
Continue the same discipline: failure triage first, single deterministic trigger,
neutral feedback text, off flag, bounded line growth, and ledger update.

## Recommended Next Step

Start the next triage cycle from the remaining minimal losses:

1. `fix-js-date-helper`
2. `new-python-csv-small`
3. `non-coding-runbook`
4. scaffold instability, especially `scaffold-rust-cli`

The strongest near-term candidate is not yet a mechanism; it is a focused triage
of why `fix-js-date-helper` still fails after replacing line-count checks with
semantic checks.
