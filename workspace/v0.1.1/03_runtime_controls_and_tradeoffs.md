# Runtime 制御と現在のトレードオフ

## 1. 何を runtime が吸収しているか

現在の Anvil は、モデルに全部を委ねず、
runtime 側がかなり多くの制御を引き受けている。

大きく分けると次の 5 系統である。

1. prompt/context shaping
2. protocol / reply recovery
3. tool execution safety
4. repo progress / quality verification
5. terminal interaction control

---

## 2. Prompt / context shaping

### 2.1 system prompt の役割分担

`src/system_prompt.rs` は、

- mode (Plan / Act)
- task profile
- tool protocol
- plan stage

に応じて system prompt を組み替える。

つまり prompt は固定文ではなく、
**runtime state に応じて組み替える policy surface** になっている。

### 2.2 runtime context

`src/agent/prompting.rs` は追加で、

- current project root
- `ANVIL.md` の project instructions
- repo context 候補

を system message として inject する。

repo context は task 文字列から候補ファイルを rank して渡すため、
**探索の初手を repo の可能性が高い場所へ寄せる**狙いがある。

### 2.3 working memory と compaction

- `src/session/store.rs`
  - active task / constraints / touched files / unresolved errors
- `src/session/compact.rs`
  - 古い message を summary 化
- `src/agent/loop_run/lifecycle.rs`
  - sidecar があれば summary に利用

これにより、
会話履歴を延々増やすのではなく、
**今の task を続けるのに必要な文脈だけを短く保つ**
方向になっている。

---

## 3. Protocol / reply recovery

### 3.1 native tools を過信しない

`turn.rs::request_assistant_reply_with_retry` では、

- native tool parser failure
- native tool transport failure

を検出すると、session 単位で native tools を無効化し、
Tagged XML fallback に downgrade する。

### 3.2 malformed tool call 回復

同じ箇所には、

- truncated tool call
- unterminated tool call
- focused edit timeout
- transport retry

に対する回復ノートと再試行制御が入っている。

つまり reply recovery は、
単なる「再送」ではなく、
**次の返答をどう狭めるか**まで含む設計である。

### 3.3 deterministic fallback

`turn.rs` / `quality.rs` / `deterministic.rs` には、

- 空 workspace に対する deterministic scaffold
- playable UI polish fallback
- mode-specific fallback

が入る。

これは純粋な agent loop というより、
**成功率が落ちやすい領域だけ runtime が手で補助する**層である。

---

## 4. Tool 実行安全

### 4.1 path と mode

- `src/safety/path_guard.rs`
  - workspace escape を禁止
- `src/tools/registry.rs`
  - Plan mode は plan file 以外に書けない
  - plan stage ごとに書ける section を制限

### 4.2 approval と offline

- `src/tools/registry.rs`
  - Bash / Write / Edit は approval 対象
- `src/tools/bash.rs`
  - command class 判定
  - offline 時の禁止
  - dangerous snippet block

### 4.3 host validation

- `src/safety/host_validation.rs`
  - Ollama host は localhost のみ

これらをまとめると、
現在の安全設計は
**モデルを信用する設計ではなく、tool surface を狭める設計**
である。

---

## 5. Repo progress / quality verification

### 5.1 repo snapshot

`src/agent/orchestration.rs` は before/after snapshot を比較し、
変更が

- implementation
- test
- setup
- other
- deleted

のどこに出たかを数える。

これは「何か返答した」ではなく
**repo にどう前進が出たか**を見るための仕組みである。

### 5.2 auto test

`src/agent/loop_run/auto_test.rs` は、

- Cargo project なら `cargo test`
- Node project なら `npm test` / `npm run build`
- Python project なら `pytest` or `py_compile`

を自動検出する。

加えて `ANVIL.md` の backtick command から
安全な preferred verify command を拾える。

### 5.3 quality gate

`src/agent/loop_run/quality.rs` と `turn.rs` は、
特に playable UI 系 request に対して

- request が UI/interactive か
- target 実装ファイルが十分な内容を持つか
- polish fallback を適用できるか

を判定する。

このため現在の Anvil は、
「コードが生成されたか」より少し踏み込んで、
**成果物の質まで runtime が補正対象にしている**。

---

## 6. Terminal interaction control

### 6.1 fixed footer

`src/agent/loop_run/footer.rs` は、

- mode
- token usage
- log level
- yes mode

を固定フッターで表示する。

単なる飾りではなく、
progress line や redraw freeze と連携しており、
terminal を一種の UI として扱っている。

### 6.2 spinner / interrupt / markdown

- `spinner.rs`
  - thinking / tool 実行中の体感制御
- `interrupt.rs`
  - ESC による turn boundary interrupt
- `tui/markdown.rs`
  - stream 中の markdown 整形
  - `<think>` 不可視化

local LLM の待ち時間とノイズに対して、
**出力レイヤでも回復と整形をかける**
のが現行実装の特徴である。

---

## 7. 現在のトレードオフ

## 7.1 操作モデルは単純だが、内部制御はかなり厚い

ユーザーから見ると Plan/Act の二状態だが、
内部では `turn.rs` が多くの回復分岐を抱える。

これは、

- UX は単純
- 実装は複雑

という明確な trade-off である。

## 7.2 native tool calling は限定採用

`transport.rs::should_use_native_tool_calls` の allowlist は非常に狭い。  
これは保守的だが、
逆に言えば **ほとんどの model は fallback 前提** で運用している。

## 7.3 deterministic fallback は成功率を上げるが、一般化しにくい

Next.js / playable UI / Python/docs 空 workspace など、
いくつかの領域では deterministic fallback が強い。

一方でこれは、
**局所最適のルールが増えやすい**
というコストも持つ。

## 7.4 成果判定は repo 寄りだが、意味理解はまだ heuristic

repo snapshot・quality gate・auto test は入っているが、
それでも「本当に user-facing value が出たか」は heuristic 依存の部分が残る。

つまり現在の Anvil は、
**単なる file existence よりは進んでいるが、
完全な semantic judge に到達したわけではない**。

## 7.5 package version と設計整理の粒度はまだずれている

`Cargo.toml` の version は `0.1.0` のままだが、
実装の中身はすでに

- fixed footer
- interrupt
- markdown renderer
- sessions CLI
- auto plan
- quality gate

などを持つ。

したがって現在の `workspace/v0.1.1` は、
**version bump の宣言**ではなく
**現状理解の整理版**として読むのが正しい。

---

## 8. いまの Anvil を一文で言うなら

> **Anvil は、Ollama を相手に local-first でコード作業を進めるための、
> Plan/Act・session・tooling・TUI・recovery を一体で持った Rust 製 CLI runtime である。**

---

## 9. 主な根拠ファイル

- `src/system_prompt.rs`
- `src/agent/prompting.rs`
- `src/agent/loop_run/commands.rs`
- `src/agent/loop_run/turn.rs`
- `src/agent/loop_run/lifecycle.rs`
- `src/agent/loop_run/auto_test.rs`
- `src/agent/loop_run/quality.rs`
- `src/agent/orchestration.rs`
- `src/tools/registry.rs`
- `src/tools/bash.rs`
- `src/safety/path_guard.rs`
- `src/safety/host_validation.rs`
- `src/session/store.rs`
- `src/session/compact.rs`
- `src/agent/loop_run/footer.rs`
- `src/agent/loop_run/interrupt.rs`
- `src/tui/markdown.rs`
