# anvilminimal eval

This directory contains the MVP eval harness for:

- minimal-loop
- step-plan
- plan-run
- ultra-plan-run
- ultra-step-run diagnostic replay

The harness is intentionally outside the Rust runtime. Python scripts under
`scripts/` read YAML suites, expand model matrices, run `anvilminimal` or the
source `anvildev` binary, execute deterministic postchecks, score plans, and
write comparable artifacts.

## Output

Each run creates one run root:

```text
workspace/eval-artifacts/anvilminimal-mvp/<timestamp>/
  preflight.json
  matrix.json
  summary.eval.tsv
  events.jsonl
  warnings.jsonl
  report.md
  runs/<run_id>/
    command.txt
    meta.json
    stdout.log
    stderr.log
    workdir/
    plans/
    snapshots/
    postcheck/
```

`summary.eval.tsv` is the comparison table. `events.jsonl` is the detailed
evidence stream. `report.md` is the human-readable aggregate.

For step plans, `summary.eval.tsv` contains two complementary scores:

- `plan_quality_score`: YAML validity, decomposition shape, responsibility
  boundaries, required paths, verify commands, dependency order, and
  repairability.
- `executable_plan_score`: whether the plan is likely to execute cleanly when
  handed to `plan-run`; this checks actionable write steps, expected path names
  in instructions, read-before-create risk, deterministic verify commands, and
  step budget fit.
- `constraint_coverage_score`: coverage of required final artifacts, required
  verify keywords, and profile-specific contracts such as the Next.js app
  layout, dependencies, build command, and port 3011.
- `verify_strength_score`: strength of verification commands. Build/test/dev
  readiness checks score higher than file existence, `cat`, or compile-only
  checks. The score also penalizes commands that look syntactically plausible
  but are not semantically useful in the target environment, such as raw
  `rustc --no-link` checks for a Cargo project.
- `execution_shape_readiness_score`: whether the YAML is shaped for clean
  execution by `plan-run`. In addition to wrapper-step and write-first risks,
  this includes environment compatibility signals that predict postcheck
  failures, such as explicit dependency/type coherence and configuration
  compatibility when a build command is the required deterministic
  verification.
- `artifact_ownership_score`: whether required artifacts are owned exactly once
  by steps, whether extra artifact ownership is introduced, and whether nested
  paths are naturally tied to their owning step.
- `lint_repair_score`: planner stability inside a run, based on schema repair,
  lint retry, parser limitation, and prompt issue signals.
- `stability_score`: populated only when the same scenario/model/mode appears
  multiple times in one run root, such as `--runs 3`.

`overall_score` uses the structural, executable, constraint, verify, artifact,
and lint-repair scores for `step-plan`, `plan-run`, and `ultra-step-run` rows so
a plan that looks well structured but is hard to execute is visible before the
runtime phase fails.

For runtime rows, additional scores explain whether a generated plan can
actually close through the minimal loop:

- `plan_run_runtime_health_score`: aggregate of runtime friction, artifact
  progress, tool policy compatibility, and finalization. This is intentionally
  separate from static YAML quality.
- `runtime_friction_raw_score`: raw tool/runtime friction before mode-specific
  interpretation. Use this for debugging provider/tool behavior, not as the
  primary cross-mode score.
- `runtime_friction_reason`: semicolon-delimited generic reason categories such
  as `tool_validation_error`, `repeated_inspection`, `max_iterations`, or
  `verify_repair_stagnation`.
- `execution_contract_adherence_score`: bridge metric that compares the saved
  plan contract with the actual produced artifacts and postcheck behavior. It
  is the capped aggregate of dependency, config, verify, and postcheck stability
  scores. The cap prevents a low bridge subscore from being hidden by unrelated
  high subscores.
- `execution_contract_adherence_raw_score`: the pre-cap bridge aggregate.
- `execution_contract_min_subscore`: the lowest numeric dependency/config/
  verify/postcheck bridge subscore used for the cap.
- `execution_contract_cap_reason`: generic cap reason, such as
  `postcheck_stability_below_60` or `min_subscore_below_40:<metric>`.
- `dependency_contract_score`: whether generated dependency manifests satisfy
  plan/profile constraints such as TypeScript major and React type package
  major compatibility.
- `config_contract_score`: whether generated config files such as
  `tsconfig.json` satisfy the plan/profile contract.
- `verify_contract_score`: whether plan verification covers the deterministic
  postcheck commands and those postchecks pass.
- `postcheck_stability_score`: whether postcheck passed without unstable
  signals such as auto-installing dependencies during build, lockfile patching,
  mixed package manager warnings, peer resolution conflicts, or compile errors.
- `postcheck_stability_reason`: semicolon-delimited generic postcheck reason
  categories. It intentionally avoids scenario-specific names.
- `prompt_contract_score`: whether the step execution prompt contains the
  source-parity context sections, such as overall goal, final artifacts,
  expected paths, verify commands, expected result, and bounded repair policy.
- `step_obligation_scope_score`: whether `plan-run` step execution treats only
  the current step's `expected_paths` as current obligations. It penalizes
  prompt-extracted final artifacts, completion-contract paths, or completion
  contract verification leaking into a step turn. Plan-level final contract
  verification is scored separately by success/failure classification.
- `phase_completion_score`: for `ultra-plan-run`, how far each ultra phase got
  through start, scaffold, step execution, profile check, and phase completion.
- `phase_*_score`: stage-level ultra phase diagnostics. These explain whether
  the failure was phase planning, scaffold, step execution, verification,
  postcheck, or finalization.
- `phase_failure_stage`: generic phase stage reported by runtime events when a
  phase fails.
- `build_verify_pass_score`: whether a strong build verify such as
  `npm run build`, `pnpm build`, `yarn build`, `cargo build`, or `tsc` passed
  in the completion contract.
- `build_repair_effectiveness_score`: whether bounded repair after a build
  verify failure edited relevant files and improved the deterministic
  diagnostic before the repair cap was exhausted.
- `compile_diagnostic_progress_score`: whether repeated verify attempts reduced
  or changed compiler/build failure signatures instead of repeating the same
  failure.
- `verify_repair_edit_score`: whether repair turns after verify failure made a
  concrete `Write`/`Edit`/`MultiEdit` change rather than only inspecting files.
- `ultra_runtime_health_score`: aggregate of phase completion, build verify,
  build repair effectiveness, diagnostic progress, and repair edit behavior.
- `finalization_score`: aggregate completion score. It is decomposed into
  `step_finalization_score`, `plan_finalization_score`,
  `deferred_verify_finalization_score`, and `postcheck_finalization_score` so
  step completion, plan-level contract completion, deferred verify, and
  postcheck completion do not collapse into one opaque number.
- `finalization_reason`: semicolon-delimited generic reason categories for
  completion failures.
- `failure_layer`: failure layer for failed rows: `planning`, `bridge`,
  `runtime`, `postcheck`, `provider`, or `environment`.
- `capability_failure_included`: whether the failed row should be included in
  agent capability score comparisons. Provider/environment failures are reported
  separately rather than mixed into capability averages.

Target metric reporting keeps the top-level scores limited and moves detailed
diagnostics into reason/subscore fields. `not_available`, blank cells, and `0`
mean different things: unavailable values are omitted from averages, blank means
not applicable or not observed, and `0` is a real score.

Existing run roots can be rescored after metric changes when their
`runs/<run_id>/anvil-events.jsonl`, postcheck logs, workdir, and plan artifacts
are still available:

```bash
python3 scripts/eval-rescore-runtime.py \
  --run-root /private/tmp/anvilminimal-eval-run \
  --suite eval/suites/mvp-smoke.yaml \
  --out-summary /private/tmp/anvilminimal-eval-run/summary.rescored.eval.tsv
```

Values that cannot be reconstructed from the stored run root are written as
`not_available` rather than `0`.

Metric guardrails:

- Scores should measure reusable contracts, not individual scenario answers.
  Profile-specific knowledge is allowed only when it is expressed as generic
  categories such as dependency coherence, config compatibility, verify
  coverage, artifact ownership, runtime friction, or postcheck stability.
- Do not add unconditional penalties for time-sensitive package versions or
  one-off failure messages. Penalize version drift only when the plan or
  scenario declared that version contract, and group log evidence into stable categories
  such as dependency mutation, lockfile mutation, package-manager mismatch,
  dependency resolution failure, config compatibility failure, or compile
  failure.
- Any new metric should be validated against at least one blind scenario or
  source/MVP comparison run before being used as a success gate.

`report.md` also includes `Plan Run Predictiveness` when both `step-plan` and
`plan-run` rows exist for the same scenario/model pair. It reports correlation,
false positives, and false negatives for the plan score as a predictor of
runtime success.

## Preflight

```bash
python3 scripts/eval-preflight.py \
  --suite eval/suites/mvp-smoke.yaml \
  --model-profile speed-cloud
```

Use `--offline-ok` when checking only the local script/suite wiring.

```bash
python3 scripts/eval-preflight.py \
  --suite eval/suites/mvp-smoke.yaml \
  --model-profile speed-cloud \
  --offline-ok
```

## Dry Run

Dry run never calls LLM providers or Ollama.

```bash
python3 scripts/eval-run.py \
  --suite eval/suites/mvp-smoke.yaml \
  --model-profile speed-cloud \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --runs 1 \
  --parallel 4 \
  --dry-run
```

To render commands for the source binary instead of the MVP binary:

```bash
python3 scripts/eval-run.py \
  --suite eval/suites/mvp-smoke.yaml \
  --model-profile speed-cloud \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --binary anvildev \
  --dry-run
```

`--binary anvildev` is auto-detected as the source CLI dialect. You can also
pass `--binary-kind anvildev` explicitly. The harness adds `--engine minimal`
and renders `--plan-run <PROMPT>` / `--ultra-plan-run <PROMPT>` as source
Anvil expects. MVP-only `--completion-contract-json` is not inserted for
`anvildev`; postchecks still run from the same suite after the child process
returns.

## Speed Cloud Eval

This excludes local LLMs and runs cloud-only rows with provider limits.

```bash
export OPENAI_API_KEY=...
export GEMINI_API_KEY=...

python3 scripts/eval-preflight.py \
  --suite eval/suites/mvp-provider-smoke.yaml \
  --model-profile speed-cloud \
  --live-provider-smoke all

python3 scripts/eval-run.py \
  --suite eval/suites/mvp-provider-smoke.yaml \
  --model-profile speed-cloud \
  --modes minimal-loop,plan-run,ultra-plan-run \
  --runs 1 \
  --parallel 4 \
  --timeout-sec 600

python3 scripts/eval-run.py \
  --suite eval/suites/mvp-smoke.yaml \
  --model-profile speed-cloud \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --runs 1 \
  --parallel 4 \
  --provider-smoke-summary workspace/eval-artifacts/anvilminimal-mvp/<provider-smoke>/summary.eval.tsv \
  --timeout-sec 1800
```

Source-binary comparison run:

```bash
python3 scripts/eval-run.py \
  --suite eval/suites/mvp-smoke.yaml \
  --model-profile speed-cloud \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --binary anvildev \
  --runs 1 \
  --parallel 4 \
  --timeout-sec 1800
```

Use `--allow-provider-smoke-failure` only for diagnostic runs where the provider
smoke failure is the subject of the investigation. Normal acceptance should keep
the provider smoke gate enabled.

## Blind Eval

`mvp-blind.yaml` is an independent holdout suite. It intentionally does not
include `mvp-smoke.yaml` or `mvp-balanced.yaml`, and its scenario IDs are tested
to stay disjoint from both. Use it after fixing failures found in the known
suites, not while tuning runtime logic from the same failure logs.

```bash
python3 scripts/eval-run.py \
  --suite eval/suites/mvp-blind.yaml \
  --model-profile speed-cloud \
  --modes minimal-loop,step-plan \
  --runs 1 \
  --parallel 4 \
  --timeout-sec 1800
```

For overfitting checks, compare the known and blind results:

```bash
python3 scripts/eval-run.py \
  --suite eval/suites/mvp-smoke.yaml \
  --model-profile speed-cloud \
  --modes minimal-loop,step-plan \
  --runs 1 \
  --parallel 4 \
  --timeout-sec 1800

python3 scripts/eval-run.py \
  --suite eval/suites/mvp-blind.yaml \
  --model-profile speed-cloud \
  --modes minimal-loop,step-plan \
  --runs 1 \
  --parallel 4 \
  --timeout-sec 1800
```

A large success-rate gap between these two runs is evidence that the latest
runtime changes are too tuned to the known suite.

## Local Eval

Local LLM rows are serial by default.

```bash
python3 scripts/eval-run.py \
  --suite eval/suites/mvp-smoke.yaml \
  --model-profile local-only \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --runs 1 \
  --parallel 1 \
  --timeout-sec 3600
```

## Full Matrix

```bash
python3 scripts/eval-run.py \
  --suite eval/suites/mvp-full.yaml \
  --model-profile full \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run,ultra-step-run \
  --runs 3 \
  --parallel 4 \
  --timeout-sec 3600
```

`ultra-step-run` is diagnostic replay. If phase snapshots are unavailable, rows
are written as `diagnostic_skipped` and are not mixed into success rate.

## Plan Scoring

```bash
python3 scripts/eval-score-plan.py \
  --plan eval/fixtures/plans/good-step-plan.yaml \
  --scenario-id nextjs-space-invaders-large
```

Re-score a run root:

```bash
python3 scripts/eval-score-plan.py \
  --run-root workspace/eval-artifacts/anvilminimal-mvp/<timestamp> \
  --rules eval/scoring_rules.yaml
```

## Postcheck

```bash
python3 scripts/eval-postcheck.py \
  --scenario eval/fixtures/postcheck/nextjs-dev-server.yaml \
  --workdir /path/to/workdir \
  --out /tmp/anvilminimal-postcheck
```

Long-running dev servers are started as foreground child processes, checked for
HTTP readiness, then stopped with a signal.

## Report

```bash
python3 scripts/eval-report.py \
  --run-root workspace/eval-artifacts/anvilminimal-mvp/<timestamp>
```

## Compare

```bash
python3 scripts/eval-compare.py \
  --baseline workspace/eval-artifacts/anvilminimal-mvp/<baseline>/summary.eval.tsv \
  --experiment workspace/eval-artifacts/anvilminimal-mvp/<experiment>/summary.eval.tsv \
  --out workspace/eval-artifacts/anvilminimal-mvp/<experiment>/compare.md
```

## Tests

```bash
cargo test
python3 -m unittest discover -s tests/eval -p 'test_*.py'
```

Live provider/network checks are not part of unit tests. Run
`eval-preflight.py --live-provider-smoke all` before cloud eval to verify the
current model/endpoint/tool-declaration contract.
