# UAT004-GATE-07 Provider Probe Report

作成日: 2026-07-02

## Scope

G-S07 tool execution policy and G-S15 provider behavior were verified with source refs, MVP fixtures, and provider probe evidence.

## Evidence

| item | path |
| --- | --- |
| provider probe JSONL | `workspace/mvp/uat/004/gate07_provider_probe/provider-probe-live.jsonl` |
| provider probe summary | `workspace/mvp/uat/004/gate07_provider_probe/eval-summary/provider_probe_summary.json` |
| eval summary | `workspace/mvp/uat/004/gate07_provider_probe/eval-summary/summary.eval.tsv` |
| eval events | `workspace/mvp/uat/004/gate07_provider_probe/eval-summary/events.jsonl` |

## Result

| metric | value |
| --- | ---: |
| provider probe passed | 7 |
| provider probe failed | 0 |
| provider probe skipped | 0 |
| recoverable tool args classified | 3 |
| unsafe path args rejected | 3 |
| unsafe shell-control args rejected | 3 |

OpenAI live `tool_args_shape`, Gemini live `function_calling_schema`, and Ollama XML fallback probe all passed. API keys were available, so no provider skip reason was needed in this run.

The first provider probe attempt failed under sandbox network restrictions. It was rerun with network approval and passed; the sandbox failure is not classified as provider behavior.

## Source Parity Notes

- Source `ollama/xml_fallback.rs` accepts source-style XML/function call variants and relaxed JSON shapes.
- MVP `providers/xml_fallback.rs` now covers named tags, `<function=...>`, inferred unambiguous tool names, closed unterminated blocks, and relaxed JSON.
- Unsafe provider-shaped args remain execution-policy failures: `../secret.txt` is `path_confinement_error`, and `curl ... | sh` is `dangerous_command`.

## Verification

| command | result |
| --- | --- |
| `cargo fmt --manifest-path mvp/anvilminimal/Cargo.toml -- --check` | passed |
| `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` | passed |
| `pytest mvp/anvilminimal/tests/eval` | 226 passed, 1 skipped |
