# Cycle 3 Task29 Rebaseline and Vibe-Local Comparison

Date: 2026-06-13 JST

Purpose: rebuild the Cycle 3 baseline after the Task29 `cd && script-run`
offline-policy fix, compare the resulting Anvil minimal loop with `vibe-local`
on the same 27B Ollama model, and run an extra focused 4-scenario variance
check.

## Inputs

Anvil runs used the release binary built from the Task29 merge commit:

| field | value |
|---|---|
| commit | `b27b79d048ec4b72c485a63556b6c263fac0e6f4` |
| dirty | `false` |
| binary | `target/release/anvil` |
| build time | `2026-06-13T01:31:25Z` |
| benchmark | `minimal-loop-expanded` |
| full matrix | 25 scenarios x 5 runs |
| focused rerun | 4 scenarios x 10 runs |
| seed | enabled for Anvil bench runs |
| flags | `--max-iterations 12 --no-auto-test --no-precautions --no-case-memory --bench-no-debug` |

`vibe-local` was run through
`/Users/maenokota/share/work/github_kewton/vibe-local/vibe-coder.py`.
It used the same Ollama host and 27B model, but the agent loop, prompt, tool
schema, seed support, isolation, and iteration budget are different.

| field | value |
|---|---|
| model | `qwen3.6:27b-coding-nvfp4` |
| vibe-local commit | `31a750d968e006fc1093897925bd043cf4f92adc` |
| seed | not supported by the wrapper used here |
| Anvil worktree dirty in vibe meta | `true`, because the local runner helper was not part of the Anvil binary under test |
| vibe-local dirty | `true` |

## Roots

| series | root |
|---|---|
| legacy-lite 27B | `.anvil/benchmarks/20260613T103318-58743` |
| Anvil minimal 27B, M001 on | `.anvil/benchmarks/20260613T124024-63887` |
| Anvil minimal 27B, M001 off | `.anvil/benchmarks/20260613T140957-31162` |
| Anvil minimal 8B, M001 on | `.anvil/benchmarks/20260613T150546-11315` |
| vibe-local 27B | `.anvil/benchmarks/20260613T163056-25350-vibe-local` |
| focused 4-scenario rerun | `.anvil/benchmarks/20260613T231941-78035` |

## Aggregate Results

| series | model | success | rc=0 | mean elapsed_s | note |
|---|---|---:|---:|---:|---|
| legacy-lite | `qwen3.6:27b-coding-nvfp4` | 73/125 | 125/125 | 60.6 | Task29-inclusive rerun |
| Anvil minimal M001 on | `qwen3.6:27b-coding-nvfp4` | 83/125 | 123/125 | 42.5 | Task29-inclusive rerun |
| Anvil minimal M001 off | `qwen3.6:27b-coding-nvfp4` | 48/125 | 122/125 | 26.4 | same binary ablation |
| Anvil minimal M001 on | `qwen3:8b` | 16/125 | 123/125 | 37.9 | frontier row |
| vibe-local | `qwen3.6:27b-coding-nvfp4` | 80/125 | 125/125 | 195.9 | same 27B model, different agent |

Task29-inclusive Anvil minimal remains ahead of legacy-lite on success
(83 vs 73) and is faster on mean elapsed time (42.5s vs 60.6s). The same 27B
model under `vibe-local` reached a similar success count (80 vs Anvil minimal
83), but took about 4.6x the mean elapsed time of Anvil minimal.

## Task29 Effect Check

The previous Task26 27B M001-on row was 88/125. The Task29-inclusive rerun is
83/125, a -5 movement. The logs do not support attributing that movement
directly to Task29:

- `tool.bash.cd_wrapper_reclassified` fired in only two unique full-matrix runs.
- Both fired runs were successful.
- None of the success-to-failure changed runs had the cd-wrapper reclassification
  event.
- The two nonzero rc runs were unrelated to cd-wrapper behavior:
  `fix-python-slugify/run-4` hit max iterations after producing a valid artifact,
  and `fix-python-retry-policy/run-3` hit an Ollama `/api/chat` 500.

The most likely reading is run-to-run variance plus unrelated runtime noise, not
a direct Task29 regression.

## Focused 4-Scenario Rerun

The focused rerun used the same Task29-inclusive Anvil binary and the same
27B model, with 10 runs each for the scenarios that moved most between Task26
and the Task29 full matrix.

| scenario | Task29 full matrix | focused rerun | mean elapsed_s |
|---|---:|---:|---:|
| `new-rust-cli-small` | 0/5 | 4/10 | 9.5 |
| `scaffold-next-dashboard` | 1/5 | 5/10 | 39.4 |
| `long-session-read-edit` | 1/5 | 2/10 | 28.6 |
| `new-large-react-kanban` | 3/5 | 1/10 | 19.7 |
| total | 5/20 | 12/40 | 24.3 |

This confirms that n=5 scenario rows can move substantially. In particular,
`new-rust-cli-small` and `scaffold-next-dashboard` look much less bad at n=10
than in the Task29 full matrix. Conversely, `new-large-react-kanban` dropped
from 3/5 in the full matrix to 1/10 in the focused rerun. Treat per-scenario
n=5 deltas as directional only unless the delta is large and reproduced.

## Vibe-Local Scenario Results

| scenario | success | mean elapsed_s |
|---|---:|---:|
| `fix-css-token-doc` | 5/5 | 161.8 |
| `fix-js-date-helper` | 2/5 | 75.2 |
| `fix-json-normalizer` | 5/5 | 137.6 |
| `fix-python-retry-policy` | 5/5 | 35.2 |
| `fix-python-slugify` | 4/5 | 75.4 |
| `fix-readme-command` | 5/5 | 23.2 |
| `fix-rust-parser-error` | 5/5 | 94.6 |
| `fix-shell-safe-clean` | 4/5 | 33.6 |
| `long-session-data-report` | 5/5 | 770.6 |
| `long-session-large-component` | 3/5 | 143.6 |
| `long-session-read-edit` | 3/5 | 254.6 |
| `multi-file-docs-and-examples` | 4/5 | 278.6 |
| `multi-file-node-package` | 2/5 | 140.6 |
| `multi-file-python-package` | 0/5 | 409.6 |
| `multi-file-rust-library` | 4/5 | 261.2 |
| `new-large-react-kanban` | 5/5 | 123.0 |
| `new-markdown-release-notes` | 5/5 | 20.4 |
| `new-python-csv-small` | 5/5 | 28.8 |
| `new-rust-cli-small` | 0/5 | 202.4 |
| `new-typescript-formatter` | 4/5 | 26.2 |
| `non-coding-research-brief` | 0/5 | 380.4 |
| `non-coding-runbook` | 0/5 | 428.0 |
| `scaffold-fastapi-service` | 3/5 | 199.0 |
| `scaffold-next-dashboard` | 2/5 | 253.0 |
| `scaffold-rust-cli` | 0/5 | 340.4 |

## Vibe-Local Behavior Notes

The data supports a mixed explanation for `vibe-local`:

- Positive cases: it often keeps checking, rewriting, and self-validating. This
  appears useful in `new-large-react-kanban`, `new-python-csv-small`, several
  `fix-*` cases, and `long-session-data-report`.
- Costly cases: it can spend many minutes and still fail. Examples are
  `multi-file-python-package` (0/5, 409.6s mean), `scaffold-rust-cli` (0/5,
  340.4s mean), `non-coding-research-brief` (0/5, 380.4s mean), and
  `non-coding-runbook` (0/5, 428.0s mean).
- The automatic sub-agent split is a double-edged mechanism. In
  `non-coding-runbook`, `vibe-local` repeatedly split the prompt into four
  sub-agents, waited several hundred seconds, and still failed to create the
  expected file.
- Some successful multi-file cases also used broad repository exploration from
  the benchmark workdir. That is useful capability, but it is not the same
  isolation model as the Anvil bench runs.

The short answer is that `vibe-local` is not simply "better because it keeps
going until success." It does gain from longer validation and repair loops in
some scenarios, but the same mechanism creates long failed runs when task
decomposition or recovery goes wrong.

## Assessment

The current Task29-inclusive comparison does not invalidate M001. The same
binary ablation is still large: 83/125 with M001 on versus 48/125 with M001
off. The Task29-specific policy fix does not show a direct success regression
in the logs, and the focused 4-scenario rerun shows that several apparent
n=5 drops are consistent with benchmark variance.

For the next cycle, the most useful comparison is not a broader full matrix
immediately. The data points instead to two narrower questions:

- why Anvil minimal still fails pure no-edit or low-effort completion patterns
  in some scenarios even after M001;
- which `vibe-local` behaviors are worth considering as candidates without
  importing its high-cost sub-agent and long-tail retry behavior wholesale.
