# anvilminimal MVP 移植計画

## 目的

`src` 配下の既存実装は変更せず、`minimal loop + YAML step plan + plan run + ultra plan run + deterministic verify` を他リポジトリへコピー可能な MVP として `./mvp` 配下へ切り出す。

移植後の成果物は独立した Rust CLI バイナリ `anvilminimal` として動作し、現在の minimal engine と同等の操作感を維持する。

## 必須操作性

CLI から engine 指定なしで TUI/REPL を起動できること。

```bash
anvilminimal --yes --context-budget 65536 --model qwen3.6:27b-coding-nvfp4 --planner-model gemini-3.5-flash --planner-provider gemini --provider ollama
```

REPL から ultra plan run を実行できること。

```text
/ultra-plan-run --profile nextjs あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。
```

CLI 引数からも同等に実行できること。

```bash
anvilminimal --provider ollama --model qwen3.6:27b-coding-nvfp4 \
  --planner-provider gemini --planner-model gemini-3.5-flash \
  --ultra-plan-run --profile nextjs \
  "あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。"
```

## API key / model 方針

MVP は `.env` と process env の両方から API key を読む。process env を優先し、未設定の場合だけ `.env` を fallback とする。

使用する key。

- `OPENAI_API_KEY`
- `GEMINI_API_KEY`

標準検証に使う model。

| 用途 | provider | model |
|---|---|---|
| OpenAI execution smoke | `openai` | `gpt-5.4-mini` |
| Gemini planner standard | `gemini` | `gemini-3.5-flash` |
| Gemini fast/light smoke | `gemini` | `gemini-3.1-flash-lite` |

API key は stdout/stderr/session/log に値を出さない。`llm-io.jsonl` に provider request を保存する場合も auth header と key value は redaction する。

model ID はユーザー指定の設定値として扱う。unit / fixture test は request payload に指定 model が保持されることを検査し、provider 側の availability は live smoke の結果として扱う。外部 API の model rename / account permission / regional availability に備え、検証時は `ANVIL_OPENAI_SMOKE_MODEL` / `ANVIL_GEMINI_SMOKE_MODEL` で上書きできるようにする。

検証コマンド例。

```bash
anvilminimal --yes --provider openai --model gpt-5.4-mini --prompt "README.md に短い smoke note を作成してください"

anvilminimal --yes --provider ollama --model qwen3.6:27b-coding-nvfp4 \
  --planner-provider gemini --planner-model gemini-3.5-flash \
  --plan-steps "小さい Rust CLI を作る"

anvilminimal --yes --provider gemini --model gemini-3.1-flash-lite \
  --prompt "notes/smoke.md に Gemini smoke note を作成してください"
```

## 現状実装の切り出し対象

### Core

| 機能 | 現在の実装 |
|---|---|
| minimal tool loop | `src/agent/minimal_loop/*` |
| 1 turn wrapper / REPL command | `src/agent/minimal_repl.rs` |
| step plan / plan run / ultra plan run | `src/agent/minimal_step_runner.rs` |
| plan lint | `src/agent/minimal_step_runner/plan_lint.rs` |
| deterministic verify | `src/agent/minimal_step_runner/verify.rs` |
| repair prompt / exhausted report | `src/agent/minimal_step_runner/repair.rs` |
| ultra profile contract | `src/agent/minimal_step_runner/profile.rs` |
| nextjs/data profiles | `src/agent/minimal_step_runner/profiles/*` |
| intent detection | `src/agent/minimal_step_runner/intent.rs` |

### Provider adapter

| 機能 | 現在の実装 |
|---|---|
| execution LLM adapter | `src/agent/minimal_llm.rs` |
| planner LLM trait/adapter | `src/agent/planner_llm.rs` |
| Ollama client | `src/ollama/*` |
| OpenAI client | `src/openai.rs` |
| Gemini client | `src/gemini.rs` |
| API key resolution | `src/api_keys.rs` |

### Minimal runtime dependency

| 機能 | 現在の実装 |
|---|---|
| tool registry | `src/tools/registry.rs` |
| built-in tools | `src/tools/{bash,read,write,edit,glob,grep}.rs` |
| session snapshot/store | `src/session/store.rs` |
| Plan/Act mode | `src/modes/plan_act.rs` |
| path guard | `src/safety/path_guard.rs` |
| workspace policy | `src/util/workspace_paths.rs` |

## 実測 LOC

`cloc --timeout 0` で確認した production 相当の実コード行数。`#[cfg(test)] mod tests` は外して算出。

| 範囲 | LOC |
|---|---:|
| minimal loop | 872 |
| step/ultra runner | 2,953 |
| REPL + LLM adapter | 608 |
| **core 合計** | **4,433** |

依存モジュールを現行からそのまま持つ場合の production LOC。

| 範囲 | LOC |
|---|---:|
| tools + path/workspace safety | 2,891 |
| session/config/cli/mode/model 周辺 | 2,524 |
| provider client 周辺 | 2,318 |

MVP は依存をそのまま丸ごと移すのではなく、minimal に必要な形へ縮約する。目標規模は **7k-8k LOC**。現行依存を丸ごと移す場合は **12k LOC 前後** になるため避ける。

スコープは 2 層に分ける。

| 層 | 含めるもの | LOC 目標への扱い |
|---|---|---|
| core MVP | CLI/REPL, provider, tools, minimal loop, plan run, ultra plan run, deterministic verify | 7k-8k LOC の対象 |
| verification harness | benchmark/recheck/seed/compare scripts, 25 scenarios, mechanism-ledger template | copy artifact には含めるが、core LOC 目標とは別枠 |

Phase 8 の harness は移植品質を守るために必要だが、core 実装へ混ぜ込まない。`src/eval` へ runtime 依存を増やす場合は、CLI 本体から独立した module として feature / script 境界を切る。

## 確認した関連資料と反映方針

今回の計画は、`workspace/v0.6.6` と `docs/eval` の評価・triage 資料を前提に更新する。

確認対象。

| 種別 | 主な資料 | 計画への反映 |
|---|---|---|
| v0.6.6 40-run / 6-round 評価 | `workspace/v0.6.6/README.md` | 低成功率の主因を recovery 追加ではなく generation / evidence / hard gate の不足として扱う |
| minimal loop の作業メモ | `workspace/v0.6.6/state-control-instruction-insights-20260606.md`, `workspace/v0.6.6/local-llm-instruction-state-control-analysis-20260606.md` | `RequiredFiles`, `EvidenceCommand`, `NoProseUntil`, controller-side evidence を plan/run の contract として扱う |
| 比較/repair ハーネス | `workspace/v0.6.6/architecture_probe_validation.py`, `workspace/v0.6.6/controller_owned_lifecycle_probe.py` | bounded repair action、post-action evidence rerun、controller-owned lifecycle を test fixture にする |
| controller-owned lifecycle | `workspace/v0.6.6/controller-owned-lifecycle-synthesis-20260606.md` | WorkMode より ObjectiveLifecycle を上位に置き、LLM の final prose だけで Done にしない |
| P0-P36 検証メモ | `workspace/v0.6.6/p*.md` | missing deliverable before evidence、evidence stage、obligation、evidence runner、repair idempotency、Rust manifest/test binding を受け入れ条件へ落とす |
| benchmarks 25 本 | `docs/eval/mechanism-ledger.md`, `workspace/eval-artifacts/minimal-loop-plan-20260612/*/summary.tsv` | `minimal-loop-expanded` 25 scenarios x 5 runs を MVP benchmark harness の対象にする |
| recheck / seed / 比較ハーネス | `scripts/bench.sh`, `scripts/compare.py`, `docs/eval/seed-smoke.md`, `docs/eval/minimal-loop-t2-4-recheck-20260612.md` | seed 記録、GPU-free recheck、summary.tsv / summary.recheck.tsv 互換を Phase 8 の条件にする |
| triage 文書群 | `docs/eval/triage/*.md` | blocked mkdir/script、parser feedback loop、no-tool progress stop、check false negative を regression fixture にする |
| mechanism-ledger | `docs/eval/mechanism-ledger.md` | M001/M002 の off flag、admission benchmark、audit status を維持し、新機構追加は測定付きに限定する |

反映する設計原則。

- 成功判定は LLM の自己申告ではなく deterministic verify / profile verifier / postcheck が決める
- exact required path は hard gate にする
- evidence は exit code だけでなく quality gate を見る
- Node は 0 tests を success にしない
- Rust は `Cargo.toml` / manifest-path / explicit `[[test]]` / parent workspace guard を検査する
- docs/data は heading/schema/key/count を controller-side に検査する
- repair は bounded action 後に必ず evidence を再実行する
- parser/tool-call fallback は provider ごとの native mode と矛盾させない
- 新しい feedback / recovery mechanism は `mechanism-ledger` 形式で off flag と admission benchmark を持たせる

## 目標ディレクトリ構造

他リポジトリへ `mvp/anvilminimal` をコピーできる構造にする。

```text
mvp/anvilminimal/
  Cargo.toml
  README.md
  src/
    main.rs
    lib.rs
    cli.rs
    config.rs
    state.rs
    mode.rs
    providers/
      mod.rs
      ollama.rs
      openai.rs
      gemini.rs
      gemini_function_calling.rs
      parsing.rs
      xml_fallback.rs
    tools/
      mod.rs
      registry.rs
      bash.rs
      read.rs
      write.rs
      edit.rs
      glob.rs
      grep.rs
      path_guard.rs
      workspace_policy.rs
    minimal_loop/
      mod.rs
      loop_run.rs
      prompt.rs
      feedback.rs
      compact.rs
    planner/
      mod.rs
      step_plan.rs
      ultra_plan.rs
      runner.rs
      verify.rs
      repair.rs
      lint.rs
      intent.rs
      profile.rs
      profiles/
        mod.rs
        nextjs.rs
        data.rs
    repl.rs
    eval/
      mod.rs
      benchmark.rs
      recheck.rs
      postcheck.rs
      seed.rs
  benchmarks/
    minimal-loop-expanded.yaml
    minimal-loop-large.yaml
  scripts/
    bench.sh
    compare.py
  tests/
    fixtures/
      gemini_function_calling/
      openai_tool_calls/
      plans/
      benchmarks/
```

## 移植方針

### 1. 元コードは変更しない

`src/agent/minimal_*` から `mvp/anvilminimal/src/*` へコピーし、移植先で依存を縮約する。既存 `src` の module path や public API は変更しない。

### 2. `Engine::Minimal` を廃止する

`anvilminimal` は minimal 専用バイナリなので `--engine minimal` を不要にする。現在 `Config` にある `Engine` 分岐は移植先では持たない。

CLI は現在の minimal 系フラグを維持する。

```text
--prompt
--model
--provider
--planner-model
--planner-provider
--ollama-host
--context-budget
--num-predict
--max-iterations
--chat-timeout-secs
--chat-retries
--yes
--fresh-session
--resume
--state-dir
--cwd
--offline
--plan-steps
--plan-run
--run-plan
--ultra-plan
--ultra-plan-run
--run-ultra-plan
--profile
--ultra-style
```

`--sidecar-model` は MVP では採用しない。現行 Anvil の sidecar は legacy engine の補助モデル選択に紐づく概念であり、minimal loop / step plan / ultra plan run の実行経路では sidecar を使わない。MVP では「実行モデル」と「計画モデル」を `--model` / `--provider` と `--planner-model` / `--planner-provider` で明示的に分ける。

### CLI 引数の意味

| 引数 | 意味 |
|---|---|
| `--prompt`, `-p` | REPL を起動せず、指定した 1 prompt を minimal loop で実行する |
| `--model`, `-m` | tool 実行を伴う main execution model 名 |
| `--provider` | main execution model の provider。`ollama`, `openai`, `gemini` |
| `--planner-model` | plan YAML / ultra plan YAML を生成する planner model 名。省略時は provider が同じなら `--model` を使う |
| `--planner-provider` | planner model の provider。省略時は `--provider` と同じ |
| `--ollama-host` | Ollama API host。例: `http://localhost:11434` |
| `--context-budget` | prompt compaction / context budgeting の上限 token 目安 |
| `--num-predict` | Ollama の生成 token 上限。MVP では provider 別に未対応なら無視せず warning を出す |
| `--max-iterations` | 1 turn 内で LLM response と tool execution を繰り返す最大回数 |
| `--chat-timeout-secs` | provider chat API 1 回あたりの timeout 秒数 |
| `--chat-retries` | provider chat API 失敗時の retry 回数 |
| `--yes`, `-y` | approval prompt を省略し、許可可能な tool 実行を自動承認する |
| `--fresh-session` | 既存 session を resume せず、新規 session で開始する |
| `--resume [ID]` | ID 指定時は該当 session、ID 省略時は現在 workspace の最新 session を再開する |
| `--state-dir` | session / log 保存先 root を上書きする |
| `--cwd` | 実行 workspace root。通常は内部/テスト用で、通常利用ではカレントディレクトリを使う |
| `--offline` | network/setup 系や mutating command の一部を tool policy で拒否する |
| `--plan-steps <goal>` | goal から step plan YAML だけを生成し、`.anvil/plans` に保存する |
| `--plan-run <goal>` | goal から step plan YAML を生成し、そのまま step ごとに実行する |
| `--run-plan <file>` | 既存 step plan YAML を読み込んで実行する |
| `--ultra-plan <goal>` | goal から ultra phase plan YAML だけを生成し、`.anvil/plans` に保存する |
| `--ultra-plan-run <goal>` | ultra phase plan YAML を生成し、phase ごとに step plan を生成して実行する |
| `--run-ultra-plan <file>` | 既存 ultra phase plan YAML を読み込んで実行する |
| `--profile` | ultra plan/profile verifier。MVP では `generic`, `nextjs`, `data` |
| `--ultra-style` | ultra plan の計画スタイル。`default`, `tdd`, `test-hardening` |

TUI/REPL 起動条件は「prompt/stdin/plan 指定がなく、stdin が TTY」で固定する。

### 3. session は minimal 専用に縮小する

移植先の `state.rs` は以下だけを持つ。

- `SessionSnapshot`
- `SessionStore`
- `ConversationMessage`
- `ModeState`
- `native_tools_disabled`
- `active_root`
- `workspace_key`
- `messages`

既存の `WorkingMemory`、Photon、case record、precaution、tmp-tests、eval log は MVP から除外する。

保存先は現行と互換に近い形を維持する。

```text
${XDG_STATE_HOME:-$HOME/.local/state}/anvilminimal/sessions/<uuid>/session.json
${XDG_STATE_HOME:-$HOME/.local/state}/anvilminimal/sessions/<uuid>/logs/llm-io.jsonl
```

`.anvil/plans` と `.anvil/repairs` は workspace 側に保存する。現行の plan YAML をそのまま読めることを優先する。

### 4. ToolRegistry は minimal 専用に縮小する

必要 tool は 6 個だけ。

- `Bash`
- `Read`
- `Write`
- `Edit`
- `Glob`
- `Grep`

削るもの。

- legacy PlanStage scope
- tester/tmp-tests 専用制約
- photon/eval/report 連携
- legacy Agent の artifact/task contract hooks

残すもの。

- workspace-relative path confinement
- Plan mode では `Read/Glob/Grep` のみ許可
- dangerous shell command block
- offline mode の mutating/network command block
- `--yes` による approval bypass
- `Bash` は verify/build/test/local script 用

workspace-relative path confinement の定義。

- tool input の path は workspace root からの相対 path として解釈する
- absolute path、`..` component、NUL byte、Windows drive prefix / UNC 形式は拒否する
- 既存 path は canonicalize 後に workspace root 配下であることを確認する
- 新規作成 path は既存の親 directory を canonicalize し、親が workspace root 配下であることを確認してから作成する
- symlink 経由で workspace 外へ出る path は拒否する
- user/LLM に返す表示 path は workspace-relative に戻し、host absolute path を通常出力に混ぜない
- `Bash` は cwd を workspace root に固定し、verify/build/test/local read-only script 以外の command は dangerous/offline policy と合わせて判定する
- `$(cat relative/path)` のような goal reference 展開も同じ path guard を通す

例: workspace root が `/repo/app` の場合、`src/main.rs` は許可、`/etc/passwd`、`../secret.txt`、`link/passwd` かつ `link -> /etc` は拒否する。

### 5. provider は `MinimalChatClient` と `PlannerLlm` だけに統一する

移植先では provider 境界を次に固定する。

```rust
trait ChatClient {
    fn supports_native_tools(&self, model: &str) -> bool;
    fn chat(
        &mut self,
        model: &str,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        native_tools_enabled: bool,
    ) -> Result<AssistantReply, String>;
}

trait PlannerLlm {
    fn chat_plan(&mut self, messages: &[ConversationMessage]) -> Result<AssistantReply, String>;
    fn label(&self) -> String;
}
```

`--provider` は execution model、`--planner-provider` は plan generation model。異なる provider の組み合わせを許可する。

必須対応。

- `--provider ollama`
- `--provider openai`
- `--provider gemini`
- `--planner-provider ollama`
- `--planner-provider openai`
- `--planner-provider gemini`

Ollama の native tool call と XML fallback は現行通り維持する。OpenAI は provider の native tool call を第一実装にし、XML fallback は tool call 非対応モデル向けの明示的 fallback としてだけ残す。

Gemini は XML fallback を使わず、公式の function calling REST 仕様に合わせて native tool call を実装する。参考: <https://ai.google.dev/gemini-api/docs/function-calling?hl=ja#rest>

Gemini execution provider の実装方針。

- `ToolSpec` を Gemini tool declaration に変換する
- tool declaration は `type: "function"`, `name`, `description`, `parameters` を持つ JSON Schema object にする
- `POST https://generativelanguage.googleapis.com/v1beta/interactions` を使う
- header は `x-goog-api-key: $GEMINI_API_KEY` と `Content-Type: application/json`
- MVP は local session store 側で履歴を持つため、`store: false` の stateless 呼び出しに寄せる
- `input` には user input、model から返った `thought` / `function_call` steps、local tool 実行結果の `function_result` を順序通りに戻す
- Gemini の `function_call` step を `ToolCall { id, name, arguments }` に変換する
- tool 実行結果は `function_result` step として `name`, `call_id`, `result: [{ type: "text", text: ... }]` で返す
- final text は `output_text` または text content step から抽出する
- XML fallback は Gemini では default 無効。障害解析用に残す場合も hidden env flag で opt-in に限定する

Gemini fixture で固定する異常系。

- `thought` step がない response
- 複数 `function_call` が同時に返る response
- unknown tool name
- JSON Schema に合わない arguments
- `call_id` が欠落または対応しない `function_result`
- provider error / rate limit / timeout
- malformed function call を XML fallback へ落とさず、retry 可能な provider error として loop に返す経路

Planner provider としての Gemini は plan YAML 生成用なので tools を渡さない。YAML 抽出/validate/lint で安定性を担保する。

### 6. plan run は YAML 互換を維持する

`StepPlan` YAML は現行と同じ shape を維持する。

```yaml
goal: "..."
steps:
  - id: "create-app"
    kind: "create"
    instruction: "..."
    expected_paths:
      - "package.json"
    verify:
      - "npm run build"
```

`run_plan` は現行と同じ順序で動く。

```text
load plan
validate
lint
for each step:
  build step prompt
  minimal loop
  verify expected_paths and commands
  if failed:
    repair loop
  if still failed:
    save .anvil/repairs/repair-*.md
    return error
```

### 7. ultra plan run は phase ごとの plan-run として維持する

`UltraPlan` YAML は現行と同じ shape を維持する。

```yaml
goal: "..."
profile: "nextjs"
style: "default"
intent: "create"
phases:
  - id: "scaffold-nextjs-app"
    prompt: "..."
```

`run_ultra_plan` は phase ごとに `generate_and_run_step_plan` を呼ぶ。Next.js / data profile verifier は MVP に含める。

`--profile nextjs` の契約として残すもの。

- `package.json` に `next/react/react-dom`
- `scripts.build` は `next build`
- 3011 port 要求がある場合 `dev` script は `next dev` 系で保持
- Tailwind を使うなら dependency/config を揃える
- `@/*` import を使うなら `tsconfig.json` paths mapping を要求
- `rootDir: ./src` で `app/` を除外しない

## 実装フェーズ

### Phase 1 以降の共通受け入れ条件

Phase 1 以降は、実装と同じ PR/commit 内で対応するテストコードも追加する。

各 Phase の完了条件。

- その Phase で追加/変更した public behavior に unit / fixture / integration test がある
- provider API を実呼びする test は default ignored または `ANVIL_LIVE_PROVIDER_TESTS=1` gated にし、通常の `cargo test` は network/API key なしで通る
- `cargo fmt --check` が通る
- `cargo test` が通る
- 該当 Phase の targeted test command を計画/README に記録する
- regression fixture は `workspace/v0.6.6` / `docs/eval` の失敗分類に対応する名前を付ける

最小の test taxonomy。

| 種別 | 用途 |
|---|---|
| unit test | parser/config/path/tool policy/YAML/profile verifier の deterministic behavior |
| fixture test | provider request/response、Gemini function calling、OpenAI tool call、plan YAML round trip |
| integration test | fake LLM client を使った minimal loop / plan run / repair loop |
| script test | benchmark/recheck/seed/compare harness の shell/Python smoke |
| live smoke | `.env` の `OPENAI_API_KEY` / `GEMINI_API_KEY` を使う provider 接続確認。通常 CI では gated |

### Phase 0: scaffold

成果物。

- `mvp/anvilminimal/Cargo.toml`
- `mvp/anvilminimal/src/main.rs`
- `mvp/anvilminimal/src/lib.rs`
- `mvp/anvilminimal/README.md`

`cargo run -- --help` が通るところまで作る。

受け入れ条件。

- `cargo check` が通る
- `cargo run -- --help` が通る
- README に build / run / symlink の最小手順がある

### Phase 1: data model / config / state

実装。

- `cli.rs`
- `config.rs`
- `state.rs`
- `mode.rs`

受け入れ条件。

- `anvilminimal --help` に必要 flag が出る
- `--engine` が存在しない
- `--provider/--planner-provider` が parse できる
- session が `anvilminimal` state root に保存される
- `.env` の `OPENAI_API_KEY` / `GEMINI_API_KEY` を process env fallback として読み、値は log に出さない
- `--planner-provider != --provider` かつ `--planner-model` 未指定時の error が deterministic
- `--sidecar-model` が存在しない。互換 hidden flag を残す場合も warning を出して無視する
- unit test: CLI parse、config precedence、`.env` fallback、state root 分離、resume id confinement
- `cargo test` が通る

### Phase 2: provider

実装。

- `providers/ollama.rs`
- `providers/openai.rs`
- `providers/gemini.rs`
- `providers/gemini_function_calling.rs`
- `providers/parsing.rs`
- `providers/xml_fallback.rs`

受け入れ条件。

- Ollama `/api/tags` と `/api/chat` が動く
- OpenAI/Gemini は env key から client 初期化できる
- execution provider と planner provider を別々に指定できる
- `--planner-model` 省略時は `--model` を使う
- provider が異なり `--planner-model` 未指定の場合は明示エラーにするか、README に既定値を明記する
- Gemini execution provider は XML fallback ではなく native function calling で `Bash/Read/Write/Edit/Glob/Grep` を呼べる
- Gemini planner provider の no-tool request は公式 Interactions REST の simple input 形式に固定する。`model` は `gemini-...` の raw ID、`input` は string、`tools` / `store` / `generation_config` は送らない
- Gemini function calling の request/response fixture test を追加し、`function_call` と `function_result` の round trip を固定する
- Gemini tool-calling request は `store: false`、`input` は `user_input` / `thought` / `function_call` / `function_result` の配列、`function_call.id` と `function_result.call_id` の round trip を固定する
- Gemini function calling の異常系 fixture test を追加し、unknown tool、schema mismatch、missing/mismatched `call_id`、multiple function calls、provider error、timeout を固定する
- OpenAI tool call fixture test で `gpt-5.4-mini` の model string が request に保持される
- Gemini fixture test で `gemini-3.5-flash` と `gemini-3.1-flash-lite` の model string が request に保持される
- `.env` の key を使う live smoke を `ANVIL_LIVE_PROVIDER_TESTS=1` gated で用意する。Gemini は request shape test だけでは不可とし、no-tool Interactions API を実HTTPで叩く ignored test を必須にする。model availability failure は unit test failure ではなく live smoke failure として報告する
- auth header と key value が `llm-io.jsonl` に保存されない redaction test を追加する
- unit/fixture test: provider normalization、schema sanitizer、retry/timeout、Gemini stateless history builder、OpenAI/Gemini/Ollama `AssistantReply` 正規化
- `cargo test` が通る

### Phase 3: tools

実装。

- `tools/registry.rs`
- `tools/bash.rs`
- `tools/read.rs`
- `tools/write.rs`
- `tools/edit.rs`
- `tools/glob.rs`
- `tools/grep.rs`
- `tools/path_guard.rs`
- `tools/workspace_policy.rs`

受け入れ条件。

- Plan mode は read-only tools のみ
- Act mode は Write/Edit/Bash が動く
- `Write` は parent dir を作る
- `Edit` は exact anchor mismatch を返す
- Bash は dangerous command を拒否
- offline mode で network/setup 系を拒否
- `Write` は parent directory を作るため、LLM に `mkdir -p` を要求しない tool catalog wording を持つ
- Bash は local build/test/read-only script 実行を許可し、file creation/setup command と区別する
- path guard は absolute path、`..`、NUL、Windows drive prefix / UNC、symlink escape、新規 file の parent escape を拒否する
- goal reference 展開、tool argument、plan YAML の `expected_paths` は同じ path guard を通る
- unit test: Plan/Act tool allowlist、Write parent dir、Edit anchor mismatch、offline Bash、dangerous command、local script allowlist、path confinement
- `cargo test` が通る

### Phase 4: minimal loop

移植。

- `minimal_loop/loop_run.rs`
- `minimal_loop/prompt.rs`
- `minimal_loop/feedback.rs`
- `minimal_loop/compact.rs`

調整。

- `crate::session::store::*` を `crate::state::*` に置換
- `crate::tools::registry::*` を MVP tool registry に置換
- `WorkspacePolicy` を MVP 版へ置換
- `ExecutionMode` を MVP `mode.rs` へ置換

受け入れ条件。

- prompt 1 回で tool call 実行後に final reply が返る
- Ollama/OpenAI の tool call 非対応モデルだけ、明示的 fallback として XML tool call を使える
- Gemini の malformed function call は XML fallback へ落とさず、provider error / retry / repair feedback として扱う
- requested artifact missing feedback が働く
- max iteration 到達時に明示エラー
- no-tool progress statement を、要求 artifact 未完了時に Done として受け入れない
- tool result 後も required artifact / evidence が未完了なら次 tool action を要求する
- native tool mode と XML fallback feedback が矛盾しない
- fake chat client integration test: Write -> final、no-tool feedback、missing requested artifact feedback、malformed tool call、max iteration、Gemini malformed function call
- `cargo test` が通る

### Phase 5: planner / step plan / plan run

移植。

- `planner/step_plan.rs`
- `planner/runner.rs`
- `planner/verify.rs`
- `planner/repair.rs`
- `planner/lint.rs`
- `planner/intent.rs`

調整。

- `minimal_step_runner.rs` の巨大ファイルは `step_plan.rs`, `ultra_plan.rs`, `runner.rs` に分割する
- YAML renderer/parser は現行互換のまま残す
- deterministic verify の allowlist は現行維持

受け入れ条件。

- `--plan-steps "..."` が `.anvil/plans/plan-*.yaml` を作る
- `--plan-run "..."` が plan 作成後に実行する
- `--run-plan .anvil/plans/plan-*.yaml` が実行できる
- `--run-plan` / `/run-plan` は workspace 内の相対 path または workspace 内 canonical absolute path だけを読む。workspace 外 absolute path / `..` / symlink escape は拒否する
- verifier failure で repair cycle が動く
- exhausted 時に `.anvil/repairs/repair-*.md` が作られる
- required path が欠けている場合は verify command を先に実行しない
- verify command の exit status が非0なら必ず `CommandFailed` とし、stdout/stderr を持っていても success 扱いしない
- verify の exit 0 だけで success にせず、Node 0 tests / Rust test binding / docs heading / data schema を quality gate にかける
- bounded repair action 後は必ず deterministic verify を再実行する
- repair exhausted は同じ target/invariant で進捗がない場合にだけ出す
- unit/fixture/integration test: `StepPlan` YAML round trip、lint、expected_paths before evidence、allowlisted verify command、Node 0 tests rejection、Rust `cargo test --manifest-path` binding、repair report、fake planner/client による `plan-run`
- `cargo test` が通る

### Phase 6: ultra plan run / profiles

移植。

- `planner/ultra_plan.rs`
- `planner/profile.rs`
- `planner/profiles/nextjs.rs`
- `planner/profiles/data.rs`

受け入れ条件。

- `--ultra-plan "..."` が `.anvil/plans/ultra-plan-*.yaml` を作る
- `--ultra-plan-run --profile nextjs "..."` が phase ごとに step plan を作って実行する
- `/ultra-plan-run --profile nextjs ...` が REPL から動く
- Next.js profile verifier が package/build/alias/tailwind 契約違反を検出する
- final phase 後も profile contract が未達なら `ultra-plan-run complete` を返さず、phase/profile invariant を含む failure を返す
- data profile verifier が raw data の変更を検出する
- phase ごとに step plan YAML が保存され、phase failure 時はどの phase / step / invariant が失敗したか報告する
- 3011 port 要求は `package.json` の dev script で検査し、欠落時は repair に戻す
- dependency 未 install は `dependency_missing` として分類し、build success を偽装しない
- unit/fixture/integration test: `UltraPlan` YAML round trip、phase lint、Next.js 3011/dev/build/dependency/alias/tailwind verifier、data raw-file protection、fake planner/client による `ultra-plan-run`
- `cargo test` が通る

### Phase 7: REPL/TUI

詳細な TUI terminal primitives の移植計画は `workspace/mvp/anvilminimal_tui_migration_plan.md`、具体作業分解は `workspace/mvp/anvilminimal_tui_work_breakdown.md` を参照する。この Phase 7 は単なる line REPL ではなく、spinner / footer / ESC interrupt / markdown rendering / raw mode 境界を含む。

移植。

- `repl.rs`
- `tui/terminal.rs`
- `tui/slash.rs`
- `tui/spinner.rs`
- `tui/footer.rs`
- `tui/interrupt.rs`
- `tui/markdown.rs`
- `tui/status.rs`

対応 command。

```text
/exit
/quit
/plan-steps <goal>
/plan-run <goal>
/run-plan <path>
/ultra-plan [--profile X] [--style Y] <goal>
/ultra-plan-run [--profile X] [--style Y] <goal>
/run-ultra-plan <path>
```

受け入れ条件。

- 指定コマンドで `anvil>` prompt が出る
- REPL spinner が TTY で表示される
- `ANVIL_NO_SPINNER` で spinner を無効化できる
- fixed footer が TTY で表示され、`--no-footer` / `ANVIL_NO_FOOTER` で無効化できる
- assistant 応答は terminal markdown renderer を通り、`<think>` を表示しない
- ESC interrupt が provider/tool/phase 境界停止として働き、prompt 入力中の raw mode と干渉しない
- `$(cat relative/path)` goal reference が workspace 外へ出られない
- `/plan-run`, `/run-plan`, `/ultra-plan-run`, `/run-ultra-plan` は CLI action と同じ path/profile/style 解釈をする
- engine 指定なしで TUI 起動できる
- slash command parser は quoted prompt / `--profile` / `--style` / file path を正しく扱う
- REPL からの `/plan-run` / `/ultra-plan-run` は CLI と同じ provider/planner/model/session を使う
- approval prompt 中は interrupt/raw-mode が干渉しない
- unit/integration test: slash command parse、goal reference confinement、TTY なしでは REPL に入らない、engine flag 不在、spinner env disable
- `cargo test` が通る

### Phase 8: E2E / copy validation

検証項目。

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- --help
cargo run -- --yes --context-budget 65536 --model qwen3.6:27b-coding-nvfp4 --planner-model gemini-3.5-flash --planner-provider gemini --provider ollama
cargo run -- --yes --provider openai --model gpt-5.4-mini --prompt "notes/openai-smoke.md を作成してください"
cargo run -- --yes --provider gemini --model gemini-3.1-flash-lite --prompt "notes/gemini-smoke.md を作成してください"
```

OpenAI/Gemini の実 API smoke は `.env` に key がある環境でだけ実施し、通常の local/CI `cargo test` には含めない。Ollama smoke は local Ollama が起動し、`ANVIL_OLLAMA_SMOKE_MODEL` または既定モデルが利用可能な環境で実施する。
ただし `ANVIL_LIVE_PROVIDER_TESTS=1 cargo test live_ -- --ignored` は request payload fixture だけで完了扱いにしない。Gemini planner 経路は no-tool Interactions API、OpenAI は no-tool Responses API、Ollama は `/api/tags` と `/api/chat` の実HTTP smoke を含め、`400 Bad Request` や model/provider contract mismatch を検出できることを完了条件にする。

local development symlink。

現行の `anvildev` と同様に、開発機では `~/.local/bin/anvilminimal` から release binary へ symlink を張る。

```bash
cargo build --release
ln -sfn /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/target/release/anvilminimal /Users/maenokota/.local/bin/anvilminimal
anvilminimal --help
```

他リポジトリへコピーした後は、コピー先の `target/release/anvilminimal` に張り替える。symlink は移植 artifact に含めず、開発環境の起動ショートカットとして扱う。

手動 UAT。

1. 空 workspace で `anvilminimal ...` を起動する
2. REPL に `/ultra-plan-run --profile nextjs ...3011ポート...` を入力する
3. `.anvil/plans/ultra-plan-*.yaml` が作られる
4. phase ごとの `.anvil/plans/plan-*.yaml` が作られる
5. `package.json`, `app/page.tsx` または `pages/index.tsx` が作られる
6. `package.json` に `next build` と 3011 port の dev script がある
7. 依存未インストール時は `dependency_missing` を報告し、build success を偽装しない

コピー検証。

```bash
tmp=$(mktemp -d)
rsync -a --exclude target mvp/anvilminimal/ "$tmp/anvilminimal/"
cd "$tmp/anvilminimal"
cargo test
cargo run -- --help
```

benchmark / recheck / compare harness。

```bash
bash scripts/bench.sh minimal-loop-expanded --model qwen3.6:27b-coding-nvfp4 --runs 1 --max-iterations 12 --bench-no-debug
bash scripts/bench.sh minimal-loop-expanded --recheck-root <BENCH_ROOT>
python3 scripts/compare.py <baseline-summary.tsv> <experiment-summary.tsv>
```

受け入れ条件。

- `benchmarks/minimal-loop-expanded.yaml` に 25 scenarios がある
- MVP の `bench.sh` は `anvilminimal` 専用で、binary 実行時に `--engine` を渡さない。engine 相当の識別子が必要な場合は `extras_json.engine_label = "anvilminimal"` に記録し、`summary.tsv` の列は増やさない
- `summary.tsv` は現行互換 header `run\tmodel\tcase\tpam_variant\trc\telapsed_sec\tworkdir\tsession_copied\textras_json` を持つ
- `summary.recheck.tsv` は元の `summary.tsv` を変更せず生成され、header は上記に `recheck_success_check_success\trecheck_success_check_reason` を追加したものにする
- seed 有効時に `bench_seed`, `bench_seed_enabled`, `ANVIL_BENCH_SEED` が meta/session に記録される
- same seed で tool sequence と postcheck result が比較可能に保存される
- `scripts/compare.py` が legacy/minimal/anvilminimal の summary を比較できる
- script test: benchmark smoke、filtered cases、recheck mode、seed metadata、compare output
- `cargo test` と script smoke が通る

## リスクと対策

| リスク | 対策 |
|---|---|
| 現行 `Config` が大きく、不要 field を引きずる | MVP `Config` を新規定義し、minimal に必要な field だけを持つ。`Engine`, Photon, PAM, legacy deterministic fallback, sidecar は移さない |
| core MVP と harness を混ぜて LOC が膨らむ | core runtime と verification harness を別層に分ける。benchmark/recheck/compare は copy artifact に含めるが、CLI runtime から独立した script/module 境界に置く |
| `--sidecar-model` への期待が残る | CLI から削除し、`--help` と README に「minimal は sidecar 不使用、planner は `--planner-model`」と明記する。既存 script 互換が必要なら hidden deprecated flag として受け取り、warning を出して無視する |
| `SessionSnapshot` が working memory / photon などを含む | `state.rs` へ minimal session を新規定義し、保存 JSON schema を固定する。旧 Anvil session を読まない代わりに、`--resume` は anvilminimal state root のみに閉じる |
| `ToolRegistry` が legacy Agent policy に依存 | `Bash/Read/Write/Edit/Glob/Grep` だけの registry に再構成する。approval, workspace confinement, offline policy, dangerous command block は registry 内で完結させる |
| workspace confinement の解釈が実装者ごとに揺れる | absolute path、`..`、NUL、drive prefix、symlink escape、新規 file parent escape、goal reference 展開を同じ path guard で拒否する。unit test に read/write/edit/glob/grep/bash/plan expected_paths の各入口を置く |
| Gemini XML fallback が不安定 | Gemini execution provider は native function calling を必須にする。XML fallback は default 無効にし、fixture test で `ToolSpec -> function declaration -> function_call -> function_result -> final text` を検証する |
| Gemini Interactions API の no-tool planner request 形式を誤る | planner provider は tools を渡さないため、公式 REST の simple input 形式 `model: "gemini-..."`, `input: "<text>"` に固定する。`models/` prefix、不要な `store` / `generation_config`、array input を送らない regression test と実HTTP smoke を置く |
| Gemini Interactions API の履歴形式を誤る | tool-calling request は `store: false` 前提で、user input、model steps、function_result を順序通り再送する履歴 builder を専用実装にする。`thought` step と `function_call.id` / `function_result.call_id` を欠落させない regression test を置く |
| Gemini tool schema が provider に拒否される | tool schema を JSON Schema object の共通 subset に制限する。unsupported keyword を落とす sanitizer を入れ、送信前に schema snapshot test を行う |
| Gemini API response shape が想定とずれる | 正常 round trip に加え、missing `thought`、multiple calls、unknown tool、schema mismatch、missing/mismatched `call_id`、provider error、timeout の fixture を置き、provider 層で `AssistantReply` / retryable error に正規化する |
| OpenAI/Gemini/Ollama の tool call 表現差で loop が分岐しすぎる | provider 内で `AssistantReply { text, tool_calls }` に正規化し、minimal loop は provider 固有構造を知らないようにする |
| provider が planner と execution で異なる場合に model default が曖昧 | `--planner-provider != --provider` かつ `--planner-model` 省略時は起動時 error にする。README の標準例では必ず planner model を指定する |
| symlink が壊れた target を指す | `cargo build --release` 後に symlink を作成する手順にし、UAT に `command -v anvilminimal` と `anvilminimal --help` を入れる。コピー先では symlink を張り替える |
| PATH 上で古い `anvilminimal` が先に見つかる | `which -a anvilminimal` を確認する手順を README に入れる。CI/検証は `cargo run --` を基準にし、symlink はローカル利便性だけに限定する |
| Next.js verifier が依存未 install を source 修正に誤誘導 | `dependency_missing` を明示し、script 偽装を禁止する。`npm run build` が実行不能な場合は success にしない |
| Next.js 3011 port 要求が plan から落ちる | nextjs profile の post-verify で `package.json` の `dev` script を検査し、`next dev -p 3011` 相当がない場合は phase repair に戻す |
| ultra phase が大きすぎて plan-run が破綻 | ultra plan lint で 2-8 phase、各 phase は focused goal に制限する。phase prompt には前 phase の profile snapshot と未達 contract だけを渡す |
| deterministic verify が強すぎて false negative になる | verifier result を `missing_path`, `command_failed`, `dependency_missing`, `profile_contract_failed` に分類し、repair prompt に失敗理由を構造化して渡す |
| verify command の非0終了を success 扱いする | agent tool 用 Bash は失敗出力を返せるままにし、deterministic verify は strict Bash 実行で非0終了を `CommandFailed` にする。`verify_command_nonzero_fails` regression test を置く |
| profile verifier の未達を ultra 完了扱いする | 中間 phase では profile 未達を repair 継続の材料にできるが、final phase 後に `ProfileContractFailed` が残っていれば必ず `ultra-plan-run` を失敗にする |
| Bash verify が危険 command を実行する | verify command は allowlist と workspace confinement を通す。`rm -rf`, absolute destructive path, credential dump, network install は `--offline` と dangerous block で止める |
| 他リポジトリへコピー後に state path が Anvil 本体と衝突 | state root を `anvilminimal` に分離し、`.anvil/plans` / `.anvil/repairs` だけ workspace local に残す |
| API key や provider request が log に残る | `llm-io.jsonl` は auth header を保存しない。env key 名だけ記録し、値は redaction する |
| `.env` 依存の live test が CI を不安定にする | live provider smoke は `ANVIL_LIVE_PROVIDER_TESTS=1` gated にし、通常の `cargo test` は fixture/mock だけで通る。ただし gated 実行時は request shape test だけでなく OpenAI/Gemini/Ollama の実HTTP smoke を含める |
| 指定 model ID の availability が provider 側で変わる | `gpt-5.4-mini`, `gemini-3.5-flash`, `gemini-3.1-flash-lite` は設定値として扱い、unit test は request payload に保持されることを検査する。実 availability は live smoke の結果で判定し、必要時は smoke model env で上書きできるようにする |
| plan YAML 互換を壊す | YAML schema snapshot test を追加し、現行の `StepPlan` / `UltraPlan` fixture を MVP で round trip する |
| Phase ごとの実装だけ進み test が後追いになる | Phase 1 以降は対応 test 追加と `cargo test` 通過を受け入れ条件にする。Phase 完了時に targeted test command を記録する |
| benchmark/recheck harness が現行 summary と非互換になる | `tests/scripts/test_bench_smoke.sh` 相当の smoke を MVP に移し、`summary.tsv`, `summary.recheck.tsv`, seed metadata, filtered recheck の header/行数を固定する |
| 25本 benchmark が能力評価ではなく check false negative を測る | `docs/eval/triage/t2-4-dead-scenarios.md` と recheck 結果を baseline とし、postcheck false negative は recheck で切り分ける |
| mechanism が測定なしで増える | `docs/eval/mechanism-ledger.md` 形式を MVP に残し、feedback/recovery 追加は off flag、target scenarios、admission benchmark、final audit date を必須にする |
| copy artifact に絶対 path が混入する | `mvp/anvilminimal` 配下には絶対 path を置かない。symlink 手順だけ README に開発機用として記載する |

## 完了条件

- `mvp/anvilminimal` 単体を他リポジトリへコピーして `cargo test` が通る
- copy 検証は `target/` を含めず clean copy で実施し、絶対 path / symlink 依存なしで `cargo test` と `cargo run -- --help` が通る
- Phase 1 以降の各 Phase に対応 test があり、targeted test と `cargo test` が通る
- `anvilminimal` binary が生成される
- ローカル開発環境では `~/.local/bin/anvilminimal` symlink から binary を起動できる。symlink は copy artifact の完了条件には含めない
- engine 指定なしで REPL/TUI が起動する
- CLI 引数で `--plan-steps`, `--plan-run`, `--run-plan`, `--ultra-plan`, `--ultra-plan-run`, `--run-ultra-plan` が動く
- Ollama/OpenAI/Gemini を execution provider / planner provider として選択できる
- `.env` の `OPENAI_API_KEY` / `GEMINI_API_KEY` を使う live smoke で `gpt-5.4-mini`, `gemini-3.5-flash`, `gemini-3.1-flash-lite` を実行できる
- `/ultra-plan-run --profile nextjs ...3011ポート...` が REPL から実行できる
- plan YAML / ultra plan YAML が現行 shape と互換
- deterministic verify と profile verifier が成功/失敗を偽装せず報告する
- verify command の非0終了、workspace 外 plan path、final profile contract 未達はすべて失敗として扱われる
- `benchmarks/minimal-loop-expanded.yaml` の 25 scenarios を実行できる
- `summary.tsv` / `summary.recheck.tsv` / seed metadata / compare output が現行 harness と互換
- mechanism-ledger 形式で M001/M002 と新規 mechanism の off flag / admission benchmark / audit status を追跡できる
