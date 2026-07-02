# UAT004-GATE-08 TUI and Diagnostics Trace Report

作成日: 2026-07-02

## Scope

G-S14 diagnostics and G-S16 TUI/manual observability were verified with source safe-stop diagnostics, MVP TUI/run-stop diagnostics, and deterministic manual run evidence.

## Evidence

| item | path |
| --- | --- |
| source trace report | `workspace/mvp/uat/004/gate08_trace/source/runtime-semantics-trace-report.json` |
| MVP trace report | `workspace/mvp/uat/004/gate08_trace/mvp/runtime-semantics-trace-report.json` |
| normalized diff | `workspace/mvp/uat/004/gate08_trace/runtime-semantics-trace-diff.json` |
| manual TUI events | `workspace/mvp/uat/004/gate08_trace/manual_tui_run/.anvil/runs/gate08-manual/events.jsonl` |
| manual TUI summary | `workspace/mvp/uat/004/gate08_trace/manual_tui_run/.anvil/runs/gate08-manual/summary.md` |

## Result

| gate | result | reason |
| --- | --- | --- |
| G-S14 | pass | source and MVP both observed `diagnostic_emitted` |
| G-S16 | pass | source and MVP both observed `manual_tui_trace_recorded` |

The scoped diff also marks unrelated gates as unobserved. That is expected for this GATE-08-only trace; those gates are covered by earlier gate artifacts or remain assigned to GATE-09.

## Source Parity Notes

- Source `agent.safe_stop.report` is normalized as diagnostics with stop reason, failure type, blocker/authority status, and next user action.
- MVP `tui_command_stop` and `run_stop` now carry command status, task status, session/REPL status, and recovery next action separately.
- Failed TUI command summaries show `Status: incomplete`, `Task status: failed`, and `Session/REPL status: repl_ready`; REPL readiness no longer implies task completion.
- Recovery UltraPlan YAML path and suggested YAML command remain visible in summary and machine-readable in events.

## Verification

| command | result |
| --- | --- |
| `cargo test --manifest-path mvp/anvilminimal/Cargo.toml eval_events::tests -- --nocapture` | passed |
| `cargo test --manifest-path mvp/anvilminimal/Cargo.toml --test tui_integration -- --nocapture` | passed |
| `cargo test --manifest-path mvp/anvilminimal/Cargo.toml run_lifecycle -- --nocapture` | passed |
| `pytest mvp/anvilminimal/tests/eval/test_runtime_semantics_trace.py` | 13 passed |
| `cargo fmt --manifest-path mvp/anvilminimal/Cargo.toml -- --check` | passed |
| `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` | passed |
| `pytest mvp/anvilminimal/tests/eval` | 227 passed, 1 skipped |
