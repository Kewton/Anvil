# Cycle 3 Full Matrix

Date: 2026-06-13 JST

Purpose: Task26 full seeded matrix after the blocked-`mkdir` trap fixes, with a
same-binary M001 on/off ablation and an initial 8B frontier row.

## Inputs

All new runs used the same clean release binary:

| field | value |
|---|---|
| commit | `b32e1bbfc08eb33617c85e15b544b725cc5101d1` |
| dirty | `false` |
| binary | `target/release/anvil` |
| build time | `2026-06-12T16:44:58Z` |
| benchmark | `minimal-loop-expanded` |
| scenarios | 25 scenarios x 5 runs |
| seed | enabled; 125 unique seeds per new series |
| flags | `--max-iterations 12 --no-auto-test --no-precautions --no-case-memory --bench-no-debug` |

New roots:

| series | root | M001 |
|---|---|---|
| A: 27B minimal | `.anvil/benchmarks/20260613T014538-362` | on |
| B: 27B minimal ablation | `.anvil/benchmarks/20260613T030920-91203` | off via `ANVIL_NO_MINIMAL_COMPLETION_WITHOUT_WRITE_FEEDBACK=1` |
| C: 8B minimal | `.anvil/benchmarks/20260613T040938-58258` | on |

Reference series:

| series | source | note |
|---|---|---|
| legacy-lite | [Task18 recheck](minimal-loop-t2-4-task18-recheck-20260612.md) | existing root rechecked after success_check fixes; not re-run with seed |

Hardware overview: Mac Studio, Apple M3 Ultra, 28 CPU cores, 256 GB memory,
Apple M3 Ultra GPU with 60 cores. The bench metadata does not currently record
hardware, so this was captured manually from the runner.

## Aggregate Results

| series | model | success | rc=0 | mean elapsed_s | median elapsed_s | max elapsed_s |
|---|---|---:|---:|---:|---:|---:|
| legacy-lite reference | `qwen3.6:27b-coding-nvfp4` | 71/125 | 125/125 | 66.6 | n/a | n/a |
| A: minimal M001 on | `qwen3.6:27b-coding-nvfp4` | 88/125 | 125/125 | 39.7 | 21.0 | 254 |
| B: minimal M001 off | `qwen3.6:27b-coding-nvfp4` | 64/125 | 125/125 | 28.5 | 14.0 | 215 |
| C: minimal M001 on | `qwen3:8b` | 16/125 | 124/125 | 32.3 | 26.0 | 219 |

The Cycle 3 27B baseline is A: 88/125. It is +17 runs over the current
legacy-lite reference while remaining faster on mean elapsed time. Because the
legacy row is an existing recheck result rather than a seeded rerun, treat it as
the historical reference series, not a same-seed paired comparison.

## M001 Re-Audit

Task26 fulfills the ledger reservation from the blocked-`mkdir` audit:

| metric | M001 off | M001 on | delta |
|---|---:|---:|---:|
| overall success | 64/125 | 88/125 | +24 |
| pure no-edit failures | 50 | 16 | -34 |
| wrote but failed success_check | 11 | 21 | +10 |
| rc != 0 / other failures | 0 | 0 | 0 |
| mean elapsed_s | 28.5 | 39.7 | +11.2 |

M001 fired in 55/125 A runs. Those fired runs were 31/55 successful; the
remaining 24 failures included 16 sessions that still made no `Write`/`Edit`
call after feedback. The residual pure no-edit class is therefore still real,
but much smaller than in the ablation.

The ablation also shows expected tradeoffs. M001 improves broad file-creation
coverage, especially `new-python-csv-small` (+4), `scaffold-next-dashboard`
(+4), `fix-css-token-doc` (+4), and `new-rust-cli-small` (+3). It also has
small negative deltas in `new-large-react-kanban` (-2) and
`multi-file-python-package` (-1). Those negative deltas should be watched, but
they are not large enough to outweigh the measured +24 headline effect.

## Scenario Results

| scenario | legacy recheck | A 27B M001 on | B 27B M001 off | A-B | A-legacy | C 8B M001 on |
|---|---:|---:|---:|---:|---:|---:|
| `fix-css-token-doc` | 5/5 | 5/5 | 1/5 | +4 | +0 | 3/5 |
| `fix-js-date-helper` | 5/5 | 2/5 | 0/5 | +2 | -3 | 0/5 |
| `fix-json-normalizer` | 5/5 | 5/5 | 5/5 | +0 | +0 | 3/5 |
| `fix-python-retry-policy` | 5/5 | 5/5 | 5/5 | +0 | +0 | 0/5 |
| `fix-python-slugify` | 4/5 | 3/5 | 3/5 | +0 | -1 | 0/5 |
| `fix-readme-command` | 5/5 | 5/5 | 4/5 | +1 | +0 | 1/5 |
| `fix-rust-parser-error` | 5/5 | 4/5 | 4/5 | +0 | -1 | 0/5 |
| `fix-shell-safe-clean` | 2/5 | 4/5 | 4/5 | +0 | +2 | 0/5 |
| `long-session-data-report` | 0/5 | 1/5 | 0/5 | +1 | +1 | 0/5 |
| `long-session-large-component` | 0/5 | 4/5 | 3/5 | +1 | +4 | 0/5 |
| `long-session-read-edit` | 0/5 | 3/5 | 2/5 | +1 | +3 | 1/5 |
| `multi-file-docs-and-examples` | 0/5 | 5/5 | 3/5 | +2 | +5 | 0/5 |
| `multi-file-node-package` | 0/5 | 4/5 | 4/5 | +0 | +4 | 0/5 |
| `multi-file-python-package` | 0/5 | 1/5 | 2/5 | -1 | +1 | 0/5 |
| `multi-file-rust-library` | 5/5 | 4/5 | 3/5 | +1 | -1 | 0/5 |
| `new-large-react-kanban` | 0/5 | 0/5 | 2/5 | -2 | +0 | 0/5 |
| `new-markdown-release-notes` | 5/5 | 5/5 | 5/5 | +0 | +0 | 4/5 |
| `new-python-csv-small` | 5/5 | 5/5 | 1/5 | +4 | +0 | 1/5 |
| `new-rust-cli-small` | 5/5 | 3/5 | 0/5 | +3 | -2 | 2/5 |
| `new-typescript-formatter` | 5/5 | 5/5 | 4/5 | +1 | +0 | 0/5 |
| `non-coding-research-brief` | 5/5 | 4/5 | 3/5 | +1 | -1 | 0/5 |
| `non-coding-runbook` | 5/5 | 5/5 | 5/5 | +0 | +0 | 1/5 |
| `scaffold-fastapi-service` | 0/5 | 2/5 | 1/5 | +1 | +2 | 0/5 |
| `scaffold-next-dashboard` | 0/5 | 4/5 | 0/5 | +4 | +4 | 0/5 |
| `scaffold-rust-cli` | 0/5 | 0/5 | 0/5 | +0 | +0 | 0/5 |

## Residual Failures

The residual failure classification below is deterministic and artifact-based:
failed run with zero `Write`/`Edit` calls is `pure no-edit`; failed run with at
least one `Write`/`Edit` call is `wrote but failed`; nonzero rc is `other`.

| series | failures | pure no-edit | wrote but failed | other |
|---|---:|---:|---:|---:|
| A: 27B M001 on | 37 | 16 | 21 | 0 |
| B: 27B M001 off | 61 | 50 | 11 | 0 |
| C: 8B M001 on | 109 | 0 | 108 | 1 |

Cycle 3 input:

- The old no-edit problem is much smaller with M001 on, but 16 A failures still
  ended without a file-changing tool. This supports keeping M002 as a candidate
  after a focused admission triage.
- The larger remaining A class is now "wrote but failed", concentrated in
  semantic or completeness failures such as `fix-js-date-helper`,
  `long-session-data-report`, `multi-file-python-package`, and scaffold tasks.
- C shows a different frontier failure mode: the 8B model usually writes
  something, but the artifact does not satisfy checks. That points to model
  capability and verifier-guided repair questions rather than no-edit recovery.

## Phase 4 Conditions

This report does not decide the default switch. Current status against the three
previously proposed conditions:

| condition | status |
|---|---|
| 1. Second model confirmation | Partial. The 8B row is now measured, but it is only 16/125, so it does not confirm parity on small models. |
| 2. Headline delta exceeds noise | Pass for the 27B seeded Cycle 3 series: A is +17 over the current legacy-lite reference and +24 over its own M001-off ablation. |
| 3. No serious interactive-use regression | Not decided by bench data. No new bench evidence points to a severe interaction regression, but real interactive usage remains a human acceptance input. |

## Assessment

Proceed with A as the Cycle 3 / Phase 4 candidate baseline for `qwen3.6`.
M001 remains admitted and its post-trap pure contribution is stronger than the
original admission measurement. The next engineering decision should not be a
generic verifier mechanism yet: first use the residual-failure table and the
blocked script-run triage to choose between a narrow M002 admission and a small
offline policy classifier fix for local script validation.
