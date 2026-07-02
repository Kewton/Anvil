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

## UAT004-GATE-04 Port Record

| gate | source authority applied | MVP implementation | local evidence | remaining trace gap |
| --- | --- | --- | --- | --- |
| G-S10 | `RepairJob` / `RepairNextAction` / `RepairAttemptOutcome` map verifier failure to a target, require bounded repair, classify no-progress, and route terminal states to diagnostic/replan/safe-stop. `verifier_repair_targeting.rs` admits concrete failure targets. | MVP keeps a smaller controller surface: `repair_target.rs` maps failure reports to target classes and `classify_repair_follow_through` emits no-change/target-not-followed/unrelated-change; `planner/runner.rs` and `minimal_loop/loop_run.rs` make those terminal failure kinds and recovery handoff instead of success. | New browser route target fixtures plus existing missing entrypoint, missing capability, no-change, target-not-followed, unrelated-change, and bounded repair exhaustion fixtures passed under full `cargo test`. | Source `RepairJob` was not ported wholesale; anvildev same-condition trace remains GATE-09. |
| G-S11 | `scaffold_pipeline.rs` records deterministic scaffold files as bootstrap/continuation and pushes continuation notes before completion. | MVP keeps scaffold artifacts out of implementation capability binding and uses profile auto-repair continuation prompts; deterministic scaffold recovery emits `used_for_completion=false`. | `deterministic_profile_fallback_requires_targeted_continuation_before_success`, `profile_auto_repair_continuation_prompt_treats_scaffold_as_incomplete`, and `plan_run_nextjs_game_scaffold_only_fails_inferred_capabilities` passed in full cargo. | Browser/manual release quality remains G-S12/G-S16. |
| G-S13 | `minimal_step_runner/repair.rs`, `repair_lifecycle.rs`, and `safe_stop_*` require exhausted repair to produce a bounded handoff/safe-stop payload, not task success. | `planner/repair.rs` writes `.md` recovery prompt and recovery UltraPlan YAML; `planner/runner.rs` validates artifacts, emits suggested prompt/YAML commands, writes incomplete summary, and executes saved recovery YAML in a fixture. | `step_repair_no_change_is_classified_and_handoff_saved`, `ultra_phase_scaffold_failure_saves_recovery_yaml_and_incomplete_handoff`, `ultra_final_acceptance_repair_failure_saves_recovery_handoff`, `plan_run_nextjs_interactive_app_records_partial_release_gate`, and `saved_recovery_ultra_plan_can_drive_fixture_recovery_success` passed. | Live provider/browser recovery run remains deferred to GATE-09. |

## UAT004-GATE-05 Port Record

| gate | source authority applied | MVP implementation | local evidence | remaining trace gap |
| --- | --- | --- | --- | --- |
| G-S12 | `verifier.rs`, `verifier_driver.rs`, `project_probe.rs`, `project_verifier.rs`, `task_contract_completion_policy.rs`, `actor_loop_flow.rs`, and `emit_verifier_events.rs` make final acceptance depend on deterministic verifier/runtime evidence and evented lifecycle, not build/path evidence alone. Runtime/browser failures stay distinct from unavailable probes. | `planner/runner.rs` final contract/release gate already requires browser readiness plus interaction evidence for release-grade pass. This gate adds explicit dev-server lifecycle stages and probe environment to both event and browser readiness evidence, preserving `partial` for unavailable browser and `failed` for HTTP 500. Existing `minimal_loop/evidence.rs`, `completion.rs`, and eval browser/parity scripts keep evidence content validation. | Browser HTTP 500, missing interaction, valid browser+interaction pass, title/static/build-only negative, malformed evidence, and final acceptance recovery fixtures passed under full local cargo and pytest. | Live browser/manual release probe and source/anvildev same-condition trace remain GATE-09 because browser/Playwright must not be normal unit-test dependencies. |

## UAT004-GATE-06 Port Record

| gate | source authority applied | MVP implementation | local evidence | remaining trace gap |
| --- | --- | --- | --- | --- |
| G-S05 | Source `minimal_step_runner/profile.rs` builds phase prompts with ultra goal, current phase, workspace snapshot, and profile contract; `minimal_step_runner.rs` carries shared session context through UltraPlan execution. | `runtime_trace.py` now derives source phase-context trace events from source `.anvil/state/sessions/*/logs/llm-io.jsonl` prompts and compares them with MVP `ultra_phase_context_attached` / `ultra_phase_context_updated` normalized events. | GATE-06 same-condition diff shows signature `match` and G-S05 `pass`; `test_compare_reports_detects_missing_phase_context` fails the gate when phase context is missing. | No remaining GATE-06 gap. Full comparative/release run still belongs to GATE-09. |
| G-S06 | Source `minimal_step_runner.rs` and `minimal_step_runner/repair.rs` construct step and repair prompts with overall goal, expected paths, verify commands, expected result, and bounded repair instructions. | `runtime_trace.py` maps source prompt logs to `step_prompt_built` and verifies contract booleans against MVP `step_prompt_contract`; eval preflight fixtures now require normalized prompt events rather than gate counts alone. | GATE-06 same-condition diff shows G-S06 `pass`; `test_compare_reports_detects_missing_expected_result_and_verify` fails the gate when expected result or verify command evidence is missing. | No remaining GATE-06 gap. Prompt/provider-sensitive changes must be rechecked in GATE-07/GATE-09. |

## UAT004-GATE-07 Port Record

| gate | source authority applied | MVP implementation | local evidence | remaining trace gap |
| --- | --- | --- | --- | --- |
| G-S07 | Source `ollama/xml_fallback.rs` recovers source-style XML/function tags, relaxed JSON, alias/wrapper drift, and inferred unambiguous tool names while leaving safety to tool execution policy. | `providers/xml_fallback.rs` now accepts those source-style XML shapes and still routes recovered arguments through `recover_tool_arguments`; unsafe path and shell args are rejected by `ToolRegistry`, `path_guard`, and `bash` policy. | XML fallback source parity fixtures passed; `provider_probe_tool_args_recovery_classification_by_provider` proves recoverable provider args execute and unsafe path/shell-control args are nonrecoverable for OpenAI/Gemini/Ollama-shaped calls. | No remaining GATE-07 trace gap; GATE-09 can re-run comparative provider smoke as release evidence. |
| G-S15 | Source has local Ollama/XML fallback behavior; MVP adds OpenAI/Gemini provider extensions that require probe evidence rather than source exact copy. | `live_provider.rs` observes live OpenAI tool args shape, live Gemini function-calling schema, Ollama XML fallback, parser fixtures, and provider-shaped recovery/rejection classifications. `eval-run.py` attaches probe status/counts and unsafe shell-control classification to eval summary. | `provider-probe-live.jsonl` and `provider_probe_summary.json` show 7 passed / 0 failed / 0 skipped; full cargo and eval pytest passed. | No GATE-07 provider diff remains. If credentials are unavailable later, record skip evidence instead of downgrading unit-test pass/fail. |

## UAT004-GATE-08 Port Record

| gate | source authority applied | MVP implementation | local evidence | remaining trace gap |
| --- | --- | --- | --- | --- |
| G-S14 | Source `summary.rs`, `safe_stop_emit.rs`, `safe_stop_payload.rs`, and `emit_verifier_events.rs` expose stop reason, failure type, blocker/authority status, and next user action as diagnostic lifecycle data rather than only terminal text. | `eval_events.rs` now separates command status, task status, session/REPL status, and recovery next action in summary output. `runtime_trace.py` normalizes source `agent.safe_stop.report` and verifier diagnostics, and keeps MVP stop/recovery fields in normalized details. | `gate08_trace/runtime-semantics-trace-diff.json` has G-S14 `pass`; `test_source_and_mvp_diagnostics_trace_can_pass_gs14` covers source/MVP diagnostic trace comparison; failed run and TUI fixtures keep concrete failure kinds. | No GATE-08 diagnostics gap remains. Full release/manual trace remains GATE-09. |
| G-S16 | Source session diagnostics keep command/task stop reason separate from interactive readiness. | `lib.rs` emits process stop with `task_status`, `session_status=process_exited`, and `repl_status=not_applicable`; `tui/slash.rs` emits TUI command stop with `task_status`, `session_status=repl_ready`, and `repl_status=ready`, so REPL readiness cannot imply task completion. Default events remain under `.anvil/runs/<run-id>/events.jsonl`. | `gate08_trace/manual_tui_run/.anvil/runs/gate08-manual/events.jsonl` and `summary.md`; `tui_slash_failure_records_run_events_and_failure_stage`, `tui_slash_success_with_partial_release_gate_is_not_complete_only`, and run lifecycle tests passed. | Deterministic manual trace fixture is registered; live terminal/manual UAT remains GATE-09. |

## Gate-Level Migration Decision

| gate | baseline status from matrix | UAT004 source-first decision | inventory note |
| --- | --- | --- | --- |
| G-S01 | fail | `full_source_port_target` | TaskContract request inference and completion authority are not restored. |
| G-S02 | pass | pass re-audit, `full_source_port_target` or documented equivalent authority | Existing pass is provisional because full TaskContract graph is not ported. |
| G-S03 | pass | `source_parity_pass` after GATE-02 re-audit | Source prompt/schema/retry/lint/fail-fast semantics are covered by source refs and local fixtures; provider probe is skipped because no prompt/provider schema changed. |
| G-S04 | fail | `source_parity_pass` for GATE-02 generation/lint scope | Final verify phase now uses existing workspace manifest+entrypoint for plan validity; downstream dependency/build/final acceptance work remains assigned to G-S09/G-S12. |
| G-S05 | fail | `source_parity_pass` after GATE-06 normalized trace comparison | Same-condition source/MVP trace matched for phase context continuity; missing context is now a semantic trace failure. |
| G-S06 | fail | `source_parity_pass` after GATE-06 normalized trace comparison | Same-condition source/MVP trace matched for step prompt construction; missing expected result or verify evidence is now a semantic trace failure. |
| G-S07 | fail | `source_parity_pass` after GATE-07 provider/tool probe | Source-style XML fallback shapes are recovered, provider-shaped recoverable args execute, and unsafe path/shell-control args are nonrecoverable. |
| G-S08 | pass | `source_parity_pass` after GATE-03 re-audit | Verify command policy fixtures and dependency-boundary classification were rerun against source refs; shell control/setup/dev-server/workspace escape remain strict. |
| G-S09 | fail | `source_parity_pass` for GATE-03 lifecycle scope | Dependency missing now flows through setup authority, deterministic setup/materialization, rerun, and verification lifecycle with setup-only/manifest-only still blocked from success. |
| G-S10 | fail | `source_parity_pass` for bounded MVP lifecycle; full source trace deferred | Repair target classification, follow-through, bounded repair exhaustion, and recovery handoff are covered by source refs and local fixtures. |
| G-S11 | fail | `source_parity_pass` for scaffold continuation boundary | Scaffold-only app completion is rejected and deterministic scaffold remains continuation target. |
| G-S12 | fail | `source_parity_pass` for local final acceptance authority; live comparative trace deferred | Final acceptance now integrates artifact/build/capability/browser/interaction evidence by content, distinguishes browser unavailable partial from HTTP 500 failure, and keeps missing interaction/static/build-only output below release-grade full success. |
| G-S13 | fail | `source_parity_pass` for recovery handoff fixture gate; live comparative run deferred | Recovery `.md`, YAML, suggested commands, incomplete status, and targeted recovery execution are covered by local fixtures. |
| G-S14 | pass | `source_parity_pass` after GATE-08 diagnostics trace comparison | Source/MVP diagnostics trace now observes G-S14 on both sides, and failed lifecycle summaries keep concrete status/failure/recovery fields. |
| G-S15 | fail | `source_parity_pass` after GATE-07 live/provider probe | OpenAI/Gemini/Ollama provider behavior was observed by probe; no API-key skip remained in this run. |
| G-S16 | fail | `source_parity_pass` after GATE-08 manual trace fixture | Manual/TUI `.anvil/runs/<run-id>/events.jsonl` evidence is registered and summary/status projection separates task failure from REPL readiness. |

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

## UAT004-GATE-09 Release Inventory Decision

GATE-09 did not add new source modules to the inventory. It used the existing source authority list plus same-condition MVP/anvildev release evidence to decide which previously scoped passes remain release-ready and which need fuller source-port trace coverage.

| area | source authority | MVP/runtime evidence | GATE-09 decision |
| --- | --- | --- | --- |
| Completion contract / artifact quality | `task_contract_completion_policy.rs`, `artifact_ledger.rs`, `worker_contract.rs` | MVP provider-smoke produced `hello.txt` with punctuation and failed deterministic verify; anvildev produced exact content and passed | Release blocker: MVP lower success is an artifact/runtime quality gap, not a pure correct-failure-detection win |
| Planner / phase / prompt trace | `minimal_step_runner.rs`, `profile.rs`, `repair.rs` | `source-mvp-runtime-trace-diff.json` lacks G-S05/G-S06 prompt/phase observations for this minimal-loop release scenario | Reopened for release trace coverage while retaining GATE-06 scoped pass |
| Verifier / dependency lifecycle | `verifier_command_policy.rs`, `project_verifier.rs`, `node_*` | Provider-smoke comparison does not observe dependency/build lifecycle and misses MVP `verify_started` parity for G-S08 | Reopened for release comparative trace; local GATE-03 fixtures remain valid |
| Repair / recovery | `repair_job.rs`, `repair_lifecycle.rs`, `minimal_step_runner/repair.rs`, `safe_stop_*` | Saved recovery YAML was executed and failed concretely with `phase_scaffold_error` / shell-control verify policy | Recovery handoff is validated as executable evidence, but not success |
| Final acceptance / browser / interaction | `verifier.rs`, `verifier_driver.rs`, `task_contract_completion_policy.rs` | Manual browser route and interaction evidence passed, but parity report release UAT still fails due TUI command failure and comparative gaps | Browser/interaction readiness alone is insufficient for release pass |
| Diagnostics / TUI observability | `summary.rs`, `safe_stop_emit.rs`, `safe_stop_payload.rs`, `emit_verifier_events.rs` | Original test0701_005 TUI events remain `tui_command_failed`; parity report uses this to block release. Eval summary has no blank failure kind. | G-S14 stays trace-pass; G-S16 remains release-blocking until current live TUI evidence passes |

Evidence root: `workspace/mvp/uat/004/gate09_release/`.
