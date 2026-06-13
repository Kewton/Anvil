# T2-4 Rerun Fix Efficacy Check

Date: 2026-06-12 JST

対象:

- Rerun BENCH_ROOT: `.anvil/benchmarks/20260612T105012-57478`
- Model: `qwen3.6:27b-coding-nvfp4`
- Engine: `minimal`
- Scope: 修正 #1021 / #1022 が rerun 実行時に有効だったかの検証

本調査はログ・mtime・実装確認のみ。コードは変更していない。

## Summary

判定:

| PR | runtime effective? | basis |
|---|---|---|
| #1021 `Restore minimal loop XML fallback downgrade` | No | rerun の request は全件旧 prompt / 旧 feedback で、parser failure 後も `tools` が残っている。 |
| #1022 `Salvage XML tool content in native replies` | No | rerun で使われた `target/release/anvil` は merge 前 mtime で、salvage flag / log event 文字列を含まない。salvage candidate はあるが発動 0。 |

結論:

- rerun は #1021/#1022 merge 後に開始されているが、`bench.sh` が使った `target/release/anvil` は merge 前にビルドされた古いバイナリだった可能性が極めて高い。
- 従って `minimal-loop-t2-4-rerun-20260612.md` の数字は「両修正の効果測定」としては無効。モデル挙動 triage に進む前に、修正後バイナリで 125 run を再実行する必要がある。

## Binary / Time Evidence

`scripts/bench.sh` は `ANVIL_BIN` が未指定の場合、存在すれば `$REPO_ROOT/target/release/anvil` を使う。

| item | timestamp |
|---|---|
| `target/release/anvil` mtime | `2026-06-12T05:09:53+0900` |
| `target/debug/anvil` mtime | `2026-06-12T09:37:03+0900` |
| #1021 merge commit time | `2026-06-12T10:48:06+0900` |
| #1022 merge commit time | `2026-06-12T10:48:41+0900` |
| rerun first `meta.json.start_ts` | `2026-06-12T01:50:12Z` = `2026-06-12T10:50:12+0900` |
| rerun last `meta.json.start_ts` | `2026-06-12T03:58:51Z` = `2026-06-12T12:58:51+0900` |

Interpretation:

- rerun started after both merge commits.
- However, the release binary mtime is more than 5 hours before both merges.
- `bench.sh` would choose this stale release binary unless `ANVIL_BIN` was set. The run meta does not record `ANVIL_BIN`, binary path, commit hash, or build time, so this cannot be proven from meta alone.

`strings target/release/anvil` evidence:

| string | release binary | debug binary |
|---|---:|---:|
| `When native tool calls are unavailable, emit exactly one XML tool call like:` | present | absent |
| `Use the runtime-provided tool call channel for tools.` | absent | present |
| `Native tool calls are disabled for the rest of this session.` | absent | present |
| `ANVIL_NO_NATIVE_XML_SALVAGE` | absent | present |
| `ollama.chat.native_xml_salvage` | absent | present |

This matches the rerun logs and indicates the release binary was older than #1021/#1022.

## Task 7 Flag Check

Implementation on current branch:

- Off flag: `ANVIL_NO_NATIVE_XML_SALVAGE`
- Default: enabled. `src/ollama/parsing.rs::native_xml_salvage_disabled_with` disables salvage only when the env var is present and non-empty.
- `bench.sh` launches `anvil` under `env -i` and only passes selected feature flags. It does not pass `ANVIL_NO_NATIVE_XML_SALVAGE`.

Therefore, if #1022 code had been present in the executed binary, the bench run would have used salvage enabled by default. The observed problem is not a default-off flag; the release binary did not contain the flag or salvage log string at all.

## Downgrade / Parser Failure Classification

The rerun root contains both canonical session logs under `state/sessions/*/logs/llm-io.jsonl` and copied logs under `run/logs/llm-io.jsonl`. Raw grep over the full root double-counts some copied logs.

| log scope | parser failure occurrences | native mode | fallback mode | sessions/logs with parser feedback |
|---|---:|---:|---:|---:|
| unique session logs only | 321 | 321 | 0 | 41 |
| all physical `llm-io.jsonl` files | 352 | 352 | 0 | 53 log files |

Downgrade firing rate:

| metric | value |
|---|---:|
| unique sessions with parser feedback | 41 |
| sessions where feedback request had `tools` removed | 0 |
| sessions where every parser feedback request still had `tools` | 41 |
| occurrence-level downgrade rate, unique logs | 0 / 321 |
| occurrence-level downgrade rate, raw physical logs | 0 / 352 |

Log evidence:

- Every parser feedback request still has `payload.tools = ["Bash","Read","Write","Edit","Glob","Grep"]`.
- System prompt in rerun request logs contains the old mixed-mode XML hint:

```text
When native tool calls are unavailable, emit exactly one XML tool call like:
<anvil_tool_call>{"name":"Read","arguments":{"path":"README.md"}}</anvil_tool_call>
```

- Feedback in rerun logs contains the old wording without downgrade notice:

```text
[minimal-feedback]
Your previous tool call was malformed and was not executed (tool call parser failed: ...). Reply with exactly one complete <anvil_tool_call>{"name":"ToolName","arguments":{...}}</anvil_tool_call> block, or plain text if the task is already complete.
```

- The #1021 wording is absent from rerun logs:

```text
Native tool calls are disabled for the rest of this session.
```

## Salvage Check

Unique session logs contain 75 native-content closed XML candidates:

| scenario | closed XML native-content candidates |
|---|---:|
| `fix-js-date-helper` | 9 |
| `fix-json-normalizer` | 4 |
| `fix-python-retry-policy` | 36 |
| `fix-python-slugify` | 3 |
| `fix-readme-command` | 5 |
| `fix-rust-parser-error` | 2 |
| `multi-file-node-package` | 10 |
| `non-coding-research-brief` | 3 |
| `scaffold-next-dashboard` | 2 |
| `scaffold-rust-cli` | 1 |
| total | 75 |

Observed `ollama.chat.native_xml_salvage` events:

| scope | count |
|---|---:|
| unique session logs | 0 |
| all physical logs | 0 |

Because the release binary lacks the salvage flag and log string, this is not evidence that #1022 failed logically. It is evidence that #1022 was not present in the executed binary.

## Scenario Check Failures: `scaffold-*` and `fix-json-normalizer`

対象 20 runs are all `rc=0` and `success_check_success=false`.

### Direct Cause Summary

| scenario | runs | direct failure pattern | classification |
|---|---:|---|---|
| `fix-json-normalizer` | 5 | `src/normalizeJson.ts` exists and grep passes, but file has 17-18 lines vs `min_lines: 30`. | success_check line-count threshold / compact implementation |
| `scaffold-fastapi-service` | 5 | 2 runs missing required files; 3 runs create all checked files but fail `min_lines` thresholds. Grep passes when `app/main.py` exists. | artifact incomplete + line-count threshold |
| `scaffold-next-dashboard` | 5 | 1 run creates no checked files; 3 runs miss one component/CSS file and/or page line threshold; 1 run has all files but `page.tsx` is 35 lines vs 50. | artifact incomplete + line-count threshold |
| `scaffold-rust-cli` | 5 | 4 runs only create `Cargo.toml` or nothing; 1 run creates `Cargo.toml` and short `src/main.rs` but misses `src/args.rs` and `tests/cli_smoke.rs`. | artifact incomplete |

### Run-Level Reasons

| scenario | run | success_check_reason |
|---|---|---|
| `fix-json-normalizer` | 1 | `min_lines:src/normalizeJson.ts:18<30` |
| `fix-json-normalizer` | 2 | `min_lines:src/normalizeJson.ts:18<30` |
| `fix-json-normalizer` | 3 | `min_lines:src/normalizeJson.ts:17<30` |
| `fix-json-normalizer` | 4 | `min_lines:src/normalizeJson.ts:17<30` |
| `fix-json-normalizer` | 5 | `min_lines:src/normalizeJson.ts:18<30` |
| `scaffold-fastapi-service` | 1 | `missing_file:app/main.py,missing_file:app/routes/health.py,missing_file:tests/test_health.py,command_failed:0` |
| `scaffold-fastapi-service` | 2 | `min_lines:app/main.py:7<25,min_lines:app/routes/health.py:8<20,min_lines:tests/test_health.py:12<20` |
| `scaffold-fastapi-service` | 3 | `min_lines:app/main.py:12<25,min_lines:app/routes/health.py:8<20,min_lines:tests/test_health.py:11<20` |
| `scaffold-fastapi-service` | 4 | `min_lines:app/main.py:7<25,min_lines:app/routes/health.py:8<20,min_lines:tests/test_health.py:11<20` |
| `scaffold-fastapi-service` | 5 | `missing_file:pyproject.toml,missing_file:app/main.py,missing_file:app/routes/health.py,missing_file:tests/test_health.py,command_failed:0` |
| `scaffold-next-dashboard` | 1 | `min_lines:src/app/page.tsx:31<50,missing_file:src/components/MetricCard.tsx` |
| `scaffold-next-dashboard` | 2 | `missing_file:src/app/page.tsx,missing_file:src/components/MetricCard.tsx,missing_file:src/app/globals.css,command_failed:0` |
| `scaffold-next-dashboard` | 3 | `min_lines:src/app/page.tsx:25<50,missing_file:src/app/globals.css` |
| `scaffold-next-dashboard` | 4 | `min_lines:src/app/page.tsx:45<50,missing_file:src/app/globals.css` |
| `scaffold-next-dashboard` | 5 | `min_lines:src/app/page.tsx:35<50` |
| `scaffold-rust-cli` | 1 | `min_lines:src/main.rs:9<30,missing_file:src/args.rs,missing_file:tests/cli_smoke.rs,command_failed:0` |
| `scaffold-rust-cli` | 2 | `missing_file:Cargo.toml,missing_file:src/main.rs,missing_file:src/args.rs,missing_file:tests/cli_smoke.rs,command_failed:0` |
| `scaffold-rust-cli` | 3 | `missing_file:src/main.rs,missing_file:src/args.rs,missing_file:tests/cli_smoke.rs,command_failed:0` |
| `scaffold-rust-cli` | 4 | `missing_file:src/main.rs,missing_file:src/args.rs,missing_file:tests/cli_smoke.rs,command_failed:0` |
| `scaffold-rust-cli` | 5 | `missing_file:src/main.rs,missing_file:src/args.rs,missing_file:tests/cli_smoke.rs,command_failed:0` |

### Check Design Notes

- No evidence of expected path mismatch was found for these 20 runs. The expected paths match the benchmark prompts.
- No evidence of offline environment causing the checked grep commands to fail. The commands are local `grep` checks. They fail when target files are missing; they pass when the file exists and contains the expected token.
- `fix-json-normalizer` is the strongest check-design concern: all 5 runs produced the expected file and symbol, but a concise 17-18 line implementation fails solely because the benchmark requires 30 lines.
- The scaffold cases are mixed. Some failures are genuine artifact incompleteness; some are line-count threshold failures even when a minimal scaffold exists.

## Required Follow-Up

Before model-behavior triage or Phase 3 decisions:

1. Rebuild `target/release/anvil` after #1021/#1022.
2. Record build commit / dirty flag / build time / active flags in benchmark meta before the next expensive run.
3. Rerun minimal 125 with the rebuilt release binary.
4. Recompute parser feedback, downgrade rate, and salvage activation rate from the new root.

The current rerun root should not be used as evidence that #1021/#1022 were ineffective.
