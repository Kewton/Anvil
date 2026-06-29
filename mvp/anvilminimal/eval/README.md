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
- `verify_adequacy_score`: whether verification is adequate for the declared
  functional contract. This is separate from command strength: a production
  build can be strong as a compiler check while still inadequate for an
  interactive-game contract.
- `semantic_verify_coverage_score`: how much of the required functional
  contract is represented by declared verification.
- `behavior_oracle_declared_score`: whether a behavior-level oracle such as a
  deterministic test or browser interaction check is declared.
- `contentless_verify_penalty`: penalty for checks that only prove file
  existence or readability, such as `cat`, `test -f`, or `node -e`
  `existsSync`/`readFileSync` patterns.
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

`plan_run_readiness_score` is a separate pre-run predictor for whether a
StepPlan is likely to survive the `plan-run` handoff. It is not a replacement
for YAML quality and it must not use scenario id, suite name, hidden postcheck
oracle, execution success/failure, run id, or stderr text. It only uses the
user prompt, StepPlan, profile contract, declared expected paths, declared
verify commands, and deterministic verify policy.

Readiness is decomposed into:

- `verify_policy_readiness_score`: whether declared verify commands match the
  runtime verify policy mirrored from Rust `planner/verify.rs`.
- `declared_contract_completeness_score`: whether the StepPlan declares goal,
  expected result, expected paths, verify, and profile-level contract evidence.
- `runner_handoff_integrity_score`: diagnostic score from boolean
  `step_prompt_contract` events showing whether the runner passed the contract
  to step execution. It is blank for pure pre-run `step-plan` rows.
- `contract_handoff_score`: the declared contract score, capped by runner
  handoff integrity when runtime contract events are available.
- `postcheck_contract_alignment_score`: whether declared paths, verify, and
  finalization point at the same declared contract. It does not read hidden
  postcheck expected artifacts.
- `dependency_ordering_score`: whether manifest/artifact ownership precedes
  build/test verify in the abstract project contract.
- `finalization_readiness_score`: whether the plan has a declared completion
  contract rather than ending in a report-only step.

Post-run calibration remains separate:

- `plan_run_missed_predictive_signal`
- `missed_predictive_signal_reason`
- `readiness_false_positive_kind`
- `readiness_false_negative_kind`

Those fields explain where a high/low readiness score failed to predict a
runtime outcome. They are not fed back into the same run's readiness score.

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
- `build_verifier_completion_score`: whether a required build verifier reached
  a real terminal state. Passing build verification scores highest; dependency
  missing, policy rejection, blocked execution, and compile failure are kept as
  distinct lower-scored states. This score is derived from runtime events and
  is not read by the runtime.
- `dependency_setup_boundary_score`: whether dependency setup was correctly
  separated from build verification. A missing `node_modules/.bin/next` before
  a Next.js build is treated as an explicit setup boundary, not as a successful
  build or as an arbitrary implementation failure.
- `repair_target_resolution_score`: whether bounded repair has a structured
  target such as dependency setup, package config, framework config,
  implementation, or test/evidence. This is diagnostic; it does not make the
  runtime choose success.
- `repair_stagnation_score`: whether verifier repair progressed instead of
  repeating inspection-only or no-edit turns.
- `profile_static_vs_build_gap_score`: whether a static profile pass was backed
  by actual build verification when a build verifier was required. This helps
  catch false positives where files and profile shape look valid but the app
  was never built.
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

## Acceptance outcomes

The legacy `success` column remains the process/postcheck success flag for
backward compatibility. Acceptance is reported separately:

- `legacy_success`: copy of the legacy process/postcheck success result.
- `process_success`: whether the CLI process exited successfully.
- `artifact_success`: whether required artifacts exist.
- `build_success`: whether declared build postchecks passed.
- `launch_success`: whether declared dev server readiness passed.
- `source_semantic_success`: static semantic oracle result for the declared
  functional contract.
- `plan_output_adherence_success`: whether the final workspace satisfies the
  concrete capabilities that the saved YAML plan itself claimed it would build.
  This is evaluated only from generic plan terms such as canvas/render loop,
  keyboard control, enemies, bullets, collision/failure rules, score/progression,
  audio, or visual effects. It does not branch on scenario id.
- `plan_output_adherence_score`: percentage of plan-claimed capabilities found
  in the final source corpus.
- `plan_output_failure_kind`: failure category when the plan-output contract is
  not satisfied, usually `plan_output_missing_required_capabilities`.
- `plan_capability_contract_score`: whether the YAML plan turns the prompt and
  profile into an explicit capability contract with expected artifacts and
  verification evidence.
- `prompt_plan_capability_coverage_score`: percentage of prompt/profile-derived
  capabilities represented in the YAML plan. A low value means the plan itself
  is too weak even if the implementation follows it.
- `plan_verify_declared_coverage_score`: pre-run score based only on YAML
  verify declarations. This is used for `step-plan` and never reads generated
  files.
- `executed_verify_coverage_score`: post-run score that may inspect generated
  verify artifacts such as `smoke-check.js`, with workspace confinement, skip
  directories, read-size limits, and binary-file skips.
- `plan_verify_coverage_score`: display score for plan capability verification.
  It uses declared coverage for `step-plan` and executed coverage for runtime
  modes when available.
- `plan_verify_gap_kind`: generic reason for weak plan verification, such as
  `build_only_verify_for_behavior_contract`,
  `contentless_verify_for_capability_contract`, or
  `semantic_capability_unverified`.
- `verify_adequacy_cap_reason`: reason `verify_adequacy_score` was capped by
  weak plan verification or weak prompt-plan capability coverage.
- `acceptance_confidence_score`: confidence in `acceptance_success`. It is a
  diagnostic score, not an additional hard gate in the initial rollout.
- `acceptance_confidence_reason`: semicolon-delimited reasons confidence was
  capped, such as `plan_verify_coverage_below_40`,
  `prompt_plan_capability_coverage_below_70`, or
  `semantic_inconclusive_needs_behavior_oracle`.
- `behavior_success`: aggregate behavior oracle result. In smoke suites this is
  currently driven by the source semantic oracle; browser interaction is an
  explicit adapter for acceptance-required suites.
- `prompt_contract_success`: whether prompt-derived required capabilities are
  satisfied by deterministic oracles.
- `acceptance_success`: layered acceptance result. This can be false even when
  legacy `success` is true.
- `acceptance_false_positive`: true when legacy success passed but acceptance
  failed.
- `oracle_gap_kind`: generic reason category for eval-method false positives,
  such as `postcheck_too_weak_for_semantic_contract` or
  `postcheck_too_weak_for_plan_contract`.
- `acceptance_oracle_version`: deterministic oracle version used for the row.

The capability chain is reported as four separate relationships:

- prompt -> plan: `prompt_plan_capability_coverage_score`
- plan -> verify: `plan_verify_coverage_score`
- plan -> output: `plan_output_adherence_score`
- final acceptance: `acceptance_success` plus `acceptance_confidence_score`

The current hard gates remain deterministic process/artifact/build/launch/source
semantic/plan-output/postcheck checks. Prompt-plan coverage, plan-verify
coverage, and confidence are diagnostic until calibrated against false
positive/false negative rates. A static semantic inconclusive result should be
reported as an oracle-confidence problem or deterministic browser-oracle need,
not routed to human review.

Scenario contracts can declare `functional_contract`, `interaction_contract`,
`quality_contract`, and `oracle_contract`. If omitted, the harness conservatively
infers broad categories from prompt/profile text, for example
`interactive-game` from game prompts. Inference must not branch on scenario id.

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

`report.md` also includes `Plan Run Readiness` for the same paired rows. This
uses `plan_run_readiness_score` as the predictor and reports average subscores,
correlation, false positives, false negatives, readiness cap reasons, and
missed predictive signal reasons.

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

## Runtime Bridge Diagnostics

Phase 019 adds bridge diagnostics for cases where a plan is structurally good
but fails while moving through dependency setup, build verification, or repair.

- `dependency_setup_bridge_score`: whether runtime-owned dependency setup was
  explicitly authorized, bounded, and completed without mixing package managers.
- `build_verifier_lifecycle_score`: whether required build verification moved
  from missing verifier to setup to actual build execution and pass/fail.
- `profile_repair_symmetry_score`: whether profile verification failures remain
  repairable by the same profile auto-repair contract.
- `step_runtime_bridge_score`: whether plan-run step verification could bridge
  setup/build boundaries without weakening verify commands.
- `repair_target_followthrough_score`: whether repair edits touched files that
  match the classified repair target.
- `plan_run_success_predictor`: a capped diagnostic score that combines runtime
  friction, finalization, execution-contract adherence, setup bridge, verifier
  lifecycle, step bridge, and repair follow-through. It is not a replacement for
  acceptance success.

`verifier_bootstrap_state` appears in `completion_verify` events to distinguish
`dependency_setup_required`, `dependency_setup_blocked`,
`dependency_setup_failed`, `verifier_ready`, `verifier_passed`, and
`verifier_failed`. Network/setup failures should be analyzed as bridge/runtime
diagnostics, not as clean plan-quality failures.

## Phase 020 Acceptance And Speed Diagnostics

Use `speed-cloud-5x` only for cloud-only speed runs. It raises the provider lane
limit to 5 and keeps local LLM profiles out of the matrix.

```bash
python3 scripts/eval-run.py \
  --suite eval/suites/mvp-smoke.yaml \
  --model-profile speed-cloud-5x \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --runs 3 \
  --parallel 5 \
  --provider-limit 5 \
  --binary target/release/anvilminimal \
  --binary-kind anvilminimal
```

If provider HTTP/rate-limit failures increase, rerun the same matrix with
`--provider-limit 3` or `--provider-limit 4` and compare provider error rates.

For source `anvildev`, pass `--binary-kind anvildev`; do not pass
`--engine minimal` to `eval-run.py` itself. The harness adds `--engine minimal`
to the child command.

New summary/report fields separate process success from accepted artifact
success:

- `capability_acceptance_success`
- `acceptance_failure_reasons`
- `eval_schema_version`
- `provider_wait_sec`
- `wall_clock_sec`
- `acceptance_oracle_sec`
- `provider_limit`
- `parallel_limit`

Acceptance gates and predictor scores are intentionally separate. A predictor
score can explain risk, but it does not force acceptance success.
