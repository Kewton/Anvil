# UAT004 Source/MVP Trace Manifest

作成日: 2026-07-02

## 目的

UAT004-GATE-00 の trace baseline として、source/MVP trace、fixture、skip evidence、次回確認タイミングを追記できる manifest を固定する。

GATE-00 では実装変更、provider probe、browser run、anvildev 比較を実行しない。実行しない比較は、skip reason、影響、次アクションをこの manifest に記録する。

## Applied Baseline Inputs

| kind | path |
| --- | --- |
| UAT events | `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0701_005/.anvil/runs/019f1e0b-3acd-7fd1-97dd-c10e03ba37eb/events.jsonl` |
| UAT summary | `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0701_005/.anvil/runs/019f1e0b-3acd-7fd1-97dd-c10e03ba37eb/summary.md` |
| UltraPlan | `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0701_005/.anvil/plans/ultra-plan-019f1e0b-9468-73b1-aa65-67d27ea5bc7b.yaml` |
| Recovery UltraPlan | `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0701_005/.anvil/plans/recovery-ultra-plan-phase-build-and-verify-019f1e11-5644-7e02-968c-8823ddff460b.yaml` |
| Generated app source | `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0701_005/src/app/page.tsx` |
| Generated CSS | `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0701_005/src/app/globals.css` |

## Baseline MVP Trace: test0701_005

| field | value |
| --- | --- |
| run id | `019f1e0b-3acd-7fd1-97dd-c10e03ba37eb` |
| event count | 249 lines |
| command | `/ultra-plan-run --profile nextjs ...` |
| profile | `nextjs` |
| provider / model | Gemini planner and runtime models recorded in `run_start` |
| UltraPlan phases | `setup-and-base-layout`, `game-engine-core`, `game-polish-and-effects`, `ui-and-responsiveness`, `build-and-verify` |
| completed phases | first 4 phases |
| failed phase | `build-and-verify` |
| failure kind | `phase_scaffold_error` |
| failure message | `invalid StepPlan after corrective retries: Next.js build verify requires an entrypoint expected path first` |
| recovery artifacts | prompt and recovery UltraPlan YAML saved; parse checks succeeded |
| TUI command stop | `ok=false`, `failure_kind=tui_command_failed` |
| process run stop | `ok=true`, `failure_kind=""`, `stop_reason=completed` |
| summary contradiction | top-level `Status: incomplete`; tail `Status: complete`, `Action: Repl`, `Stop reason: completed` |

## Normalized Lifecycle Baseline

| lifecycle stage | observed event / artifact | status | correct failure detection or regression |
| --- | --- | --- | --- |
| run start | `run_start`, `tui_command_start` | observed | baseline |
| UltraPlan generation | `ultra_plan_generation_succeeded`, phase count 5 | observed | correct progress |
| phase context | `ultra_context_initialized`, `ultra_phase_context_attached`, `ultra_phase_context_updated` | observed | requires source trace comparison |
| step prompt contract | `step_prompt_contract` contains overall goal, expected paths, required capabilities/evidence | observed | correct diagnostic surface, not yet completion authority |
| step obligation scope | every sampled plan-run step has `completion_contract_verification_enabled=false` and `completion_contract_path_merge_enabled=false` | observed | regression / source parity gap |
| static capability evidence | `step_capability_evidence_check` starts as fail, later becomes pass without browser/interaction evidence paths | observed | partial diagnostic, possible false positive |
| dependency/build lifecycle | `dependency_build_lifecycle` observed in completed phases | observed | requires G-S09 source comparison |
| final phase scaffold | planner errors culminate in `ultra_phase_failed` at scaffold | observed | correct failure detection for invalid StepPlan |
| recovery handoff | `recovery_prompt_saved`, `ultra_partial_artifact_summary` with YAML and prompt parse OK | observed | correct handoff evidence; not success |
| TUI command stop | `tui_command_stop ok=false` | observed | correct command failure |
| outer run stop | `run_stop ok=true`, blank failure kind | observed | regression for task/process status projection |

## Trace / Evidence Manifest By Gate

| gate | classification | MVP baseline trace | source trace status | provider/browser/anvildev evidence | skip reason in GATE-00 | next confirmation timing | rollback condition |
| --- | --- | --- | --- | --- | --- | --- | --- |
| G-S01 | `full_source_port_target` | `step_prompt_contract`, `step_obligation_scope`, generated `page.tsx` shallow implementation | pending | not required for GATE-00 | baseline fixture fixed only; no implementation yet | UAT004-GATE-01 after TaskContract restoration | do not allow interactive app/game path-only or static-evidence completion |
| G-S02 | pass re-audit / `full_source_port_target` | `completion_contract_verification_enabled=false` in plan-run steps | pending | not required for GATE-00 | existing pass is provisional | UAT004-GATE-01 source parity or intentional-difference evidence | do not regress non-interactive required path completion |
| G-S03 | pass re-audit | planner schema/lint retry events and final scaffold errors | pending | provider probe pending | no live provider/API probe in baseline doc phase | UAT004-GATE-02 planner source parity audit | no blank planner failure kind; no deterministic fallback success false positive |
| G-S04 | `full_source_port_target` | UltraPlan created 5 phases; final phase scaffold failed after retries | pending | anvildev comparison pending | no same-condition source run in GATE-00 | UAT004-GATE-02 | do not treat invalid final verify plan as success |
| G-S05 | `source_trace_verification_target` | phase context attach/update events observed | pending | anvildev comparison pending | trace comparison not executed in baseline doc phase | UAT004-GATE-06 same-condition trace | no loss of prior phase failure/repair target context |
| G-S06 | `source_trace_verification_target` | step prompt contract fields observed | pending | anvildev comparison pending | trace comparison not executed in baseline doc phase | UAT004-GATE-06 | no step prompt missing goal, verify, expected result, paths |
| G-S07 | `source_trace_verification_target` | no targeted tool args/provider probe in test0701_005 baseline | pending | provider probe pending | API/network not required for GATE-00 | UAT004-GATE-07 | do not relax unsafe args, hidden metadata, workspace escape rejection |
| G-S08 | pass re-audit / `full_source_port_target` | final verify scaffold failure and verify policy errors observed | pending | local targeted eval pending | GATE-00 does not run cargo/pytest/eval | UAT004-GATE-03 | shell control rejection and dependency ordering must remain strict |
| G-S09 | `full_source_port_target` | dependency/build lifecycle events observed; final verify dependency order failure observed | pending | anvildev comparison pending | no implementation or source run in GATE-00 | UAT004-GATE-03 | dependency/setup/build cannot be helper-only or build-only success |
| G-S10 | `full_source_port_target` | final failure writes recovery handoff; no RepairJob parity trace | pending | anvildev comparison pending | RepairJob not executed under source in GATE-00 | UAT004-GATE-04 | no no-change or target-misdirected repair success |
| G-S11 | `full_source_port_target` | generated project scaffold exists; shallow interactive implementation remains | pending | release/browser evidence pending | browser and manual UAT not rerun in GATE-00 | UAT004-GATE-04 | scaffold-only output cannot be final completion |
| G-S12 | `full_source_port_target` | browser/interaction evidence paths are blank in step checks; generated app lacks robust interaction | pending | browser readiness/interaction pending | browser run not executed in GATE-00 | UAT004-GATE-05 | full release pass requires artifact/build/capability/browser/interaction evidence |
| G-S13 | `full_source_port_target` | recovery prompt/YAML saved and parse OK; recovery run not executed | pending | anvildev comparison pending | recovery execution not part of baseline fixture | UAT004-GATE-04 | saved handoff alone is never task success |
| G-S14 | pass re-audit / `source_trace_verification_target` | failure kind present for command failure, blank in outer `run_stop` | pending | normalized source diagnostics pending | source trace not collected in GATE-00 | UAT004-GATE-08 | no blank failure kind for failed lifecycle stages |
| G-S15 | `source_trace_verification_target` | no live provider behavior probe in fixture | pending | provider probe pending | API key/network not required for baseline docs | UAT004-GATE-07 | fake fixture alone cannot close prompt/provider changes |
| G-S16 | `source_trace_verification_target` | `summary.md`, `tui_command_stop`, `run_stop` show contradiction | pending | manual TUI UAT pending | manual rerun not executed in GATE-00 | UAT004-GATE-08 | no silent exit and no task failure overwritten by REPL ready |

## UAT004-GATE-01 Trace Update

| item | status | evidence |
| --- | --- | --- |
| source refs inspected | complete | `src/agent/loop_run/task_contract.rs`, `task_contract_core.rs`, `task_contract_completion_policy.rs`, `task_contract_request_inference.rs`, `task_contract_artifact_contract.rs`, `task_contract_deliverable_lifecycle.rs`, `evidence_binding.rs`, `artifact_ledger.rs`, `artifact_ownership.rs`, `artifact_target_alignment.rs`, `worker_contract.rs` |
| MVP refs changed | complete | `mvp/anvilminimal/src/minimal_loop/evidence.rs`, `mvp/anvilminimal/src/minimal_loop/loop_run.rs`, `mvp/anvilminimal/src/planner/runner.rs` |
| G-S01 trace evidence | source-equivalent local fixture | `artifact_only_verify_does_not_satisfy_source_first_implementation_contract`, existing title/docs/style/scaffold/canvas/browser acceptance tests, and step-level `completion_verify` now reporting `capability_evidence_bindings` / `obligation_repair_targets` |
| G-S02 trace evidence | source-equivalent local fixture | `implement_plan_step_keeps_completion_contract_authority_enabled`, `plan_step_non_interactive_completion_preserves_path_only_success`, `plan_run_external_completion_contract_checks_required_evidence`, `plan_run_nextjs_game_scaffold_only_fails_inferred_capabilities`, `plan_run_nextjs_game_docs_only_fails_inferred_capabilities` |
| required local tests | passed | `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`: passed; `pytest mvp/anvilminimal/tests/eval`: 222 passed, 1 skipped |
| source/anvildev same-condition run | skipped | This gate restored deterministic TaskContract/completion semantics using source refs and local fixtures. Same-condition source/anvildev runtime comparison remains deferred to UAT004-GATE-09 because provider/browser/manual execution is not a normal unit-test requirement. |
| provider/browser evidence | skipped | No live provider or browser readiness/interaction run was required for GATE-01; G-S12/G-S15/G-S16 remain covered by later gates. |

## UAT004-GATE-02 Trace Update

| item | status | evidence |
| --- | --- | --- |
| source refs inspected | complete | `src/agent/minimal_step_runner.rs`, `src/agent/minimal_step_runner/profile.rs`, `src/agent/minimal_step_runner/plan_lint.rs`, `src/agent/minimal_step_runner/verify.rs`, `src/agent/minimal_step_runner/repair.rs`, `src/agent/minimal_step_runner/profiles/nextjs.rs` |
| MVP refs changed | complete | `mvp/anvilminimal/src/planner/lint.rs`, `mvp/anvilminimal/src/planner/runner.rs` |
| G-S03 re-audit evidence | source-equivalent local fixture | Existing MVP generator already rejects tool calls, retries schema/lint failures, emits concrete `planner_error_kind`, and fails invalid UltraPlan after retry exhaustion without saving a plan. `invalid_ultra_plan_generation_does_not_save_plan_file` now also asserts `ultra_plan_generation_failed` with `planner_schema_error` and no success event. |
| G-S04 final verify fixture | source-equivalent local fixture | `workspace_manifest_and_entrypoint_allow_final_nextjs_verify`, `workspace_manifest_without_entrypoint_still_rejects_nextjs_build`, and `generated_final_verify_uses_existing_workspace_nextjs_artifacts` prove final build verify can use existing workspace manifest+entrypoint while package-only/no-entrypoint still fails with concrete dependency-order lint. |
| deterministic fallback boundary | preserved | `deterministic_profile_fallback_requires_targeted_continuation_before_success` continues to require targeted implementation continuation after deterministic scaffold recovery and records `used_for_completion=false`. |
| required local tests | passed | `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`: passed; `pytest mvp/anvilminimal/tests/eval`: 222 passed, 1 skipped |
| provider probe | skipped | GATE-02 did not change provider request schema, provider parser, tool-call shape, or prompt text. The change is deterministic lint/generation validation. Provider probe remains deferred to UAT004-GATE-07/GATE-09 if prompt/provider-sensitive behavior changes. |
| anvildev same-condition run | skipped | Local source refs plus deterministic fixtures cover the scoped planner lint/fail-fast semantics. Comparative anvildev/MVP run remains a final UAT004-GATE-09 responsibility. |
| browser/manual evidence | skipped | GATE-02 does not close final browser readiness or interaction acceptance. Those remain G-S12/G-S16 work in GATE-05/GATE-08. |

## Skip Evidence

| skipped item | reason | impact | next action |
| --- | --- | --- | --- |
| source/anvildev same-condition run | GATE-00 is a baseline documentation phase; no runtime comparison requested | source parity remains pending for all gates | collect source/MVP normalized trace in the target gate before marking pass |
| provider live probe | API key/network must not be normal unit-test requirement | G-S07/G-S15 remain fail/pending | run provider probe only in UAT004-GATE-07 or opt-in release gate |
| browser readiness / interaction run | browser/dev-server must not be required for normal unit tests | G-S12/G-S16 remain pending for release-grade pass | run browser readiness and interaction evidence in UAT004-GATE-05 or final gate |
| `cargo test` / `pytest` / targeted eval | no code changes in GATE-00; objective is inventory and fixture fixation | no implementation correctness claim is made | run targeted tests in each implementation gate |
| recovery UltraPlan execution | baseline fixture records that recovery YAML exists; execution is a later lifecycle gate | G-S13 remains fail | execute recovery run in UAT004-GATE-04 or final comparative gate |

## Required Trace Fields For Future Rows

Each later gate update must append enough data to answer:

- source refs read and MVP refs changed.
- source trace path and MVP trace path, or explicit skip evidence.
- normalized lifecycle stages compared.
- positive fixture and negative fixture result.
- provider/browser/manual UAT evidence path when relevant.
- rollback condition and whether the change would reintroduce a known false positive.
- whether success-rate movement is correct failure detection or runtime regression.
