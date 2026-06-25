# anvilminimal TUI work breakdown

## 目的

`workspace/mvp/anvilminimal_tui_migration_plan.md` を、実装可能な作業単位へ分解する。

この作業分解は `mvp/anvilminimal` の line REPL を、現行 Anvil 由来の terminal UX primitives を持つ TUI へ拡張するためのもの。full-screen UI は対象外で、TTY 上の prompt / slash command / spinner / footer / markdown / ESC interrupt を対象にする。

## 前提

- 現行 MVP は `mvp/anvilminimal/src/repl.rs` の `std::io::stdin().read_line` ベース REPL
- `rustyline` dependency は存在するが現状未使用
- approval prompt は現行 MVP には未実装
- provider usage token は OpenAI/Ollama では取得可能、Gemini は現状 `None`
- `workspace/` 配下は gitignore 対象のため、この文書を commit する場合は `git add -f` が必要

## 成果物

| 成果物 | path |
|---|---|
| TUI module | `mvp/anvilminimal/src/tui/*` |
| CLI flag | `mvp/anvilminimal/src/cli.rs` |
| config field | `mvp/anvilminimal/src/config.rs` |
| runtime hook | `mvp/anvilminimal/src/minimal_loop/loop_run.rs`, `src/planner/runner.rs` |
| tests | module unit tests, `mvp/anvilminimal/tests/tui_*.rs` |
| docs | `mvp/anvilminimal/README.md` |

## 実装順序

| Phase | 目的 | 依存 | 完了判定 |
|---|---|---|---|
| T0 | REPL を TUI 境界へ分離 | なし | line REPL の既存操作性が維持される |
| T1 | markdown renderer | T0 | raw session / rendered terminal の分離 |
| T2 | spinner | T0 | provider/tool 境界の spinner |
| T3 | interrupt | T0/T2 | ESC 境界停止 |
| T4 | footer | T0/T2 | prompt/spinner/markdown と競合しない footer |
| T5 | integrated TUI loop | T1-T4 | TTY UI と non-interactive CLI の分離 |
| T6 | PTY/UAT | T5 | TTY 実行の snapshot/smoke |

## Phase T0: REPL を TUI 境界へ分離

目的: 既存の `repl.rs` を壊さず、TUI module へ移す足場を作る。

| ID | 作業 | 変更先 | テスト |
|---|---|---|---|
| T0-01 | `src/tui/mod.rs` を追加する | `src/tui/mod.rs` | compile |
| T0-02 | 既存 REPL を `src/tui/repl.rs` に移す | `src/repl.rs`, `src/tui/repl.rs` | `cargo test repl` |
| T0-03 | slash parser を `src/tui/slash.rs` に分離する | `src/tui/slash.rs` | quoted/profile/style/path tests |
| T0-04 | `src/repl.rs` を wrapper にする | `src/repl.rs` | public API compatibility |
| T0-05 | `rustyline` 採用判断を実装で確定する | `Cargo.toml`, `src/tui/repl.rs` | dependency unused check |
| T0-06 | non-TTY action なしの error を維持する | `src/tui/repl.rs` | `tui_non_tty_requires_action` |
| T0-07 | CLI/REPL action parity fixture を追加する | `tests/tui_repl.rs` | `cli_repl_action_parity` |

受け入れ条件。

- `anvilminimal ...` で `anvil>` prompt が出る
- `/exit` と `/quit` が動く
- `/plan-run`, `/ultra-plan-run`, `/run-plan`, `/run-ultra-plan` が既存通り dispatch される
- `--profile` / `--style` / path 解釈が CLI と一致する
- `rustyline` を採用する場合は history file path と raw mode 境界を実装する
- `rustyline` を採用しない場合は dependency を削除する

targeted test。

```bash
cargo test repl
cargo test slash_command
cargo test cli_repl_action_parity
```

## Phase T1: markdown renderer

目的: terminal には markdown-rendered text を出し、session には raw LLM text を保存する。

| ID | 作業 | 変更先 | テスト |
|---|---|---|---|
| T1-01 | `src/tui/markdown.rs` を移植する | `src/tui/markdown.rs` | markdown unit tests |
| T1-02 | `OutputRenderer` trait を定義する | `src/tui/mod.rs` or `src/tui/output.rs` | trait compile |
| T1-03 | `TerminalMarkdownRenderer` を実装する | `src/tui/markdown.rs` | SGR-only snapshot |
| T1-04 | `PlainRenderer` を実装する | `src/tui/markdown.rs` | disabled/no-color tests |
| T1-05 | `<think>` strip の cross-chunk test を移植する | `src/tui/markdown.rs` | think block tests |
| T1-06 | long line bounded buffer test を移植する | `src/tui/markdown.rs` | max buffer test |
| T1-07 | one-shot output と TUI output へ renderer を接続する | `src/lib.rs`, `src/tui/repl.rs` | raw session storage test |

受け入れ条件。

- `<think>` は terminal に出ない
- session JSON には raw LLM text が残る
- SGR 以外の CSI cursor movement / clear / DECSTBM を renderer が出さない
- `ANVIL_NO_MARKDOWN=1` で plain output
- `NO_COLOR=1` で color なし
- non-TTY stdout では plain output

targeted test。

```bash
cargo test markdown
cargo test tui_markdown_raw_session_storage
```

## Phase T2: spinner

目的: provider/planner/tool の待ち時間を TTY で見えるようにし、prompt や final output を壊さない。

| ID | 作業 | 変更先 | テスト |
|---|---|---|---|
| T2-01 | `src/tui/spinner.rs` を移植する | `src/tui/spinner.rs` | spinner env tests |
| T2-02 | label sanitizer を移植する | `src/tui/spinner.rs` | control/escape/bidi tests |
| T2-03 | `UiGuard` を定義する | `src/tui/mod.rs` | drop stops spinner |
| T2-04 | model/planner call 境界を hook する | `src/providers/*`, `src/planner/runner.rs` | fake UI guard calls |
| T2-05 | tool execution 境界を hook する | `src/minimal_loop/loop_run.rs` | tool spinner fake test |
| T2-06 | `ANVIL_NO_SPINNER` / non-TTY no-op を実装する | `src/tui/spinner.rs` | no thread spawn test |
| T2-07 | spinner clear before prompt/final output を保証する | `src/tui/repl.rs`, `src/tui/spinner.rs` | output snapshot |

受け入れ条件。

- TTY では provider/planner/tool 実行中に spinner が出る
- non-TTY では spinner が起動しない
- `ANVIL_NO_SPINNER=1` で worker thread を spawn しない
- label に escape/control/bidi が混ざっても terminal escape injection しない
- prompt 行と assistant final output の前に spinner が消える

targeted test。

```bash
cargo test spinner
cargo test tui_spinner_footer_do_not_corrupt_prompt_snapshot
```

## Phase T3: ESC interrupt

目的: ESC による境界停止を追加し、raw mode が prompt 入力を壊さないようにする。

| ID | 作業 | 変更先 | テスト |
|---|---|---|---|
| T3-01 | `src/tui/interrupt.rs` を移植する | `src/tui/interrupt.rs` | interrupt env tests |
| T3-02 | `InterruptFlag` / `InterruptMonitor` を MVP 用に縮約する | `src/tui/interrupt.rs` | preset flag tests |
| T3-03 | prompt 入力中の pause/resume を実装する | `src/tui/repl.rs` | pause/resume fixture |
| T3-04 | minimal loop iteration 境界に interrupt check を入れる | `src/minimal_loop/loop_run.rs` | boundary stop test |
| T3-05 | step/phase 境界に interrupt check を入れる | `src/planner/runner.rs` | phase stop test |
| T3-06 | `ANVIL_NO_INTERRUPT` / non-TTY no-op を実装する | `src/tui/interrupt.rs` | no-op test |
| T3-07 | Drop raw mode restore test を追加する | `src/tui/interrupt.rs` | raw mode restoration |

受け入れ条件。

- ESC 後、現在の provider/tool call が終わった境界で停止する
- tool process を mid-flight kill しない
- prompt 入力中は raw mode を pause する
- Drop 後に raw mode が戻る
- `ANVIL_NO_INTERRUPT=1` で no-op
- interactive approval prompt はこの Phase で新規実装しない

targeted test。

```bash
cargo test interrupt
cargo test interrupt_boundaries
```

## Phase T4: footer

目的: context/model/mode/provider/yes/token を TTY footer で表示しつつ、出力を壊さない。

| ID | 作業 | 変更先 | テスト |
|---|---|---|---|
| T4-01 | `src/tui/footer.rs` を移植する | `src/tui/footer.rs` | footer pure ansi tests |
| T4-02 | `--no-footer` を追加する | `src/cli.rs`, `src/config.rs` | CLI parse/help test |
| T4-03 | `ANVIL_NO_FOOTER` / non-TTY no-op を実装する | `src/tui/footer.rs` | env no-op test |
| T4-04 | `UiStatus` を定義する | `src/tui/status.rs` | status snapshot test |
| T4-05 | provider token usage を status へ流す | `src/providers/*`, `src/minimal_loop/loop_run.rs` | token aggregation test |
| T4-06 | token unknown は `n/a` 表示に固定する | `src/tui/footer.rs` | no fake token test |
| T4-07 | footer freeze/clear coordinator を実装する | `src/tui/footer.rs`, `src/tui/mod.rs` | no prompt corruption snapshot |
| T4-08 | DECSTBM reset / panic/drop cleanup を実装する | `src/tui/footer.rs` | reset snapshot |

受け入れ条件。

- TTY では footer が出る
- `--no-footer` と `ANVIL_NO_FOOTER=1` で no-op
- non-TTY では no-op
- token usage がない provider は `tokens: n/a`
- prompt / spinner / markdown output を footer が壊さない
- rows < 2 では self-disable
- Drop/panic path で DECSTBM reset

targeted test。

```bash
cargo test footer
cargo test tui_spinner_footer_do_not_corrupt_prompt_snapshot
```

## Phase T5: integrated TUI loop

目的: T1-T4 を REPL と runtime に接続し、interactive TUI と non-interactive CLI を分離する。

| ID | 作業 | 変更先 | テスト |
|---|---|---|---|
| T5-01 | `InteractionUi` trait を実装する | `src/tui/mod.rs` | trait tests |
| T5-02 | `TerminalUi` を実装する | `src/tui/mod.rs` | TTY fake tests |
| T5-03 | `NoopUi` を実装する | `src/tui/mod.rs` | non-TTY tests |
| T5-04 | `OutputRenderer` を `lib.rs` one-shot path に接続する | `src/lib.rs` | one-shot output compatibility |
| T5-05 | `TerminalUi` を `run_repl` に接続する | `src/tui/repl.rs` | TUI prompt smoke |
| T5-06 | `InteractionUi` を minimal loop に渡す | `src/minimal_loop/loop_run.rs` | fake guard call order |
| T5-07 | `InteractionUi` を planner runner に渡す | `src/planner/runner.rs` | plan/ultra guard calls |
| T5-08 | interrupt stop reason を user-facing error にする | loop/runner | boundary stop message |
| T5-09 | non-interactive CLI で spinner/footer/interrupt を起動しない | `src/lib.rs`, `src/tui/mod.rs` | non-interactive no-op test |

受け入れ条件。

- `anvilminimal ...` で TTY interactive UI が起動する
- `/ultra-plan-run --profile nextjs ...3011...` 実行中に planner/model/tool state が見える
- non-interactive `--prompt`, `--plan-run`, `--ultra-plan-run` の stdout は既存互換
- session JSON は raw LLM text
- terminal だけ markdown-rendered text
- ESC は境界停止

targeted test。

```bash
cargo test tui_integration
cargo test tui_non_tty_requires_action
cargo test tui_prompt_exit
cargo test tui_ultra_plan_run_smoke_fake_clients
```

## Phase T6: PTY / snapshot UAT

目的: unit test だけでは拾えない TTY 表示崩れを PTY で検出する。

| ID | 作業 | 変更先 | テスト |
|---|---|---|---|
| T6-01 | PTY smoke test helper 方針を決める | `tests/tui_pty.rs` | helper availability |
| T6-02 | `/exit` PTY smoke を追加する | `tests/tui_pty.rs` | `tui_pty_exit` |
| T6-03 | disabled env PTY smoke を追加する | `tests/tui_pty.rs` | spinner/footer/markdown disabled |
| T6-04 | `NO_COLOR=1` PTY smoke を追加する | `tests/tui_pty.rs` | no SGR color snapshot |
| T6-05 | ESC interrupt PTY smoke を追加する | `tests/tui_pty.rs` | boundary interrupt smoke |
| T6-06 | release/manual UAT command を README に追記する | `README.md` | docs command present |
| T6-07 | symlink release binary UAT を追記する | `README.md` | `anvilminimal --help` |

受け入れ条件。

- 通常 `cargo test` は PTY helper 不在で不安定化しない
- `ANVIL_PTY_TESTS=1 cargo test tui_pty_smoke -- --ignored` は release/manual UAT で skip されずに通る
- PTY 上で `anvil>` が見える
- spinner/footer/markdown disabled env が効く
- `NO_COLOR=1` が効く
- ESC interrupt smoke が通る

targeted test。

```bash
ANVIL_PTY_TESTS=1 cargo test tui_pty_smoke -- --ignored
```

## Cross-cutting tasks

| ID | 作業 | 変更先 | テスト |
|---|---|---|---|
| X-01 | README の TUI section を更新する | `README.md` | docs grep |
| X-02 | help に `--no-footer` を追加する | `cli.rs` | help snapshot |
| X-03 | copy validation に TUI docs を含める | README / docs | clean copy smoke |
| X-04 | `.gitignore` に PTY snapshot temp を入れる必要があれば追加 | `.gitignore` | no generated file status |
| X-05 | `workspace/mvp/anvilminimal_mvp_work_breakdown.md` Phase 7 と整合を保つ | workspace docs | doc consistency check |

## 完了条件

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- `ANVIL_LIVE_PROVIDER_TESTS=1 cargo test live_ -- --ignored`
- `ANVIL_PTY_TESTS=1 cargo test tui_pty_smoke -- --ignored` が release/manual UAT で skip されずに通る
- `cargo build --release`
- `anvilminimal --help` に `--no-footer` が出る
- TTY で `anvilminimal ...` が `anvil>` prompt を出す
- `/ultra-plan-run --profile nextjs ...3011...` が TUI から実行できる
- spinner/footer/markdown/interrupt が TTY で有効、disable env で no-op
- non-interactive CLI の stdout が既存互換
- session JSON は raw LLM text を保持する
- clean copy 先で `cargo test` と `cargo run -- --help` が通る

## Final command set

`mvp/anvilminimal` で実行する。

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
ANVIL_LIVE_PROVIDER_TESTS=1 cargo test live_ -- --ignored
cargo build --release
cargo run -- --help
```

release/manual UAT。

```bash
ANVIL_PTY_TESTS=1 cargo test tui_pty_smoke -- --ignored
anvilminimal --yes --context-budget 65536 --model qwen3.6:27b-coding-nvfp4 --planner-model gemini-3.5-flash --planner-provider gemini --provider ollama
```

REPL 入力。

```text
/ultra-plan-run --profile nextjs あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。
```

disable env smoke。

```bash
ANVIL_NO_SPINNER=1 anvilminimal --yes --context-budget 65536 --model qwen3.6:27b-coding-nvfp4 --planner-model gemini-3.5-flash --planner-provider gemini --provider ollama
ANVIL_NO_FOOTER=1 anvilminimal --yes --context-budget 65536 --model qwen3.6:27b-coding-nvfp4 --planner-model gemini-3.5-flash --planner-provider gemini --provider ollama
ANVIL_NO_INTERRUPT=1 anvilminimal --yes --context-budget 65536 --model qwen3.6:27b-coding-nvfp4 --planner-model gemini-3.5-flash --planner-provider gemini --provider ollama
ANVIL_NO_MARKDOWN=1 anvilminimal --yes --context-budget 65536 --model qwen3.6:27b-coding-nvfp4 --planner-model gemini-3.5-flash --planner-provider gemini --provider ollama
NO_COLOR=1 anvilminimal --yes --context-budget 65536 --model qwen3.6:27b-coding-nvfp4 --planner-model gemini-3.5-flash --planner-provider gemini --provider ollama
```

## 実装時の注意

- TUI は provider 固有構造を知らない
- `InteractionUi` は session を変更しない
- `OutputRenderer` は terminal 出力だけに使う
- footer は token usage を推測しない
- prompt 入力中は interrupt raw mode を pause する
- approval prompt はこの計画では新規実装しない
- PTY smoke は release/manual UAT で skip されないことを確認する
- generated PTY snapshot / temp file を commit しない
