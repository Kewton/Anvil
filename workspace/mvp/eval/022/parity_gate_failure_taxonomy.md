# Parity Gate Failure Taxonomy

作成日: 2026-06-29

## 1. 目的

`failure_kind` 空欄を gate failure にする。これは成功率を下げるためではなく、失敗を planning / runtime / bridge / provider / acceptance のどこに帰属させるかを機械的に追えるようにするためである。

## 2. Blank Failure Kind Rule

以下の row は `failure_kind` を必須にする。

- `rc != 0`
- `process_success == false`
- `acceptance_success == false`
- `postcheck_success == false`
- `phase_completion_score` が未達で mode が `ultra-plan-run`
- `finalization_score` が未達で expected artifacts が不足

対象外:

- success row
- dry-run row
- diagnostic skipped row
- explicit user abort / cancel
- API key missing による provider probe skip

## 3. Current Known Blank Counts

| Run | Blank failure kind count |
| --- | ---: |
| `/private/tmp/anvilminimal-eval-0219-mvp-smoke/summary.eval.tsv` | 10 |
| `/private/tmp/anvilminimal-eval-0219-provider-smoke/summary.eval.tsv` | 5 |
| total current known | 15 |

この 15 件は G-S14 の gate failure として扱う。

## 4. Lifecycle Stage Mapping

| failure_kind | failure_layer | lifecycle_stage | Typical evidence |
| --- | --- | --- | --- |
| `planner_schema_error` | planning | `plan_generated` | missing goal, invalid YAML, invalid UltraPlan phase |
| `planner_verify_command_policy_error` | planning | `plan_linted` | verify command uses shell control syntax |
| `planner_lint_error` | planning | `plan_linted` | duplicate expected path ownership, missing expected paths |
| `phase_scaffold_error` | bridge | `phase_started` / `plan_generated` | ultra phase scaffold missing id/prompt or invalid fallback |
| `provider_http_status` | provider | `plan_generated` / `tool_requested` | provider 4xx/5xx |
| `provider_tool_args_shape_error` | provider | `tool_requested` | function_call.arguments string/object mismatch, malformed args |
| `provider_function_call_schema_error` | provider | `tool_requested` | Gemini/OpenAI function schema mismatch |
| `tool_validation_error` | runtime | `tool_requested` | invalid path, invalid command, workspace confinement |
| `tool_execution_error` | runtime | `tool_executed` | command failed, file operation failed |
| `dependency_setup_missing` | bridge | `dependency_boundary_checked` | build/test verify before manifest/setup |
| `dependency_setup_blocked` | bridge | `dependency_boundary_checked` | install authority missing |
| `dependency_build_rerun_failed` | runtime | `dependency_setup_attempted` / `verify_started` | setup succeeded but build still fails |
| `verify_command_policy_error` | planning | `plan_linted` / `verify_started` | unsafe verify command |
| `step_verify_failure` | runtime | `verify_failed` | deterministic verify failed |
| `step_verify_repair_no_change` | bridge | `repair_attempted` | repair turn wrote/edited nothing relevant |
| `repair_target_not_followed` | bridge | `repair_attempted` | target classified but changed unrelated artifact |
| `bounded_repair_exhausted` | bridge | `repair_exhausted` | max repair attempts reached |
| `recovery_handoff_missing` | bridge | `recovery_handoff_saved` | repair exhausted without prompt/suggested command |
| `plan_final_contract_failure` | acceptance | `acceptance_failed` | expected artifact/capability/postcheck missing |
| `capability_evidence_missing` | acceptance | `acceptance_failed` | build passed but required capability absent |
| `source_semantic_acceptance_failure` | acceptance | `acceptance_failed` | static shell/title-only does not satisfy domain task |
| `postcheck_failure` | acceptance | `acceptance_failed` | suite postcheck failed |
| `max_iterations` | runtime | `repair_exhausted` / `acceptance_failed` | minimal loop reached cap without accepted output |

## 5. Summary Transfer Rule

If `events.jsonl` contains a concrete event such as `step_verify_failure`, `profile_repair_failed`, `recovery_prompt_saved`, or `final_acceptance_failure`, the eval summary must not collapse it to a blank `failure_kind`.

Priority:

1. Explicit provider error kind
2. Planner schema/lint/policy error
3. Tool validation/execution error
4. Dependency lifecycle error
5. Verify/repair lifecycle error
6. Final acceptance/postcheck error
7. Max iteration/finalization error
8. `unclassified_process_failure` only when all above are absent and raw stderr is attached

## 6. Gate Acceptance

- Latest smoke failure blank count must become 0 before G-S14 can pass.
- `unclassified_process_failure` is allowed only if raw stderr/events are preserved and no known pattern matches.
- `unclassified_process_failure` count growth is a gate warning even when blank count is 0.
