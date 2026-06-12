# Minimal Loop T2-4 Rerun After Parser Fixes

- Date: 2026-06-12 JST
- Model: `qwen3.6:27b-coding-nvfp4`
- Benchmark: `minimal-loop-expanded`
- Matrix rerun: 25 cases x 5 runs x minimal engine = 125 runs
- Compare baseline: legacy engine from the initial T2-4 run
- Legacy source BENCH_ROOT: `.anvil/benchmarks/20260612T024532-70925`
- Minimal source BENCH_ROOT: `.anvil/benchmarks/20260612T105012-57478`
- Combined comparison BENCH_ROOT: `.anvil/benchmarks/20260612T105012-combined-t2-4-rerun`
- Conditions: `--max-iterations 12 --no-auto-test --no-precautions --no-case-memory --bench-no-debug`

## Merged Fixes

| PR | merge commit | note |
|---|---|---|
| #1021 `Restore minimal loop XML fallback downgrade` | `e0813d824c8befb898ce528e20c08c10e477e40d` | Parser failure downgrades the session to XML fallback. |
| #1022 `Salvage XML tool content in native replies` | `767f7836470231362f134f13d21d91fd53b84a02` | Well-formed XML in native content is delegated to XML fallback extraction before malformed rejection. |

## Run Command

```sh
scripts/bench.sh minimal-loop-expanded \
  --model qwen3.6:27b-coding-nvfp4 \
  --runs 5 \
  --engine minimal \
  --max-iterations 12 \
  --no-auto-test \
  --no-precautions \
  --no-case-memory \
  --bench-no-debug
```

## Row Summary

| engine / run | rows | rc=0 | rc!=0 | success_check ok | success_check ng | success_check null | elapsed total sec |
|---|---:|---:|---:|---:|---:|---:|---:|
| legacy initial | 125 | 125 | 0 | 46 | 79 | 0 | 8319 |
| minimal initial | 125 | 95 | 30 | 35 | 60 | 30 | 6905 |
| minimal rerun | 125 | 96 | 29 | 37 | 59 | 29 | 7779 |

## Scenario Summary For Minimal Rerun

| scenario | rc=0 | rc!=0 | success ok | success ng | success null |
|---|---:|---:|---:|---:|---:|
| fix-css-token-doc | 5 | 0 | 4 | 1 | 0 |
| fix-js-date-helper | 4 | 1 | 0 | 4 | 1 |
| fix-json-normalizer | 5 | 0 | 0 | 5 | 0 |
| fix-python-retry-policy | 2 | 3 | 2 | 0 | 3 |
| fix-python-slugify | 3 | 2 | 3 | 0 | 2 |
| fix-readme-command | 4 | 1 | 3 | 1 | 1 |
| fix-rust-parser-error | 3 | 2 | 1 | 2 | 2 |
| fix-shell-safe-clean | 3 | 2 | 1 | 2 | 2 |
| long-session-data-report | 5 | 0 | 0 | 5 | 0 |
| long-session-large-component | 5 | 0 | 1 | 4 | 0 |
| long-session-read-edit | 5 | 0 | 1 | 4 | 0 |
| multi-file-docs-and-examples | 5 | 0 | 5 | 0 | 0 |
| multi-file-node-package | 2 | 3 | 1 | 1 | 3 |
| multi-file-python-package | 4 | 1 | 3 | 1 | 1 |
| multi-file-rust-library | 5 | 0 | 0 | 5 | 0 |
| new-large-react-kanban | 5 | 0 | 5 | 0 | 0 |
| new-markdown-release-notes | 5 | 0 | 5 | 0 | 0 |
| new-python-csv-small | 0 | 5 | 0 | 0 | 5 |
| new-rust-cli-small | 5 | 0 | 0 | 5 | 0 |
| new-typescript-formatter | 2 | 3 | 0 | 2 | 3 |
| non-coding-research-brief | 0 | 5 | 0 | 0 | 5 |
| non-coding-runbook | 4 | 1 | 2 | 2 | 1 |
| scaffold-fastapi-service | 5 | 0 | 0 | 5 | 0 |
| scaffold-next-dashboard | 5 | 0 | 0 | 5 | 0 |
| scaffold-rust-cli | 5 | 0 | 0 | 5 | 0 |

## Compare.py Output

```text
# compare report

- schema_version: 1
- model_slug: qwen3.6-27b-coding-nvfp4:legacy->minimal
- baseline_dir: .anvil/benchmarks/20260612T105012-combined-t2-4-rerun
- experiment_dir: .anvil/benchmarks/20260612T105012-combined-t2-4-rerun
- generated_at: 2026-06-12T04:05:38Z
- threshold: 0.05

| metric | baseline (mean [CI]) | experiment (mean [CI]) | delta | delta_pct | verdict |
|---|---|---|---|---|---|
| elapsed_s | 66.552 [50.691, 82.413] (n=125) | 62.232 [45.984, 78.480] (n=125) | -4.320 | -6.49% | improved |
| error_500_count | 0.000 [0.000, 0.000] (n=125) | 0.000 [0.000, 0.000] (n=125) | 0.000 | null | unchanged |
| iter_count | 2.712 [2.468, 2.956] (n=125) | 4.021 [3.659, 4.382] (n=96) | 1.309 | 48.26% | regressed |
| page_tsx_has_game_keywords | 0.000 [0.000, 0.434] (n=5) | 0.000 [0.000, 0.490] (n=4) | 0.000 | null | unchanged |
| postcheck_success | 0.248 [0.181, 0.330] (n=125) | 0.160 [0.106, 0.234] (n=125) | -0.088 | null | regressed |
| rc | 1.000 [0.970, 1.000] (n=125) | 0.768 [0.687, 0.833] (n=125) | -0.232 | null | regressed |
| we_total | 1.752 [1.542, 1.962] (n=125) | 1.008 [0.762, 1.254] (n=125) | -0.744 | null | informational |

## failure categories

| category | baseline | experiment |
|---|---:|---:|
| completed | 60 | 96 |
| control_loop_exhausted | 6 | 0 |
| failed | 0 | 29 |
| missing_deliverable | 40 | 0 |
| missing_evidence | 19 | 0 |
```

## Findings

- The two parser fixes were merged before the rerun, but the expected large recovery did not appear. Minimal moved from 95/125 to 96/125 `rc=0`, and `success_check ok` moved from 35/125 to 37/125.
- The two previously critical scenarios remain fully failing on return code: `new-python-csv-small` is 0/5 `rc=0`, and `non-coding-research-brief` is 0/5 `rc=0`.
- Minimal is still faster than legacy on elapsed mean, but still worse on `rc`, `postcheck_success`, and `iter_count`.
- The rerun logs contain 352 occurrences of parser failure text and no `ollama.chat.native_xml_salvage` activations. This suggests the task 7 native-content salvage path did not materially participate in this run.
- `scripts/report.py` reported `no runs discovered` for this rerun root, while `scripts/compare.py` successfully analyzed the same combined root with meta fallback. The comparison above uses `compare.py` as the source of truth.

## Assessment

The parser downgrade restoration and native XML salvage are merged, but this rerun does not support moving to Phase 3 on the assumption that the parser issue was the dominant remaining blocker. The remaining failures should be triaged from the new run root, with priority on `new-python-csv-small`, `non-coding-research-brief`, and runs that still contain parser failure feedback after downgrade.
