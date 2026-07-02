# Step runner source parity matrix

## Scope

`anvildev` source minimal step runner と MVP `anvilminimal` の function-level parity を追跡する。

Baseline:

- MVP: `/private/tmp/anvilminimal-eval-009-mvp-step-plan-run`
- anvildev: `/private/tmp/anvilminimal-eval-009-anvildev-step-plan-run-v2`

## Matrix

| Source function / contract | MVP counterpart | Status before Phase 010 | Implementation phase | Required tests | Acceptance evidence |
| --- | --- | --- | --- | --- | --- |
| `src/agent/minimal_step_runner.rs::run_plan` step loop | `mvp/anvilminimal/src/planner/runner.rs::run_step_plan_with_ui` | implemented | Phase 1, 5, 7 | fake plan-run prompt history, step event test | `prompt_contract_score` 100.0 in both eval runs |
| `src/agent/minimal_step_runner.rs::build_step_prompt` | `planner/runner.rs::build_step_prompt` | implemented | Phase 1 | `step_execution_prompt_includes_source_contract`, `run_plan_passes_step_contract_to_execution_client` | prompt contains goal, step id, verify, expected_result, bounded repair policy |
| `src/agent/minimal_step_runner/repair.rs::build_repair_prompt` | `planner/repair.rs::build_repair_prompt_with_context` | implemented | Phase 2 | `repair_prompt_includes_source_contract` | repair prompt includes failure + step contract |
| `src/agent/minimal_step_runner/profile.rs::build_profiled_phase_prompt` | `planner/runner.rs::ultra_phase_prompt` | implemented | Phase 6 | `required_final_artifacts_are_preserved_in_ultra_phase_prompt` | ultra phase prompt includes snapshot/profile required files/verify |
| `src/agent/minimal_step_runner.rs` report semantics | `planner/runner.rs::plan_generation_system_prompt`, `planner/lint.rs::step_plan_quality_report` | implemented | Phase 3 | `planner_prompt_report_is_blocker_not_success`, `terminal_report_step_is_retryable_quality_for_implementation_task`, `blocker_report_step_is_allowed` | terminal report retry issue, blocker report allowed |
| `src/agent/minimal_step_runner.rs` inspect/report wrapper policy | `planner/lint.rs::step_plan_quality_report` | implemented | Phase 4 | `fresh_workspace_inspect_is_retryable_quality_issue`, `fresh_workspace_unknown_context_does_not_penalize_inspect` | context-aware fresh workspace gate |
| `src/agent/minimal_step_runner.rs` prior artifact context for verify | `planner/runner.rs::StepPromptContext` | implemented | Phase 5 | `step_execution_prompt_includes_source_contract`, runtime `prompt_contract_score` | prompt contract event has prior artifact context when applicable |
| `src/agent/minimal_step_runner.rs::StepRunSummary` / `StepOutcome` | eval events + summary columns | partial implemented | Phase 7 | `test_runtime_scoring.py`, `test_summary_schema.py` | prompt contract summary captured; full source StepOutcome remains intentionally not copied |
| `src/agent/minimal_step_runner.rs::validate_step_id` | `planner/lint.rs::lint_step_plan_report` | implemented | Phase 8 | `invalid_step_id_is_rejected`, `valid_step_id_examples_are_accepted` | invalid id rejected, valid ids accepted |
| `src/agent/minimal_step_runner.rs::validate_step_plan` length/text checks | `planner/lint.rs::lint_step_plan_report` | partial implemented | Phase 8 | `invalid_step_id_is_rejected`, existing contract tests | id/goal/instruction/placeholder checks added; deeper source parity deferred |
| `src/agent/minimal_step_runner/verify.rs` diagnostics | `planner/verify.rs`, `planner/repair.rs` | partial implemented | Phase 8 | `repair_prompt_includes_source_contract` | repair prompt includes command failures and step contract; richer diagnostic excerpt remains follow-up |

## Gap Mapping

| Gap | Matrix row |
| --- | --- |
| SR-GAP-01 | `build_step_prompt`, `run_plan` |
| SR-GAP-02 | `build_repair_prompt` |
| SR-GAP-03 | report semantics |
| SR-GAP-04 | `build_profiled_phase_prompt` |
| SR-GAP-05 | `StepRunSummary` / `StepOutcome` |
| SR-GAP-06 | `validate_step_id`, `validate_step_plan` |
| SR-GAP-07 | intentional difference: kind collapse remains deferred |
| SR-GAP-08 | inspect/report wrapper policy |
| SR-GAP-09 | verify diagnostics |
| SR-GAP-10 | prompt history / event tests |

## Status Update Rule

- Each Phase 1-8 must update the relevant row from `missing` or `partial` to `implemented`, `intentional-difference`, or `deferred`.
- `implemented` requires a test name and command.
- `intentional-difference` requires a rationale and a compensating test or eval gate.
- `deferred` requires a follow-up file and explicit risk.

## Phase 010 Eval Evidence

- Run 1: `/private/tmp/anvilminimal-eval-010-mvp-step-plan-run-1-net`
  - step-plan: 12/12 success
  - plan-run: 0/12 success
  - plan-run `prompt_contract_score`: 100.0
  - failures: `required_artifacts_missing` 5, `missing_tool_call` 5, `verify_command_policy_error` 1, `tool_validation_error` 1
- Run 2: `/private/tmp/anvilminimal-eval-010-mvp-step-plan-run-2-net`
  - step-plan: 12/12 success
  - plan-run: 0/12 success
  - plan-run `prompt_contract_score`: 100.0
  - failures: `missing_tool_call` 6, `required_artifacts_missing` 4, `verify_command_policy_error` 1, `planner_lint_error` 1

Interpretation:

- The Phase 010 source prompt contract miss is fixed and measured.
- Remaining plan-run failures are no longer explained by missing step prompt contract; they require a follow-up focused on execution model tool-use finalization and artifact completion behavior.
