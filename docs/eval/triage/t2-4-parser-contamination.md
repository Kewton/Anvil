# T2-4 Parser Contamination Triage

Date: 2026-06-12 JST

対象:

- Benchmark root: `.anvil/benchmarks/20260612T051125-8470`
- Model: `qwen3.6:27b-coding-nvfp4`
- Engine: `minimal`
- Matrix subset: 25 scenarios x 5 runs = 125 minimal runs

## 判定

修正前 minimal run の混在は認めない。

理由:

- `git log` 上の parser feedback 修正コミットは `4eeea5bba537368e6fbc009ebcb76f9c136521df` (`Record minimal loop GPU comparison`) で、commit/author date は `2026-06-12T07:18:04+09:00`。
- ただし対象 benchmark は commit 前の uncommitted worktree で実行されているため、commit 時刻だけでは実行バイナリの内容を判定できない。
- 実行バイナリ `target/release/anvil` の mtime は `2026-06-12T05:09:53+09:00`。
- 修正対象ソースの mtime は `feedback.rs = 2026-06-12T05:08:37+09:00`、`loop_run.rs = 2026-06-12T05:08:51+09:00`。
- 対象 minimal run の `meta.json.start_ts` は `2026-06-11T20:11:25Z` から `2026-06-11T22:06:44Z`、JST では `2026-06-12T05:11:25+09:00` から `2026-06-12T07:06:44+09:00`。
- すべての run は、修正済み source mtime と `target/release/anvil` mtime より後に開始している。
- rc != 0 の run でも `[minimal-feedback] Your previous tool call was malformed ... (tool call parser failed: ...)` が llm-io ログに残っている。この文言は修正後の recoverable feedback 経路で生成されるため、修正前の即時終了経路ではない。

制約:

- `meta.json` には実行バイナリの commit hash / build id は記録されていない。従って、最終判定は `target/release/anvil` mtime、source mtime、llm-io の修正後 feedback 文言を突き合わせたもの。

## 全体数

| item | count |
|---|---:|
| minimal run meta.json | 125 |
| rc = 0 | 95 |
| rc != 0 | 30 |
| success_check_success = true | 35 |
| success_check_success = false | 90 |

## rc != 0 の内訳

| classification | count | interpretation |
|---|---:|---|
| parser feedback repeated, no artifact | 25 | parser failure は recoverable feedback になっているが、モデルが有効な tool call に戻れず `max_iterations (12)` に到達 |
| parser feedback seen, artifact incomplete or exhausted | 4 | parser failure 後に一部 Write は成功したが、必要成果物不足・行数不足・または `max_iterations (12)` で rc != 0 |
| no parser feedback, max_iterations | 1 | parser とは別原因。成果物と success_check は ok だが、tool loop が終了せず `max_iterations (12)` に到達 |

集約すると、rc != 0 の 30 件中 29 件は parser feedback が関与している。ただしこれは「修正前 run の混在」ではなく、修正後の recovery path が発火したうえで、モデルが malformed tool call を繰り返して完走できなかった失敗。

## rc != 0 詳細

| scenario | run | start_ts UTC | class | parser feedback count | artifact files | success_check_reason |
|---|---|---|---|---:|---:|---|
| `fix-python-retry-policy` | `run-1` | `2026-06-11T20:36:02Z` | parser no artifact | 11 | 0 | `missing_file:src/retry_policy.py,command_failed:0` |
| `fix-python-retry-policy` | `run-2` | `2026-06-11T20:37:18Z` | parser incomplete/exhausted | 10 | 1 | `min_lines:src/retry_policy.py:19<25` |
| `fix-python-retry-policy` | `run-3` | `2026-06-11T20:37:56Z` | parser no artifact | 11 | 0 | `missing_file:src/retry_policy.py,command_failed:0` |
| `fix-python-retry-policy` | `run-5` | `2026-06-11T20:39:26Z` | parser no artifact | 11 | 0 | `missing_file:src/retry_policy.py,command_failed:0` |
| `fix-python-slugify` | `run-4` | `2026-06-11T20:28:50Z` | max iterations, no parser | 0 | 1 | `ok` |
| `fix-shell-safe-clean` | `run-1` | `2026-06-11T20:48:17Z` | parser no artifact | 7 | 0 | `missing_file:scripts/safe_clean.sh,command_failed:0` |
| `multi-file-docs-and-examples` | `run-1` | `2026-06-11T20:54:28Z` | parser no artifact | 9 | 0 | `missing_file:docs/getting-started.md,missing_file:examples/basic-config.toml,command_failed:0` |
| `multi-file-node-package` | `run-1` | `2026-06-11T20:50:11Z` | parser no artifact | 10 | 0 | `missing_file:package.json,missing_file:src/index.js,missing_file:test/index.test.js,command_failed:0` |
| `multi-file-node-package` | `run-2` | `2026-06-11T20:50:28Z` | parser no artifact | 9 | 0 | `missing_file:package.json,missing_file:src/index.js,missing_file:test/index.test.js,command_failed:0` |
| `multi-file-node-package` | `run-3` | `2026-06-11T20:51:18Z` | parser no artifact | 11 | 0 | `missing_file:package.json,missing_file:src/index.js,missing_file:test/index.test.js,command_failed:0` |
| `multi-file-python-package` | `run-1` | `2026-06-11T20:59:31Z` | parser no artifact | 8 | 0 | `missing_file:pyproject.toml,missing_file:src/miniloop/tokens.py,missing_file:tests/test_tokens.py,command_failed:0` |
| `multi-file-python-package` | `run-2` | `2026-06-11T21:00:29Z` | parser incomplete/exhausted | 3 | 4 | `ok` |
| `multi-file-python-package` | `run-3` | `2026-06-11T21:01:53Z` | parser incomplete/exhausted | 5 | 3 | `missing_file:tests/test_tokens.py` |
| `multi-file-python-package` | `run-4` | `2026-06-11T21:04:00Z` | parser no artifact | 9 | 0 | `missing_file:pyproject.toml,missing_file:src/miniloop/tokens.py,missing_file:tests/test_tokens.py,command_failed:0` |
| `multi-file-python-package` | `run-5` | `2026-06-11T21:04:19Z` | parser incomplete/exhausted | 8 | 2 | `missing_file:src/miniloop/tokens.py,missing_file:tests/test_tokens.py,command_failed:0` |
| `new-markdown-release-notes` | `run-1` | `2026-06-11T20:18:06Z` | parser no artifact | 11 | 0 | `missing_file:docs/release-notes-template.md,command_failed:0` |
| `new-markdown-release-notes` | `run-4` | `2026-06-11T20:19:26Z` | parser no artifact | 11 | 0 | `missing_file:docs/release-notes-template.md,command_failed:0` |
| `new-markdown-release-notes` | `run-5` | `2026-06-11T20:20:19Z` | parser no artifact | 11 | 0 | `missing_file:docs/release-notes-template.md,command_failed:0` |
| `new-python-csv-small` | `run-1` | `2026-06-11T20:11:54Z` | parser no artifact | 11 | 0 | `missing_file:tools/csv_stats.py,command_failed:0` |
| `new-python-csv-small` | `run-2` | `2026-06-11T20:12:57Z` | parser no artifact | 11 | 0 | `missing_file:tools/csv_stats.py,command_failed:0` |
| `new-python-csv-small` | `run-3` | `2026-06-11T20:13:59Z` | parser no artifact | 11 | 0 | `missing_file:tools/csv_stats.py,command_failed:0` |
| `new-python-csv-small` | `run-4` | `2026-06-11T20:14:58Z` | parser no artifact | 11 | 0 | `missing_file:tools/csv_stats.py,command_failed:0` |
| `new-python-csv-small` | `run-5` | `2026-06-11T20:16:00Z` | parser no artifact | 11 | 0 | `missing_file:tools/csv_stats.py,command_failed:0` |
| `new-rust-cli-small` | `run-4` | `2026-06-11T20:11:36Z` | parser no artifact | 9 | 0 | `missing_file:src/bin/word_count.rs,command_failed:0` |
| `non-coding-research-brief` | `run-1` | `2026-06-11T21:30:53Z` | parser no artifact | 11 | 0 | `missing_file:reports/local-llm-brief.md,command_failed:0` |
| `non-coding-research-brief` | `run-2` | `2026-06-11T21:38:50Z` | parser no artifact | 11 | 0 | `missing_file:reports/local-llm-brief.md,command_failed:0` |
| `non-coding-research-brief` | `run-3` | `2026-06-11T21:46:44Z` | parser no artifact | 11 | 0 | `missing_file:reports/local-llm-brief.md,command_failed:0` |
| `non-coding-research-brief` | `run-4` | `2026-06-11T21:54:21Z` | parser no artifact | 11 | 0 | `missing_file:reports/local-llm-brief.md,command_failed:0` |
| `non-coding-runbook` | `run-3` | `2026-06-11T22:06:11Z` | parser no artifact | 1 | 0 | `missing_file:runbooks/incident-triage.md,command_failed:0` |
| `non-coding-runbook` | `run-5` | `2026-06-11T22:06:44Z` | parser no artifact | 10 | 0 | `missing_file:runbooks/incident-triage.md,command_failed:0` |

## minimal 側再実行の要否

parser 修正前 run の混在は認めないため、混在除去を目的とした minimal 125 run の再実行は不要。

一方で、rc != 0 の 30 件中 29 件は修正後 parser feedback 経路に入った後も malformed tool call から復帰できていない。これはデータ汚染ではなく minimal loop の failure-triage 入力として扱うべき失敗である。

従って次の判断は以下。

- T2-4 の比較データは「修正前 run 混在」を理由に破棄しない。
- タスク2・3では、この 29 件を `truncated_output` / `no_edit_loop` / `crash` などのエンジン中立分類に再分類する。
- parser feedback の改善や prompt 変更は本タスクでは禁止。必要なら別タスク・別 PR で、ベンチ敗北シナリオを根拠に検討する。
