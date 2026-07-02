# UAT004 Source-first Gate Status

作成日: 2026-07-02

## Status Definitions

| status | meaning |
| --- | --- |
| `baseline_fixed` | GATE-00 recorded the current fixture/inventory/trace baseline. No implementation pass is claimed. |
| `fail` | Current matrix or UAT evidence shows missing semantics or false positive risk. |
| `pass_reaudit_required` | Previously marked pass, but source-first policy requires renewed source trace / fixture / UAT evidence. |
| `pending_source_trace` | MVP event or implementation exists, but source same-condition trace is missing. |
| `pending_live_probe` | Provider/browser/manual evidence is intentionally skipped in normal tests and must be collected in a later gate. |
| `source_parity_pass` | Scoped source-first semantics were restored or proven equivalent with source refs, MVP refs, targeted fixtures, and required local tests. |

## UAT004-GATE-00 Status

| item | status | evidence | remaining issue | next action |
| --- | --- | --- | --- | --- |
| failure fixture | `baseline_fixed` | `test0701_005_regression_fixture.md` records final phase scaffold failure, summary contradiction, disabled completion contract, shallow interactive app state | fixture is documentation-only; no executable test added in GATE-00 | convert into targeted positive/negative fixtures in implementation gates |
| source module inventory | `baseline_fixed` | `source_runtime_module_inventory.md` records all GATE-00 minimum source modules | source semantics not yet ported | use inventory per gate before changing MVP modules |
| gate status report skeleton | `baseline_fixed` | this file covers G-S01 through G-S16 | no gate promoted to pass by GATE-00 | update each gate with source refs, MVP refs, evidence, rollback condition |
| source/MVP trace manifest | `baseline_fixed` | `source_mvp_trace_manifest.md` records MVP baseline and skip evidence | source/anvildev trace not collected | collect same-condition trace in target gates |
| correct failure detection vs regression split | `baseline_fixed` | `test0701_005_regression_fixture.md` separates correct detection from regression | needs executable assertions later | add targeted fixtures without Space Invaders-specific string dependency |

## UAT004-GATE-01 Status

| gate | status | evidence | remaining issue | next action | rollback condition |
| --- | --- | --- | --- | --- | --- |
| G-S01 | `source_parity_pass` | Source refs read: `task_contract*.rs`, `task_contract_artifact_contract.rs`, `task_contract_deliverable_lifecycle.rs`, `evidence_binding.rs`, `artifact_ledger.rs`, `artifact_ownership.rs`, `artifact_target_alignment.rs`, `worker_contract.rs`. MVP changes: `minimal_loop/evidence.rs` now treats weak verifier evidence as blocking whenever source-first runtime requirements are present; `minimal_loop/loop_run.rs` emits step-level capability/evidence bindings from `completion_verify`; existing and added tests reject title-only, docs-only, style-only, scaffold-only, canvas/browser oracle failures, and artifact-only verifier completion. Verification: `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` passed; `pytest mvp/anvilminimal/tests/eval` passed with 222 passed / 1 skipped. | No same-condition anvildev/source runtime trace was executed in this gate; browser/provider evidence remains in later gates by design. | Carry G-S01 as closed for UAT004-GATE-01 and re-check in GATE-05/GATE-09 with browser/release evidence. | Revert or block any change that lets interactive app/game completion pass with path-only, title-only, style-only, docs-only, scaffold-only, or weak static verifier evidence. |
| G-S02 | `source_parity_pass` | `RunSessionOptions::plan_step(Implement)` now enables completion contract verification and path merge; generated or explicit `CompletionContract` path is injected into implement step config from `planner/runner.rs`, so `step_obligation_scope` for implement records `completion_contract_verification_enabled=true` and `completion_contract_path_merge_enabled=true`. Tests cover non-interactive file creation path-only success preservation, external contract required evidence failure at step level, and generated contract binding for interactive Next.js game steps. | Full source TaskContract graph is not byte-for-byte ported; this gate closes the scoped completion authority parity via equivalent MVP contract/evidence authority. Source trace execution remains skipped with this documented reason. | Keep G-S02 re-audit closed unless GATE-09 comparative UAT finds divergence; later gates must not use `completion_contract_verification_enabled=false` as release pass evidence for interactive implementation steps. | Preserve non-interactive required path completion while rejecting interactive false positives and weak verifier-only evidence. |

## UAT004-GATE-02 Status

| gate | status | evidence | remaining issue | next action | rollback condition |
| --- | --- | --- | --- | --- | --- |
| G-S03 | `source_parity_pass` | Source refs read: `src/agent/minimal_step_runner.rs`, `minimal_step_runner/profile.rs`, `minimal_step_runner/plan_lint.rs`, `minimal_step_runner/verify.rs`, `minimal_step_runner/repair.rs`, `minimal_step_runner/profiles/nextjs.rs`. MVP audit confirmed existing source-shaped UltraPlan/StepPlan prompts, schema-only output, corrective retry, lint retry, tool-call rejection, and invalid fail-fast path in `planner/runner.rs`. MVP changes route StepPlan generation and plan-file lint through `lint_step_plan_report_with_workspace`, matching source `lint_plan_with_workspace`. Fixtures: `generated_final_verify_uses_existing_workspace_nextjs_artifacts`, `invalid_ultra_plan_generation_does_not_save_plan_file`. Verification: `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` passed with 434 lib tests plus integration/doc tests; `pytest mvp/anvilminimal/tests/eval` passed with 222 passed / 1 skipped. | No live provider probe or same-condition anvildev trace was run. This gate did not change provider request schema or prompt text; provider probe is documented as not required for this local lint/fail-fast fix and remains available in GATE-07/GATE-09. | Keep G-S03 closed for local source parity; re-check under GATE-07 only if prompt/provider behavior changes, and under GATE-09 with comparative run evidence. | Do not reintroduce deterministic fallback UltraPlan as normal success, planner success without schema/lint pass, blank planner failure kind, or shell-control relaxation. |
| G-S04 | `source_parity_pass` | Source profile/build-order semantics from `minimal_step_runner/plan_lint.rs` and `profiles/nextjs.rs` are represented in MVP lint: final `npm run build` can use existing workspace `package.json` plus Next.js entrypoint, while package-only/no-entrypoint build still fails with `dependency_order`. Fixtures: `workspace_manifest_and_entrypoint_allow_final_nextjs_verify`, `workspace_manifest_without_entrypoint_still_rejects_nextjs_build`, and existing deterministic scaffold recovery fixture still records `used_for_completion=false`. Verification: `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` and `pytest mvp/anvilminimal/tests/eval` passed. | This closes UltraPlan/StepPlan generation and final verify plan validity only. Dependency setup/build rerun lifecycle, browser readiness, and final acceptance remain in G-S09/G-S12. | Carry G-S04 as source parity for GATE-02 scope; hand off dependency/setup/build execution to GATE-03 and browser/final acceptance to GATE-05. | A final verify phase with real manifest+entrypoint must not be rejected solely for lacking plan-local expected path re-ownership; package-only/no-entrypoint or invalid plan must still fail-fast. |

## Gate Status Skeleton

| gate | lifecycle stage | matrix status | UAT004 baseline status | classification | GATE-00 evidence / basis | remaining issue | next action | rollback condition |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| G-S01 | request understanding | fail | `fail` | `full_source_port_target` | `step_prompt_contract` contains required capabilities/evidence, but step completion still uses local required paths and disabled contract verification | Source TaskContract request inference and completion authority are not restored | UAT004-GATE-01: restore TaskContract chain or prove equivalent authority | do not allow docs-only, style-only, title-only, or path-only app completion |
| G-S02 | task contract | pass | `pass_reaudit_required` | `full_source_port_target` / pass re-audit | `step_obligation_scope` repeatedly shows `completion_contract_verification_enabled=false`; matrix says full TaskContract graph is unported | Existing pass cannot be fixed without source parity evidence | UAT004-GATE-01: reclassify to source parity pass or documented intentional difference | preserve non-interactive required path completion while rejecting interactive false positives |
| G-S03 | plan generation | pass | `pass_reaudit_required` | `full_source_port_target` / pass re-audit | planner retry/lint events exist; final phase still fails with verify/dependency ordering errors | source prompt/schema/retry/lint/fail-fast trace and provider evidence missing | UAT004-GATE-02: compare source/MVP planner behavior | do not reintroduce blank planner failure kind or deterministic fallback success |
| G-S04 | ultra phase generation | fail | `fail` | `full_source_port_target` | UltraPlan generation produced 5 phases, but final `build-and-verify` phase failed at scaffold after corrective retries | source phase generation and final verification semantics not restored | UAT004-GATE-02: source-first UltraPlan/StepPlan generation | invalid final verify plan cannot be accepted as success |
| G-S05 | phase context continuity | fail | `pending_source_trace` | `source_trace_verification_target` | MVP trace has `ultra_phase_context_attached` and `ultra_phase_context_updated` events | source same-condition trace missing | UAT004-GATE-06: normalized source/MVP trace comparison | do not lose prior phase outcome/failure/repair context |
| G-S06 | step prompt construction | fail | `pending_source_trace` | `source_trace_verification_target` | MVP step prompts record overall goal, verify commands, expected paths/result, final capabilities/evidence | source same-condition prompt trace missing | UAT004-GATE-06: compare prompt contract fields | do not remove contract fields from plan-run or ultra-plan-run prompts |
| G-S07 | tool execution policy | fail | `pending_live_probe` | `source_trace_verification_target` | no targeted provider/tool args fixture in GATE-00 | live/source provider trace missing | UAT004-GATE-07: provider probe and source XML/tool args trace | unsafe args, shell control, hidden metadata, and workspace escape must stay rejected |
| G-S08 | deterministic verify | pass | `pass_reaudit_required` | `full_source_port_target` / pass re-audit | final phase emitted `verify_dependency_order_error` and `verify_command_policy_error`; latest source/eval trace not attached | source verifier command policy parity and targeted eval missing | UAT004-GATE-03: verify policy fixture and source comparison | do not relax shell control or setup/build ordering to raise success rate |
| G-S09 | dependency/setup/build lifecycle | fail | `fail` | `full_source_port_target` | dependency/build lifecycle events observed, but final verify ordering rejects existing app state | setup authority/build rerun lifecycle not source-parity | UAT004-GATE-03: restore source dependency/setup/build lifecycle | build-only or manifest-only cannot pass final interactive acceptance |
| G-S10 | repair targeting | fail | `fail` | `full_source_port_target` | recovery handoff saved after final phase scaffold failure | RepairJob / verifier repair targeting source parity missing | UAT004-GATE-04: port or prove RepairJob lifecycle equivalence | no-change or target-misdirected repair must not be success |
| G-S11 | scaffold fallback | fail | `fail` | `full_source_port_target` | generated app and scaffold artifacts exist, but output is shallow and final phase failed | scaffold continuation/completion boundary not source-parity | UAT004-GATE-04: scaffold as continuation target | scaffold-only output cannot be final completion |
| G-S12 | final acceptance | fail | `fail` | `full_source_port_target` | browser/interaction evidence paths are blank; generated app lacks robust input, scoring, collision, win/lose/restart | final acceptance is not integrated as completion authority | UAT004-GATE-05: connect artifact/build/capability/browser/interaction evidence | full release pass requires actual acceptance evidence, not source tokens only |
| G-S13 | recovery handoff | fail | `fail` | `full_source_port_target` | recovery prompt and recovery YAML saved and parse OK | recovery run gate is not proven; saved handoff is not success | UAT004-GATE-04: recovery handoff through successful recovery path | recovery handoff persistence alone must never mark task success |
| G-S14 | diagnostics | pass | `pass_reaudit_required` | `source_trace_verification_target` / pass re-audit | command failure has concrete kind, but outer `run_stop` has blank `failure_kind` and summary tail says complete | source diagnostics trace and lifecycle projection parity missing | UAT004-GATE-08: normalized diagnostics trace | failed lifecycle stages cannot end with blank kind or misleading completion |
| G-S15 | provider behavior | fail | `pending_live_probe` | `source_trace_verification_target` | provider request/response events exist, but no targeted live provider probe | provider-specific parser/tool-call behavior unverified | UAT004-GATE-07: provider probe with skip evidence when credentials unavailable | fake fixtures alone cannot close provider/prompt-sensitive changes |
| G-S16 | TUI/manual run observability | fail | `fail` | `source_trace_verification_target` | summary has incomplete at top and complete/REPL at tail; `tui_command_stop ok=false` but `run_stop ok=true` | task status and REPL/process status are conflated | UAT004-GATE-08: TUI/manual trace and summary projection parity | no silent exit and no task failure overwritten by REPL ready |

## Gate Reference Index

| gate | source refs | MVP refs | baseline fixture / evidence |
| --- | --- | --- | --- |
| G-S01 | `task_contract*.rs`, `worker_contract.rs`, `artifact_ledger.rs` | `completion.rs`, `evidence.rs`, `loop_run.rs`, `runner.rs` | F-004, F-005, F-006 |
| G-S02 | `task_contract_core.rs`, `task_contract_completion_policy.rs`, `artifact_ledger.rs` | `completion.rs`, `evidence.rs`, `repair_target.rs`, `runner.rs` | F-004 |
| G-S03 | `minimal_step_runner.rs`, `minimal_step_runner/plan_lint.rs` | `planner/runner.rs`, `planner/step_plan.rs`, `planner/lint.rs` | F-001 |
| G-S04 | `minimal_step_runner.rs`, `minimal_step_runner/profile.rs`, `minimal_step_runner/profiles/nextjs.rs` | `planner/runner.rs`, `planner/ultra_plan.rs` | F-001 |
| G-S05 | `minimal_step_runner.rs`, `minimal_step_runner/profile.rs` | `planner/runner.rs` shared session/context events | `ultra_phase_context_attached`, `ultra_phase_context_updated` |
| G-S06 | `minimal_step_runner.rs`, step prompt builders | `StepPromptContext`, `planner/runner.rs` | `step_prompt_contract` |
| G-S07 | `tool_execution.rs`, `tool_call_execution.rs`, `tool_call_prepare.rs`, `src/ollama/xml_fallback.rs` | `tools/args_recovery.rs`, provider parsers | pending provider/tool fixture |
| G-S08 | `verifier*.rs`, `verifier_command_policy.rs`, `minimal_step_runner/verify.rs` | `planner/verify.rs`, `planner/lint.rs` | F-001 planner verify errors |
| G-S09 | `node_*`, `project_probe.rs`, `project_verifier.rs`, `minimal_step_runner/profiles/nextjs.rs` | `dependency_setup.rs`, `build_verifier.rs`, `planner/runner.rs` | F-001, F-007 |
| G-S10 | `repair_job.rs`, `repair_lifecycle.rs`, `repair_job_dispatch.rs`, `verifier_repair_targeting.rs` | `repair_target.rs`, `planner/repair.rs`, `planner/runner.rs` | F-002 |
| G-S11 | `scaffold_pipeline.rs`, `task_contract_recovery.rs` | `planner/profiles/nextjs.rs`, `completion.rs`, `runner.rs` | F-006 |
| G-S12 | `verifier.rs`, `verifier_driver.rs`, `task_contract_completion_policy.rs` | `evidence.rs`, `planner/runner.rs`, `acceptance_outcome.py` | F-004, F-005, F-006, F-007 |
| G-S13 | `minimal_step_runner/repair.rs`, `repair_lifecycle.rs`, `safe_stop_*` | `planner/repair.rs`, `planner/runner.rs` | F-002 |
| G-S14 | verifier event emitters, `summary.rs`, `safe_stop_payload.rs` | `eval_events.rs`, `runtime_trace.py`, `failure_classification.py` | F-003 |
| G-S15 | `src/ollama/client.rs`, `src/ollama/xml_fallback.rs`, provider handling | `providers/*`, `eval-run.py`, `live_provider.rs` | pending provider probe |
| G-S16 | `summary.rs`, `safe_stop_emit.rs`, `safe_stop_payload.rs` | `.anvil/runs`, `eval_events.rs`, `lib.rs`, `tui/slash.rs` | F-003 |

## Pass Gate Re-audit Record

| gate | prior status | UAT004 re-audit requirement | required evidence before final pass |
| --- | --- | --- | --- |
| G-S02 | pass | required | full TaskContract graph restored, or alternative completion authority proven by source trace, positive/negative fixture, and UAT evidence |
| G-S03 | pass | completed in GATE-02 | source/MVP planner prompt/schema/retry/lint/fail-fast comparison completed by source refs plus local fixtures; provider probe skipped because no provider request schema or prompt text changed |
| G-S08 | pass | required | verifier command policy positive/negative fixture, latest local eval, and source verifier command policy comparison |
| G-S14 | pass | required | normalized source/MVP diagnostics trace showing lifecycle stage, failure kind, recovery fields, and summary projection parity |

## Correct Failure Detection vs Regression

| category | baseline item | classification |
| --- | --- | --- |
| correct failure detection | final `build-and-verify` phase fails instead of being silently accepted | useful detection to preserve |
| correct failure detection | recovery prompt and recovery UltraPlan YAML are saved and parse OK | useful handoff, not success |
| correct failure detection | early capability checks report missing evidence | useful diagnostic surface |
| regression / source parity gap | plan-run step completion contract verification is disabled | must be fixed in G-S01/G-S02/G-S12 |
| regression / source parity gap | shallow interactive app can reach later phases with static/source evidence | must be rejected as false positive |
| regression / diagnostic gap | summary reports incomplete and then appends complete/REPL status | must be fixed in G-S14/G-S16 |
| regression / diagnostic gap | outer `run_stop` has `ok=true` and blank failure kind after command failure | must be fixed or explicitly projected as REPL-only status |
