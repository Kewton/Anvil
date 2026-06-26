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
  checks.
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
- `prompt_contract_score`: whether the step execution prompt contains the
  source-parity context sections, such as overall goal, final artifacts,
  expected paths, verify commands, expected result, and bounded repair policy.
- `step_obligation_scope_score`: whether `plan-run` step execution treats only
  the current step's `expected_paths` as current obligations. It penalizes
  prompt-extracted final artifacts, completion-contract paths, or completion
  contract verification leaking into a step turn. Plan-level final contract
  verification is scored separately by success/failure classification.

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
