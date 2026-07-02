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

## UAT004-GATE-03 Trace Update

| item | status | evidence |
| --- | --- | --- |
| source refs inspected | complete | `src/agent/loop_run/node_request_helpers.rs`, `node_runner_manifest.rs`, `project_probe.rs`, `project_verifier.rs`, `verifier_command_policy.rs`, `node_test_evidence_quality.rs`, `actor_loop_flow.rs` |
| MVP refs changed | complete | `mvp/anvilminimal/src/planner/lint.rs`, `mvp/anvilminimal/src/planner/verify.rs`, `mvp/anvilminimal/src/minimal_loop/build_verifier.rs` |
| G-S09 lifecycle evidence | source-equivalent local fixture | Dependency lifecycle fixtures cover setup blocked, setup allowed, deterministic Node test-runner manifest completion without network, build/test rerun, and verification pass/fail. New fixtures add missing `package.json` boundary for existing Next.js entrypoint and Node test artifact so planner lint does not absorb manifest/dependency missing. |
| G-S08 verify policy evidence | source-equivalent local fixture | `planner/verify.rs` positive/negative fixtures were rerun: shell control, setup/dev-server, workspace escape, and unsafe planner split remain rejected; safe npm/cargo/python verifier shapes remain accepted. |
| setup-only / manifest-only boundary | preserved | Existing `manifest_only_nextjs_build_verify_is_not_success`, `minimal_loop_nextjs_required_paths_only_does_not_complete`, `plan_run_nextjs_game_setup_only_fails_inferred_obligation`, and setup/scaffold negative evidence fixtures continue to prevent setup-only/manifest-only completion. |
| required local tests | passed | `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`: passed with 438 lib tests plus integration/doc tests; `pytest mvp/anvilminimal/tests/eval`: 222 passed, 1 skipped |
| network install | skipped | No real network install was executed. Setup execution remains guarded by `NodeDependencySetupAuthority`, package manager/workspace state, package contract, and profile contract; allowed setup fixtures use fake/local deterministic setup or manifest materialization. |
| source/anvildev same-condition run | skipped | Local source refs plus deterministic lifecycle fixtures cover GATE-03 scope. Full comparative source/MVP run remains UAT004-GATE-09. |
| browser/manual evidence | skipped | GATE-03 does not close final browser readiness or interaction acceptance; G-S12/G-S16 remain later gates. |

## UAT004-GATE-04 Trace Update

| item | status | evidence |
| --- | --- | --- |
| source refs inspected | complete | `src/agent/loop_run/repair_job.rs`, `repair_lifecycle.rs`, `repair_job_dispatch.rs`, `verifier_repair_targeting.rs`, `scaffold_pipeline.rs`, `no_progress_recovery.rs`, `safe_stop_emit.rs`, `safe_stop_payload.rs`, `actor_loop_flow.rs`, `verifier_driver.rs`, `src/agent/minimal_step_runner/repair.rs` |
| MVP refs changed | complete | `mvp/anvilminimal/src/minimal_loop/repair_target.rs` adds browser route failure repair-target fixtures. Existing audited refs: `minimal_loop/repair_progress.rs`, `minimal_loop/loop_run.rs`, `planner/repair.rs`, `planner/runner.rs`, `eval_events.rs`. |
| G-S10 repair target evidence | source-equivalent local fixture | Missing entrypoint and missing capability targets are covered by existing `classifies_missing_entrypoint`, `classifies_capability_missing`, `step_repair_missing_entrypoint_followthrough_creates_expected_artifact`. New `browser_http_500_route_failure_targets_framework_config` and `browser_route_failure_targets_test_or_evidence` fix browser route failure as repair targets. |
| G-S10 follow-through evidence | source-equivalent local fixture | `step_repair_no_change_is_classified_and_handoff_saved`, `step_repair_target_not_followed_is_classified_and_handoff_saved`, and `step_repair_unrelated_change_is_classified_and_handoff_saved` classify no-change, target-misdirected, and unrelated repair with dedicated failure kinds and recovery handoff. |
| G-S11 scaffold continuation evidence | source-equivalent local fixture | `deterministic_profile_fallback_requires_targeted_continuation_before_success` emits `used_for_completion=false`, `profile_auto_repair_continuation_prompt_treats_scaffold_as_incomplete` keeps scaffold as recovery scaffold, and `plan_run_nextjs_game_scaffold_only_fails_inferred_capabilities` rejects scaffold-only completion. |
| G-S13 recovery handoff evidence | source-equivalent local fixture | `ultra_phase_scaffold_failure_saves_recovery_yaml_and_incomplete_handoff`, `ultra_final_acceptance_repair_failure_saves_recovery_handoff`, `plan_run_nextjs_interactive_app_records_partial_release_gate`, and `saved_recovery_ultra_plan_can_drive_fixture_recovery_success` prove `.md`, recovery YAML, suggested commands, incomplete status, and targeted recovery execution. |
| required local tests | passed | `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`: passed with 440 lib tests plus integration/doc tests; `pytest mvp/anvilminimal/tests/eval`: passed with 222 passed, 1 skipped |
| source/anvildev same-condition run | skipped | GATE-04 did not run the full source `RepairJob` under anvildev. The scoped parity claim is based on source refs plus deterministic MVP fixtures. Full source/MVP comparative trace remains UAT004-GATE-09. |
| provider/browser/manual evidence | skipped | No live provider or browser run was required for GATE-04. Browser route failures are fixed as deterministic repair-target fixtures; release-grade browser/interaction execution remains GATE-05/GATE-09. |

## UAT004-GATE-05 Trace Update

| item | status | evidence |
| --- | --- | --- |
| source refs inspected | complete | `src/agent/loop_run/verifier.rs`, `verifier_driver.rs`, `project_probe.rs`, `project_verifier.rs`, `task_contract_completion_policy.rs`, `actor_loop_flow.rs`, `emit_verifier_events.rs` |
| MVP refs changed | complete | `mvp/anvilminimal/src/planner/runner.rs` adds dev-server lifecycle stage list and probe environment to browser readiness evidence and event payloads. Existing audited refs: `minimal_loop/evidence.rs`, `minimal_loop/completion.rs`, `minimal_loop/build_verifier.rs`, `planner/profiles/nextjs.rs`, `scripts/eval_lib/browser_oracle.py`, `scripts/eval_lib/parity_gate.py`. |
| browser readiness evidence | source-equivalent local fixture | `nextjs_dev_route_probe_disabled_records_lifecycle_stages` asserts browser unavailable is `partial`, not `failed`, and that readiness evidence plus `dev_server_lifecycle` events contain `start`, `wait`, `probe`, `cleanup`, `PORT`, and `NODE_ENV`/probe env keys. |
| browser failure evidence | source-equivalent local fixture | `plan_run_nextjs_browser_http_500_fails_final_contract`, `plan_run_nextjs_tailwind_dev_route_failure_keeps_failure_kind`, `browser_http_500_fails_runtime_acceptance`, and eval HTTP 500 fixtures keep route/browser HTTP 500 as final acceptance failure, not full success. |
| interaction evidence | source-equivalent local fixture | `plan_run_nextjs_browser_ready_without_interaction_is_partial` blocks release-grade pass without interaction evidence; `plan_run_nextjs_browser_and_interaction_evidence_passes_release_gate` proves the positive pass path when browser readiness and interaction evidence content are valid. |
| static/build-only false positive evidence | preserved | `title_only_output_does_not_satisfy_interactive_game_evidence`, docs/style/scaffold/setup-only fixtures, eval false-positive fixtures, and parity gate malformed/missing evidence fixtures keep path/title/static/build-only output below release-grade full success. |
| final acceptance repair/recovery lifecycle | source-equivalent local fixture | `plan_run_nextjs_interactive_app_records_partial_release_gate`, `ultra_final_acceptance_failure_runs_bounded_repair`, `ultra_final_acceptance_repair_failure_saves_recovery_handoff`, and lifecycle masking tests keep final acceptance failure connected to bounded repair/recovery/diagnostics instead of success. |
| required local tests | passed | `cargo fmt --manifest-path mvp/anvilminimal/Cargo.toml -- --check`: passed; `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`: passed with 440 lib tests plus integration/doc tests; `pytest mvp/anvilminimal/tests/eval`: passed with 222 passed, 1 skipped |
| source/anvildev same-condition run | skipped | GATE-05 closes local source-first final acceptance authority by source refs and deterministic fixtures. Full source/MVP comparative browser trace remains UAT004-GATE-09. |
| live browser/manual evidence | skipped | Browser/Playwright/dev-server execution must not become a normal unit-test dependency. Local fixtures verify unavailable/failed/pass classification and evidence content; live release probe remains opt-in for GATE-09. |

## UAT004-GATE-06 Trace Update

| item | status | evidence |
| --- | --- | --- |
| source refs inspected | complete | `src/agent/minimal_step_runner.rs`, `src/agent/minimal_step_runner/profile.rs`, `src/agent/minimal_step_runner/repair.rs` |
| MVP refs changed | complete | `mvp/anvilminimal/scripts/eval_lib/runtime_trace.py`, `mvp/anvilminimal/tests/eval/test_runtime_semantics_trace.py`, `mvp/anvilminimal/tests/eval/test_eval_cli_contract.py` |
| source trace | collected from existing same-condition run root | `/private/tmp/anvilminimal-eval-0229-anvildev-net-timeout` normalized into `workspace/mvp/uat/004/gate06_trace/source/runtime-semantics-trace-report.json` and `runtime-semantics-normalized-events.jsonl` |
| MVP trace | collected from existing same-condition run root | `/private/tmp/anvilminimal-eval-0229-mvp-net-timeout` normalized into `workspace/mvp/uat/004/gate06_trace/mvp/runtime-semantics-trace-report.json` and `runtime-semantics-normalized-events.jsonl` |
| normalized comparison | pass for G-S05/G-S06 | `workspace/mvp/uat/004/gate06_trace/runtime-semantics-trace-diff.json`: same-condition signatures `match` (source 48 / MVP 48), `semantic_findings=[]`, G-S05/G-S06 both `pass` |
| G-S05 trace fields | observed | source `phase_context_attached=94`, MVP `phase_context_attached=75`; source prompt logs contain ultra goal, current phase, workspace snapshot, and profile contract; MVP events carry phase context continuity fields |
| G-S06 trace fields | observed | source `step_prompt_built=100`, MVP `step_prompt_built=141`; both sides carry overall goal, expected paths, verify commands, and expected result |
| missing-field detection | executable fixtures | `test_compare_reports_detects_missing_phase_context`, `test_compare_reports_detects_missing_expected_result_and_verify`, and `test_source_anvildev_llm_prompts_produce_phase_and_step_trace` |
| trace-missing detection | enforced | `runtime_trace.py` now treats missing normalized G-S05/G-S06 trace as semantic failure; `test_compare_reports_does_not_pass_gate_counts_without_prompt_trace` asserts gate counts alone cannot pass, and `test_eval_preflight_writes_comparative_parity_gate_report` fixture was updated to include normalized phase/step prompt events |
| required local tests | passed | `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`: passed with 440 lib tests plus integration/doc tests; `pytest mvp/anvilminimal/tests/eval`: passed with 226 passed, 1 skipped |
| new provider/browser run | skipped | GATE-06 is trace normalization/comparison over existing same-condition anvildev/MVP run roots. It does not require a new provider call, browser, or manual run. |

## UAT004-GATE-07 Trace Update

| item | status | evidence |
| --- | --- | --- |
| source refs inspected | complete | `src/ollama/client.rs`, `src/ollama/xml_fallback.rs`, `src/agent/minimal_step_runner.rs`, `src/agent/minimal_step_runner/repair.rs` |
| MVP refs changed | complete | `mvp/anvilminimal/src/providers/xml_fallback.rs`, `mvp/anvilminimal/tests/live_provider.rs`, `mvp/anvilminimal/scripts/eval-run.py`, `mvp/anvilminimal/tests/eval/test_eval_cli_contract.py` |
| provider probe JSONL | passed | `workspace/mvp/uat/004/gate07_provider_probe/provider-probe-live.jsonl`: 7 provider probe events, 0 failed, 0 skipped |
| provider probe summary | attached | `workspace/mvp/uat/004/gate07_provider_probe/eval-summary/provider_probe_summary.json`: `status=passed`, `passed=7`, `failed=0`, `skipped=0`, `recoverable_tool_args_classified=3`, `unsafe_tool_args_rejected=3`, `unsafe_shell_control_rejected=3` |
| eval summary attachment | passed | `workspace/mvp/uat/004/gate07_provider_probe/eval-summary/summary.eval.tsv` records `provider_probe_required=true`, `provider_probe_status=passed`, `provider_probe_passed=7`, `provider_probe_failed=0`, `provider_probe_skipped=0` |
| OpenAI probe | passed | Live `tool_args_shape` returned one `Write` tool call with object arguments; provider parser fixtures also recover JSON-string aliased args. |
| Gemini probe | passed | Live `function_calling_schema` returned one `Write` function call with object arguments; provider parser fixtures also recover JSON-string aliased args. |
| Ollama/XML probe | passed | Local XML fallback probe recovers source-style function-call output. Source parity fixtures cover source-style named tags, `<function=...>`, inferred unambiguous tool names, closed unterminated blocks, and relaxed JSON payloads. |
| unsafe args policy | passed | Provider-shaped recoverable `Write` aliases execute; provider-shaped `../secret.txt` is rejected as `path_confinement_error`; provider-shaped `curl ... | sh` is rejected as `dangerous_command`; both unsafe classes are nonrecoverable. |
| skip evidence | not used | API keys were available for OpenAI/Gemini. The initial sandbox network failure was re-run with network approval and did not represent provider behavior. |
| required local tests | passed | `cargo fmt --manifest-path mvp/anvilminimal/Cargo.toml -- --check`: passed; `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`: passed with 446 lib tests plus integration/doc tests; `pytest mvp/anvilminimal/tests/eval`: passed with 226 passed, 1 skipped |
| full source port escalation | not required | No G-S07/G-S15 provider/tool diff remains after source-style XML fallback support and live provider probe evidence. |

## UAT004-GATE-08 Trace Update

| item | status | evidence |
| --- | --- | --- |
| source refs inspected | complete | `src/agent/loop_run/summary.rs`, `safe_stop_emit.rs`, `safe_stop_payload.rs`, `emit_verifier_events.rs` |
| MVP refs changed | complete | `mvp/anvilminimal/src/eval_events.rs`, `src/lib.rs`, `src/tui/slash.rs`, `scripts/eval_lib/runtime_trace.py`, `tests/tui_integration.rs`, `tests/eval/test_runtime_semantics_trace.py` |
| source diagnostics trace | normalized | `workspace/mvp/uat/004/gate08_trace/source/runtime-semantics-trace-report.json` and `runtime-semantics-normalized-events.jsonl` include source `agent.safe_stop.report` as `diagnostic_emitted` with `failure_type`, `stop_reason`, authority/blocker, and next-user-action fields. |
| MVP TUI diagnostics trace | normalized | `workspace/mvp/uat/004/gate08_trace/mvp/runtime-semantics-trace-report.json` and `runtime-semantics-normalized-events.jsonl` include `tui_command_start`, `tui_command_stop`, and `run_stop` with command/task/session status fields and recovery next action. |
| normalized comparison | passed for G-S14/G-S16 | `workspace/mvp/uat/004/gate08_trace/runtime-semantics-trace-diff.json`: same-condition status `match`; G-S14 `pass`; G-S16 `pass`; unrelated unobserved gates remain failed in this scoped trace. |
| manual TUI run event artifact | present | `workspace/mvp/uat/004/gate08_trace/manual_tui_run/.anvil/runs/gate08-manual/events.jsonl` and `summary.md` show `.anvil/runs/<run-id>/events.jsonl`, failed command status, failed task status, `Session/REPL status: repl_ready`, recovery YAML path, and suggested YAML command. |
| failed `/ultra-plan-run` summary behavior | fixed by fixture | `tui_slash_failure_records_run_events_and_failure_stage` and `run_lifecycle_records_incomplete_stop_reason` assert failed command/process summaries end with `Status: incomplete`, not unconditional `Status: complete`. |
| recovery YAML path preservation | fixed by fixture | `tui_slash_success_with_partial_release_gate_is_not_complete_only` asserts `recovery_ultra_plan_path` and suggested YAML command remain in `tui_command_stop` events and summary. |
| required local tests | passed | `cargo fmt --manifest-path mvp/anvilminimal/Cargo.toml -- --check` passed; `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` passed with 446 lib tests plus integration/doc tests; `pytest mvp/anvilminimal/tests/eval` passed with 227 passed / 1 skipped. |
| source trace diff scope | documented | The GATE-08 diff intentionally observes diagnostics/TUI stages only. G-S05/G-S06 semantic missing findings in this diff are not regressions because they are covered by `gate06_trace`; full all-gate comparative trace remains GATE-09. |

## Skip Evidence

| skipped item | reason | impact | next action |
| --- | --- | --- | --- |
| source/anvildev same-condition run | GATE-00 was a baseline documentation phase; later gate updates override this row when trace is collected, as in UAT004-GATE-06 | source parity remains pending only for gates without later trace evidence | collect source/MVP normalized trace in the target gate before marking pass |
| provider live probe | GATE-00 skipped provider live probe; UAT004-GATE-07 later ran provider probe successfully with available API keys and network approval | no remaining GATE-07 provider skip impact; future no-key environments must record skip reason | re-run provider probe only in provider-sensitive gates or opt-in release gate |
| live browser readiness / interaction run | browser/dev-server must not be required for normal unit tests; GATE-05 used deterministic browser evidence fixtures and skipped live browser/manual execution | G-S12 local source parity is closed; G-S16/manual and comparative live release trace remain pending | run live browser readiness and interaction evidence in UAT004-GATE-09 or opt-in final gate |
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

## UAT004-GATE-09 Trace Update

| item | status | evidence |
| --- | --- | --- |
| release build | passed | `cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml` completed before comparative runs; release binary used: `mvp/anvilminimal/target/release/anvilminimal` |
| cargo verification | passed | `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` passed with 446 lib tests plus integration/doc tests |
| eval verification | passed | `pytest mvp/anvilminimal/tests/eval` passed with 227 passed / 1 skipped |
| initial MVP eval attempt | expected harness/confinement failure | `gate09_eval/mvp-provider-smoke/.../stderr.log` records external completion contract path outside workspace/temp; not used as provider result |
| MVP same-condition eval | executed with network approval | `gate09_release/mvp-provider-smoke-live.summary.eval.tsv`, `mvp-provider-smoke-live.anvil-events.jsonl`, `mvp-runtime-semantics-trace-report.json`; result `success=false`, `failure_kind=verify_repair_no_change`, `release_gate_status=failed` |
| source/anvildev same-condition eval | executed with network approval | `gate09_release/anvildev-provider-smoke-live.summary.eval.tsv`, `anvildev-runtime-semantics-trace-report.json`; command used `anvildev --engine minimal`; result `success=true`, `release_gate_status=pass` |
| normalized comparison | compared | `gate09_release/source-mvp-runtime-trace-diff.json`; `same_condition.status=match`; passed gates G-S02/G-S12/G-S14; failed gates G-S01/G-S03/G-S04/G-S05/G-S06/G-S07/G-S08/G-S09/G-S10/G-S11/G-S13/G-S15/G-S16 |
| release parity report | generated and schema-valid | `gate09_release/parity_gate_report.json`; validation errors `[]`; `success_rate_delta_pp=-100.0`; `success_delta_classification=release_quality_blocker_detected`; `correct_failure_detection=false`; `recovery_item_status.status=open` |
| browser route UAT | passed | `gate09_release/browser-readiness.json`: HTTP 200, route rendered, DOM ready; source app copied to `/private/tmp/anvil-uat004-gate09/test0701_005_browser_uat` |
| interaction UAT | passed for basic route interaction | `gate09_release/interaction-evidence.json`: clicked `DIFF 1`, canvas count changed 0 -> 1, state changed |
| dev server lifecycle | recorded | `gate09_release/dev-server-events.jsonl` records start/wait/probe/cleanup. Initial sandbox `listen EPERM` was rerun with approval; default Playwright browser missing was resolved by existing cached Chromium executable. |
| manual TUI UAT fixture | failed as expected | `gate09_release/test0701_005.original-events.jsonl` and `test0701_005.original-summary.md` preserve original `tui_command_stop ok=false` plus REPL/process complete contradiction as release-blocking evidence |
| recovery handoff execution | executed and failed concretely | `gate09_release/recovery-run.events.jsonl`, `recovery-run.summary.md`, `recovery-run.stderr.log`; saved recovery YAML ran and stopped with `phase_scaffold_error` / `verify command may not use shell control syntax` |
| provider probe status | reused from GATE-07 plus live comparative eval | GATE-07 provider probe remains `passed`; GATE-09 OpenAI provider was reachable after network approval and produced tool calls. No API-key skip was used. |
| full eval | skipped | Targeted provider-smoke comparative eval already exposed a release-quality blocker and source/MVP divergence. Full eval is not required until the blocker is fixed or a release candidate is promoted. |

### GATE-09 Success-Rate Classification

The lower MVP success rate is not accepted as a pure correct-failure-detection improvement. MVP correctly avoided a false full success after producing `provider smoke ok.`, but source/anvildev produced the exact accepted artifact. Therefore GATE-09 classifies the delta as a release-quality/runtime artifact gap while preserving the important negative invariant: failed verification did not become release-grade success.
