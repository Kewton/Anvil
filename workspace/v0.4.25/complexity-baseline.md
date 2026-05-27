# Complexity Baseline

This baseline was produced after adding `scripts/complexity_report.py`.
The report is intentionally approximate and non-blocking. Its purpose is to
make structural hotspots visible and prevent future refactors from relying on
manual one-off measurements.

## Command

```bash
python3 scripts/complexity_report.py --top 20 \
  src/agent/loop_run/turn.rs \
  src/agent/loop_run/repair_job.rs \
  src/agent/loop_run/active_job_arbiter.rs \
  src/agent/loop_run/progress_text.rs \
  src/agent/loop_run/tool_history.rs
```

## Current Focused Baseline

| File | Functions | Avg Rough CC | Max Rough CC | CC >= 15 | CC >= 50 | Function LOC |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `src/agent/loop_run/turn.rs` | 1036 | 3.03 | 362 | 30 | 1 | 33288 |
| `src/agent/loop_run/repair_job.rs` | 236 | 2.37 | 26 | 3 | 0 | 5806 |
| `src/agent/loop_run/model_request.rs` | 9 | 3.11 | 8 | 0 | 0 | 153 |
| `src/agent/loop_run/active_job_arbiter.rs` | 79 | 2.01 | 11 | 0 | 0 | 1064 |
| `src/agent/loop_run/tool_history.rs` | 23 | 3.09 | 11 | 0 | 0 | 373 |
| `src/agent/loop_run/progress_text.rs` | 25 | 2.36 | 9 | 0 | 0 | 202 |

## Top Hotspots

| Rough CC | LOC | Line | Function |
| ---: | ---: | ---: | --- |
| 362 | 2836 | 5532 | `turn.rs::run_actor_loop` |
| 42 | 300 | 10791 | `turn.rs::build_request_messages` |
| 40 | 197 | 10193 | `turn.rs::request_assistant_reply_with_retry` |
| 38 | 459 | 8369 | `turn.rs::run_task_contract_verifier_once` |
| 31 | 255 | 13557 | `turn.rs::execute_tool_call` |
| 30 | 321 | 12776 | `turn.rs::run_verifier_repair_pass_and_apply` |
| 28 | 42 | 1175 | `turn.rs::answer_only_script_command_allowed` |
| 26 | 43 | 578 | `repair_job.rs::rejected_reason_for_repair_error` |
| 25 | 347 | 8913 | `turn.rs::drive_task_contract_verifier` |
| 25 | 239 | 9430 | `turn.rs::drive_repair_job_verifier` |
| 24 | 71 | 2311 | `turn.rs::verifier_repair_pass_retry_message` |
| 22 | 331 | 11977 | `turn.rs::run_verifier_diagnostic_pass` |
| 21 | 117 | 10391 | `turn.rs::request_assistant_reply` |
| 21 | 68 | 2411 | `turn.rs::verifier_file_excerpt_for_line` |
| 21 | 38 | 1712 | `turn.rs::classify_verifier_timeout` |

## Interpretation

- The current dominant risk remains `run_actor_loop`.
- The extracted support modules are not currently the main complexity source.
- The report confirms the next code work should target `turn.rs` boundaries,
  especially request/message, verifier, repair, and tool execution paths.
- This baseline should be used to detect regressions, not as an immediate CI
  failure threshold.

## Phase Status

- Phase 1 is implemented as a non-blocking local script.
- Phase 3 has started by extracting focused-edit request sizing,
  streaming-transport selection, non-streaming timeout selection, and
  non-streaming request execution into `model_request.rs`.
- CI gating is intentionally deferred until the report has been stable across
  a few refactors.
- The next implementation phase should extract message composition.
