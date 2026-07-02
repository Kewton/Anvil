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

## Re-run Notes

Before closing an implementation gate, update:

- `source_first_gate_status.md` with gate status, evidence, remaining issue, next action, rollback condition.
- `source_mvp_trace_manifest.md` with source/MVP refs, trace or skip evidence, provider/browser/anvildev status.
- `source_runtime_module_inventory.md` with source authority applied and MVP implementation.
- `test0701_005_regression_fixture.md` when a baseline fixture gains executable assertions.
