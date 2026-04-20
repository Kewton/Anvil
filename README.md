# Anvil

Ollama 直結の local-first コーディングエージェント。`workspace/v0.1.0` の方針に従い、旧 Anvil の汎用状態機械を捨てて、ローカル LLM が追いやすい小さい実装へゼロベースで作り直した。

## v0.1.0 の方針

- Ollama 専用
- prompt / protocol / loop を最小化
- tool-first
- Plan / Act の二段階だけを持つ
- session persistence を持つ
- JSON tool call が崩れた場合の XML fallback を持つ
- `Bash` `Read` `Write` `Edit` `Glob` `Grep` を built-in tools として持つ
- リリース導線は従来どおり `cargo build --release` と GitHub Releases を維持

## 実装済み機能

- Ollama `/api/tags` `/api/chat` への直結クライアント
- 同期 / streaming 両対応の chat loop
- モデル自動選択と optional sidecar 選択
- `$XDG_STATE_HOME/anvil/sessions/{session_id}/session.json` へのセッション保存（workdir 外）
- `/plan` と `/approve` による Plan / Act 切り替え
- Git checkpoint / rollback
- `<think>` 除去と `<tool_call>...</tool_call>` XML fallback
- FileWatcher と AutoTest command
- 軽量 TUI
- local skills loader
- MCP config registry
- read-only parallel analysis command
- 読み取り専用の Plan mode 制御
- live Ollama E2E を含む unit / integration / ignored E2E tests

## まだ入れていないもの

重い full-screen TUI、実際の MCP protocol transport、tool-enabled subagent delegation はまだ最小実装止まり。v0.1.0 では local-first なコア経路を優先し、後段拡張は軽量 slice に留めている。

## クイックスタート

### 1. Ollama を起動

```bash
ollama serve
ollama pull qwen3:8b
```

### 2. ビルド

```bash
cargo build --release
./target/release/anvil --help
```

### 3. 対話モード

```bash
./target/release/anvil
```

### 4. ワンショット

```bash
./target/release/anvil -p "README.md を要約して"
echo "src を調べて plan を作って" | ./target/release/anvil --oneshot
```

## CLI

```text
anvil [OPTIONS]

  -p, --prompt <PROMPT>              one-shot prompt
  -m, --model <MODEL>                main model
      --sidecar-model <MODEL>        sidecar model
      --ollama-host <URL>            Ollama base URL (localhost only)
      --context-budget <TOKENS>      message budget for compaction
      --max-iterations <N>           max agent loop iterations
      --debug                        verbose logging
      --stream                       stream assistant text in interactive turns
      --tui                          run the lightweight terminal UI
      --watch                        enable file watcher on startup
      --auto-test <COMMAND>          run a shell command when watcher sees changes
  -y, --yes                          auto-approve Bash / Write / Edit
      --fresh-session                ignore saved session, start new session_id
      --state-dir <PATH>             override XDG state root (default: $XDG_STATE_HOME/anvil)
      --oneshot                      read one prompt from CLI or stdin
```

## スラッシュコマンド

- `/help`
- `/status`
- `/model`
- `/plan`
- `/approve`
- `/compact`
- `/checkpoint [label]`
- `/rollback`
- `/yes`
- `/no`
- `/watch`
- `/autotest <command>`
- `/skills`
- `/skill <name>`
- `/mcp`
- `/logs path [<session_id>]`
- `/parallel task1 || task2`
- `/exit`

## 設定

`.anvil/config` に `key=value` 形式で記述できる。

```ini
model=qwen3:8b
sidecar_model=qwen3:1.7b
ollama_host=http://127.0.0.1:11434
context_budget=24000
max_iterations=12
stream=false
tui=false
watch=false
auto_test_command=
yes_mode=false
```

環境変数も使える。

```bash
export ANVIL_MODEL=qwen3:8b
export ANVIL_SIDECAR_MODEL=qwen3:1.7b
export ANVIL_OLLAMA_HOST=http://127.0.0.1:11434
export ANVIL_CONTEXT_BUDGET=24000
export ANVIL_MAX_ITERATIONS=12
export ANVIL_STREAM=1
export ANVIL_TUI=1
export ANVIL_WATCH=1
export ANVIL_AUTO_TEST="cargo test --lib"
export ANVIL_YES=1
export ANVIL_STATE_DIR=/custom/path/to/anvil-state
```

優先順位は `CLI > 環境変数 > .anvil/config > デフォルト値`。

## 永続化とログの保存先

セッションデータ・LLM I/O ログ・プランは **workdir 外の XDG state 領域**に保存され、`npx create-next-app .` 等の scaffold ツールによる workdir wipe を生き残る。

```
$XDG_STATE_HOME/anvil/           (未設定時: ~/.local/state/anvil)
  sessions/
    {session_id}/
      session.json               ← 会話履歴・モード状態
      logs/
        llm-io.jsonl             ← LLM I/O ログ（--debug 時）
      plans/
        plan.md                  ← Plan mode のプランファイル
```

workdir 側の `.anvil/logs/` `.anvil/sessions/` `.anvil/plans/` は上記へのベストエフォート symlink（削除されても次回起動時に再作成）。

`--state-dir <PATH>` または `ANVIL_STATE_DIR=<PATH>` で保存先を上書きできる。テストでは `ANVIL_STATE_DIR` を tempdir に設定することで実 `$HOME` を汚染しない。

## 安全性

- Ollama host は `localhost` / `127.0.0.1` / `::1` のみ許可
- ツールのファイル操作は workspace 外へ出られない
- Plan mode では `Read` `Glob` `Grep` と plan file のみ許可
- `Bash` `Write` `Edit` は確認対象で、`-y` がない場合は対話承認が必要
- `rm -rf /` など一部の明白に危険な Bash 断片はブロック

## ベンチマーク・レポート

`scripts/` 配下のハーネス群で 5-run ベンチマークと集計レポートを生成できる。

```bash
# 1モデル 5-run ベンチマーク
bash scripts/bench.sh run5 <bench_root> <model_slug>

# 実行結果を JSON に集計
python3 scripts/analyze_run.py <bench_root>/<model_slug>/run-1/

# 全 run を Markdown レポートに集計
python3 scripts/report.py <bench_root>/

# A/B 比較レポート
python3 scripts/report.py --compare <bench_root_a>/ <bench_root_b>/
```

| スクリプト | 役割 |
|-----------|------|
| `scripts/bench.sh` | smoke / 5-run / matrix ベンチマーク実行 |
| `scripts/analyze_run.py` | 1 run-dir を解析して JSON を出力 |
| `scripts/report.py` | BENCH_ROOT 配下の全 run を集計して Markdown を出力 |

## テストと検証

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo test --test e2e_local_llm live_ollama_can_write_a_file -- --ignored --nocapture
ANVIL_E2E_RUNS=2 cargo test --test e2e_local_llm live_ollama_multi_run_file_write_stability -- --ignored --nocapture
cargo build --release
```

## リリースプロセス

リリース手順自体は従来のまま維持している。

- バイナリ名は `anvil`
- `cargo build --release --target ...` で生成
- `.github/workflows/release.yml` が `anvil-linux-*` / `anvil-darwin-*` を gzip 化
- `v*` タグ push で GitHub Release を作成

つまり、内部実装は全面刷新したが、配布導線は壊していない。
