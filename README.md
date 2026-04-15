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
- モデル自動選択と optional sidecar 選択
- `.anvil/sessions/session.json` へのセッション保存
- `/plan` と `/approve` による Plan / Act 切り替え
- Git checkpoint / rollback
- `<think>` 除去と `<tool_call>...</tool_call>` XML fallback
- 読み取り専用の Plan mode 制御
- 主要ツールの unit / integration tests

## まだ入れていないもの

`workspace/v0.1.0/03_rust_port_plan.md` の Phase 6 以降にある file watcher、auto test loop、重い TUI、MCP、skills、parallel agents はまだ未実装。v0.1.0 のコア経路を先に安定させるため、後段へ分離している。

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
  -y, --yes                          auto-approve Bash / Write / Edit
      --fresh-session                ignore saved session
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
- `/exit`

## 設定

`.anvil/config` に `key=value` 形式で記述できる。

```ini
model=qwen3:8b
sidecar_model=qwen3:1.7b
ollama_host=http://127.0.0.1:11434
context_budget=24000
max_iterations=12
yes_mode=false
```

環境変数も使える。

```bash
export ANVIL_MODEL=qwen3:8b
export ANVIL_SIDECAR_MODEL=qwen3:1.7b
export ANVIL_OLLAMA_HOST=http://127.0.0.1:11434
export ANVIL_CONTEXT_BUDGET=24000
export ANVIL_MAX_ITERATIONS=12
export ANVIL_YES=1
```

優先順位は `CLI > 環境変数 > .anvil/config > デフォルト値`。

## 安全性

- Ollama host は `localhost` / `127.0.0.1` / `::1` のみ許可
- ツールのファイル操作は workspace 外へ出られない
- Plan mode では `Read` `Glob` `Grep` と plan file のみ許可
- `Bash` `Write` `Edit` は確認対象で、`-y` がない場合は対話承認が必要
- `rm -rf /` など一部の明白に危険な Bash 断片はブロック

## テストと検証

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo build --release
```

## リリースプロセス

リリース手順自体は従来のまま維持している。

- バイナリ名は `anvil`
- `cargo build --release --target ...` で生成
- `.github/workflows/release.yml` が `anvil-linux-*` / `anvil-darwin-*` を gzip 化
- `v*` タグ push で GitHub Release を作成

つまり、内部実装は全面刷新したが、配布導線は壊していない。
