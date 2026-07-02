# UAT004 Source Runtime Module Inventory

作成日: 2026-07-02

## 目的

UAT004-GATE-00 の baseline として、source-first runtime semantics 復元で参照する source runtime module を固定する。

この inventory は実装修正ではなく、以降の gate で「読んだ source」「対応する MVP 実装」「移植判断」「未確認 trace」を追跡するための台帳である。

## 適用した共通条件

- 共通参照ファイル:
  - `workspace/mvp/uat/004/README.md`
  - `workspace/mvp/uat/004/source_first_runtime_semantics_gate_reclassification.md`
  - `workspace/mvp/uat/004/source_first_runtime_semantics_gate_implementation_breakdown.md`
  - `workspace/mvp/uat/004/test0701_005_source_first_repair_plan.md`
  - `workspace/mvp/uat/004/test0701_005_root_cause_countermeasures.md`
  - `workspace/mvp/uat/004/test0701_005_uat_current_state_issues.md`
  - `workspace/mvp/eval/022/runtime_semantics_gate_matrix.md`
  - `workspace/mvp/recovery/002/migration_gap_inventory.md`
- 共通制約:
  - 無制限 repair loop / auto replan は入れない。
  - workspace confinement と shell control syntax rejection は緩めない。
  - recovery handoff 保存だけを success と扱わない。
  - Space Invaders 固有文字列ではなく generic interactive app/game capability で扱う。
  - API key / network / browser を通常 unit test 必須にしない。
  - source-first 対象を MVP lite API への再解釈だけで完了扱いしない。

## Inventory Status

| item | status |
| --- | --- |
| source minimum target files | all present |
| line count / symbol scan | recorded via `wc -l` and `rg` |
| source semantics port status | not ported in GATE-00; classified for later gates |
| MVP counterpart scan | recorded as module groups |
| source trace collection | not executed in GATE-00; trace manifest records skip reason and next timing |
| provider/browser/anvildev comparison | not executed in GATE-00; skip evidence is in `source_mvp_trace_manifest.md` |

## Source Minimum Target Checklist

| group | source file | lines | role in source runtime semantics | gates | MVP refs to inspect in later gates | GATE-00 decision |
| --- | --- | ---: | --- | --- | --- | --- |
| TaskContract | `src/agent/loop_run/task_contract.rs` | 8000 | task kind, artifact roles, completion decision, evidence authority | G-S01, G-S02, G-S12 | `mvp/anvilminimal/src/minimal_loop/completion.rs`, `mvp/anvilminimal/src/minimal_loop/evidence.rs`, `mvp/anvilminimal/src/planner/runner.rs` | `full_source_port_target` |
| TaskContract | `src/agent/loop_run/task_contract_core.rs` | 187 | core contract data model and lifecycle inputs | G-S01, G-S02 | same as above | `full_source_port_target` |
| TaskContract | `src/agent/loop_run/task_contract_completion_policy.rs` | 228 | completion policy and evidence sufficiency | G-S01, G-S02, G-S12 | `completion.rs`, `evidence.rs`, `runner.rs` | `full_source_port_target` |
| TaskContract | `src/agent/loop_run/task_contract_request_inference.rs` | 813 | request inference, artifact/capability extraction | G-S01 | `planner/step_plan.rs`, `planner/runner.rs`, `minimal_loop/evidence.rs` | `full_source_port_target` |
| TaskContract | `src/agent/loop_run/task_contract_recovery.rs` | 1094 | contract-aligned recovery action selection | G-S10, G-S13 | `minimal_loop/repair_target.rs`, `planner/repair.rs`, `planner/runner.rs` | `full_source_port_target` |
| Repair | `src/agent/loop_run/repair_job.rs` | 9860 | repair job state, verifier response, rerun target, exhausted handoff | G-S10, G-S13 | `minimal_loop/repair_target.rs`, `planner/repair.rs`, `planner/runner.rs` | `full_source_port_target` |
| Repair | `src/agent/loop_run/repair_lifecycle.rs` | 290 | bounded repair lifecycle and repeated attempt detection | G-S10, G-S13 | `planner/repair.rs`, `minimal_loop/repair_progress.rs` | `full_source_port_target` |
| Repair | `src/agent/loop_run/repair_job_dispatch.rs` | 727 | dispatch after verifier pass/fail/no verifier/safe stop | G-S10, G-S13 | `planner/runner.rs`, `minimal_loop/repair_target.rs` | `full_source_port_target` |
| Actor loop | `src/agent/loop_run/actor_loop_flow.rs` | 5884 | controller lifecycle from model/tool turns to recovery and completion | G-S01, G-S05, G-S10, G-S12, G-S16 | `minimal_loop/loop_run.rs`, `planner/runner.rs`, `lib.rs`, `tui/slash.rs` | `full_source_port_target` where completion/repair authority is missing |
| Verifier | `src/agent/loop_run/verifier.rs` | 2730 | verifier assessment, artifact diagnostics, obligation evidence | G-S08, G-S12, G-S14 | `planner/verify.rs`, `minimal_loop/evidence.rs`, `planner/runner.rs` | `full_source_port_target` / trace parity required |
| Verifier | `src/agent/loop_run/verifier_driver.rs` | 794 | verifier command execution and timeout/dependency classification | G-S08, G-S09, G-S12 | `planner/verify.rs`, `minimal_loop/build_verifier.rs` | `full_source_port_target` |
| Verifier | `src/agent/loop_run/verifier_repair_targeting.rs` | 1123 | maps verifier failures to repair targets | G-S10 | `minimal_loop/repair_target.rs`, `planner/repair.rs` | `full_source_port_target` |
| Verifier | `src/agent/loop_run/verifier_command_policy.rs` | 171 | allowed verifier commands and policy boundary | G-S08, G-S09 | `planner/verify.rs`, `planner/lint.rs` | `full_source_port_target` / pass re-audit |
| Scaffold | `src/agent/loop_run/scaffold_pipeline.rs` | 2584 | scaffold as continuation target, not completion | G-S11 | `planner/profiles/nextjs.rs`, `minimal_loop/completion.rs`, `planner/runner.rs` | `full_source_port_target` |
| Project probe | `src/agent/loop_run/project_probe.rs` | 1109 | workspace facts, project unit, verifier candidates | G-S09, G-S12 | `minimal_loop/build_verifier.rs`, `minimal_loop/dependency_setup.rs`, `planner/runner.rs` | `full_source_port_target` |
| Project verifier | `src/agent/loop_run/project_verifier.rs` | 937 | syntax/project verification with safe temp execution | G-S08, G-S09 | `planner/verify.rs`, `minimal_loop/build_verifier.rs` | `full_source_port_target` |
| Artifact ledger | `src/agent/loop_run/artifact_ledger.rs` | 2133 | artifact event admission, ownership and path evidence | G-S01, G-S02, G-S12 | `minimal_loop/evidence.rs`, `planner/runner.rs` | `full_source_port_target` |
| Artifact ledger | `src/agent/loop_run/artifact_ownership.rs` | 1284 | path ownership, ignored dirs, confinement checks | G-S01, G-S02, G-S12 | `tools/path_guard.rs`, `minimal_loop/evidence.rs` | `full_source_port_target`; confinement must remain strict |
| Artifact ledger | `src/agent/loop_run/artifact_target_alignment.rs` | 155 | target alignment for produced artifacts | G-S01, G-S02, G-S12 | `minimal_loop/evidence.rs`, `planner/runner.rs` | `full_source_port_target` |
| Recovery | `src/agent/loop_run/no_progress_recovery.rs` | 706 | no-progress safe recovery escalation | G-S10, G-S13 | `minimal_loop/repair_progress.rs`, `planner/runner.rs` | `full_source_port_target` where bounded recovery is missing |
| Safe stop | `src/agent/loop_run/safe_stop_emit.rs` | 433 | safe stop event/report emission | G-S13, G-S16 | `eval_events.rs`, `lib.rs`, `tui/slash.rs` | `source_trace_verification_target` |
| Safe stop | `src/agent/loop_run/safe_stop_payload.rs` | 292 | bounded safe stop payload and next action classification | G-S13, G-S16 | `eval_events.rs`, `planner/runner.rs` | `source_trace_verification_target` |
| Node lifecycle | `src/agent/loop_run/node_request_helpers.rs` | 317 | Node stack request and artifact inference | G-S09 | `minimal_loop/dependency_setup.rs`, `planner/profiles/nextjs.rs` | `full_source_port_target` |
| Node lifecycle | `src/agent/loop_run/node_runner_manifest.rs` | 217 | Node test runner manifest binding | G-S09 | `minimal_loop/dependency_setup.rs`, `minimal_loop/build_verifier.rs` | `full_source_port_target` |
| Node lifecycle | `src/agent/loop_run/node_test_evidence_quality.rs` | 288 | quality of Node test evidence and self-reference detection | G-S08, G-S09 | `minimal_loop/evidence.rs`, `planner/verify.rs` | `full_source_port_target` / pass re-audit |
| Diagnostics | `src/agent/loop_run/summary.rs` | 931 | source run summary and status projection | G-S14, G-S16 | `eval_events.rs`, `lib.rs`, `tui/slash.rs`, `planner/runner.rs` | `source_trace_verification_target` |
| Diagnostics | `src/agent/loop_run/emit_verifier_events.rs` | 103 | verifier event emission | G-S14 | `eval_events.rs`, `runtime_trace.py`, `failure_classification.py` | `source_trace_verification_target` / pass re-audit |
| Worker contract | `src/agent/loop_run/worker_contract.rs` | 2931 | lifecycle stage worker contract, capability defaults, output contract | G-S01, G-S05, G-S06, G-S12 | `planner/runner.rs`, `minimal_loop/prompt.rs`, `minimal_loop/completion.rs` | `full_source_port_target` or documented alternative authority |
| Minimal runner | `src/agent/minimal_step_runner.rs` | 2950 | source StepPlan/UltraPlan generation, run loop, prompt, validation | G-S03, G-S04, G-S05, G-S06 | `planner/runner.rs`, `planner/step_plan.rs`, `planner/ultra_plan.rs` | `full_source_port_target` / pass re-audit |
| Minimal runner | `src/agent/minimal_step_runner/profile.rs` | 147 | source profile runtime contract and snapshot | G-S03, G-S04, G-S09 | `planner/profile.rs`, `planner/profiles/nextjs.rs` | `full_source_port_target` |
| Minimal runner | `src/agent/minimal_step_runner/plan_lint.rs` | 252 | source StepPlan lint, dependency/setup/build ordering | G-S03, G-S04, G-S08 | `planner/lint.rs`, `planner/verify.rs` | `full_source_port_target` / pass re-audit |
| Minimal runner | `src/agent/minimal_step_runner/verify.rs` | 392 | source verify execution policy and diagnostics | G-S08, G-S09 | `planner/verify.rs`, `minimal_loop/build_verifier.rs` | `full_source_port_target` / pass re-audit |
| Minimal runner | `src/agent/minimal_step_runner/repair.rs` | 386 | source repair prompt and recovery UltraPlan handoff | G-S10, G-S13 | `planner/repair.rs`, `planner/runner.rs` | `full_source_port_target` |
| Minimal runner | `src/agent/minimal_step_runner/profiles/nextjs.rs` | 318 | Next.js source profile checks and verification | G-S04, G-S09, G-S11, G-S12 | `planner/profiles/nextjs.rs`, `planner/runner.rs` | `full_source_port_target` |

## MVP Runtime Counterpart Inventory

| MVP group | files | source authority to compare | gates |
| --- | --- | --- | --- |
| Completion / evidence | `mvp/anvilminimal/src/minimal_loop/completion.rs`, `mvp/anvilminimal/src/minimal_loop/evidence.rs`, `mvp/anvilminimal/src/minimal_loop/loop_run.rs` | `TaskContract`, completion policy, evidence binding, actor loop completion authority | G-S01, G-S02, G-S12 |
| Plan / UltraPlan | `mvp/anvilminimal/src/planner/runner.rs`, `planner/step_plan.rs`, `planner/ultra_plan.rs`, `planner/lint.rs`, `planner/profile.rs` | `minimal_step_runner.rs`, `profile.rs`, `plan_lint.rs` | G-S03, G-S04, G-S05, G-S06 |
| Verify / dependency lifecycle | `planner/verify.rs`, `minimal_loop/build_verifier.rs`, `minimal_loop/dependency_setup.rs`, `planner/profiles/nextjs.rs` | `verifier*.rs`, `verifier_command_policy.rs`, `project_probe.rs`, `project_verifier.rs`, `node_*` | G-S08, G-S09 |
| Repair / recovery | `minimal_loop/repair_target.rs`, `minimal_loop/repair_progress.rs`, `planner/repair.rs`, `planner/runner.rs` | `repair_job.rs`, `repair_lifecycle.rs`, `repair_job_dispatch.rs`, `task_contract_recovery.rs` | G-S10, G-S13 |
| Final acceptance / release gate | `planner/runner.rs`, `minimal_loop/evidence.rs`, `eval_events.rs` | `verifier.rs`, `verifier_driver.rs`, `task_contract_completion_policy.rs`, `summary.rs` | G-S11, G-S12, G-S16 |
| Tool / provider behavior | `tools/args_recovery.rs`, `providers/xml_fallback.rs`, `providers/openai.rs`, `providers/gemini.rs`, `providers/ollama.rs` | source tool runner, Ollama XML fallback, tool args policy | G-S07, G-S15 |
| TUI / diagnostics | `eval_events.rs`, `lib.rs`, `tui/slash.rs`, `tui/repl.rs`, `scripts/eval_lib/runtime_trace.py`, `failure_classification.py` | `summary.rs`, `safe_stop_emit.rs`, `safe_stop_payload.rs`, verifier event emitters | G-S14, G-S16 |

## UAT004-GATE-01 Port Record

| gate | source authority applied | MVP implementation | local evidence | remaining trace gap |
| --- | --- | --- | --- | --- |
| G-S01 | `TaskContract::evaluate*`, `CompletionPolicy`, request inference, artifact contract, evidence binding, artifact ledger/ownership/target alignment, worker contract | `minimal_loop/evidence.rs` blocks weak verifier evidence under source-first runtime requirements and keeps implementation/style/docs/scaffold roles separate; `minimal_loop/loop_run.rs` exposes capability/evidence bindings in step-level completion events | focused artifact-only verifier fixture plus existing title-only, docs-only, style-only, scaffold-only, canvas/browser negative fixtures passed under `cargo test` | source/anvildev same-condition trace deferred to GATE-09 |
| G-S02 | Source completion authority is deterministic and tied to required roles/evidence, not path existence alone | `RunSessionOptions::plan_step(Implement)` enables completion contract verification/path merge; `planner/runner.rs` passes generated or explicit contract path into implement step config; setup/inspect/verify/report step side effects remain disabled | `implement_plan_step_keeps_completion_contract_authority_enabled`, non-interactive path completion fixture, external contract evidence fixture, and interactive scaffold/docs negative plan fixtures passed | browser/provider/release evidence remains GATE-05/GATE-09 |

## UAT004-GATE-02 Port Record

| gate | source authority applied | MVP implementation | local evidence | remaining trace gap |
| --- | --- | --- | --- | --- |
| G-S03 | `minimal_step_runner.rs` generation loop rejects tool calls, validates schema/lint, retries with corrective prompts, and fails invalid plans instead of returning fallback success | Existing `planner/runner.rs` source-shaped UltraPlan/StepPlan generation was re-audited; StepPlan generation now calls workspace-aware lint so source `lint_plan_with_workspace` semantics are used before success | `generated_final_verify_uses_existing_workspace_nextjs_artifacts`, `invalid_ultra_plan_generation_does_not_save_plan_file`, existing tool-call/schema/lint retry tests; full `cargo test` and `pytest mvp/anvilminimal/tests/eval` passed | provider probe skipped because no prompt/provider schema changed; final comparative source/MVP run deferred to GATE-09 |
| G-S04 | `minimal_step_runner/plan_lint.rs` and `profiles/nextjs.rs` allow final build verification to rely on existing workspace entrypoint while keeping early build/order failures strict | `planner/lint.rs` adds `lint_step_plan_report_with_workspace`, checks existing `package.json`/`node_modules` context and Next.js entrypoints, and `planner/runner.rs` uses it for generation and plan-file validation | `workspace_manifest_and_entrypoint_allow_final_nextjs_verify`, `workspace_manifest_without_entrypoint_still_rejects_nextjs_build`, existing deterministic scaffold recovery completion-boundary fixture; full local tests passed | dependency setup/build rerun remains G-S09; browser/final acceptance remains G-S12 |

## UAT004-GATE-03 Port Record

| gate | source authority applied | MVP implementation | local evidence | remaining trace gap |
| --- | --- | --- | --- | --- |
| G-S08 | `verifier_command_policy.rs` admits only deterministic verifier command shapes and rejects shell control/setup/dev-server/workspace escape; `project_verifier.rs` keeps cheap checks structured and non-shell | `planner/verify.rs` keeps verify diagnosis and planner split policy strict; `planner/lint.rs` no longer turns manifest/dependency missing for existing verifier artifacts into planner dependency-order errors | verify policy positive/negative fixtures rerun in full `cargo test`; new dependency-boundary fixtures prove missing manifest is classified by verifier lifecycle | same-condition source/anvildev trace deferred to GATE-09 |
| G-S09 | `node_request_helpers.rs` + `node_runner_manifest.rs` treat Node manifest completion as deterministic setup under authority; project probe/verifier and actor flow connect dependency missing to setup/retry/verifier lifecycle | `minimal_loop/build_verifier.rs` records dependency check, setup authority, setup attempted/blocked/passed, build/test rerun, final verification status; `dependency_setup.rs` gates network setup by authority and uses deterministic Node test manifest completion; `planner/lint.rs` routes existing entrypoint/test artifacts to dependency boundary | setup blocked/allowed/build rerun fixtures, Node test runner manifest fixtures, setup-only/manifest-only negative fixtures, and full local tests passed | browser/final acceptance remains G-S12; comparative source/MVP run remains GATE-09 |

## Gate-Level Migration Decision

| gate | baseline status from matrix | UAT004 source-first decision | inventory note |
| --- | --- | --- | --- |
| G-S01 | fail | `full_source_port_target` | TaskContract request inference and completion authority are not restored. |
| G-S02 | pass | pass re-audit, `full_source_port_target` or documented equivalent authority | Existing pass is provisional because full TaskContract graph is not ported. |
| G-S03 | pass | `source_parity_pass` after GATE-02 re-audit | Source prompt/schema/retry/lint/fail-fast semantics are covered by source refs and local fixtures; provider probe is skipped because no prompt/provider schema changed. |
| G-S04 | fail | `source_parity_pass` for GATE-02 generation/lint scope | Final verify phase now uses existing workspace manifest+entrypoint for plan validity; downstream dependency/build/final acceptance work remains assigned to G-S09/G-S12. |
| G-S05 | fail | `source_trace_verification_target` | Phase context exists in MVP events; source same-condition trace is missing. |
| G-S06 | fail | `source_trace_verification_target` | Step prompt contract events exist; source same-condition trace is missing. |
| G-S07 | fail | `source_trace_verification_target` | Provider/tool recovery requires live/source trace; no provider probe in GATE-00. |
| G-S08 | pass | `source_parity_pass` after GATE-03 re-audit | Verify command policy fixtures and dependency-boundary classification were rerun against source refs; shell control/setup/dev-server/workspace escape remain strict. |
| G-S09 | fail | `source_parity_pass` for GATE-03 lifecycle scope | Dependency missing now flows through setup authority, deterministic setup/materialization, rerun, and verification lifecycle with setup-only/manifest-only still blocked from success. |
| G-S10 | fail | `full_source_port_target` | RepairJob / repair lifecycle / verifier targeting parity is missing. |
| G-S11 | fail | `full_source_port_target` | Scaffold must remain continuation target and not completion. |
| G-S12 | fail | `full_source_port_target` | Final acceptance must integrate artifact/build/capability/postcheck/browser evidence. |
| G-S13 | fail | `full_source_port_target` | Recovery handoff must lead to recovery run gate, not just saved files. |
| G-S14 | pass | pass re-audit, `source_trace_verification_target` | Blank failure kind improved, but source diagnostics trace is missing. |
| G-S15 | fail | `source_trace_verification_target` | Provider behavior requires probe/trace, with API-key absence as skip evidence. |
| G-S16 | fail | `source_trace_verification_target` | Manual TUI observability trace is not registered in gate manifest. |

## Baseline Fixture Binding

| fixture | bound source groups | bound MVP groups | expected later use |
| --- | --- | --- | --- |
| `test0701_005` final phase scaffold failure | minimal runner lint/verify, verifier command policy, project probe | `planner/lint.rs`, `planner/verify.rs`, `planner/runner.rs` | G-S04/G-S08/G-S09 regression fixture |
| `test0701_005` completion contract disabled in plan-run steps | TaskContract, completion policy, artifact ledger, worker contract | `completion.rs`, `evidence.rs`, `loop_run.rs`, `runner.rs` | G-S01/G-S02/G-S12 negative fixture |
| `test0701_005` shallow interactive app accepted by intermediate steps | TaskContract, verifier, worker contract, scaffold pipeline | `evidence.rs`, `planner/runner.rs`, `planner/profiles/nextjs.rs` | G-S01/G-S11/G-S12 negative fixture |
| `test0701_005` recovery YAML saved but run still failed | repair job, repair lifecycle, safe stop, summary | `planner/repair.rs`, `planner/runner.rs`, `eval_events.rs`, `tui/slash.rs` | G-S10/G-S13/G-S16 fixture |
| `test0701_005` summary incomplete plus final complete | summary, safe stop emit/payload | `lib.rs`, `tui/slash.rs`, `eval_events.rs` | G-S14/G-S16 regression fixture |

## Rollback Conditions

Rollback is not allowed if it reintroduces any of the following:

- `failure_kind` blank for failed command or lifecycle stage.
- build-only / path-only / scaffold-only false positive for interactive app/game tasks.
- recovery handoff missing when bounded repair or final acceptance fails.
- TUI or summary showing task completion when command/task failed or remains incomplete.
- relaxed workspace confinement or shell control syntax acceptance.

Success-rate regression must be classified separately from correct failure detection. A lower success rate is acceptable when it comes from newly correct rejection of shallow interactive output, missing browser evidence, or invalid final verify planning.
