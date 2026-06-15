# Minimal Large-Task `/ultra-plan-run` Evaluation 2026-06-15

## Summary

This is the first large-task benchmark slice for minimal `/ultra-plan-run`.
It adds six cases outside the existing small/medium 25-case matrix:

- Next.js app development
- Next.js app modification
- FastAPI app development
- FastAPI app modification
- Rust CLI development
- Rust CLI modification

Result: `success_check` passed `1/6`. Execution return code was `rc=0` for
`3/6`. The headline success number is intentionally not a stable benchmark yet:
this was `runs=1` because the slice took about 92 minutes for six runs.

The more important finding is structural: `/ultra-plan-run` can decompose and
execute large tasks, but the new benchmark needs semantic checks and stronger
artifact-path contracts. Several failures were caused by brittle line-count
thresholds or plan/check mismatch rather than a total inability to complete the
task.

## Changes Under Evaluation

- Added `benchmarks/minimal-loop-large.yaml`.
- Extended `scripts/bench.sh` so a benchmark case can run with
  `run_mode: ultra-plan-run`.
- Recorded `RUN_MODE=ultra-plan-run`, `ULTRA_PROFILE=...`, and the deterministic
  bench seed in `active_flags`.
- Added a smoke test covering benchmark-driven `/ultra-plan-run` execution.

## Run

- Root: `.anvil/benchmarks/20260615T091127-59453`
- Command:

```bash
BENCH_DEBUG=0 bash scripts/bench.sh minimal-loop-large \
  --model qwen3.6:27b-coding-nvfp4 \
  --engine minimal \
  --runs 1 \
  --bench-no-debug \
  --no-precautions \
  --no-case-memory \
  --no-auto-test
```

- Model: `qwen3.6:27b-coding-nvfp4`
- Engine: `minimal`
- Mode: `/ultra-plan-run`
- Runs: `1` per case
- Total elapsed: `5530s` (`92.2m`)

Note: run-level `active_flags` correctly recorded the run mode, profile, and
seed. The `meta.json` build fields (`git_revision`, `git_dirty`, `binary_path`,
`build_time`) were `null` in this run and should be fixed before using this
suite for release-grade comparisons.

## Results

| Case | rc | success_check | elapsed | Direct reason |
| --- | ---: | --- | ---: | --- |
| `large-nextjs-app-development` | 1 | fail | 782s | Path mismatch and incomplete game phase: created `app/components/SpaceOpsGame.tsx` while check expected `components/SpaceOpsGame.tsx`; app shell was short; phase failed during generated plan lint. |
| `large-nextjs-app-modification` | 1 | fail | 168s | Failed in the first generated phase due plan lint around build verification; no substantial modification happened. |
| `large-fastapi-app-development` | 0 | fail | 995s | Built service, routes, catalog, and tests; 18 tests passed. Failed only `min_lines` for `app/main.py` and `app/models.py`. |
| `large-fastapi-app-modification` | 1 | fail | 1575s | Implemented service/routes/tests, but generated pytest step failed. Postcheck only failed `app/routes/orders.py` line threshold (`40<50`). |
| `large-rust-cli-development` | 0 | fail | 1126s | Built project and passed `cargo test`; failed only `src/args.rs` line threshold (`36<65`). |
| `large-rust-cli-modification` | 0 | pass | 884s | Passed. Implemented parsing, summary, CLI wiring, and tests; 26 tests passed. |

## Findings

1. `/ultra-plan-run` is doing real large-task work.
   The successful and near-successful runs produced multi-file projects, ran
   verification commands, and continued across multiple phases. This is a clear
   improvement over a single-turn minimal loop for large tasks.

2. The new benchmark cannot rely on `min_lines` as the main signal.
   `large-fastapi-app-development` and `large-rust-cli-development` both passed
   meaningful tests but failed because one file was shorter than expected. These
   should be converted to semantic checks.

3. Artifact-path contracts need to be stricter.
   The Next.js new-app case requested `components/SpaceOpsGame.tsx`, but the
   model created `app/components/SpaceOpsGame.tsx`. The benchmark should either
   accept one canonical project structure or the runner should surface the
   exact required artifact paths more strongly.

4. Plan lint is useful but currently too coarse for existing setup files.
   The Next.js modification case failed early because a generated step planned
   `npm run build` before lint believed the required Next.js entry paths were
   present. The setup files did exist, so the lint/check context needs review.

5. Step granularity is still too large in some phases.
   Several steps reached `minimal loop reached max_iterations (8)` but were
   still accepted by the step runner. This is acceptable for initial exploration,
   but release-grade large-task benchmarks should record step-level stop reasons
   and treat repeated max-iteration stops as a first-class signal.

## Recommended Next Work

1. Convert the new large cases from line-count checks to semantic checks:
   framework build/test commands, expected exports/routes, executable behavior,
   and small local verification scripts.
2. Tighten `/ultra-plan-run` artifact contracts so required paths are carried
   into each generated phase and step.
3. Fix benchmark `meta.json` build fields for this run mode.
4. Re-run this slice with `runs=3` after the check fixes. `runs=5` is probably
   too expensive until the suite is stable.
5. Keep the six cases separate from the existing 25-case Phase 3/4 matrix until
   the checks are stable. They measure a different operating regime.

