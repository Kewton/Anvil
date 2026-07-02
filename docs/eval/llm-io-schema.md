# llm-io.jsonl Schema

`llm-io.jsonl` is an append-only JSON Lines stream stored under each session's
`logs/` directory. Every line has this envelope:

```json
{
  "ts_ms": 1700000000000,
  "event": "ollama.chat.request",
  "payload": {}
}
```

All payloads are passed through the logging secret masker before they are
written.

## Ollama Request Events

Events:

- `ollama.generate.request`
- `ollama.chat.request`

Stable payload fields:

| Field | Type | Description |
| --- | --- | --- |
| `base_url` | string | Ollama base URL. |
| `model` | string | Requested model name. |
| `stream` | boolean | Whether streaming was requested. |
| `temperature` | number | Request temperature. |
| `num_ctx` | number | Context window passed to Ollama. |
| `num_predict` | number | Generation cap passed to Ollama. |
| `tools` | array[string] | Tool names available to this request. |
| `messages` | array[object] | Controller-side request messages before transport rendering. |
| `prompt_metrics` | object | Prompt observability schema, versioned below. |

`ollama.chat.request` also includes `format`, which is either a string or null.

### `prompt_metrics` Version 1

`prompt_metrics.schema_version` is `1`.

| Field | Type | Description |
| --- | --- | --- |
| `schema_version` | number | Prompt metrics schema version. |
| `final_prompt` | string | Controller-side final prompt rendering. For `/api/generate`, this is the exact prompt sent with `raw: true`. For `/api/chat`, this is Anvil's deterministic ChatML-style rendering of the same message list because Ollama's model template expansion is not observable client-side. |
| `prompt_char_count` | number | Unicode scalar count of `final_prompt`. |
| `approx_prompt_tokens` | number | Deterministic estimate using `chars / 4`, rounded up. |
| `injection_blocks` | array[object] | Per-message breakdown used to attribute prompt growth. |

Each `injection_blocks[]` item has this shape:

| Field | Type | Description |
| --- | --- | --- |
| `index` | number | Zero-based message index in the request. |
| `kind` | string | One of `system_prompt`, `system_injection`, `user_message`, `assistant_history`, `tool_result`, or `message`. |
| `role` | string | Original `ConversationMessage.role`. |
| `name` | string or null | Tool result name when present. |
| `char_count` | number | Unicode scalar count of the message content. |
| `approx_tokens` | number | Deterministic estimate for this block, including fixed per-message overhead. |
| `tool_call_count` | number | Assistant tool-call count attached to this message. |

## Ollama Reply Events

Events:

- `ollama.generate.reply_final`
- `ollama.chat.reply_final`

Stable payload fields:

| Field | Type | Description |
| --- | --- | --- |
| `content` | string | Final assistant text after parser cleanup. |
| `tool_calls` | array[object] | Parsed tool calls. |
| `prompt_tokens` | number or null | Ollama `prompt_eval_count`, when provided. |
| `completion_tokens` | number or null | Ollama `eval_count`, when provided. |

## Raw Response Events

Events:

- `ollama.generate.response_raw`
- `ollama.chat.response_raw`

Non-streaming payloads include `model`, `stream`, and truncated raw `body`.
Streaming payloads include `stream: true` and truncated line `chunks`.

## Runtime Tool Events

Runtime events are emitted by deterministic controller code when a tool-policy
decision needs later measurement.

### `tool.bash.cd_wrapper_reclassified`

Emitted when Bash recognizes the narrow `cd <dir> && <tail>` wrapper form and
classifies `<tail>` as the effective command class. The `<dir>` must resolve to
the current tool cwd or a descendant; otherwise this event is not emitted and
the command is classified normally.

Stable payload fields:

| Field | Type | Description |
| --- | --- | --- |
| `shape` | string | Currently always `cd_and_tail`. |
| `tail_class` | string | Classification of `<tail>`, using `BashCommandClass::as_str()`. |
| `effective_class` | string | Class applied to the full command. |
| `offline_allowed_class` | boolean | Whether the effective class is one of the offline-allowed classes introduced by this wrapper path: `script_run` or `build_test`. |
