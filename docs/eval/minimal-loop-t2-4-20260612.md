# Minimal Loop T2-4 Initial Full Comparison

- Date: 2026-06-12 JST
- Model: `qwen3.6:27b-coding-nvfp4`
- Benchmark: `minimal-loop-expanded`
- Matrix: 25 cases x 5 runs x 2 engines = 250 runs
- Combined BENCH_ROOT: `.anvil/benchmarks/20260612T051125-combined-t2-4`
- Legacy source BENCH_ROOT: `.anvil/benchmarks/20260612T024532-70925`
- Minimal source BENCH_ROOT: `.anvil/benchmarks/20260612T051125-8470`
- Conditions: `--max-iterations 12 --no-auto-test --no-precautions --no-case-memory --bench-no-debug`

## T1-8 Smoke

`heavy-space-invaders` was run with `qwen3:8b` and `--engine minimal` after the parser-feedback fix.

- BENCH_ROOT: `.anvil/benchmarks/20260612T051004-7069`
- Result: `rc=0`, `success_check_success=true`, `elapsed_sec=46`
- Generated files: `src/components/SpaceInvaders.tsx` (229 lines), `src/app/page.tsx` (10 lines)

## T2-4 Row Summary

| engine | rows | rc=0 | success_check ok | success_check ng | elapsed total sec |
|---|---:|---:|---:|---:|---:|
| legacy | 125 | 125 | 46 | 79 | 8319 |
| minimal | 125 | 95 | 35 | 90 | 6905 |

## Compare.py Output

# compare report

- schema_version: 1
- model_slug: qwen3.6-27b-coding-nvfp4:legacy->minimal
- baseline_dir: .anvil/benchmarks/20260612T051125-combined-t2-4
- experiment_dir: .anvil/benchmarks/20260612T051125-combined-t2-4
- generated_at: 2026-06-11T22:13:40Z
- threshold: 0.05

| metric | baseline (mean [CI]) | experiment (mean [CI]) | delta | delta_pct | verdict |
|---|---|---|---|---|---|
| elapsed_s | 66.552 [50.691, 82.413] (n=125) | 55.240 [40.070, 70.410] (n=125) | -11.312 | -17.00% | ✅ improved |
| error_500_count | 0.000 [0.000, 0.000] (n=125) | 0.000 [0.000, 0.000] (n=125) | 0.000 | null | ➖ unchanged |
| iter_count | 2.712 [2.468, 2.956] (n=125) | 3.884 [3.477, 4.292] (n=95) | 1.172 | 43.22% | ❌ regressed |
| page_tsx_has_game_keywords | 0.000 [0.000, 0.434] (n=5) | 0.000 [0.000, 0.562] (n=3) | 0.000 | null | ➖ unchanged |
| postcheck_success | 0.248 [0.181, 0.330] (n=125) | 0.168 [0.113, 0.243] (n=125) | -0.080 | null | ❌ regressed |
| rc | 1.000 [0.970, 1.000] (n=125) | 0.760 [0.678, 0.826] (n=125) | -0.240 | null | ❌ regressed |
| we_total | 1.752 [1.542, 1.962] (n=125) | 0.824 [0.615, 1.033] (n=125) | -0.928 | null | ℹ️ informational |

## failure categories

| category | baseline | experiment |
|---|---:|---:|
| completed | 60 | 95 |
| control_loop_exhausted | 6 | 0 |
| failed | 0 | 30 |
| missing_deliverable | 40 | 0 |
| missing_evidence | 19 | 0 |


## Notes

- A default full comparison was attempted first, but legacy entered long control/verification loops. On `qwen3:8b`, `new-large-react-kanban` legacy run 1 took 1,071 sec and ended as `no_repo_progress`; on `qwen3.6:27b-coding-nvfp4`, default legacy entered verifier repair loops. The committed full comparison therefore uses the explicit ablation flags above to isolate the loop behavior and make the 250-run matrix tractable.
- During the first minimal run, real Ollama output exposed an unterminated `<anvil_tool_call>` parser failure. Minimal loop now treats `tool call parser failed` as recoverable ephemeral feedback instead of exiting immediately.
- `compare.py` now includes `meta.json` fallback for failed runs that exit before `session.json` is copied, so rc/postcheck failures are counted instead of silently skipped.
- Interpretation: minimal improves elapsed time, but regresses rc and success_check rates under this benchmark. Phase 3 should not adopt minimal broadly yet; it should target the failing categories and scenarios first.
