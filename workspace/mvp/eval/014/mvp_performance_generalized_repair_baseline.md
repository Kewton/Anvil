# MVP Performance Generalized Repair Baseline

作成日: 2026-06-27

対象計画:

- `workspace/mvp/eval/014/mvp_performance_generalized_repair_plan.md`
- `workspace/mvp/eval/014/mvp_performance_generalized_repair_work_breakdown.md`

## Baseline Inputs

MVP:

- run root: `/private/tmp/anvilminimal-eval-013-live-mvp-rerun-net`
- report: `/private/tmp/anvilminimal-eval-013-live-mvp-rerun-net/report.md`
- summary: `/private/tmp/anvilminimal-eval-013-live-mvp-rerun-net/summary.eval.tsv`

anvildev:

- run root: `/private/tmp/anvilminimal-eval-013-live-anvildev-rerun-net`
- report: `/private/tmp/anvilminimal-eval-013-live-anvildev-rerun-net/report.md`
- summary: `/private/tmp/anvilminimal-eval-013-live-anvildev-rerun-net/summary.eval.tsv`

Comparison:

- `/private/tmp/anvilminimal-eval-013-live-rerun-compare.md`

Conditions:

- suite: `mvp-smoke.yaml`
- model profile: `speed-cloud`
- local LLM: unused
- modes: `step-plan,plan-run,ultra-plan-run`
- runs: 1

## Success Baseline

| binary | total | step-plan | plan-run | ultra-plan-run |
|---|---:|---:|---:|---:|
| MVP | 26/36 | 11/12 | 8/12 | 7/12 |
| anvildev | 22/36 | 8/12 | 9/12 | 5/12 |

Interpretation:

- MVP leads total success by +11.1pt.
- MVP leads step-plan and ultra-plan-run.
- anvildev is ahead by 1 case on plan-run.
- Provider/API failures remain included in raw success and must be shown separately from capability success.

## Score Baseline

| metric | anvildev | MVP | delta |
|---|---:|---:|---:|
| success_rate | 61.1 | 72.2 | +11.1 |
| valid_plan_generated_rate | 80.6 | 91.7 | +11.1 |
| plan_quality_score_avg | 68.0 | 85.0 | +17.0 |
| executable_plan_score_avg | 63.1 | 76.5 | +13.5 |
| plan_run_predictive_score_avg | 67.3 | 79.1 | +11.7 |
| execution_shape_readiness_score_avg | 84.6 | 71.2 | -13.4 |
| verify_strength_score_avg | 50.5 | 71.5 | +21.0 |
| artifact_ownership_score_avg | 31.1 | 97.8 | +66.7 |
| lint_repair_score_avg | 100.0 | 79.2 | -20.8 |
| overall_score_avg | 56.3 | 73.0 | +16.6 |

MVP strengths:

- YAML plan quality
- executable plan score
- verify strength
- artifact ownership
- overall score

MVP risks:

- execution shape readiness is lower than anvildev.
- lint repair score is lower, matching the observed planning-layer failures.

## MVP Failure Baseline

| failure kind | count | layer | representative mode |
|---|---:|---|---|
| `planner_schema_error` | 2 | planning | ultra-plan-run |
| `verify_command_policy_error` | 3 | planning | step-plan / plan-run / ultra-plan-run |
| `planner_lint_error` | 1 | planning | ultra-plan-run |
| `step_verify_failure` | 1 | bridge | plan-run |
| `max_iterations` | 1 | runtime | plan-run |
| `postcheck_failure` | 1 | postcheck | plan-run |
| `provider_http_status` | 1 | provider | ultra-plan-run |

Layer distribution:

| layer | count | improvement treatment |
|---|---:|---|
| planning | 6 | primary implementation target |
| bridge | 1 | step repair/finalization target |
| runtime | 1 | progress/finalization target |
| postcheck | 1 | bounded contract-derived repair or eval diagnosis |
| provider | 1 | classify separately; not capability improvement |

## Representative Failed Rows

| scenario | mode | main/planner | failure kind | layer | key metric signal |
|---|---|---|---|---|---|
| `docs-heading-update-small` | step-plan | gemini/openai | `verify_command_policy_error` | planning | invalid verify command, no valid plan |
| `docs-heading-update-small` | plan-run | gemini/openai | `verify_command_policy_error` | planning | same planner failure propagates |
| `fix-js-date-helper-small` | plan-run | gemini/openai | `step_verify_failure` | bridge | finalization 50.0, step finalization 20.0 |
| `rust-cli-medium` | plan-run | openai/gemini | `max_iterations` | runtime | high plan score but runtime finalization stall |
| `nextjs-space-invaders-large` | plan-run | openai/gemini | `postcheck_failure` | postcheck | postcheck_stability 25.0, ECA cap 55.0 |
| `fix-js-date-helper-small` | ultra-plan-run | openai/gemini | `planner_schema_error` | planning | phase_completion 13.3 |
| `nextjs-space-invaders-large` | ultra-plan-run | openai/gemini | `planner_lint_error` | planning | phase_completion 46.7 |
| `nextjs-space-invaders-large` | ultra-plan-run | gemini/openai | `verify_command_policy_error` | planning | phase_completion 46.7 |
| `repair-exhausted-report-large` | ultra-plan-run | openai/gemini | `planner_schema_error` | planning | phase_completion 13.3 |
| `docs-heading-update-small` | ultra-plan-run | openai/gemini | `provider_http_status` | provider | raw provider HTTP 400 |

## Target Runtime Metric Baseline

| mode | metric | success avg | failure avg | interpretation |
|---|---|---:|---:|---|
| plan-run | `postcheck_stability_score` | 96.9 | 25.0 | strong postcheck failure separator |
| plan-run | `runtime_friction_score` | 65.2 | 37.0 | useful but not sufficient alone |
| plan-run | `finalization_score` | 87.5 | 46.9 | useful for missing finalization |
| plan-run | `execution_contract_adherence_score` | 97.8 | 76.7 | bridge signal, not complete runtime predictor |
| ultra-plan-run | `phase_completion_score` | 100.0 | 30.0 | strongest ultra failure separator |
| ultra-plan-run | `finalization_score` | 87.5 | 54.2 | phase closure signal |
| ultra-plan-run | `execution_contract_adherence_score` | 100.0 | 91.2 | not useful for ultra planning failures alone |

## Impact Matrix

| route | entry point | planner/execution split | StepPlan validation | completion contract scope | regression guard |
|---|---|---|---|---|---|
| CLI `--plan-steps` | `lib.rs::Action::PlanSteps` | planner only | `generate_step_plan_with_ui` | no execution contract | valid plan no-op repair |
| CLI `--plan-run` | `lib.rs::Action::PlanRun` | planner + execution | generation + run lint | step contract + plan final contract | step prompt/repair context |
| CLI `--run-plan` | `lib.rs::Action::RunPlan` | execution only | run-file lint | plan final contract | invalid file rejects |
| CLI `--ultra-plan` | `lib.rs::Action::UltraPlan` | planner only | ultra lint or deterministic fallback | no execution contract | valid ultra lint |
| CLI `--ultra-plan-run` | `lib.rs::Action::UltraPlanRun` | planner + execution | ultra lint + phase step plan validation | phase execution + profile contract | phase stage events |
| CLI `--run-ultra-plan` | `lib.rs::Action::RunUltraPlan` | planner + execution | ultra file lint + phase validation | phase execution + profile contract | invalid phase stops early |
| TUI `/plan-steps` | `tui/slash.rs::handle_command` | planner only | same planner route | no execution contract | slash parse and output path |
| TUI `/plan-run` | `tui/slash.rs::handle_command` | planner + execution | same plan-run route | same plan-run route | no silent exit |
| TUI `/ultra-plan-run` | `tui/slash.rs::handle_command` | planner + execution | same ultra route | same ultra route | visible phase progress |
| minimal-loop `--prompt` | `lib.rs::Action::Prompt` | execution only | not StepPlan based | completion contract from config/prompt | no plan-run contract leakage |
| eval `anvilminimal` | `scripts/eval-run.py` | selected by binary kind | mode-specific | eval-generated completion contract | raw/provider-excluded split |
| eval `anvildev` | `scripts/eval-run.py` | `--binary-kind anvildev` | source binary route | suite postcheck outside child | comparison remains readable |

## Negative Controls

These must not regress:

- A valid StepPlan passes without extra repair events.
- Invalid StepPlan does not execute unless it becomes valid after repair.
- `minimal-loop --prompt` does not receive plan-run-only step contract.
- Verify policy is not weakened to make bad plans pass.
- Postcheck oracle from eval suite is not hard-coded into normal runtime.
- Provider failure is not counted as planner/runtime/postcheck capability failure.
- Existing TUI slash route continues to call the same planner runner as CLI.

## Reproduction Commands

Report generation:

```bash
python3 mvp/anvilminimal/scripts/eval-report.py \
  --run-root /private/tmp/anvilminimal-eval-013-live-mvp-rerun-net
```

```bash
python3 mvp/anvilminimal/scripts/eval-report.py \
  --run-root /private/tmp/anvilminimal-eval-013-live-anvildev-rerun-net
```

Comparison:

```bash
python3 mvp/anvilminimal/scripts/eval-compare.py \
  --baseline /private/tmp/anvilminimal-eval-013-live-anvildev-rerun-net/summary.eval.tsv \
  --experiment /private/tmp/anvilminimal-eval-013-live-mvp-rerun-net/summary.eval.tsv \
  --out /private/tmp/anvilminimal-eval-014-baseline-compare.md
```

## Phase 0 Acceptance

- Baseline run roots exist.
- Raw success and provider-excluded/capability success are distinguished in the plan.
- Failure kind/layer distribution is fixed before implementation.
- Impact matrix covers CLI, TUI, eval, and anvildev comparison paths.
- Negative controls are explicitly listed.
