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

## Re-run Notes

Before closing an implementation gate, update:

- `source_first_gate_status.md` with gate status, evidence, remaining issue, next action, rollback condition.
- `source_mvp_trace_manifest.md` with source/MVP refs, trace or skip evidence, provider/browser/anvildev status.
- `source_runtime_module_inventory.md` with source authority applied and MVP implementation.
- `test0701_005_regression_fixture.md` when a baseline fixture gains executable assertions.
