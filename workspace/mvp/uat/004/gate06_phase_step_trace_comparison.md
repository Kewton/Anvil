# UAT004-GATE-06 Phase / Step Trace Comparison

作成日: 2026-07-02

## Scope

G-S05 phase context continuity and G-S06 step prompt construction were verified by same-condition normalized trace comparison. Source evidence comes from runtime `llm-io.jsonl` prompt logs, not from code reading alone.

## Trace Inputs

| item | path |
| --- | --- |
| source run root | `/private/tmp/anvilminimal-eval-0229-anvildev-net-timeout` |
| MVP run root | `/private/tmp/anvilminimal-eval-0229-mvp-net-timeout` |
| source report | `workspace/mvp/uat/004/gate06_trace/source/runtime-semantics-trace-report.json` |
| MVP report | `workspace/mvp/uat/004/gate06_trace/mvp/runtime-semantics-trace-report.json` |
| normalized diff | `workspace/mvp/uat/004/gate06_trace/runtime-semantics-trace-diff.json` |

## Result

| gate | result | source count | MVP count | semantic findings |
| --- | --- | ---: | ---: | --- |
| G-S05 | `pass` | 94 | 75 | 0 |
| G-S06 | `pass` | 100 | 141 | 0 |

Same-condition signature comparison matched: source 48 signatures and MVP 48 signatures, with no missing or extra signatures.

The overall diff still lists unrelated failed gates outside G-S05/G-S06. They do not change the GATE-06 decision.

## Detection Fixtures

| detection | fixture |
| --- | --- |
| source prompt logs produce phase and step trace | `test_source_anvildev_llm_prompts_produce_phase_and_step_trace` |
| missing phase context fails G-S05 | `test_compare_reports_detects_missing_phase_context` |
| missing expected result / verify fails G-S06 | `test_compare_reports_detects_missing_expected_result_and_verify` |
| trace omission cannot pass comparative preflight | `test_compare_reports_does_not_pass_gate_counts_without_prompt_trace`; `test_eval_preflight_writes_comparative_parity_gate_report` fixture includes normalized G-S05/G-S06 events |

## Verification

| command | result |
| --- | --- |
| `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` | passed |
| `pytest mvp/anvilminimal/tests/eval` | 226 passed, 1 skipped |
