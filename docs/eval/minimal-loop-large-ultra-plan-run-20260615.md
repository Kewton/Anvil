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

Initial result: `success_check` passed `1/6`. Execution return code was `rc=0`
for `3/6`. After replacing line-count-heavy checks with semantic checks and
rechecking the same artifacts, the result is `4/6`. The headline success number
is intentionally not a stable benchmark yet: this was `runs=1` because the slice
took about 92 minutes for six runs.

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
- Replaced the large slice's primary `min_lines` checks with semantic checks
  using existing generic `success_check.commands`.

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

| Case | rc | initial check | semantic recheck | elapsed | Direct reason |
| --- | ---: | --- | --- | ---: | --- |
| `large-nextjs-app-development` | 1 | fail | fail | 782s | Path mismatch and incomplete game phase: created `app/components/SpaceOpsGame.tsx` while check expected `components/SpaceOpsGame.tsx`; phase failed during generated plan lint. |
| `large-nextjs-app-modification` | 1 | fail | fail | 168s | Failed in the first generated phase due plan lint around build verification; no substantial modification happened. |
| `large-fastapi-app-development` | 0 | fail | pass | 995s | Built service, routes, catalog, and tests; semantic checks pass. Initial failure was line-count false negative. |
| `large-fastapi-app-modification` | 1 | fail | pass | 1575s | Implemented service/routes/tests. Semantic checks pass despite a failed generated pytest step; initial postcheck was line-count false negative. |
| `large-rust-cli-development` | 0 | fail | pass | 1126s | Built project and passed `cargo test`; semantic CLI/file checks pass. Initial failure was line-count false negative. |
| `large-rust-cli-modification` | 0 | pass | pass | 884s | Passed. Implemented parsing, summary, CLI wiring, and tests; 26 tests passed. |

## Findings

1. `/ultra-plan-run` is doing real large-task work.
   The successful and near-successful runs produced multi-file projects, ran
   verification commands, and continued across multiple phases. This is a clear
   improvement over a single-turn minimal loop for large tasks.

2. The new benchmark cannot rely on `min_lines` as the main signal.
   Semantic recheck changed the saved-artifact result from `1/6` to `4/6`
   without rerunning the model. That confirms the initial line-count checks were
   measuring implementation shape more than user-visible behavior.

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

1. Re-run this slice with the semantic checks now in `minimal-loop-large.yaml`.
2. Tighten `/ultra-plan-run` artifact contracts so required paths are carried
   into each generated phase and step.
3. Fix benchmark `meta.json` build fields for this run mode.
4. Use `runs=3` for the next measurement. `runs=5` is probably too expensive
   until the suite is stable.
5. Keep the six cases separate from the existing 25-case Phase 3/4 matrix until
   the checks are stable. They measure a different operating regime.
