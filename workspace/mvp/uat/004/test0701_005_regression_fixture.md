# test0701_005 Regression Fixture

作成日: 2026-07-02

## Purpose

UAT004-GATE-00 の failure fixture として、`test0701_005` の失敗を再現可能な evidence set に固定する。

この fixture は Space Invaders 固有文字列ではなく、generic interactive app/game task の runtime semantics として扱う。

## Fixture Identity

| field | value |
| --- | --- |
| workspace | `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0701_005` |
| run id | `019f1e0b-3acd-7fd1-97dd-c10e03ba37eb` |
| run events | `.anvil/runs/019f1e0b-3acd-7fd1-97dd-c10e03ba37eb/events.jsonl` |
| event count | 249 lines |
| run summary | `.anvil/runs/019f1e0b-3acd-7fd1-97dd-c10e03ba37eb/summary.md` |
| UltraPlan | `.anvil/plans/ultra-plan-019f1e0b-9468-73b1-aa65-67d27ea5bc7b.yaml` |
| Recovery UltraPlan | `.anvil/plans/recovery-ultra-plan-phase-build-and-verify-019f1e11-5644-7e02-968c-8823ddff460b.yaml` |
| generated source | `src/app/page.tsx` |
| generated CSS | `src/app/globals.css` |
| command shape | `/ultra-plan-run --profile nextjs ...` |
| task kind | Next.js interactive app/game on port 3011 |

## UltraPlan Baseline

The generated UltraPlan has 5 phases:

| phase | baseline result |
| --- | --- |
| `setup-and-base-layout` | completed |
| `game-engine-core` | completed |
| `game-polish-and-effects` | completed |
| `ui-and-responsiveness` | completed |
| `build-and-verify` | failed before execution at scaffold |

The final phase prompt is a deterministic build verification request: run `npm run build` and ensure TypeScript/React/configuration errors are absent.

## Failure Fixtures

### F-001: Final Phase Scaffold Failure

| field | evidence |
| --- | --- |
| event | `ultra_phase_failed` |
| phase | `build-and-verify` |
| stage | `scaffold` |
| reason | `invalid StepPlan after corrective retries: Next.js build verify requires an entrypoint expected path first` |
| supporting planner errors | `verify_dependency_order_error`, `verify_command_policy_error`, `phase_scaffold_error` |
| expected classification | correct failure detection |
| gates | G-S03, G-S04, G-S08, G-S09 |

Expected future behavior:

- A final verify phase must inspect existing workspace state before rejecting valid verification.
- An invalid final verify plan must not be accepted as success.
- Shell control rejection and setup/build ordering must remain strict.

### F-002: Recovery Handoff Exists But Is Not Success

| field | evidence |
| --- | --- |
| events | `recovery_prompt_saved`, `ultra_partial_artifact_summary` |
| recovery prompt | `.anvil/repairs/repair-phase-build-and-verify-019f1e11-5644-7e02-968c-881734c65602.md` |
| recovery YAML | `.anvil/plans/recovery-ultra-plan-phase-build-and-verify-019f1e11-5644-7e02-968c-8823ddff460b.yaml` |
| parse checks | `recovery_prompt_parse_ok=true`, `recovery_yaml_parse_ok=true`, `recovery_command_targets_valid=true` |
| expected classification | correct handoff evidence, not task success |
| gates | G-S10, G-S13, G-S16 |

Expected future behavior:

- Recovery artifact persistence is a useful failure outcome.
- It must not promote the original task to success.
- Later gates must verify that the suggested recovery run can actually repair or cleanly fail.

### F-003: Summary Contradiction

| field | evidence |
| --- | --- |
| summary top | `Status: incomplete` |
| failed phase | `build-and-verify (phase_scaffold_error)` |
| summary tail | `Status: complete`, `Action: Repl`, `Stop reason: completed` |
| TUI command stop | `tui_command_stop ok=false`, `failure_kind=tui_command_failed` |
| process run stop | `run_stop ok=true`, `failure_kind=""`, `stop_reason=completed` |
| expected classification | regression / diagnostic gap |
| gates | G-S14, G-S16 |

Expected future behavior:

- Task status, command status, and REPL/process readiness must be projected separately.
- A failed command must not be summarized as unconditional task completion.
- A failed lifecycle stage must not end with blank failure kind.

### F-004: Plan-run Completion Contract Disabled

| field | evidence |
| --- | --- |
| repeated event | `step_obligation_scope` |
| observed values | `completion_contract_verification_enabled=false`, `completion_contract_path_merge_enabled=false`, `completion_contract_paths=[]`, `contract_paths_merged=false` |
| affected step kinds | setup, implement, verify, inspect |
| expected classification | regression / source parity gap |
| gates | G-S01, G-S02, G-S12 |

Expected future behavior:

- Interactive implementation steps must not release-pass with completion contract verification disabled.
- Non-interactive file creation should still complete via required paths when appropriate.
- Source TaskContract authority or a trace-proven equivalent must decide completion.

### F-005: Static Capability Evidence Can Become Pass Without Browser/Interaction Evidence

| field | evidence |
| --- | --- |
| event | `step_capability_evidence_check` |
| early result | `ok=false`, missing interactive evidence such as implementation, visible surface, input handler, state update, challenge/adversary, score/progression, collision/failure evidence |
| later result | `ok=true`, `primary_reason=pass` while `browser_readiness_evidence_path=""` and `interaction_evidence_path=""` |
| expected classification | partial diagnostic surface plus false positive risk |
| gates | G-S01, G-S11, G-S12 |

Expected future behavior:

- Static source hints are not enough for full interactive app/game acceptance.
- Browser readiness and interaction evidence are release-grade gates, with skip evidence when unavailable.
- Correct failure detection must be separated from success-rate regression.

### F-006: Generated Interactive App Is Shallow

From `src/app/page.tsx`:

| requirement class | fixture evidence | expected result |
| --- | --- | --- |
| player input | no `keydown`/`keyup`/keyboard listener; touch handlers are empty | should fail interactive acceptance |
| projectile mechanics | `bullets` is declared but never spawned, updated, rendered, or collided | should fail challenge/combat acceptance |
| collision/failure | no player damage, enemy projectile, lives, win, lose, or game-over transition setter | should fail failure/collision rule acceptance |
| scoring/progression | `score` is reset but never incremented; high score is read but not written | should fail progression evidence |
| requested polish | no Web Audio implementation and no power-up behavior | should not pass rich gameplay capability |

This fixture must not be implemented as a Space Invaders keyword check. The generic negative fixture is: an interactive app/game that has a visual surface and static source tokens but lacks input-driven state transitions, challenge/adversary resolution, score/progression, and failure/restart behavior.

### F-007: Browser Readiness Environment Sensitivity

From the UAT current-state record and `src/app/globals.css`:

| condition | observed baseline |
| --- | --- |
| `NODE_ENV=production` with `npm run dev` | `/` returned HTTP 500 in manual UAT record |
| failing source | `src/app/globals.css` starts with Tailwind directives |
| `NODE_ENV` unset with `npm run dev` | `/` returned HTTP 200 in manual UAT record |
| expected classification | release/browser readiness evidence required; environment must be recorded |
| gates | G-S09, G-S12, G-S16 |

Expected future behavior:

- Browser readiness must record environment, port, HTTP status, and failure kind.
- HTTP 500 is failure; browser unavailable is partial/skip evidence.
- Browser checks must not become normal unit-test requirements.

## Correct Failure Detection vs Regression

| item | classification | preserve or fix |
| --- | --- | --- |
| final invalid StepPlan rejected | correct failure detection | preserve; later fix source parity so valid final verify is accepted |
| recovery prompt/YAML saved and parse OK | correct failure detection / handoff | preserve; do not count as success |
| early missing capability/evidence reports | correct diagnostic surface | preserve; bind to completion authority later |
| step completion contract disabled | regression / source parity gap | fix |
| shallow interactive app reaching later phases | regression / false positive risk | fix |
| summary says incomplete and complete | regression / diagnostics gap | fix |
| `run_stop` blank failure kind after failed command | regression / diagnostics gap | fix |
| browser readiness depends on unrecorded `NODE_ENV` | regression / acceptance evidence gap | fix or record as explicit skip/failure |

## Positive And Negative Fixture Expectations

| fixture type | expected future assertion |
| --- | --- |
| positive | A valid existing Next.js workspace with entrypoint, package manifest, and build script can run a final build verification phase without requiring the final StepPlan to re-own entrypoint paths. |
| positive | A failed phase saves recovery prompt/YAML and exposes structured suggested commands without claiming original task success. |
| negative | An interactive app/game with only a title screen, canvas/static render, empty touch handlers, no input state, no scoring/progression, no collision/failure, and no browser/interaction evidence cannot pass final acceptance. |
| negative | `completion_contract_verification_enabled=false` cannot be a full release pass for interactive implementation steps. |
| negative | Summary/TUI cannot overwrite a failed task with unconditional `Status: complete`. |

## UAT004-GATE-02 Executable Fixture Binding

| baseline fixture | executable assertion | expected classification |
| --- | --- | --- |
| F-001 valid final verify over existing workspace | `workspace_manifest_and_entrypoint_allow_final_nextjs_verify` and `generated_final_verify_uses_existing_workspace_nextjs_artifacts` accept a final verify-only StepPlan when `package.json` and a Next.js entrypoint already exist in the workspace | regression fixed: valid final verify is no longer rejected only because the StepPlan did not re-own entrypoint expected paths |
| F-001 invalid early/final verify without entrypoint | `workspace_manifest_without_entrypoint_still_rejects_nextjs_build` still emits `dependency_order` for `npm run build` when only the manifest exists | correct failure detection preserved |
| invalid UltraPlan output | `invalid_ultra_plan_generation_does_not_save_plan_file` records `ultra_plan_generation_failed` with `planner_schema_error`, does not emit success, and does not save `.anvil/plans` | correct failure detection preserved; deterministic fallback is not treated as normal success |
| scaffold fallback boundary | existing `deterministic_profile_fallback_requires_targeted_continuation_before_success` keeps `deterministic_scaffold_recovery` as `used_for_completion=false` until targeted implementation continuation succeeds | correct boundary preserved |

## UAT004-GATE-03 Executable Fixture Binding

| baseline fixture | executable assertion | expected classification |
| --- | --- | --- |
| F-001 manifest/dependency missing after valid final verify planning | `workspace_entrypoint_without_manifest_routes_build_to_dependency_boundary`, `next_build_without_manifest_reports_manifest_boundary_before_execution`, and `nextjs_build_missing_manifest_is_dependency_boundary_not_command_execution` route an existing Next.js entrypoint with missing `package.json` to dependency lifecycle instead of planner lint or command execution | regression fixed: manifest/dependency missing is a dependency boundary, not a planner scaffold failure |
| Node test runner manifest boundary | `workspace_node_test_without_manifest_routes_test_to_dependency_boundary`, `node_test_runner_missing_manifest_setup_blocked_records_lifecycle`, and `node_test_runner_setup_allowed_then_test_rerun_records_lifecycle` cover test artifact present, missing runner manifest, setup authority, deterministic manifest materialization, and test rerun | source lifecycle restored for Node verifier binding |
| setup-only / manifest-only false positive | `manifest_only_nextjs_build_verify_is_not_success`, `minimal_loop_nextjs_required_paths_only_does_not_complete`, and `plan_run_nextjs_game_setup_only_fails_inferred_obligation` continue to reject setup-only or manifest-only output as task success | correct false-positive rejection preserved |
| G-S08 verifier command policy | `verify_command_rejects_shell_control_syntax`, `verify_command_rejects_install_or_dev_server`, `rust_manifest_path_escape_rejected`, `planner_verify_normalization_splits_only_allowlisted_and_commands`, and `planner_verify_normalization_rejects_unsafe_shell_syntax` were rerun in full `cargo test` | source verifier command policy parity re-audited |

## UAT004-GATE-04 Executable Fixture Binding

| baseline fixture | executable assertion | expected classification |
| --- | --- | --- |
| F-002 recovery YAML saved after failed run | `step_repair_no_change_is_classified_and_handoff_saved`, `ultra_phase_scaffold_failure_saves_recovery_yaml_and_incomplete_handoff`, and `ultra_final_acceptance_repair_failure_saves_recovery_handoff` assert recovery `.md`, recovery UltraPlan YAML, suggested commands, and `Status: incomplete` | correct handoff preserved; saved recovery artifacts are not original task success |
| F-002 recovery YAML must be executable | `saved_recovery_ultra_plan_can_drive_fixture_recovery_success` parses a saved recovery YAML and executes it through `run_ultra_plan` to create and verify the missing entrypoint | regression fixed: recovery handoff is tied to a targeted recovery run gate |
| F-002 repair no-progress / no-change | `step_repair_no_change_is_classified_and_handoff_saved`, `step_repair_target_not_followed_is_classified_and_handoff_saved`, and `step_repair_unrelated_change_is_classified_and_handoff_saved` emit dedicated failure kinds instead of success | correct failure detection preserved |
| F-006 scaffold-only interactive app | `deterministic_profile_fallback_requires_targeted_continuation_before_success`, `profile_auto_repair_continuation_prompt_treats_scaffold_as_incomplete`, and `plan_run_nextjs_game_scaffold_only_fails_inferred_capabilities` keep scaffold as continuation target and reject scaffold-only app completion | false-positive completion blocked |
| F-007 browser route failure | `browser_http_500_route_failure_targets_framework_config`, `browser_route_failure_targets_test_or_evidence`, `plan_run_nextjs_browser_http_500_fails_final_contract`, and `plan_run_nextjs_tailwind_dev_route_failure_keeps_failure_kind` fix browser route failures as repair/recovery targets | browser route failure cannot be hidden behind build/path success |

## UAT004-GATE-05 Executable Fixture Binding

| baseline fixture | executable assertion | expected classification |
| --- | --- | --- |
| F-004 completion contract disabled for interactive plan step | `plan_run_nextjs_interactive_app_records_partial_release_gate`, `plan_run_nextjs_browser_ready_without_interaction_is_partial`, and `minimal_loop::loop_run::tests::implement_plan_step_keeps_completion_contract_authority_enabled` prove interactive implementation cannot become release-grade success without final acceptance evidence | regression fixed: completion authority is connected to browser/interaction final acceptance |
| F-005 shallow interactive app accepted by static/source evidence | `title_only_output_does_not_satisfy_interactive_game_evidence`, `interactive_game_requires_dynamic_source`, `plan_run_nextjs_game_docs_only_fails_inferred_capabilities`, and eval false-positive fixtures keep title/static/docs/style/build-only app output below interactive acceptance | correct false-positive rejection preserved |
| F-006 scaffold/build-only app | `plan_run_nextjs_game_scaffold_only_fails_inferred_capabilities`, `deterministic_profile_fallback_requires_targeted_continuation_before_success`, and setup/build-only negative fixtures keep scaffold/setup/build-only artifacts as continuation or failure evidence, not final success | scaffold/build-only completion remains blocked |
| F-007 browser readiness and route failure | `nextjs_dev_route_probe_disabled_records_lifecycle_stages` records dev-server start/wait/probe/cleanup and probe environment in event plus evidence with browser unavailable as `partial`; `plan_run_nextjs_browser_http_500_fails_final_contract` and `plan_run_nextjs_tailwind_dev_route_failure_keeps_failure_kind` keep HTTP 500 as `failed` | browser unavailable and browser failure are separated; HTTP 500 cannot be masked by build success |
| release evidence content validation | `plan_run_nextjs_browser_and_interaction_evidence_passes_release_gate` covers the positive pass path, while `plan_run_nextjs_browser_ok_without_render_detail_is_partial`, malformed JSON eval fixtures, missing interaction eval fixtures, and parity gate tests reject path-only or malformed evidence | evidence path existence alone is not a pass |
| final acceptance failure lifecycle | `ultra_final_acceptance_failure_runs_bounded_repair`, `ultra_final_acceptance_repair_failure_saves_recovery_handoff`, `run_lifecycle_does_not_mask_browser_http_500_release_failure_as_complete`, and `run_lifecycle_does_not_mask_partial_release_gate_as_complete` connect final acceptance failure to repair/recovery/diagnostics instead of completion | correct failure detection preserved |

## UAT004-GATE-06 Executable Fixture Binding

| baseline fixture | executable assertion | expected classification |
| --- | --- | --- |
| F-001 phase context continuity needs source trace | `workspace/mvp/uat/004/gate06_trace/runtime-semantics-trace-diff.json` compares source and MVP same-condition normalized traces; G-S05 is `pass` only when phase context trace exists on both sides and no semantic finding is present | source parity proven for GATE-06 scope; trace absence remains failure |
| F-001 step prompt contract needs source trace | The same GATE-06 diff compares source prompt log observations with MVP `step_prompt_contract`; G-S06 is `pass` only when overall goal, expected paths, verify commands, and expected result are present | source parity proven for GATE-06 scope; prompt-field absence remains failure |
| missing phase context must fail | `test_compare_reports_detects_missing_phase_context` marks G-S05 `semantic_trace_contract_missing` with `missing_context` | correct failure detection preserved |
| missing expected result / verify must fail | `test_compare_reports_detects_missing_expected_result_and_verify` marks G-S06 `semantic_trace_contract_missing` with `missing_expected_result` and `missing_verify` | correct failure detection preserved |
| source prompt trace must be runtime evidence | `test_source_anvildev_llm_prompts_produce_phase_and_step_trace` proves source `llm-io.jsonl` request prompts are normalized into G-S05/G-S06 trace events; `test_compare_reports_does_not_pass_gate_counts_without_prompt_trace` and `test_eval_preflight_writes_comparative_parity_gate_report` require normalized phase/step events instead of gate counts only | regression fixed: gate counts alone cannot satisfy phase/prompt trace comparison |

## UAT004-GATE-07 Executable Fixture Binding

| baseline fixture | executable assertion | expected classification |
| --- | --- | --- |
| provider/tool args trace missing | `provider_probe_openai_tool_args_shape_skips_without_key`, `provider_probe_gemini_function_calling_schema_skips_without_key`, `provider_probe_ollama_xml_fallback_tool_like_output`, and `provider_probe_parser_fixtures_cover_tool_argument_shapes` produced `provider-probe-live.jsonl` with 7 passed / 0 failed / 0 skipped | G-S15 provider behavior is observed, not closed by fake fixtures only |
| recoverable provider args | `provider_probe_tool_args_recovery_classification_by_provider` parses OpenAI/Gemini/Ollama-shaped `Write` aliases and executes them through `ToolRegistry` | recoverable alias/wrapper/json drift is accepted only after policy validation |
| unsafe provider path args | the same fixture parses `../secret.txt` provider-shaped `Write` args and rejects them as `path_confinement_error`, with `recoverable_tool_error=false` | unsafe workspace escape remains a hard rejection |
| unsafe provider shell args | the same fixture parses provider-shaped Bash `cmd` with `curl ... | sh` and rejects it as `dangerous_command`, with `recoverable_tool_error=false` | shell-control / installer pipe remains a hard rejection |
| source XML fallback parity | `providers::xml_fallback` tests cover source-style named tags, `<function=...>`, inferred unambiguous tool names, closed unterminated blocks, relaxed JSON, and ambiguous inference rejection | source-style Ollama/XML fallback is recovered without loosening unsafe execution policy |
| eval summary attachment | `test_provider_probe_results_are_recorded_in_dry_run_summary` verifies probe result paths, status, skipped count, recoverable count, unsafe path rejection, and unsafe shell-control rejection are recorded | provider probe skip/pass/fail evidence is connected to gate reporting |

## UAT004-GATE-08 Executable Fixture Binding

| baseline fixture | executable assertion | expected classification |
| --- | --- | --- |
| F-003 summary incomplete plus final complete | `run_lifecycle_records_incomplete_stop_reason`, `run_lifecycle_does_not_mask_partial_release_gate_as_complete`, `run_lifecycle_does_not_mask_browser_http_500_release_failure_as_complete`, and `tui_slash_failure_records_run_events_and_failure_stage` assert failed/partial lifecycle summaries do not end as unconditional `Status: complete` | regression fixed: command/process completion is separated from task/release status |
| F-003 REPL ready overwrites task failure | `tui_slash_failure_records_run_events_and_failure_stage` asserts failed slash command events contain `task_status=failed`, `session_status=repl_ready`, and `repl_status=ready`, while summary shows `Task status: failed` and `Session/REPL status: repl_ready` | correct projection: REPL readiness is session status, not task completion |
| F-002 recovery YAML path must remain visible | `tui_slash_success_with_partial_release_gate_is_not_complete_only` asserts `recovery_ultra_plan_path` and suggested YAML command remain in `tui_command_stop` events, rendered TUI output, and summary `Recovery handoff` block | recovery handoff path is machine-readable and user-visible |
| G-S14 source diagnostics comparison | `test_source_and_mvp_diagnostics_trace_can_pass_gs14` normalizes source `agent.safe_stop.report` and MVP `run_stop` diagnostics, then compares reports with G-S14 `pass` | source diagnostics parity is based on trace evidence, not code reading alone |
| G-S16 manual trace registration | `test_run_start_without_eval_override_is_manual_trace_evidence` and `gate08_trace/manual_tui_run/.anvil/runs/gate08-manual/events.jsonl` prove manual/TUI run events are registered under `.anvil/runs/<run-id>/events.jsonl` | silent exit or missing manual trace remains a gate failure |

## Fixture Reproduction Notes

No new executable tests are created in GATE-00. The fixed baseline is document-level and must be converted into targeted automated fixtures in later gates.

The following checks were used to inspect the baseline:

- `wc -l` on `events.jsonl` confirmed 249 event lines.
- `jq` selected `step_obligation_scope` and `step_capability_evidence_check` events.
- `rg` checked generated `page.tsx` for input, projectile, collision, score, storage, audio, and touch handler signals.
- `sed` inspected `summary.md`, UltraPlan YAML, recovery UltraPlan YAML, `page.tsx`, and `globals.css`.

## Rollback Guard

Do not merge a later change that makes any of these fixture failures disappear by weakening the gate:

- accepting invalid planner output as success,
- allowing setup/scaffold/style/build-only output to count as a completed interactive app/game,
- hiding browser/interaction evidence gaps,
- dropping recovery handoff evidence,
- removing concrete failure kinds,
- showing REPL readiness as task completion.

## UAT004-GATE-09 Release Fixture Binding

| fixture / evidence | GATE-09 assertion | classification |
| --- | --- | --- |
| `gate09_release/test0701_005.original-events.jsonl` | Original manual TUI event stream still contains `tui_command_stop ok=false` and is release-blocking evidence in `parity_gate_report.json` | correct failure evidence preserved; REPL/process readiness is not release success |
| `gate09_release/test0701_005.original-summary.md` | Original summary remains non-passing UAT evidence: incomplete task plus failed TUI command is not hidden by a later complete/REPL tail | diagnostics regression remains guarded by G-S14/G-S16 |
| `gate09_release/browser-readiness.json` | Copied test0701_005 app served HTTP 200 and route rendered under `env -u NODE_ENV npm run dev` | positive browser route evidence, not sufficient alone for release pass |
| `gate09_release/interaction-evidence.json` | Playwright clicked `DIFF 1` and observed visible state change to canvas | positive basic interaction evidence, still not sufficient while TUI/comparative gates fail |
| `gate09_release/dev-server-events.jsonl` | Manual UAT recorded dev-server start, wait, probe, browser interaction probe, and cleanup | dev server lifecycle evidence must remain visible |
| `gate09_release/mvp-provider-smoke-live.summary.eval.tsv` | MVP release binary failed provider-smoke with `verify_repair_no_change` and `release_gate_status=failed` after producing an incorrect artifact | correct failure detection preserved, but source comparison shows artifact quality gap |
| `gate09_release/anvildev-provider-smoke-live.summary.eval.tsv` | Same-condition `anvildev --engine minimal` passed and produced the accepted artifact | MVP is below source for accepted artifact quality |
| `gate09_release/recovery-run.events.jsonl` and `.summary.md` | Saved recovery YAML was executed, then failed concretely as `phase_scaffold_error` / `verify command may not use shell control syntax` | recovery handoff is executable evidence, not success |
| `gate09_release/parity_gate_report.json` | Release report is schema-valid, has no blank failure kind violations, and remains open/fail with G-S02/G-S12/G-S14 pass and the other gates failed/reopened | comparative release gate is not passed; rollback must not re-allow false positives |

GATE-09 separates the two outcomes:

- Preserved correct failure detection: MVP did not mark the bad `provider smoke ok.` artifact as full success, and recovery handoff did not become success.
- Remaining runtime regression / release gap: anvildev produced exactly `provider smoke ok`, while MVP produced punctuation and then no-change repair. This is recorded as `release_quality_blocker_detected`, not as an intentional non-port candidate.
