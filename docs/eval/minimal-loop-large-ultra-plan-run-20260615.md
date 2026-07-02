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

## Runs=3 Semantic Remeasurement

- Root: `.anvil/benchmarks/20260615T121321-79889`
- Command:

```bash
BENCH_DEBUG=0 bash scripts/bench.sh minimal-loop-large \
  --model qwen3.6:27b-coding-nvfp4 \
  --engine minimal \
  --runs 3 \
  --bench-no-debug \
  --no-precautions \
  --no-case-memory \
  --no-auto-test
```

- Rows: `18`
- `success_check`: `2/18`
- Elapsed sum: `12459s` (`207.7m`)

| Case | Success | Primary failure shape |
| --- | ---: | --- |
| `large-nextjs-app-development` | `0/3` | Required artifact path drift (`components/SpaceOpsGame.tsx`) in 2 runs; the remaining run created the required path but failed semantic verification. |
| `large-nextjs-app-modification` | `0/3` | Two runs missed `components/AnalyticsPanel.tsx`; failures also exposed plan lint false positives around existing Next.js setup and HTML tag text in natural-language instructions. |
| `large-fastapi-app-development` | `0/3` | One run missed all required app/test files; two created substantial artifacts but failed semantic commands. |
| `large-fastapi-app-modification` | `0/3` | Two runs created `tests/test_orders_service.py` but missed required `tests/test_orders.py`; one run reached the required file but still failed semantic verification. |
| `large-rust-cli-development` | `0/3` | Required integration test or semantic CLI behavior remained incomplete. |
| `large-rust-cli-modification` | `2/3` | Two runs passed; one failed early with compile/semantic command failures. |

### Runs=3 Findings

The `runs=3` result is worse than the initial saved-artifact semantic recheck,
but it is more informative: semantic checks now expose true large-task instability
instead of line-count false negatives. The strongest actionable signals are:

1. Required artifact paths are not consistently preserved from benchmark contract
   to ultra phases and generated step plans.
2. Plan lint is still too broad. It rejected build steps even when setup files
   already existed in the workspace, and it treated natural-language HTML tag
   mentions such as `<dl>` as shell syntax.
3. Step-level outcome logging is insufficient. Several steps printed
   `minimal loop reached max_iterations (8)` and were then reported as `ok`,
   hiding whether the step completed cleanly, passed after max-iteration stop,
   or was repaired.

The next code change should therefore stay methodological: carry required
artifact paths forward, narrow plan lint to clear contradictions, and record
step stop reasons. It should not add a task-specific success rule or another
feedback mechanism yet.

## Targeted Fix Verification

After the `runs=3` remeasurement, the runner was updated in four methodological
areas:

- `success_check.files` are appended to benchmark-driven `/ultra-plan-run`
  prompts as required final artifacts and recorded as
  `ULTRA_ARTIFACT_CONTRACT=success_check.files`.
- Generated phase and step plans preserve the original user goal instead of a
  model-summarized goal, so artifact contracts survive plan generation.
- Next.js plan lint now considers existing workspace files, accepts natural
  language mentions of HTML tags, rejects `node --check` for `.ts/.tsx`, and
  skips file-name specificity checks for verification-only steps.
- Step execution now records `stop_reason` values such as `completed`,
  `max_iterations_verified`, and `verification_failed_after_max_iterations`.

A targeted Next.js rerun exposed two additional issues, both outside the
original artifact-path contract bug:

1. Large source reads polluted the session context. A generated
   `components/SpaceOpsGame.tsx` was hundreds of lines long; full-file `cat` or
   `Read` pushed large raw file bodies back into the next prompt. Bash `cat`
   and full-file `Read` now summarize large file dumps with line/char counts
   plus head/tail excerpts. Build/test output remains unchanged.
2. The generated Next.js app used `@/components/SpaceOpsGame` but omitted
   `compilerOptions.baseUrl: "."` in `tsconfig.json`. The Next.js profile
   contract and verifier now treat `@/*` imports without the matching
   `baseUrl`/`paths` config as a profile violation.

### One-Case Rerun

- Root: `.anvil/benchmarks/20260615T182802-82255`
- Case: `large-nextjs-app-development`
- Runs: `1`
- Result: `success_check` failed (`command_failed:1`)
- Elapsed: `745s`
- Active flags included:
  `RUN_MODE=ultra-plan-run`, `ULTRA_PROFILE=nextjs`,
  `ULTRA_ARTIFACT_CONTRACT=success_check.files`, and a deterministic seed.

Direct result:

| Case | rc | success | elapsed | Direct reason |
| --- | ---: | --- | ---: | --- |
| `large-nextjs-app-development` | 1 | fail | 745s | Required artifacts were created at the correct paths, but `npm run build` failed because `app/page.tsx` imported `@/components/SpaceOpsGame` while `tsconfig.json` lacked `baseUrl: "."`. |

Interpretation:

- The original path drift (`app/components/...` instead of
  `components/...`) was fixed in this run.
- The previous `node --check .tsx` trap was fixed.
- The previous large `cat` stall was reduced enough for the run to reach build
  verification, but full-file `Read` still inflated one prompt to roughly 43k
  characters before the `Read` summarization fix was added.
- The remaining failure is a real semantic/build failure, not a line-count
  false negative.

This means the next stable measurement should be run only after the large
`Read` summarization and Next.js alias contract changes are included in the
release binary. A full six-case `runs=3` rerun has not yet been performed after
these two latest fixes.

### Aborted Latest-Binary Probe

- Root: `.anvil/benchmarks/20260615T184803-11364`
- Case: `large-nextjs-app-development`
- Result: aborted manually (`rc=130`), not included in comparisons.

This probe used the later binary that included both large `Read` summarization
and the Next.js `@/*` alias contract. It was stopped because the early
`create-package-config` step took several minutes with only `package.json`
created. Just before termination, the step had reached
`stop_reason=max_iterations_verified` and moved to step 2. The observation is
therefore operational rather than semantic: `/ultra-plan-run` can still spend a
large amount of time inside an early step even when the created artifact is
already sufficient for that step's expected paths.

Do not use this aborted root as a success-rate datapoint.

### Verifier Repair Direction

The `npm run build` failure in the targeted Next.js rerun should not be treated
as something to prevent entirely in planning. It is a normal development-loop
failure: implementation reached the verifier, the verifier produced a concrete
diagnostic, and the agent should repair the source/config then rerun the same
check.

The step runner now treats deterministic verifier failure as a bounded repair
loop:

- up to two file-changing repair attempts per failed step;
- up to four repair turns, so an inspection-only `Read` turn does not consume a
  file-changing repair slot;
- each cycle receives the failed command diagnostics, expected paths, required
  final artifacts, and the current repair cycle number;
- the repair prompt explicitly says not to call a verifier failure "deferred"
  when the log identifies a fixable source/config/test problem;
- after each repair turn, the same verification commands are rerun.

This is intentionally narrower than adding another general recovery mechanism.
It keeps the model loop simple and makes the existing
`implement -> verify -> repair -> verify` development cycle explicit.

### Repair Loop Follow-Up

- Root: `.anvil/benchmarks/20260615T191943-32601`
- Case: `large-nextjs-app-development`
- Runs: `1`
- Result: `rc=1`, but `success_check_success=true` and
  `postcheck_success=true`
- Elapsed: `393s`

This run reached all required artifacts and passed the benchmark semantic
check, but the internal step verifier still failed during `npm run build`.
The useful finding is in the repair trace:

1. The first repair turn only inspected `app/page.tsx` and `package.json`.
2. The second repair turn fixed the concrete import error by changing
   `@/components/SpaceOpsGame` to `../components/SpaceOpsGame`.
3. The next build exposed a different environment/dependency failure, but the
   previous implementation had already consumed both repair cycles.

The runner was updated so `expected_paths` early-success is used only when a
path was missing at the start of the turn. If a verification step already has
its expected files, a `Read` tool call no longer short-circuits the repair turn.
This keeps inspection and editing in the same repair turn when the model emits
them sequentially. Unit coverage:

- `run_plan_does_not_early_stop_repair_when_expected_path_already_exists`
- `run_plan_allows_second_repair_cycle_for_verifier_failures`
- `early_success_paths_stop_after_tool_execution`

### Aborted Repair-Binary Probe

- Root: `.anvil/benchmarks/20260615T193247-80966`
- Case: `large-nextjs-app-development`
- Result: aborted manually (`rc=130`), not included in comparisons.

The latest release binary created all required source artifacts at the correct
paths:

- `package.json`
- `app/layout.tsx`
- `app/page.tsx`
- `app/globals.css`
- `components/SpaceOpsGame.tsx` (`583` lines)

The run was stopped while `verify-build` was running because the probe had
already consumed about ten minutes. A manual `npm run build` in the generated
workdir failed with Next.js attempting to install TypeScript dependencies using
offline Yarn:

```text
It looks like you're trying to use TypeScript but do not have the required package(s) installed.
Installing devDependencies (yarn):
- typescript
- @types/react
- @types/node
error Couldn't find any versions for "next" that matches "^14.2.0" in our cache
```

Interpretation: the current remaining failure is no longer the artifact-path
contract or one-step verifier repair bug. It is an offline clean-workdir
verification mismatch: source files and `package.json` are present, but
`npm run build` in a fresh directory cannot type-check a generated Next.js
TypeScript app unless dependencies are installed or available in the package
manager cache. The benchmark semantic check intentionally avoids that install
dependency and should remain the comparison authority until the large-suite
verifier profile is adjusted.
