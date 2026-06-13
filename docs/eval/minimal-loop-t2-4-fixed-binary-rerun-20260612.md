# Minimal Loop T2-4 Fixed Binary Rerun

- Date: 2026-06-12 JST
- Model: `qwen3.6:27b-coding-nvfp4`
- Benchmark: `minimal-loop-expanded`
- Matrix rerun: 25 cases x 5 runs x minimal engine = 125 runs
- Legacy source BENCH_ROOT: `.anvil/benchmarks/20260612T024532-70925`
- Minimal source BENCH_ROOT: `.anvil/benchmarks/20260612T145227-40162`
- Combined comparison BENCH_ROOT: `.anvil/benchmarks/20260612T145227-combined-t2-4-fixed-rerun`
- Conditions: `--max-iterations 12 --no-auto-test --no-precautions --no-case-memory --bench-no-debug`

## Merge And Build Verification

| item | result |
|---|---|
| #1021 | merged as `e0813d824c8befb898ce528e20c08c10e477e40d` |
| #1022 | merged as `767f7836470231362f134f13d21d91fd53b84a02` |
| #1024 | merged as `1964e054a5dac98a0bfc8215884e4f15e03ec3da` |
| release build | `cargo build --release` completed successfully |
| binary path | `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/minimal-loop-plan/target/release/anvil` |
| binary mtime | `2026-06-12T14:46:47+0900` |
| recorded git revision | `1964e054a5dac98a0bfc8215884e4f15e03ec3da-dirty` |
| recorded build time | `2026-06-12T05:46:47Z` |

The worktree was dirty at build/run time because of untracked evaluation notes already present under `docs/eval/`.

`meta.json` now records the expected Task 9 build metadata. Representative run:

```json
{
  "build": {
    "git_commit": "1964e054a5dac98a0bfc8215884e4f15e03ec3da",
    "git_dirty": true,
    "git_revision": "1964e054a5dac98a0bfc8215884e4f15e03ec3da-dirty",
    "binary_path": "/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/minimal-loop-plan/target/release/anvil",
    "build_time": "2026-06-12T05:46:47Z"
  },
  "active_flags": [
    "BENCH_DEBUG=0",
    "ANVIL_BIN=/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/minimal-loop-plan/target/release/anvil",
    "ENGINE=minimal",
    "MAX_ITERATIONS=12",
    "CHAT_RETRIES=2",
    "ANVIL_NO_REMINDER=1",
    "ANVIL_NO_CASE_RETRIEVAL=1",
    "ANVIL_NO_CASE_RECORD=1",
    "ANVIL_NO_AUTO_TEST=1"
  ]
}
```

## Run Command

```sh
ANVIL_BIN=/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/minimal-loop-plan/target/release/anvil \
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

An earlier attempt at `.anvil/benchmarks/20260612T144807-25898` is invalid: it ran inside the sandbox without access to local Ollama, so all 125 runs exited immediately with `failed to contact Ollama: ... /api/tags`. The valid rerun above was executed outside the sandbox with Ollama access.

## Row Summary

`success_check` below uses `meta.json` fallback when `summary.tsv` has null extras for a failed run. The only such case is `fix-js-date-helper/run-5`, which hit `minimal loop reached max_iterations (12)` but still has `success_check_success=true` in `meta.json`.

| engine / run | rows | rc=0 | rc!=0 | success_check ok | success_check ng | success_check null | postcheck ok | elapsed total sec |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| legacy initial | 125 | 125 | 0 | 46 | 79 | 0 | 31 | 8319 |
| minimal fixed-binary rerun | 125 | 124 | 1 | 45 | 80 | 0 | 24 | 3610 |

For comparison with earlier minimal runs:

| minimal run | rc=0 | success_check ok | note |
|---|---:|---:|---|
| initial T2-4 | 95/125 | 35/125 | parser feedback failures present |
| old-binary rerun | 96/125 | 37/125 | #1021/#1022 were not present in the release binary |
| fixed-binary rerun | 124/125 | 45/125 | #1021/#1022/#1024 present in `target/release/anvil` |

## Scenario Summary

| scenario | legacy success | minimal success | delta | legacy rc=0 | minimal rc=0 |
|---|---:|---:|---:|---:|---:|
| fix-css-token-doc | 5/5 | 3/5 | -2 | 5/5 | 5/5 |
| fix-js-date-helper | 5/5 | 1/5 | -4 | 5/5 | 4/5 |
| fix-json-normalizer | 0/5 | 0/5 | +0 | 5/5 | 5/5 |
| fix-python-retry-policy | 0/5 | 5/5 | +5 | 5/5 | 5/5 |
| fix-python-slugify | 4/5 | 2/5 | -2 | 5/5 | 5/5 |
| fix-readme-command | 5/5 | 2/5 | -3 | 5/5 | 5/5 |
| fix-rust-parser-error | 5/5 | 1/5 | -4 | 5/5 | 5/5 |
| fix-shell-safe-clean | 2/5 | 5/5 | +3 | 5/5 | 5/5 |
| long-session-data-report | 0/5 | 0/5 | +0 | 5/5 | 5/5 |
| long-session-large-component | 0/5 | 1/5 | +1 | 5/5 | 5/5 |
| long-session-read-edit | 0/5 | 1/5 | +1 | 5/5 | 5/5 |
| multi-file-docs-and-examples | 0/5 | 2/5 | +2 | 5/5 | 5/5 |
| multi-file-node-package | 0/5 | 5/5 | +5 | 5/5 | 5/5 |
| multi-file-python-package | 0/5 | 2/5 | +2 | 5/5 | 5/5 |
| multi-file-rust-library | 0/5 | 0/5 | +0 | 5/5 | 5/5 |
| new-large-react-kanban | 0/5 | 2/5 | +2 | 5/5 | 5/5 |
| new-markdown-release-notes | 5/5 | 5/5 | +0 | 5/5 | 5/5 |
| new-python-csv-small | 5/5 | 0/5 | -5 | 5/5 | 5/5 |
| new-rust-cli-small | 0/5 | 0/5 | +0 | 5/5 | 5/5 |
| new-typescript-formatter | 0/5 | 0/5 | +0 | 5/5 | 5/5 |
| non-coding-research-brief | 5/5 | 5/5 | +0 | 5/5 | 5/5 |
| non-coding-runbook | 5/5 | 3/5 | -2 | 5/5 | 5/5 |
| scaffold-fastapi-service | 0/5 | 0/5 | +0 | 5/5 | 5/5 |
| scaffold-next-dashboard | 0/5 | 0/5 | +0 | 5/5 | 5/5 |
| scaffold-rust-cli | 0/5 | 0/5 | +0 | 5/5 | 5/5 |

## Compare.py Output

```text
# compare report

- schema_version: 1
- model_slug: qwen3.6-27b-coding-nvfp4:legacy->minimal
- baseline_dir: .anvil/benchmarks/20260612T145227-combined-t2-4-fixed-rerun
- experiment_dir: .anvil/benchmarks/20260612T145227-combined-t2-4-fixed-rerun
- generated_at: 2026-06-12T06:55:38Z
- threshold: 0.05

| metric | baseline (mean [CI]) | experiment (mean [CI]) | delta | delta_pct | verdict |
|---|---|---|---|---|---|
| elapsed_s | 66.552 [50.691, 82.413] (n=125) | 28.880 [22.728, 35.032] (n=125) | -37.672 | -56.61% | improved |
| error_500_count | 0.000 [0.000, 0.000] (n=125) | 0.000 [0.000, 0.000] (n=125) | 0.000 | null | unchanged |
| iter_count | 2.712 [2.468, 2.956] (n=125) | 3.363 [3.100, 3.626] (n=124) | 0.651 | 24.00% | regressed |
| page_tsx_has_game_keywords | 0.000 [0.000, 0.434] (n=5) | 0.000 [0.000, 0.658] (n=2) | 0.000 | null | unchanged |
| postcheck_success | 0.248 [0.181, 0.330] (n=125) | 0.200 [0.139, 0.279] (n=125) | -0.048 | null | unchanged |
| rc | 1.000 [0.970, 1.000] (n=125) | 0.992 [0.956, 0.999] (n=125) | -0.008 | null | unchanged |
| we_total | 1.752 [1.542, 1.962] (n=125) | 1.192 [0.925, 1.459] (n=125) | -0.560 | null | informational |

## failure categories

| category | baseline | experiment |
|---|---:|---:|
| completed | 60 | 124 |
| control_loop_exhausted | 6 | 0 |
| failed | 0 | 1 |
| missing_deliverable | 40 | 0 |
| missing_evidence | 19 | 0 |
```

## Parser Fix Evidence

The valid rerun logs contain no parser feedback failures:

| signal | count |
|---|---:|
| `tool call parser failed` | 0 |
| `native tool parser failed` | 0 |
| `ollama.chat.native_xml_salvage` | 0 |
| `Native tool calls are disabled for the rest of this session` | 0 |

Interpretation: #1021/#1022 were present and the native tool-call path no longer produced the previous parser failure pattern. #1022 salvage did not fire because no native-content XML salvage event was needed in this run.

## Findings

- The old-binary concern is resolved for this run: `meta.json` records #1024 HEAD, dirty state, binary path, build time, and active flags.
- Parser-related `rc!=0` failures collapsed from the old rerun's 29 failures to 0 observed parser feedback failures. Overall `rc=0` improved to 124/125.
- `success_check` with meta fallback is essentially tied with legacy: legacy 46/125 vs minimal 45/125. Minimal no longer clearly loses on success_check, but it also does not clearly beat legacy.
- Minimal is materially faster: compare.py elapsed mean improves from 66.552s to 28.880s, and raw elapsed total is 8319s vs 3610s.
- Remaining weak scenarios are now mostly product/quality/check failures rather than parser transport failures: `new-python-csv-small`, `new-rust-cli-small`, `new-typescript-formatter`, `fix-json-normalizer`, `multi-file-rust-library`, and all scaffold scenarios are still 0/5 success.
- `non-coding-research-brief` recovered to 5/5 success in the fixed-binary rerun.

## Assessment

This rerun is valid evidence for #1021/#1022/#1024. The parser transport bug is no longer the dominant blocker. Phase 3 should not proceed on parser triage alone; the next useful work is scenario-level outcome triage, especially the 0/5 success scenarios and the summary/meta mismatch for max-iteration runs that still produce valid artifacts.
