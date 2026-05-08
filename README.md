# Anvil

Ollama 直結の local-first コーディングエージェント。multi-provider 化ではなく、ローカル LLM が苦手にしやすい tool call 崩れ、no-edit ループ、検証不足、コンテキスト汚染を CLI 側の小さな protocol と安全ガードで支える。

## 方針

- Ollama 専用
- tool-first
- Plan / Act の二段階だけを持つ
- session persistence を持つ
- JSON tool call が崩れた場合の XML fallback を持つ
- `Bash` `Read` `Write` `Edit` `Glob` `Grep` を built-in tools として持つ
- WorkMode / protocol / verifier は構造化状態として扱い、会話履歴のノイズから分離する
- deterministic fallback は recovery context であり、それ単体を完了証明にしない
- リリース導線は従来どおり `cargo build --release` と GitHub Releases を維持

## 機能の安定度

| 区分 | 内容 |
| --- | --- |
| Stable | Ollama 直結、Plan / Act、built-in tools、session resume、approval、XML fallback、workspace path guard、localhost host validation |
| Stable | `sessions list/show/clean`、structured logs、`ANVIL.md` project instruction、WorkMode policy、AutoTest candidate selection |
| Experimental | Tester Skill、Temporary Test Workspace、Case Memory、AntiPattern、RepoGraph、Verifier/Reminder skill、dataset export、Photon サイドカー連携（context_pack / evaluate）（詳細: [docs/photon-ops.md](docs/photon-ops.md)） |
| Not implemented | 実 MCP transport、tool-enabled subagent delegation、重い full-screen TUI |
| Known limitations | live Ollama E2E はモデル・量子化・ローカル toolchain に揺らぐ。Bash は完全 sandbox ではなくユーザー権限で実行される |

## 実装済み機能

- Ollama `/api/tags` `/api/chat` への直結クライアント
- 同期 / streaming 両対応の chat loop
- モデル自動選択と optional sidecar 選択
- `$XDG_STATE_HOME/anvil/sessions/{session_id}/session.json` へのセッション保存（workdir 外）
- `/plan` と `/approve` による Plan / Act 切り替え
- Git checkpoint / rollback
- `<think>` 除去と `<tool_call>...</tool_call>` XML fallback
- WorkMode evidence / alternatives を持つ mode classification
- AutoTestRunner の verifier candidate selection
- Tester Skill / Temporary Test Workspace
- Case Memory / AntiPattern / RepoGraph / structured eval log
- 読み取り専用の Plan mode 制御
- live Ollama E2E を含む unit / integration / ignored E2E tests

## クイックスタート

### 1. Ollama を起動

```bash
ollama serve
ollama pull qwen3:8b
```

### 2. インストール

GitHub Releases の prebuilt binary を使う場合:

```bash
curl -fsSL 'https://raw.githubusercontent.com/Kewton/Anvil/main/scripts/install.sh' | bash
anvil --help
```

手元でビルドする場合:

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
anvil [OPTIONS] [COMMAND]

Commands:
  sessions  Inspect / clean stored sessions

Options:
  -p, --prompt <PROMPT>              one-shot prompt
  -m, --model <MODEL>                main model
      --sidecar-model <MODEL>        sidecar model
      --ollama-host <URL>            Ollama base URL (localhost only)
      --context-budget <TOKENS>      message budget for compaction
      --max-iterations <N>           max agent loop iterations
      --chat-timeout-secs <SECONDS>  per-request Ollama chat timeout
      --chat-retries <N>             transport retry count
      --verbose                      anvil-side DEBUG logs
      --trace                        all-crate TRACE logs
      --stream                       stream assistant text in interactive turns
  -y, --yes                          auto-approve Bash / Write / Edit
      --fresh-session                ignore saved session, start new session_id
      --oneshot                      read one prompt from CLI or stdin
      --auto-plan                    classify broad tasks and enter Plan mode first
      --offline                      block network/package-install style shell work
      --deterministic-fallback <MODE>
                                      recovery: off | hint-only | minimal-patch | full-template
                                      aliases: support-only, full
      --no-footer                    disable fixed footer status bar
      --resume [<ID>]                resume latest workspace session or specific UUID
      --state-dir <PATH>             override XDG state root
```

Session subcommands:

```text
anvil sessions list      [--all] [--json]
anvil sessions show      <ID> [--all] [--json]
anvil sessions clean     [--older-than <DAYS>] [--keep <N>] [--all] [--force] [<ID>]
anvil sessions tmp-tests <promote|discard|list> ...
anvil sessions export    [--output <FILE>] [--success-only|--failed-only] [--all] [--session <ID>]
```

`--resume` は直近の `user` メッセージを自動で再投入し、履歴のまま会話を継続する。`--resume <ID>` で UUID v7 を明示指定でき、現在の workspace と一致しない session はエラーになる（他 workspace の閲覧は後述 `sessions show --all` 経由）。`--resume` は `--fresh-session` / `--prompt` / `--oneshot` と排他。

`sessions list|show|clean` は Ollama / Agent を起動せずオフラインで完結する。既定は現 workspace のみが対象で、`--all` で他 workspace 分も表示する。`clean` は既定 dry-run で、実削除には `--force` が必要。`resolve_session_id` が指す現セッションは常に保護される。

## UX（ESC 割り込み）

エージェント実行中（LLM 推論中・ツール実行中）に `ESC` キーを押すと、現在のイテレーションを安全に完了させてから REPL に戻る（`✘ interrupted`）。`Ctrl+C` でプロセス全体を殺す従来の強制終了と異なり、中断時も session は永続化され `--resume` で続行できる。以下の条件で自動的に無効化される:

- stdin が TTY でない（パイプ / リダイレクト / CI）: `echo 'msg' | anvil ...` では ESC 検出が起動しない
- `ANVIL_NO_INTERRUPT` が非空値で設定: ESC 検出を一切行わない（TTY でも無効）
- スラッシュコマンド（`/status` 等）実行中: REPL 境界でのみ rustyline が raw mode を扱うため、interrupt monitor は起動しない
- Bash / Write / Edit で承認（approve prompt）が必要な場合: prompt 表示中は monitor を一時停止（`stdin().read_line` との競合を回避）

割り込み挙動のスコープ:
- ✅ ツール完了後 / LLM 応答完了後の境界で `ExitReason::Interrupted` に遷移
- ❌ 実行中のツール（Bash の子プロセス等）は中断しない — 完了を待つ
- ❌ LLM の mid-flight cancel は行わない — 応答が完了してから break（Ollama 応答境界）

注意: monitor 有効区間（raw mode on）では `Ctrl+C` が SIGINT として発生しなくなる（crossterm の `cfmakeraw` 仕様）。どうしても即殺したい場合は別ターミナルから `kill <pid>` するか、`ANVIL_NO_INTERRUPT=1` で monitor を無効化して起動する。

## UX（スピナー表示）

LLM 推論中・ツール実行中は stderr に 80ms 間隔のスピナーを表示する（例: `⠋ thinking... (gpt-oss-20b) 3s`、`⠙ running Bash... 1s`）。以下の条件で自動的に無効化される:

- stderr が TTY でない（パイプ / リダイレクト）: `anvil -p '...' 2>spinner.log` のように stderr を非 TTY にすると完全無効
- `NO_COLOR` が非空値で設定: モノクロ表示（フレーム文字のみ）
- `ANVIL_NO_SPINNER` が非空値で設定: スピナーを一切描画しない（TTY でも無効）
- `LC_ALL` / `LANG` が UTF-8 でない: ASCII フレーム `| / - \` へフォールバック
- Bash / Write / Edit で承認（approve prompt）が必要な場合: prompt 表示中の干渉回避のためスピナーは起動しない

## Tester Skill

Act-mode で `AutoTestRunner::detect == None`（明示的な test verifier が無い repo）かつ Rust / Node / Python のいずれかが検出されたとき、main model を 1 回だけ同期呼び出し（`tools=None`、JSON-only、`<think>` strip + first JSON object 抽出、`tool_calls` 非空は abort）して smoke test を生成し、`state_root/sessions/<id>/tmp-tests/files/` に保存したうえで固定テンプレートの Bash（Rust: `cargo test --manifest-path ...`、Node: `node --check`、Python: `python3 -m py_compile`）を 30 秒の明示 timeout 付きで実行する。Rust 経路は `tester-runs/<run_id>/` に transient harness（path dependency = workspace package）を書き出して走らせ、終了後に best-effort cleanup する。結果は `FeedbackFrame` として `WorkingMemory.last_feedback` に記録され、Reminder Sidecar 経路と接続して `Precaution` の自動生成に繋がる。

`AutoTestRunner` は verifier を単一キーワードで即決せず、`ANVIL.md` の安全な preferred command、Cargo manifest、`package.json` の `scripts.*`、Python test surface、`py_compile` fallback を候補化して confidence / evidence 付きで選択する。`package.json` は JSON として読み、トップレベルの `"test"` 文字列だけでは `npm test` を選ばない。

Protocol success は work mode ごとに判定される。deterministic fallback はローカル LLM が詰まったときの recovery context として扱い、ファイルが生成されてもそれ単体では完了扱いにしない。完了には model-produced work、または protocol に合う成果物と verifier の通過が必要になる。

以下の条件で自動的に無効化される:

- per-turn cap = 1（`tester_called_this_turn` で同一ターン内 2 回目以降を抑止）
- Plan モード中（`/plan` 解除前は smoke test 生成・実行とも行わない）
- `ANVIL_NO_TESTER` が非空値で設定: Tester Skill を一切起動しない
- `AutoTestRunner::detect` が `Some(_)` を返す repo（`npm test` / `cargo test` with `tests/` / `pytest` 等の明示 verifier が既にある場合は従来の auto_test 経路を使う）

承認モードは runtime から派生する: `--yes` 起動時は Auto、TTY 環境では Interactive（`y` / `yes` 以外は abort）、それ以外（CI / 非 TTY）は Forbidden で即 abort。Tester 起動中は registry 層で `ToolContext.tester_active = true` が立ち、Edit / Write の対象 path が `tmp-tests/` 配下でない場合は reject される。Logging は `agent.tester.{llm_call_started,llm_call_completed,llm_call_failed,completed,failed,skipped}` を `llm-io.jsonl` へ追記する。

## スラッシュコマンド

対話モード（REPL）で利用できる 12 コマンド。これらが Tab 補完の候補になり、`/help` の出力と完全に一致する。

- `/help`
- `/status`
- `/model`
- `/yes`
- `/no`
- `/plan`
- `/approve`
- `/compact`
- `/logs path [<session_id>]`
- `/precautions [add <text>|retire <id>|clear]`
- `/tests`
- `/exit`

エイリアス: `/act`（= `/approve`）, `/quit`（= `/exit`）。補完候補には出さない。

### 未提供（将来構想のプレースホルダ）

以下はコマンドとしては受け付けるが、現行の軽量 core では unavailable を返すだけ。補完候補にも `/help` 出力にも含めない。

- `/checkpoint [label]`
- `/rollback`
- `/watch`
- `/autotest <command>`
- `/skills`
- `/skill <name>`
- `/mcp`
- `/parallel task1 || task2`

### 対話モードの入力

TTY 環境で起動した場合は rustyline ベースの入力ハンドラを使う。上下キーで履歴呼び出し、Tab で上記 11 コマンドの補完、Ctrl+A/E/K/U/C/D が働く。パイプ入力・リダイレクト・CI 等の非 TTY 環境では従来どおり素朴な `read_line` にフォールバックする。

履歴は `$XDG_STATE_HOME/anvil/history`（`--state-dir <PATH>` override を尊重）に最大 1000 行まで保存される。先頭に半角スペースを付けた入力は履歴に保存されないので、機密入力はこの opt-out を使う。

### /help 出力順

`/help` 出力は `/help /status /model /yes /no /plan /approve /compact /logs /precautions /tests /exit` の順に固定している。

## 設定

`.anvil/config` に `key=value` 形式で記述できる。

```ini
model=qwen3:8b
sidecar_model=qwen3:1.7b
ollama_host=http://127.0.0.1:11434
context_budget=24000
max_iterations=12
yes_mode=false
log_level=info            # info | verbose | trace
deterministic_fallback=minimal-patch # off | hint-only | minimal-patch | full-template
```

環境変数も使える。

```bash
export ANVIL_MODEL=qwen3:8b
export ANVIL_SIDECAR_MODEL=qwen3:1.7b
export ANVIL_OLLAMA_HOST=http://127.0.0.1:11434
export ANVIL_CONTEXT_BUDGET=24000
export ANVIL_MAX_ITERATIONS=12
export ANVIL_STREAM=1
export ANVIL_YES=1
export ANVIL_STATE_DIR=/custom/path/to/anvil-state
export ANVIL_LOG_LEVEL=info     # info | verbose | trace
export ANVIL_DETERMINISTIC_FALLBACK=minimal-patch # off | hint-only | minimal-patch | full-template
```

優先順位は `CLI > 環境変数 > .anvil/config > デフォルト値`。

`deterministic_fallback` は product-quality fallback の強さを切り替える。
デフォルトは `minimal-patch`。scaffold / support recovery に限定し、テンプレート単体を
完了扱いにしない。`hint-only` は deterministic write を行わずモデルへの継続ヒントだけを出し、
`full-template` は従来互換のテンプレート補完を明示的に許可する。`off` は deterministic
recovery write を無効化する。互換 alias として `support-only` は `minimal-patch`、
`full` は `full-template` として扱う。
path guard、localhost validation、危険な Bash のブロックなどの安全境界はこの設定に関係なく維持される。

## 永続化とログの保存先

セッションデータ・LLM I/O ログ・プランは **workdir 外の XDG state 領域**に保存され、`npx create-next-app .` 等の scaffold ツールによる workdir wipe を生き残る。

```
$XDG_STATE_HOME/anvil/           (未設定時: ~/.local/state/anvil)
  sessions/
    {session_id}/
      session.json               ← 会話履歴・モード状態
      logs/
        llm-io.jsonl             ← LLM I/O ログ（log level によらず常時保存）
      plans/
        plan.md                  ← Plan mode のプランファイル
```

workdir 側の `.anvil/logs/` `.anvil/sessions/` `.anvil/plans/` は上記へのベストエフォート symlink（削除されても次回起動時に再作成）。

`--state-dir <PATH>` または `ANVIL_STATE_DIR=<PATH>` で保存先を上書きできる。テストでは `ANVIL_STATE_DIR` を tempdir に設定することで実 `$HOME` を汚染しない。

### Session 継続と検査

- `anvil` を引数なしで起動すると、現 workspace 直下にある最新の session を自動復元する（暗黙リストア）。
- `--resume` は復元に加え、**最後の `user` メッセージを自動で再投入** して run を再開する。500 / max_iter で中断した直近ターンの続きを流し直したいときに使う。
- `--resume <UUID>` で session を明示指定できる。UUID v7 以外・symlink・`state_root/sessions/` 外を指すものは拒否され、他 workspace の session もエラーになる。
- `anvil sessions list` で現 workspace の session 一覧（`ID / updated_at / messages / last_tool / mode`）を更新日降順で表示する。`--all` で他 workspace 分も一覧に含め、`workspace_key` 空の古い session は `unassigned` として表示される。
- `anvil sessions show <ID>` は 1 session の概要（`id / workspace_key / active_root / messages 件数 / 先頭 user prompt / 最終 assistant or tool / checkpoints 件数 / mode_state`）を出す。`--json` で機械可読出力（transcript 本体は含まない）。
- `anvil sessions clean` は既定 dry-run。`--older-than 30d` で 30 日超、`--keep 5` で直近 5 件以外を候補にする。`<UUID>` 直接指定も可能。`--force` を付けたときのみ削除する。**現在 `resolve_session_id` が解決する session は常に保護**される。

## 安全性

- Ollama host は `localhost` / `127.0.0.1` / `::1` のみ許可
- ツールのファイル操作は workspace 外へ出られない
- Plan mode では `Read` `Glob` `Grep` と plan file のみ許可
- `Bash` `Write` `Edit` は確認対象で、`-y` がない場合は対話承認が必要
- `rm -rf /` など一部の明白に危険な Bash 断片はブロック

## ベンチマーク・レポート

`scripts/` 配下のハーネス群で 5-run ベンチマークと集計レポートを生成できる。
実践的な E2E/UAT の評価設計は `docs/e2e-uat-evaluation.md` にまとめている。
CI / release confidence checks は [docs/ci.md](docs/ci.md) にまとめている。
Git 管理に含める artifacts の方針は [docs/repository-hygiene.md](docs/repository-hygiene.md) にまとめている。

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
bash scripts/check_cli_help_snapshot.sh
bash scripts/check_repo_hygiene.sh
cargo test --test e2e_local_llm live_ollama_can_write_a_file -- --ignored --nocapture
ANVIL_E2E_RUNS=2 cargo test --test e2e_local_llm live_ollama_multi_run_file_write_stability -- --ignored --nocapture
cargo build --release
```

## リリースプロセス

リリース手順自体は従来のまま維持している。

- バイナリ名は `anvil`
- `cargo build --release --target ...` で生成
- `.github/workflows/release.yml` が `anvil-linux-*` / `anvil-darwin-*` を gzip 化
- 各 artifact に `.sha256` checksum を添付
- `v*` タグ push で GitHub Release を作成
- 対応OS、manual install、Homebrew plan、既知制限は [docs/install.md](docs/install.md) を参照
- release branch の自動チェックと manual live E2E は [docs/ci.md](docs/ci.md) を参照

つまり、内部実装は全面刷新したが、配布導線は壊していない。
