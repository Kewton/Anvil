# T2-4 Parser Failure Deepdive

Date: 2026-06-12 JST

対象:

- Source triage: `docs/eval/triage/t2-4-parser-contamination.md`
- Benchmark root: `.anvil/benchmarks/20260612T051125-8470`
- Model: `qwen3.6:27b-coding-nvfp4`
- Engine: `minimal`
- Scope: rc != 0 かつ parser feedback 関与ありの 29 runs

本調査はログ解析のみ。`minimal_loop`、parser、prompt のコードは変更していない。

## Summary

主因は **プロンプトの形式提示不足というより、native tool mode と XML feedback の形式衝突**。

根拠:

- 274 件の malformed occurrence はすべて `ollama.chat.request.payload.tools` が存在する native tool call mode で発生していた。
- その状態で assistant は `message.tool_calls` ではなく `message.content` に `<anvil_tool_call>...` を出していた。
- parser error 後の ephemeral feedback も XML 形式を要求しており、次の request も native tools 有効のままだった。
- `done_reason=length` は 0 件で、num_predict 上限による切断は確認されない。
- 187/274 件は `<anvil_tool_call>` が閉じていない。うち 95 件は `</function>` / `</tool_call>` など別形式の closing tag で終わっていた。
- 87/274 件は XML と JSON が一見正しく、`src/ollama/xml_fallback.rs::extract_tool_calls` に渡せば救済可能に見えるが、native path では `src/ollama/parsing.rs::finalize_native_reply` -> `detect_malformed_tool_call` により rejected されている。

従って、主因は parser 実装バグ単独ではない。`native tools` が有効な request で、system prompt と feedback が XML 例を見せ続け、モデルが native tool call ではなく XML content を出すループに入っている。

## Extraction Method

各 run の `llm-io.jsonl` から以下を対応付けた。

1. `ollama.chat.response_raw.payload.body.message.content`
2. 直後の `ollama.chat.request` に入った `[minimal-feedback]` user message
3. feedback 内の parser error: `tool call parser failed: ...`
4. 直前 request の `payload.tools` 有無による mode 判定

full raw payload は各 run の `state/sessions/*/logs/llm-io.jsonl` の response line に存在する。全 payload は抽出済みで、下表では response line と class を記録する。

## Failure Type Distribution

| type | definition | occurrences |
|---|---|---:|
| a | `<anvil_tool_call>` が `</anvil_tool_call>` で閉じていない | 187 |
| b | args / JSON 不正 | 0 |
| c | 形式自体が想定外 | 0 |
| d | 一見正しい XML tool call だが native path で拒否 | 87 |
| e | 出力切断 (`done_reason=length`) | 0 |
| total |  | 274 |

type a の内訳:

| subtype | occurrences |
|---|---:|
| `<anvil_tool_call>` の close tag なし | 92 |
| `</function>` / `</tool_call>` など別形式で閉じた | 95 |

parser error 文言:

| parser error | occurrences |
|---|---:|
| `tool call parser failed: unterminated <anvil_tool_call> block` | 187 |
| `tool call parser failed: malformed tool call markup` | 87 |

mode:

| mode at malformed response | occurrences |
|---|---:|
| native tool call mode (`payload.tools` present) | 274 |
| xml_fallback mode (`payload.tools` absent) | 0 |

## Scenario Cross Table

| scenario | content shape | a | b | c | d | e | total |
|---|---|---:|---:|---:|---:|---:|---:|
| `fix-python-retry-policy` | Python code | 1 | 0 | 0 | 42 | 0 | 43 |
| `fix-shell-safe-clean` | shell script | 7 | 0 | 0 | 0 | 0 | 7 |
| `multi-file-docs-and-examples` | markdown + TOML | 5 | 0 | 0 | 4 | 0 | 9 |
| `multi-file-node-package` | package.json JSON + JS/tests | 20 | 0 | 0 | 10 | 0 | 30 |
| `multi-file-python-package` | TOML + Python/tests | 27 | 0 | 0 | 6 | 0 | 33 |
| `new-markdown-release-notes` | markdown + HTML comments | 24 | 0 | 0 | 9 | 0 | 33 |
| `new-python-csv-small` | Python code + XML-like `<csv_path>` string | 55 | 0 | 0 | 0 | 0 | 55 |
| `new-rust-cli-small` | Rust code | 3 | 0 | 0 | 6 | 0 | 9 |
| `non-coding-research-brief` | long markdown + tables | 44 | 0 | 0 | 0 | 0 | 44 |
| `non-coding-runbook` | long markdown runbook + shell fences | 1 | 0 | 0 | 10 | 0 | 11 |

Run-level cross table:

| scenario | run | a | b | c | d | e | total |
|---|---|---:|---:|---:|---:|---:|---:|
| `fix-python-retry-policy` | `run-1` | 0 | 0 | 0 | 11 | 0 | 11 |
| `fix-python-retry-policy` | `run-2` | 0 | 0 | 0 | 10 | 0 | 10 |
| `fix-python-retry-policy` | `run-3` | 0 | 0 | 0 | 11 | 0 | 11 |
| `fix-python-retry-policy` | `run-5` | 1 | 0 | 0 | 10 | 0 | 11 |
| `fix-shell-safe-clean` | `run-1` | 7 | 0 | 0 | 0 | 0 | 7 |
| `multi-file-docs-and-examples` | `run-1` | 5 | 0 | 0 | 4 | 0 | 9 |
| `multi-file-node-package` | `run-1` | 10 | 0 | 0 | 0 | 0 | 10 |
| `multi-file-node-package` | `run-2` | 1 | 0 | 0 | 8 | 0 | 9 |
| `multi-file-node-package` | `run-3` | 9 | 0 | 0 | 2 | 0 | 11 |
| `multi-file-python-package` | `run-1` | 4 | 0 | 0 | 4 | 0 | 8 |
| `multi-file-python-package` | `run-2` | 2 | 0 | 0 | 1 | 0 | 3 |
| `multi-file-python-package` | `run-3` | 4 | 0 | 0 | 1 | 0 | 5 |
| `multi-file-python-package` | `run-4` | 9 | 0 | 0 | 0 | 0 | 9 |
| `multi-file-python-package` | `run-5` | 8 | 0 | 0 | 0 | 0 | 8 |
| `new-markdown-release-notes` | `run-1` | 9 | 0 | 0 | 2 | 0 | 11 |
| `new-markdown-release-notes` | `run-4` | 5 | 0 | 0 | 6 | 0 | 11 |
| `new-markdown-release-notes` | `run-5` | 10 | 0 | 0 | 1 | 0 | 11 |
| `new-python-csv-small` | `run-1` | 11 | 0 | 0 | 0 | 0 | 11 |
| `new-python-csv-small` | `run-2` | 11 | 0 | 0 | 0 | 0 | 11 |
| `new-python-csv-small` | `run-3` | 11 | 0 | 0 | 0 | 0 | 11 |
| `new-python-csv-small` | `run-4` | 11 | 0 | 0 | 0 | 0 | 11 |
| `new-python-csv-small` | `run-5` | 11 | 0 | 0 | 0 | 0 | 11 |
| `new-rust-cli-small` | `run-4` | 3 | 0 | 0 | 6 | 0 | 9 |
| `non-coding-research-brief` | `run-1` | 11 | 0 | 0 | 0 | 0 | 11 |
| `non-coding-research-brief` | `run-2` | 11 | 0 | 0 | 0 | 0 | 11 |
| `non-coding-research-brief` | `run-3` | 11 | 0 | 0 | 0 | 0 | 11 |
| `non-coding-research-brief` | `run-4` | 11 | 0 | 0 | 0 | 0 | 11 |
| `non-coding-runbook` | `run-3` | 1 | 0 | 0 | 0 | 0 | 1 |
| `non-coding-runbook` | `run-5` | 0 | 0 | 0 | 10 | 0 | 10 |

## Content-Type Correlation

- `new-python-csv-small` は 5/5 run、55/55 occurrence が type a。payload 内に `print("Usage: ... <csv_path>")` が繰り返し出る。`<csv_path>` 自体は JSON string 内なので parser error の直接原因ではないが、XML-like token を含む Python code を XML wrapper 内に入れる形が、closing tag 崩れを強く誘発している。
- `non-coding-research-brief` は 4/5 run、44/44 occurrence が type a。payload は平均約 5,006 chars、最大 6,927 chars の長い markdown table / prose。全件 `done_reason=stop` なので truncation ではなく、長い Write payload を XML wrapper で包み切れないモデル出力崩れ。
- Python retry のような短い code payload は 42/43 occurrence が type d。XML と JSON はほぼ正しいが native path で rejected されるため、ここは parser salvage gap ではある。
- markdown / TOML / package.json / tests など multi-file 系は type a と type d が混在。複数 artifact の途中で `Bash mkdir` や `Read .` を XML content として出す pattern も目立つ。

## Representative Payloads

### Type a: wrong closing tags

Source: `new-python-csv-small/run-1`, response line 3, parser error `unterminated <anvil_tool_call> block`.

```text
<anvil_tool_call>{"name":"Write","arguments":{"path":"tools/csv_stats.py","content":"import csv\nimport sys\n\n\ndef main():\n    if len(sys.argv) < 2:\n        print(\"Usage: python csv_stats.py <csv_path>\")\n        sys.exit(1)\n...
"}}
</function>
</tool_call>
```

The JSON body is closeable, but the `<anvil_tool_call>` wrapper is never closed with `</anvil_tool_call>`.

### Type a: no closing tag after long markdown

Source: `non-coding-research-brief/run-1`, response line 5, parser error `unterminated <anvil_tool_call> block`.

```text
<anvil_tool_call>{"name":"Write","arguments":{"path":"reports/local-llm-brief.md","content":"# Local LLM Trade-offs: A Concise Research Brief\n\n**Date:** 2026-06-12  **Status:** Draft\n\n## Overview\n\nRunning large language models locally ...
```

The payload continues for thousands of characters and ends without `</anvil_tool_call>`. `done_reason=stop`, so this is not output length truncation.

### Type d: closed XML that native path rejects

Source: `fix-python-retry-policy/run-1`, response line 5, parser error `malformed tool call markup`.

```text
<anvil_tool_call>{"name":"Write","arguments":{"path":"src/retry_policy.py","content":"def should_retry(status_code, attempt):\n    \"\"\"Determine whether a request should be retried.\n...
"}}
</anvil_tool_call>
```

This is valid enough for the XML fallback parser shape. It was rejected because the request was still native mode and the response had no native `message.tool_calls`.

### Type d: closed read call

Source: `new-rust-cli-small/run-4`, response line 9.

```text
<anvil_tool_call>{"name":"Read","arguments":{"path":"Cargo.toml"}}
</anvil_tool_call>
```

This should be salvageable by `src/ollama/xml_fallback.rs::extract_tool_calls`, but the native path goes through `src/ollama/parsing.rs::finalize_native_reply` and then `detect_malformed_tool_call`.

## Parser Function Assessment

Relevant functions:

- `src/ollama/parsing.rs::finalize_native_reply`
- `src/ollama/parsing.rs::parse_native_tool_calls`
- `src/ollama/parsing.rs::detect_malformed_tool_call`
- `src/ollama/xml_fallback.rs::extract_tool_calls`
- `src/ollama/xml_fallback.rs::extract_between`
- `src/ollama/xml_fallback.rs::extract_unterminated_trailing_block`
- `src/ollama/xml_fallback.rs::parse_tool_call_object`

Assessment:

- Type a failures are model output malformedness, not an XML parser bug. The model either omitted `</anvil_tool_call>` or mixed Anthropic/OpenAI-style tags (`</function>`, `</tool_call>`) with Anvil tags.
- Type d is an implementation improvement candidate in `src/ollama/parsing.rs::finalize_native_reply`: when native `message.tool_calls` is empty but `message.content` contains a valid `<anvil_tool_call>...</anvil_tool_call>`, the code currently calls `detect_malformed_tool_call` and fails instead of delegating to `xml_fallback::extract_tool_calls`.
- However, type d is not the primary cause of all 29 failed runs. `new-python-csv-small` and `non-coding-research-brief` are pure type a and would still fail without a better strategy for malformed/long XML payloads.

If treated as an implementation bug later, a reproduction fixture should cover:

```json
{
  "message": {
    "role": "assistant",
    "content": "<anvil_tool_call>{\"name\":\"Read\",\"arguments\":{\"path\":\"Cargo.toml\"}}</anvil_tool_call>",
    "tool_calls": []
  },
  "done_reason": "stop"
}
```

Expected future behavior would be to parse this as one `Read` tool call when native tool calls are absent from the model response. This report does not implement that change.

## Ephemeral Feedback Assessment

Observed feedback strings:

```text
[minimal-feedback]
Your previous tool call was malformed and was not executed (tool call parser failed: unterminated <anvil_tool_call> block). Reply with exactly one complete <anvil_tool_call>{"name":"ToolName","arguments":{...}}</anvil_tool_call> block, or plain text if the task is already complete.
```

```text
[minimal-feedback]
Your previous tool call was malformed and was not executed (tool call parser failed: malformed tool call markup). Reply with exactly one complete <anvil_tool_call>{"name":"ToolName","arguments":{...}}</anvil_tool_call> block, or plain text if the task is already complete.
```

The feedback shows a syntactically correct XML wrapper example, but it is generic (`ToolName`, `{...}`), not a concrete allowed tool call. More importantly, it asks for XML while the next request still includes native tools. This likely reinforces the wrong output channel.

## Payload Index

Common path pattern:

`.anvil/benchmarks/20260612T051125-8470/qwen3.6-27b-coding-nvfp4/minimal/<scenario>/default/<run>/state/sessions/*/logs/llm-io.jsonl`

The full raw payload is `ollama.chat.response_raw.payload.body.message.content` at the response lines listed below.

| scenario | run | response lines | classes |
|---|---|---|---|
| `fix-python-retry-policy` | `run-1` | 3,5,7,9,11,13,15,17,19,21,23 | d x11 |
| `fix-python-retry-policy` | `run-2` | 3,5,7,9,14,16,18,20,22,24 | d x10 |
| `fix-python-retry-policy` | `run-3` | 3,5,7,9,11,13,15,17,19,21,23 | d x11 |
| `fix-python-retry-policy` | `run-5` | 3,5,7,9,11,13,15,17,19,21,23 | a x1, d x10 |
| `fix-shell-safe-clean` | `run-1` | 15,17,19,21,23,25,27 | a x7 |
| `multi-file-docs-and-examples` | `run-1` | 9,11,13,15,17,19,21,23,25 | a x5, d x4 |
| `multi-file-node-package` | `run-1` | 6,8,10,12,14,16,18,20,22,24 | a x10 |
| `multi-file-node-package` | `run-2` | 9,11,13,15,17,19,21,23,25 | a x1, d x8 |
| `multi-file-node-package` | `run-3` | 3,5,7,9,11,13,15,17,19,21,23 | a x9, d x2 |
| `multi-file-python-package` | `run-1` | 9,11,13,15,17,19,21,23 | a x4, d x4 |
| `multi-file-python-package` | `run-2` | 9,11,13 | a x2, d x1 |
| `multi-file-python-package` | `run-3` | 3,5,7,9,11 | a x4, d x1 |
| `multi-file-python-package` | `run-4` | 9,11,13,15,17,19,21,23,25 | a x9 |
| `multi-file-python-package` | `run-5` | 12,14,16,18,20,22,24,26 | a x8 |
| `new-markdown-release-notes` | `run-1` | 3,5,7,9,11,13,15,17,19,21,23 | a x9, d x2 |
| `new-markdown-release-notes` | `run-4` | 3,5,7,9,11,13,15,17,19,21,23 | a x5, d x6 |
| `new-markdown-release-notes` | `run-5` | 3,5,7,9,11,13,15,17,19,21,23 | a x10, d x1 |
| `new-python-csv-small` | `run-1` | 3,5,7,9,11,13,15,17,19,21,23 | a x11 |
| `new-python-csv-small` | `run-2` | 3,5,7,9,11,13,15,17,19,21,23 | a x11 |
| `new-python-csv-small` | `run-3` | 3,5,7,9,11,13,15,17,19,21,23 | a x11 |
| `new-python-csv-small` | `run-4` | 3,5,7,9,11,13,15,17,19,21,23 | a x11 |
| `new-python-csv-small` | `run-5` | 3,5,7,9,11,13,15,17,19,21,23 | a x11 |
| `new-rust-cli-small` | `run-4` | 9,11,13,15,17,19,21,23,25 | a x3, d x6 |
| `non-coding-research-brief` | `run-1` | 3,5,7,9,11,13,15,17,19,21,23 | a x11 |
| `non-coding-research-brief` | `run-2` | 3,5,7,9,11,13,15,17,19,21,23 | a x11 |
| `non-coding-research-brief` | `run-3` | 3,5,7,9,11,13,15,17,19,21,23 | a x11 |
| `non-coding-research-brief` | `run-4` | 3,5,7,9,11,13,15,17,19,21,23 | a x11 |
| `non-coding-runbook` | `run-3` | 33 | a x1 |
| `non-coding-runbook` | `run-5` | 6,8,10,12,14,16,18,20,22,24 | d x10 |

## Rerun Opinion

この調査結果だけでは、minimal 側 125 run の即時再実行は不要。

理由:

- 今回の目的は失敗型の分類であり、既存ログから全 29 run / 274 occurrence を分類できた。
- `done_reason=length` は 0 で、num_predict 上限に起因する run 汚染は見つからない。
- parser 実装修正や prompt/feedback 修正を行っていないため、同一条件での再実行は分類結果を置き換える根拠にならない。

ただし、将来 `finalize_native_reply` の XML salvage、または feedback/prompt の native-mode 整合を変更した場合は、変更後の minimal 125 run を再実行して T2-4 と比較し直す必要がある。
