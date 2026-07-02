# anvilminimal TUI migration plan

## 目的

`mvp/anvilminimal` の現状は `anvil>` prompt を持つ行入力 REPL であり、現行 Anvil の terminal UX primitives はまだ移植対象として十分に定義されていない。

この計画では、`anvilminimal` に移植する TUI を次の範囲に限定して具体化する。

- TTY 起動時の interactive loop
- slash command 実行
- inference / tool / planner 実行中の spinner
- assistant 応答の markdown terminal rendering
- ESC interrupt
- fixed footer status line
- raw mode / prompt 入力の干渉防止。approval prompt は別途実装する場合だけ対象に含める
- non-TTY / env disable / `NO_COLOR` fallback

ここでの TUI は full-screen ratatui app ではない。現行 Anvil の `src/agent/loop_run/{spinner,footer,interrupt}.rs` と `src/tui/markdown.rs` を、MVP 用に縮約して移植する terminal UI 層を指す。

## 現状の不足

既存計画では `REPL/TUI` と書いているが、実装済み MVP は主に `mvp/anvilminimal/src/repl.rs` の line REPL である。

不足しているもの。

- spinner がない
- footer がない
- ESC interrupt がない
- assistant markdown rendering がない
- `ANVIL_NO_SPINNER` / `ANVIL_NO_INTERRUPT` / `ANVIL_NO_FOOTER` / `ANVIL_NO_MARKDOWN` の TUI disable path がない
- raw mode と prompt 入力の境界がない
- TUI 付き interactive loop と one-shot CLI の出力経路が分離されていない

## レビュー反映事項

この計画では次を明示的に補正する。

- `rustyline` は Cargo dependency に存在するが、現状 REPL は `std::io::stdin().read_line` であり未使用。TUI 移植では `rustyline` を採用するか、未使用 dependency を削除するかを Phase T0 の完了条件で決める
- MVP には interactive approval prompt が未実装。TUI Phase では新規 approval UX を暗黙に追加せず、raw mode pause/resume は prompt 入力と将来の approval 境界に限定して設計する
- token usage は provider response にある場合だけ footer に表示し、未取得時は推測値を表示しない
- UI hook と assistant markdown rendering は責務を分ける。spinner/footer/interrupt は `InteractionUi`、terminal markdown は `OutputRenderer` に分離する
- PTY test は platform/tool availability に依存するため、通常 `cargo test` を不安定にしない。ただし release 前の manual/CI UAT では必須 smoke として実行する

## 移植元

| 現行 file | MVP 移植先 | 採用方針 |
|---|---|---|
| `src/agent/loop_run/spinner.rs` | `mvp/anvilminimal/src/tui/spinner.rs` | ほぼ移植。`tracing` 呼び出しは MVP では削除するか simple stderr warning に縮約 |
| `src/agent/loop_run/interrupt.rs` | `mvp/anvilminimal/src/tui/interrupt.rs` | ESC boundary interrupt を移植。raw mode は prompt 入力中に pause する。approval prompt は別途実装する場合だけ同じ扱いにする |
| `src/agent/loop_run/footer.rs` | `mvp/anvilminimal/src/tui/footer.rs` | 状態表示を縮約して移植。token/mode/provider/model/yes/log だけにする |
| `src/tui/markdown.rs` | `mvp/anvilminimal/src/tui/markdown.rs` | SGR-only renderer と `<think>` strip を移植 |
| `mvp/anvilminimal/src/repl.rs` | `mvp/anvilminimal/src/tui/repl.rs` | 既存 REPL を TUI orchestration 層へ移す |
| `src/cli.rs` の `--no-footer` 周辺 | `mvp/anvilminimal/src/cli.rs` | `--no-footer` を追加。engine / legacy flags は追加しない |

## 移植しないもの

- legacy engine / sidecar / Photon / PAM / working memory footer 詳細
- full-screen ratatui layout
- terminal scrollback を破壊する cursor movement-heavy UI
- GUI / browser preview
- provider 固有 UI abstraction

## 追加 dependency

`mvp/anvilminimal/Cargo.toml` に次を追加する。

- `crossterm`: raw mode、ESC key polling、terminal size

既存の `rustyline` は history / line editing に使う。ただし現状 REPL は `std::io::stdin().read_line` であり、`rustyline` dependency は未使用である。Phase T0 で `rustyline` 採用を実装し、採用しない場合は dependency を削除する。

PTY integration test は追加 dependency を増やす前に、標準ツールで安定実行できるか検証する。標準ツールだけで安定しない場合は dev-dependency として PTY helper crate を検討する。通常 unit test は PTY helper の有無に依存させない。

## 目標 UX

次で TUI が起動する。

```bash
anvilminimal --yes --context-budget 65536 \
  --model qwen3.6:27b-coding-nvfp4 \
  --planner-model gemini-3.5-flash \
  --planner-provider gemini \
  --provider ollama
```

起動後。

```text
anvil> /ultra-plan-run --profile nextjs あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。
```

期待挙動。

- prompt 入力中は spinner/footer が入力行を壊さない
- planner / execution provider 呼び出し中は spinner が出る
- tool 実行中は tool 名付き spinner が出る
- assistant 応答は markdown renderer 経由で見やすく表示される
- `<think>...</think>` は terminal に出さない
- footer は token/context/mode/provider/model/yes を表示する。token は provider response から取得できる場合だけ集計し、未取得時は `tokens: n/a` とする
- ESC 入力で「現在の provider/tool 境界後に停止」する
- `ANVIL_NO_*` env と non-TTY では安全に no-op になる

## 新規構造

```text
mvp/anvilminimal/src/tui/
  mod.rs
  terminal.rs
  repl.rs
  slash.rs
  spinner.rs
  footer.rs
  interrupt.rs
  markdown.rs
  status.rs
  tests.rs
```

役割。

| module | 役割 |
|---|---|
| `terminal.rs` | TTY / color / utf8 / env disable 判定 |
| `repl.rs` | TTY interactive loop。既存 `repl.rs` の移動先 |
| `slash.rs` | slash command parse。CLI action と parity を持つ |
| `spinner.rs` | RAII spinner。stderr に描画 |
| `footer.rs` | fixed footer。stdout に描画。`--no-footer` / `ANVIL_NO_FOOTER` 対応 |
| `interrupt.rs` | ESC monitor。raw mode pause/resume 対応 |
| `markdown.rs` | SGR-only assistant renderer。`ANVIL_NO_MARKDOWN` / `NO_COLOR` 対応 |
| `status.rs` | token/mode/provider/model/yes/log の軽量 snapshot |

`mvp/anvilminimal/src/repl.rs` は互換 wrapper にして、最終的に `crate::tui::repl::run` を呼ぶだけにする。

## Runtime UI API

minimal loop / planner runner から terminal 実装へ依存を漏らさないため、MVP では小さい trait を置く。

spinner/footer/interrupt は interaction boundary、markdown は output rendering で責務が違うため、trait を分ける。

```rust
pub trait InteractionUi {
    fn before_model_call(&self, label: &str) -> UiGuard;
    fn before_tool_call(&self, name: &str) -> UiGuard;
    fn publish_status(&self, status: UiStatus);
    fn interrupted(&self) -> bool;
}

pub trait OutputRenderer {
    fn render_assistant(&self, raw_text: &str) -> anyhow::Result<()>;
}
```

実装。

- `TerminalUi`: TTY interactive 用 interaction UI
- `NoopUi`: one-shot / non-TTY / tests 用 interaction UI
- `TerminalMarkdownRenderer`: TTY stdout 用 renderer
- `PlainRenderer`: non-TTY / disabled markdown 用 renderer

`UiGuard` は Drop で spinner を止める。footer freeze も同じ guard に集約する。prompt / spinner / footer / markdown の描画は `TerminalCoordinator` 相当の小さい lock に集約し、同時描画で行を壊さないようにする。

`InteractionUi` は session message を変更しない。session JSON には raw LLM text だけを保存し、`OutputRenderer` は terminal 出力時だけ適用する。

## 実装 Phase

具体的な作業ID、変更先、テストコマンドは `workspace/mvp/anvilminimal_tui_work_breakdown.md` を参照する。

### Phase T0: 現状 REPL を TUI 境界へ分離

作業。

- `src/tui/mod.rs`, `src/tui/repl.rs`, `src/tui/slash.rs`, `src/tui/status.rs` を作る
- 既存 `src/repl.rs` の parser と handler を `tui` 配下へ移す
- `src/repl.rs` は thin wrapper にする
- `rustyline` を採用して history/line editing を実装するか、採用しないなら dependency を削除する

受け入れ条件。

- 既存の `anvilminimal ...` で `anvil>` が出る
- `/plan-run`, `/ultra-plan-run`, `/run-plan`, `/run-ultra-plan` が現行通り動く
- CLI action と slash command の profile/style/path 解釈が一致する
- `rustyline` 採用/削除の判断が完了し、未使用 dependency が残らない
- `cargo test repl` が通る
- `cargo test slash_command` が通る
- `cargo test cli_repl_action_parity` が通る

### Phase T1: markdown renderer

作業。

- `src/tui/markdown.rs` を MVP に移植する
- `ANVIL_NO_MARKDOWN`, `NO_COLOR`, non-TTY fallback を追加する
- one-shot `--prompt` と TUI assistant output の両方で使う

受け入れ条件。

- `<think>` が terminal output に出ない
- code fence / inline code / heading / bold / list が SGR-only で描画される
- SGR 以外の CSI cursor movement を出さない
- long line で buffer が無制限に増えない
- session storage は raw LLM text のまま保持する
- `cargo test markdown` が通る

### Phase T2: spinner

作業。

- `src/agent/loop_run/spinner.rs` を `src/tui/spinner.rs` へ縮約移植する
- model call / planner call / tool execution の境界に `UiGuard` を挿入する
- `ANVIL_NO_SPINNER`, `NO_COLOR`, UTF-8 fallback, non-TTY no-op を実装する

受け入れ条件。

- provider call 中に spinner が出る
- tool execution 中に spinner が出る
- spinner label は control char / escape / bidi を sanitize する
- prompt line / final assistant line の前に spinner が消える
- `ANVIL_NO_SPINNER=1` で thread を spawn しない
- `cargo test spinner` が通る

### Phase T3: ESC interrupt

作業。

- `src/agent/loop_run/interrupt.rs` を `src/tui/interrupt.rs` へ縮約移植する
- `minimal_loop` と `planner::runner` の turn / step / phase 境界で `interrupted()` を確認する
- prompt 入力中は raw mode を pause する
- interactive approval prompt を別途実装する場合だけ、その prompt 中も raw mode を pause する。TUI 移植だけで approval UX を暗黙追加しない
- `ANVIL_NO_INTERRUPT` と non-TTY no-op を実装する

受け入れ条件。

- ESC 入力後、現在の provider/tool call 境界後に停止する
- tool 実行途中の強制 kill はしない
- raw mode が prompt 入力を壊さない
- Drop で raw mode が必ず戻る
- `ANVIL_NO_INTERRUPT=1` で no-op になる
- `cargo test interrupt` が通る
- `cargo test interrupt_boundaries` が通る

### Phase T4: footer

作業。

- `src/agent/loop_run/footer.rs` を `src/tui/footer.rs` へ縮約移植する
- `--no-footer` と `ANVIL_NO_FOOTER` を追加する
- `UiStatus` から mode / provider / model / context budget / token usage / yes を描画する
- provider response の token usage を集計する。Gemini など token usage が取れない provider は `n/a` と表示し、推測値を出さない
- prompt 入力、spinner、markdown output と競合しない freeze protocol を入れる

受け入れ条件。

- TTY では fixed footer が表示される
- non-TTY では no-op
- `--no-footer` / `ANVIL_NO_FOOTER=1` で no-op
- footer 描画が assistant output / prompt line を壊さない
- token usage 未取得時に偽の token count を表示しない
- panic/drop 後に DECSTBM が reset される
- terminal rows < 2 では self-disable する
- `cargo test footer` が通る

### Phase T5: integrated TUI loop

作業。

- `TerminalUi` を `run_repl` に組み込む
- `minimal_loop::run_session_with_required_paths` に `&dyn InteractionUi` または `UiHandle` を渡す
- `planner::generate_*`, `run_step_plan`, `run_ultra_plan` に UI hooks を渡す
- one-shot CLI は `NoopUi` を使う。assistant output は `OutputRenderer` で別処理し、stdout が TTY の場合だけ markdown rendering を有効化できる

受け入れ条件。

- 指定 command で TUI 起動できる
- `/ultra-plan-run --profile nextjs ...3011...` 実行中に planner/model/tool の状態が見える
- ESC で境界停止できる
- footer/spinner/markdown/prompt が互いに表示を壊さない
- `--prompt` / `--plan-run` / `--ultra-plan-run` の non-interactive CLI 出力は既存互換
- non-interactive CLI では spinner/footer/interrupt を起動しない
- `cargo test tui_integration` が通る

### Phase T6: PTY / snapshot UAT

作業。

- PTY integration test を追加する
- `script` または test helper で `anvilminimal` を pseudo-terminal 起動する
- PTY helper が存在しない環境では通常 `cargo test` を skip できるようにする。ただし release/manual UAT では skip を許可しない
- `/exit`, `/ultra-plan`, `/ultra-plan-run` の表示崩れを snapshot / regex で確認する

受け入れ条件。

- PTY 上で `anvil>` が出る
- spinner disabled / footer disabled / markdown disabled の各 env が効く
- `NO_COLOR=1` で色なし表示になる
- ESC interrupt の smoke が通る
- release/manual UAT では PTY smoke を実行し、skip されていないことを確認する
- release binary と symlink 経由で `anvilminimal --help` が通る

## CLI / env 追加

追加 flag。

| flag | 意味 |
|---|---|
| `--no-footer` | fixed footer を無効化する |

追加 env。

| env | 意味 |
|---|---|
| `ANVIL_NO_SPINNER` | spinner を無効化 |
| `ANVIL_NO_INTERRUPT` | ESC interrupt を無効化 |
| `ANVIL_NO_FOOTER` | fixed footer を無効化 |
| `ANVIL_NO_MARKDOWN` | markdown rendering を無効化 |
| `NO_COLOR` | color を無効化 |
| `ANVIL_NO_RESIZE` | footer width polling を無効化 |

## テスト計画

unit。

- `markdown_strips_think`
- `markdown_emits_sgr_only`
- `spinner_disabled_by_env`
- `spinner_sanitizes_label`
- `footer_builds_decstbm`
- `footer_disables_on_small_terminal`
- `interrupt_disabled_by_env`
- `interrupt_pause_resume_raw_mode`
- `slash_command_profile_style_parity`

integration。

- `tui_non_tty_requires_action`
- `tui_prompt_exit`
- `tui_ultra_plan_run_smoke_fake_clients`
- `tui_spinner_footer_do_not_corrupt_prompt_snapshot`
- `tui_markdown_raw_session_storage`
- `tui_pty_smoke`。通常 `cargo test` では platform/helper availability に応じて skip 可。manual/release UAT では必須

manual UAT。

```bash
cargo build --release
anvilminimal --yes --context-budget 65536 \
  --model qwen3.6:27b-coding-nvfp4 \
  --planner-model gemini-3.5-flash \
  --planner-provider gemini \
  --provider ollama
```

REPL。

```text
/ultra-plan-run --profile nextjs あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。
```

PTY smoke。

```bash
ANVIL_PTY_TESTS=1 cargo test tui_pty_smoke -- --ignored
```

この command は release/manual UAT では skip されていないことを確認する。

環境別 UAT。

```bash
ANVIL_NO_SPINNER=1 anvilminimal ...
ANVIL_NO_FOOTER=1 anvilminimal ...
ANVIL_NO_INTERRUPT=1 anvilminimal ...
ANVIL_NO_MARKDOWN=1 anvilminimal ...
NO_COLOR=1 anvilminimal ...
```

## リスクと対策

| リスク | 対策 |
|---|---|
| raw mode が prompt 入力を壊す | interrupt monitor に pause/resume を持たせ、readline 中は必ず pause。approval prompt は別途実装する場合だけ同じ pause 境界に載せる |
| footer と spinner が同じ行を壊す | footer は stdout、spinner は stderr、prompt/assistant 出力前に freeze/clear を必ず通す |
| markdown renderer が footer の scroll region を壊す | SGR-only 出力を test で固定し、cursor movement / clear / DECSTBM を禁止する |
| TUI が non-TTY CI を壊す | `IsTerminal` 判定で no-op。non-TTY で action なしなら明示エラー |
| ESC interrupt が tool/process を中途半端に kill する | mid-flight cancel はしない。provider/tool/phase 境界停止に限定 |
| TUI 移植で MVP core が肥大化する | `tui` module に閉じ、provider/planner/tool は `InteractionUi` の最小 hook だけを見る |
| 既存 REPL 操作性が変わる | slash parser parity test と PTY smoke で固定する |
| PTY test がローカル環境差で不安定になる | 通常 unit test では skip 可能にし、release/manual UAT では `ANVIL_PTY_TESTS=1` で必須実行する |
| token count を footer が偽装する | provider usage がない場合は `n/a` 表示に固定し、推測 token count は表示しない |

## 完了条件

- `anvilminimal ...` で engine 指定なしに TTY interactive UI が起動する
- `anvil>` prompt から `/ultra-plan-run --profile nextjs ...3011...` が実行できる
- spinner / footer / markdown / interrupt が TTY で有効、non-TTY と disable env で no-op
- one-shot CLI の既存操作性を壊さない
- session JSON は raw LLM text を保存し、terminal だけ markdown-rendered text を出す
- ESC interrupt は境界停止として働く
- release/manual UAT で PTY integration smoke が skip されずに通る
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, gated live smoke が通る

## 既存計画への反映ポイント

- `minimal_loop_ultra_plan_migration_plan.md` の Phase 7 は `REPL/TUI` ではなく `TUI terminal primitives` として再定義する
- `anvilminimal_mvp_work_breakdown.md` の Phase 7 に T1-T6 を取り込む
- Phase 8 final acceptance に PTY smoke と disable env smoke を追加する
