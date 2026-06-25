# Provider Tool Call Cross Review

対象: `mvp/anvilminimal`

実施日: 2026-06-25

## 結論

今回の minimal-loop 全敗は、OpenAI Responses API の `function_call.arguments` shape と Gemini Interactions API の model/endpoint availability を unit test と preflight が代表できていなかったことが主因だった。

対策として、runtime 側は provider parser で tool arguments を object へ正規化し、recoverable な tool validation failure を loop feedback に変換する。eval 側は `ANVIL_EVAL_EVENTS` と `failure_kind` を導入し、provider/tool/postcheck の失敗を `process_failure` に潰さない。

## Provider Matrix

| provider | request path | native tools | arguments input accepted | malformed handling | events |
|---|---|---:|---|---|---|
| OpenAI | `/v1/responses` | yes | object, JSON encoded string | provider parse error | `provider_request`, `provider_response`, `provider_error`, `provider_parse_error` |
| Gemini | `/v1beta/interactions` | yes | object, JSON encoded string | provider parse error | `provider_request`, `provider_response`, `provider_error`, `provider_parse_error` |
| Ollama | `/api/chat` | no native tools in MVP path | XML fallback payload only | XML feedback / fallback | provider events not yet added |

## Tool Argument Handling

| shape | OpenAI | Gemini | registry |
|---|---|---|---|
| object | accepted | accepted | executed |
| JSON string object | decoded to object | decoded to object | executed after decode |
| JSON string array/scalar | provider parse error | provider parse error | not reached |
| malformed string | provider parse error | provider parse error | not reached |
| null/missing | provider parse error | provider parse error | not reached |
| object missing required arg | recoverable feedback | recoverable feedback | `tool_validation_error` |

## Hard vs Recoverable

| error | handling | reason |
|---|---|---|
| missing required arg | recoverable tool result feedback | model can retry with valid schema |
| unknown tool | recoverable tool result feedback | model can choose available tool |
| malformed provider arguments | provider parse error | registry cannot safely infer intent |
| path escape | hard error | workspace confinement violation |
| dangerous command | hard error | safety boundary |
| approval required | hard error in non-interactive eval | acceptance uses `--yes` |
| interrupted | hard error | explicit user/system intent |

## Eval Evidence

`ANVIL_EVAL_EVENTS` now records:

- provider request provider/model/tool count
- provider HTTP status and redacted body snippet on final failure
- provider parse error kind
- raw tool call name and argument shape
- tool validation error kind and missing arg name
- tool execution hard error kind

`eval-run.py` merges each run's `anvil-events.jsonl` into the harness `events.jsonl`, and writes `extras_json.failure_kind` into `summary.eval.tsv` for failed rows.

## Planner / Plan Run / Ultra Plan Run

`plan-run` and `ultra-plan-run` execute each step through `run_session_with_required_paths_with_ui`, so the same minimal-loop provider events and tool validation events are emitted during execution steps.

Planner-only calls use no native tools. Provider request/response/error events are still emitted by OpenAI/Gemini clients for planner calls.

## Residual Risks

| risk | impact | mitigation |
|---|---|---|
| Gemini Interactions model availability changes | cloud eval can fail before task execution | `eval-preflight.py --live-provider-smoke all` and `mvp-provider-smoke` gate |
| OpenAI/Gemini add new output variants | text/tool extraction may miss content | parser fixtures for unknown variants should be added when seen in events |
| Ollama XML fallback has less structured evidence | local-only failures may still be coarser | add provider-equivalent XML parse events in a follow-up |
| tool argument type coercion remains strict | number/bool-as-string mismatch can become feedback loop | keep strict for safety; add targeted coercion only with explicit tool schema need |

## Follow-up Work Items

- Add Ollama/XML fallback events matching provider parser events.
- Add fixture tests for unknown OpenAI/Gemini output variants from real provider artifacts.
- Add a CI job that runs `mvp-provider-smoke` with live credentials when secrets are available.
