# UAT004 Source-first Gate Runbook

作成日: 2026-07-02

## Purpose

UAT004 の各 gate で、実行した検証、skip evidence、rollback 条件、次回再実行手順を同じ形式で残す。

## Common Commands

| command | when | expected handling |
| --- | --- | --- |
| `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` | each implementation gate | must pass before the gate is closed, unless a concrete blocker is recorded |
| `pytest mvp/anvilminimal/tests/eval` | each implementation gate with eval fixtures | must pass before the gate is closed, unless a concrete blocker is recorded |
| targeted `cargo test ... <fixture_name>` | before full test runs | records the positive/negative fixture tied to the gate |
| provider probe | only for provider/prompt-sensitive changes or UAT004-GATE-07/GATE-09 | skip is allowed when no provider-sensitive behavior changed or credentials/network are unavailable |
| browser/manual UAT | GATE-05/GATE-08/GATE-09 | skip is allowed outside those gates with explicit impact and next timing |

## UAT004-GATE-02 Run Record

| item | result |
| --- | --- |
| source refs | `src/agent/minimal_step_runner.rs`, `profile.rs`, `plan_lint.rs`, `verify.rs`, `repair.rs`, `profiles/nextjs.rs` |
| MVP refs | `mvp/anvilminimal/src/planner/lint.rs`, `mvp/anvilminimal/src/planner/runner.rs` |
| targeted fixtures | `workspace_manifest_and_entrypoint_allow_final_nextjs_verify`, `workspace_manifest_without_entrypoint_still_rejects_nextjs_build`, `generated_final_verify_uses_existing_workspace_nextjs_artifacts`, `invalid_ultra_plan_generation_does_not_save_plan_file` |
| full local verification | `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` passed; `pytest mvp/anvilminimal/tests/eval` passed with 222 passed / 1 skipped |
| provider probe | skipped; no provider request schema, parser, tool-call shape, or prompt text changed |
| browser/manual UAT | skipped; final browser readiness and interaction acceptance remain G-S12/G-S16 |
| anvildev comparison | skipped; final comparative source/MVP run remains GATE-09 |
| rollback guard | do not accept deterministic fallback UltraPlan as success; do not reject final verify solely for plan-local entrypoint re-ownership when manifest+entrypoint exist; do not relax shell control rejection |

## UAT004-GATE-03 Run Record

| item | result |
| --- | --- |
| source refs | `src/agent/loop_run/node_request_helpers.rs`, `node_runner_manifest.rs`, `project_probe.rs`, `project_verifier.rs`, `verifier_command_policy.rs`, `node_test_evidence_quality.rs`, `actor_loop_flow.rs` |
| MVP refs | `mvp/anvilminimal/src/minimal_loop/dependency_setup.rs`, `minimal_loop/build_verifier.rs`, `planner/verify.rs`, `planner/lint.rs`, `planner/runner.rs` |
| targeted fixtures | `workspace_entrypoint_without_manifest_routes_build_to_dependency_boundary`, `workspace_node_test_without_manifest_routes_test_to_dependency_boundary`, `next_build_without_manifest_reports_manifest_boundary_before_execution`, `nextjs_build_missing_manifest_is_dependency_boundary_not_command_execution`; existing setup blocked/allowed/build rerun and G-S08 verify policy fixtures rerun in full test suite |
| full local verification | `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` passed with 438 lib tests plus integration/doc tests; `pytest mvp/anvilminimal/tests/eval` passed with 222 passed / 1 skipped |
| network install | skipped; no real network install was executed, and setup remains gated by authority/workspace/profile contract |
| browser/manual UAT | skipped; final browser readiness and interaction acceptance remain G-S12/G-S16 |
| anvildev comparison | skipped; final comparative source/MVP run remains GATE-09 |
| rollback guard | do not execute dependency setup without authority; do not let setup-only, manifest-only, build-only, or dependency handoff-only output become task success; do not relax verifier shell-control/workspace-escape rejection |

## UAT004-GATE-04 Run Record

| item | result |
| --- | --- |
| source refs | `src/agent/loop_run/repair_job.rs`, `repair_lifecycle.rs`, `repair_job_dispatch.rs`, `verifier_repair_targeting.rs`, `scaffold_pipeline.rs`, `no_progress_recovery.rs`, `safe_stop_emit.rs`, `safe_stop_payload.rs`, `actor_loop_flow.rs`, `verifier_driver.rs`, `src/agent/minimal_step_runner/repair.rs` |
| MVP refs | `mvp/anvilminimal/src/minimal_loop/repair_target.rs`, `minimal_loop/repair_progress.rs`, `minimal_loop/loop_run.rs`, `planner/repair.rs`, `planner/runner.rs`, `eval_events.rs` |
| targeted fixtures | `browser_http_500_route_failure_targets_framework_config`, `browser_route_failure_targets_test_or_evidence`; existing missing-entrypoint, no-change, target-not-followed, unrelated-change, scaffold-continuation, recovery-handoff, and saved-recovery-run fixtures rerun in full cargo |
| full local verification | `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` passed with 440 lib tests plus integration/doc tests; `pytest mvp/anvilminimal/tests/eval` passed with 222 passed / 1 skipped |
| source/anvildev comparison | skipped; source `RepairJob` was inspected but not run under a same-condition anvildev trace. Comparative normalized source/MVP lifecycle trace remains GATE-09. |
| browser/manual UAT | skipped; browser route failures are covered by deterministic fixtures, while release-grade browser/interaction execution remains GATE-05/GATE-09 |
| provider probe | skipped; no provider request schema, parser, tool-call shape, or prompt text changed |
| rollback guard | do not allow no-change repair, target-misdirected repair, scaffold-only output, or recovery handoff persistence to become task success; browser route failures must remain repair/recovery targets |

## UAT004-GATE-05 Run Record

| item | result |
| --- | --- |
| source refs | `src/agent/loop_run/verifier.rs`, `verifier_driver.rs`, `project_probe.rs`, `project_verifier.rs`, `task_contract_completion_policy.rs`, `actor_loop_flow.rs`, `emit_verifier_events.rs` |
| MVP refs | `mvp/anvilminimal/src/planner/runner.rs`, `minimal_loop/evidence.rs`, `minimal_loop/completion.rs`, `minimal_loop/build_verifier.rs`, `planner/profiles/nextjs.rs`, `scripts/eval_lib/browser_oracle.py`, `scripts/eval_lib/parity_gate.py` |
| implementation | `planner/runner.rs` now writes dev-server lifecycle stages and probe environment into `dev_server_lifecycle` events and browser readiness evidence; final acceptance continues to require valid browser readiness plus interaction evidence content for release-grade pass |
| targeted fixtures | `nextjs_dev_route_probe_disabled_records_lifecycle_stages`, `plan_run_nextjs_interactive_app_records_partial_release_gate`, `plan_run_nextjs_browser_http_500_fails_final_contract`, `plan_run_nextjs_browser_ready_without_interaction_is_partial`, `plan_run_nextjs_browser_and_interaction_evidence_passes_release_gate`; title/static/build-only, malformed evidence, missing interaction, and final acceptance repair/recovery fixtures rerun in full suites |
| full local verification | `cargo fmt --manifest-path mvp/anvilminimal/Cargo.toml -- --check` passed; `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` passed with 440 lib tests plus integration/doc tests; `pytest mvp/anvilminimal/tests/eval` passed with 222 passed / 1 skipped |
| browser/manual UAT | live browser/manual execution skipped; browser/Playwright/dev-server must not be required by normal unit tests. Deterministic fixtures cover unavailable/failed/pass classification and evidence content. |
| source/anvildev comparison | skipped; final comparative source/MVP browser and manual release trace remains GATE-09 |
| rollback guard | do not allow build-only/title-only/static-only output, evidence path existence alone, malformed evidence, missing interaction evidence, browser unavailable, or browser HTTP 500 to become release-grade full success; keep browser unavailable `partial` and HTTP 500 `failed` |

## UAT004-GATE-06 Run Record

| item | result |
| --- | --- |
| source refs | `src/agent/minimal_step_runner.rs`, `src/agent/minimal_step_runner/profile.rs`, `src/agent/minimal_step_runner/repair.rs` |
| MVP refs | `mvp/anvilminimal/scripts/eval_lib/runtime_trace.py`, `mvp/anvilminimal/tests/eval/test_runtime_semantics_trace.py`, `mvp/anvilminimal/tests/eval/test_eval_cli_contract.py` |
| implementation | `runtime_trace.py` normalizes source runtime prompt logs from `.anvil/state/sessions/*/logs/llm-io.jsonl` into phase-context and step-prompt trace events, keeps source/MVP prompt contract booleans in normalized events, fails G-S05/G-S06 for missing semantic fields, and treats trace absence as non-pass. |
| trace commands | `python3 mvp/anvilminimal/scripts/eval-trace.py --run-root /private/tmp/anvilminimal-eval-0229-anvildev-net-timeout --subject source-anvildev ... --output-dir workspace/mvp/uat/004/gate06_trace/source`; same command for `/private/tmp/anvilminimal-eval-0229-mvp-net-timeout`; then `--compare-source-report ... --compare-mvp-report ... --diff-output workspace/mvp/uat/004/gate06_trace/runtime-semantics-trace-diff.json` |
| normalized comparison | `same_condition.status=match`, source signatures 48, MVP signatures 48, `semantic_findings=[]`; G-S05 source/MVP counts 94/75 and G-S06 source/MVP counts 100/141 both pass with `source_and_mvp_gate_observed` |
| targeted fixtures | `test_source_anvildev_llm_prompts_produce_phase_and_step_trace`, `test_compare_reports_detects_missing_phase_context`, `test_compare_reports_detects_missing_expected_result_and_verify`, `test_compare_reports_does_not_pass_gate_counts_without_prompt_trace`, and comparative preflight fixture update for normalized G-S05/G-S06 events |
| full local verification | `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` passed with 440 lib tests plus integration/doc tests; `pytest mvp/anvilminimal/tests/eval` passed with 226 passed / 1 skipped |
| provider/browser/manual UAT | skipped; GATE-06 uses existing same-condition runtime traces and does not require new provider calls, browser readiness, or manual TUI execution |
| remaining unrelated gates | overall diff still has unrelated failed gate ids outside G-S05/G-S06; they remain assigned to later UAT004 gates |
| rollback guard | do not allow trace-missing reports, missing phase context, missing expected result, or missing verify commands to pass G-S05/G-S06; do not remove source prompt-log normalization unless source emits equivalent native runtime trace events |

## UAT004-GATE-07 Run Record

| item | result |
| --- | --- |
| source refs | `src/ollama/client.rs`, `src/ollama/xml_fallback.rs`, `src/agent/minimal_step_runner.rs`, `src/agent/minimal_step_runner/repair.rs` |
| MVP refs | `mvp/anvilminimal/src/providers/xml_fallback.rs`, `providers/openai.rs`, `providers/gemini.rs`, `providers/ollama.rs`, `tools/args_recovery.rs`, `tools/registry.rs`, `tests/live_provider.rs`, `scripts/eval-run.py` |
| implementation | MVP XML fallback now accepts source-style XML/function tag variants and relaxed JSON while preserving tool policy enforcement; provider probe summary now records unsafe shell-control rejection. |
| provider probe | `ANVIL_PROVIDER_PROBE=1 ANVIL_PROVIDER_PROBE_OUT=/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/uat/004/gate07_provider_probe/provider-probe-live.jsonl cargo test --manifest-path mvp/anvilminimal/Cargo.toml --test live_provider provider_probe -- --nocapture` passed with 5 provider probe tests. Initial sandbox network failure was rerun with network approval. |
| provider probe result | `provider_probe_summary.json` status `passed`: 7 passed, 0 failed, 0 skipped; OpenAI live tool args shape passed, Gemini live function-calling schema passed, Ollama XML fallback passed, recoverable provider args 3, unsafe path rejection 3, unsafe shell-control rejection 3. |
| eval attachment | `python3 scripts/eval-run.py --suite eval/suites/mvp-provider-smoke.yaml --model-profile openai-only --modes minimal-loop --runs 1 --scenario write-one-file-small --run-root .../gate07_provider_probe/eval-summary --provider-probe-results .../provider-probe-live.jsonl --dry-run` wrote summary/events with provider probe status `passed`. |
| targeted fixtures | `providers::xml_fallback`, `provider_probe_tool_args_recovery_classification_by_provider`, `test_provider_probe_results_are_recorded_in_dry_run_summary` |
| full local verification | `cargo fmt --manifest-path mvp/anvilminimal/Cargo.toml -- --check` passed; `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` passed with 446 lib tests plus integration/doc tests; `pytest mvp/anvilminimal/tests/eval` passed with 226 passed / 1 skipped |
| skip evidence | no provider probe skip in this run; API keys were available. Future no-key runs must record `missing_openai_api_key` / `missing_gemini_api_key` skip reasons instead of failing normal unit tests. |
| full source port escalation | not required for G-S07/G-S15; no provider/tool diff remained after XML fallback parity and live provider probe evidence. |
| rollback guard | do not treat provider parse/network/HTTP failures as runtime success; do not recover workspace escapes, hidden metadata access, or dangerous shell commands; do not close provider-sensitive changes with fake fixtures alone. |

## Re-run Notes

Before closing an implementation gate, update:

- `source_first_gate_status.md` with gate status, evidence, remaining issue, next action, rollback condition.
- `source_mvp_trace_manifest.md` with source/MVP refs, trace or skip evidence, provider/browser/anvildev status.
- `source_runtime_module_inventory.md` with source authority applied and MVP implementation.
- `test0701_005_regression_fixture.md` when a baseline fixture gains executable assertions.
