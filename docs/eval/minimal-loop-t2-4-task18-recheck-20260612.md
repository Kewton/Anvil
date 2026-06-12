# Minimal Loop T2-4 Task18 Recheck

- Date: 2026-06-12 JST
- Benchmark: `minimal-loop-expanded`
- Model: `qwen3.6:27b-coding-nvfp4`
- Purpose: re-evaluate existing roots after the remaining check false-negative
  fixes for `fix-js-date-helper` and `fix-python-retry-policy`.

## Inputs

| series | root | rows used |
|---|---|---:|
| legacy-lite | `.anvil/benchmarks/20260612T024532-70925` | 125 canonical `legacy` rows |
| Task14 fixed-binary minimal | `.anvil/benchmarks/20260612T145227-40162` | 125 `minimal` rows |
| Task15 feedback minimal | `.anvil/benchmarks/20260612T174200-32077` | 125 `minimal` rows |

The legacy root contains 147 rows because older extra rows are present. This
report filters it to rows whose engine is `legacy` and whose workdir is under
`/legacy/`.

## Check Changes

| scenario | change | reason |
|---|---|---|
| `fix-js-date-helper` | `min_lines:25` to `min_lines:12`, plus Node semantic check | Valid 20-22 line Task15 implementations were rejected, but older 15-18 line samples include incorrect behavior; semantic validation is needed. |
| `fix-python-retry-policy` | `min_lines:25` to `min_lines:12`, plus Python semantic check | Valid 13-24 line implementations were rejected solely by line count. |

## Aggregate Result

| series | previous recheck | Task18 recheck | delta | rc=0 | elapsed mean |
|---|---:|---:|---:|---:|---:|
| legacy-lite | 66/125 | 71/125 | +5 | 125/125 | 66.6 sec |
| Task14 fixed-binary minimal | 69/125 | 68/125 | -1 | 124/125 | 28.9 sec |
| Task15 feedback minimal | 80/125 | 83/125 | +3 | 122/125 | 38.5 sec |

Task15 remains ahead of the updated legacy-lite baseline by +12 runs
(83/125 vs 71/125) while retaining the speed advantage.

## Scenario Summary

| scenario | legacy final | Task14 fixed-binary final | Task15 feedback final | Task15 vs Task14 | Task15 vs legacy |
|---|---:|---:|---:|---:|---:|
| `fix-css-token-doc` | 5/5 | 3/5 | 4/5 | +1 | -1 |
| `fix-js-date-helper` | 5/5 | 0/5 | 2/5 | +2 | -3 |
| `fix-json-normalizer` | 5/5 | 5/5 | 5/5 | +0 | +0 |
| `fix-python-retry-policy` | 5/5 | 5/5 | 4/5 | -1 | -1 |
| `fix-python-slugify` | 4/5 | 2/5 | 5/5 | +3 | +1 |
| `fix-readme-command` | 5/5 | 2/5 | 5/5 | +3 | +0 |
| `fix-rust-parser-error` | 5/5 | 1/5 | 4/5 | +3 | -1 |
| `fix-shell-safe-clean` | 2/5 | 5/5 | 5/5 | +0 | +3 |
| `long-session-data-report` | 0/5 | 2/5 | 1/5 | -1 | +1 |
| `long-session-large-component` | 0/5 | 1/5 | 5/5 | +4 | +5 |
| `long-session-read-edit` | 0/5 | 1/5 | 1/5 | +0 | +1 |
| `multi-file-docs-and-examples` | 0/5 | 2/5 | 4/5 | +2 | +4 |
| `multi-file-node-package` | 0/5 | 5/5 | 5/5 | +0 | +5 |
| `multi-file-python-package` | 0/5 | 2/5 | 1/5 | -1 | +1 |
| `multi-file-rust-library` | 5/5 | 5/5 | 4/5 | -1 | -1 |
| `new-large-react-kanban` | 0/5 | 2/5 | 1/5 | -1 | +1 |
| `new-markdown-release-notes` | 5/5 | 5/5 | 5/5 | +0 | +0 |
| `new-python-csv-small` | 5/5 | 2/5 | 3/5 | +1 | -2 |
| `new-rust-cli-small` | 5/5 | 0/5 | 4/5 | +4 | -1 |
| `new-typescript-formatter` | 5/5 | 4/5 | 5/5 | +1 | +0 |
| `non-coding-research-brief` | 5/5 | 5/5 | 5/5 | +0 | +0 |
| `non-coding-runbook` | 5/5 | 3/5 | 3/5 | +0 | -2 |
| `scaffold-fastapi-service` | 0/5 | 5/5 | 1/5 | -4 | +1 |
| `scaffold-next-dashboard` | 0/5 | 1/5 | 1/5 | +0 | +1 |
| `scaffold-rust-cli` | 0/5 | 0/5 | 0/5 | +0 | +0 |

## Corrected Scenarios

### `fix-js-date-helper`

| series | result | notes |
|---|---:|---|
| legacy-lite | 5/5 | Already passed; remains passed. |
| Task14 fixed-binary minimal | 0/5 | Previous 1/5 was a false positive under line-count-only checking; semantic check rejects all five. |
| Task15 feedback minimal | 2/5 | Two concise valid implementations now pass; two missing-file runs and one behavioral failure remain failed. |

### `fix-python-retry-policy`

| series | result | notes |
|---|---:|---|
| legacy-lite | 5/5 | Recovers from 0/5; all concise 13-16 line implementations satisfy semantic checks. |
| Task14 fixed-binary minimal | 5/5 | Unchanged. |
| Task15 feedback minimal | 4/5 | Recovers one 24-line false negative; one missing-file run remains failed. |

## Assessment

Task18 removes the remaining line-count false negatives without blindly
lowering the checks. `fix-js-date-helper` shows why semantic checks matter:
several short implementations mention `inclusiveDays` but do not satisfy the
actual same-day/reversed-range requirements.

This becomes the current Phase 3 first-cycle baseline:

- legacy-lite: 71/125
- Task14 fixed-binary minimal: 68/125
- Task15 feedback minimal: 83/125

The completion-without-write mechanism still clears the admission bar after the
check corrections, but its headline margin should now be reported as 83 vs 71,
not 80 vs 66.
