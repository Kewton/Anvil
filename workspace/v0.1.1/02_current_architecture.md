# 現在のアーキテクチャ

## 1. 全体像

現在の Anvil は、だいたい次の 6 層で見ると把握しやすい。

| 層 | 主な責務 | 主なファイル |
| --- | --- | --- |
| CLI / bootstrap | 引数解釈、config 解決、session/log 初期化、model 選択 | `src/main.rs`, `src/lib.rs`, `src/cli.rs`, `src/config.rs`, `src/logging.rs`, `src/model_registry.rs` |
| Session / mode state | 会話履歴、working memory、active root、plan 状態、resume | `src/session/store.rs`, `src/session/compact.rs`, `src/session/discovery.rs`, `src/session/sessions_cli.rs`, `src/modes/plan_act.rs` |
| Agent runtime | REPL / oneshot / resume、Plan/Act 切替、actor loop、回復制御 | `src/agent/loop_run.rs`, `src/agent/loop_run/commands.rs`, `src/agent/loop_run/turn.rs`, `src/agent/loop_run/lifecycle.rs` |
| Model I/O | Ollama 通信、stream/non-stream、native tools、fallback parse | `src/ollama/client.rs`, `src/ollama/transport.rs`, `src/ollama/parsing.rs`, `src/ollama/xml_fallback.rs` |
| Tool execution | Read/Write/Edit/Glob/Grep/Bash の仕様と実行 | `src/tools/registry.rs`, `src/tools/*.rs` |
| UX / safety | fixed footer、spinner、interrupt、markdown、path/host 制約 | `src/agent/loop_run/footer.rs`, `src/agent/loop_run/spinner.rs`, `src/agent/loop_run/interrupt.rs`, `src/tui/markdown.rs`, `src/safety/*.rs` |

---

## 2. 起動から agent 構築まで

### 2.1 起動入口

- `src/main.rs`
  - `CliArgs::parse()` を `anvil::run_cli` に渡す
- `src/lib.rs`
  - 実質的なアプリ本体

### 2.2 `run_cli` の役割

`src/lib.rs::run_cli` は以下を順に行う。

1. CLI validation
2. `anvil sessions ...` の short-circuit
3. `Config::load` による file/env/CLI merge
4. state root と workspace key の決定
5. session id 解決
6. log / plan directory 作成
7. Ollama client 作成
8. model 一覧取得
9. main / sidecar model 選択
10. session snapshot load
11. footer lease 取得
12. `Agent::new`
13. oneshot / resume / REPL へ分岐

この構成により、CLI 側は
**「agent を安全に立ち上げるための初期化層」**
として独立している。

---

## 3. Config / model / state の責務分離

## 3.1 Config

`src/config.rs` は、設定を

- `.anvil/config`
- environment
- CLI

から読み、後勝ちで merge する。

最終 `Config` には、

- model 指定
- context budget
- max iterations
- timeout / retries
- log level
- stream / yes / auto_plan / offline
- resume / fresh_session
- footer 有無

が入る。

### 3.2 Model selection

`src/model_registry.rs` は、

- 利用可能 model 一覧
- 総メモリ量
- requested main / sidecar

から `RuntimeModels { main, sidecar }` を決める。

このため現在の Anvil は、
**model routing を provider 共通層ではなく local runtime policy として持つ**
構造になっている。

### 3.3 Session snapshot

`src/session/store.rs` の `SessionSnapshot` は、

- `mode_state`
- `messages`
- `checkpoints`
- `active_root`
- `native_tools_disabled`
- `working_memory`
- `id`
- `workspace_key`

を保持する。

`working_memory` はさらに、

- active task
- constraints
- touched files
- unresolved errors

を短く保つ。

つまり現在の session は、
**単なる会話ログ**ではなく、
**runtime が次の一手を選ぶための作業記憶**も持っている。

---

## 4. Agent runtime の中心

## 4.1 `Agent`

`src/agent/loop_run.rs` の `Agent` は、

- config
- models
- Ollama client
- session store
- session snapshot
- current work root
- native tools の有効状態
- tool registry
- repo context cache
- footer handle

を束ねる実行主体である。

## 4.2 実行入口

`src/agent/loop_run/commands.rs` が主に扱うのは、

- `run_oneshot`
- `run_resume`
- `run_repl_loop`
- slash commands
- auto plan
- plan approve

であり、ユーザー interaction の外側を担当する。

## 4.3 実行中の 1 turn

1 turn の本体は `src/agent/loop_run/turn.rs` にある。

大まかな流れは次の通り。

1. user message を session に積む
2. work mode を推定
3. 必要なら session compact
4. action expectation を決める
5. actor loop に入る
6. assistant reply を取得
7. tool calls を normalize
8. guard を通して tool 実行
9. repo progress / plan progress / test 結果を見る
10. fallback or continue を決める
11. final prose か次 iteration へ進む

ここで重要なのは、
**1 turn = 1 回のモデル呼び出し**ではなく、
**複数 iteration を含む actor loop** であること。

---

## 5. Plan / Act の実装位置

Plan / Act は `ModeState` と `system_prompt` の両方で成立している。

| 要素 | 役割 | 主なファイル |
| --- | --- | --- |
| `ModeState` | 現在 mode / work_mode / task_profile / plan_stage を保持 | `src/modes/plan_act.rs` |
| `commands.rs` | plan へ入る、approve して act へ移る | `src/agent/loop_run/commands.rs` |
| `system_prompt.rs` | mode ごとに tool と行動原則を切り替える | `src/system_prompt.rs` |
| `registry.rs` | Plan mode 中の tool 書き込み範囲を plan file のみに制約 | `src/tools/registry.rs` |
| `lifecycle.rs` | plan stage 判定、plan 完成度判定、plan summary 作成 | `src/agent/loop_run/lifecycle.rs` |

Plan mode は単なる label ではなく、

- tool 許可範囲
- plan file path
- stage ごとの編集可能 section
- system prompt の振る舞い

まで一貫して変える。

---

## 6. Model 呼び出しレイヤ

## 6.1 client

`src/ollama/client.rs` は、

- model list (`/api/tags`)
- chat / generate
- streaming / non-streaming
- conversation summary
- task classifier
- stage-three fallback classifier

を提供する。

### 6.2 transport

`src/ollama/transport.rs` は request schema を持ち、

- generate request
- chat request
- tool definition serialization

を担当する。

`should_use_native_tool_calls` が allowlist 方式で非常に狭いのが特徴で、
native tool calling は「標準経路」ではなく
**条件付き最適化**として扱われている。

### 6.3 parsing / fallback

`src/ollama/parsing.rs` は、

- Ollama response の decode
- stream chunk の集約
- native tool call parse

を担当する。

`src/ollama/xml_fallback.rs` は、

- `<think>` 除去
- XML / function tag の tool call 回収
- alias 正規化

を担当する。

つまりモデル I/O 層は、
**transport**
→ **parse**
→ **tool fallback salvage**
の 3 段で成り立っている。

---

## 7. Tool 実行レイヤ

`src/tools/registry.rs` が単一の dispatcher になっており、
現在の built-in tools は次の 6 つに絞られている。

- `Bash`
- `Read`
- `Write`
- `Edit`
- `Glob`
- `Grep`

各 tool の特徴:

- `Read`
  - file 読み / directory 一覧 / 行範囲
- `Write`
  - create or overwrite
- `Edit`
  - exact replace + normalized-line fallback + token-anchor fallback
- `Glob` / `Grep`
  - repo 探索
- `Bash`
  - command class 判定
  - offline policy
  - background 化
  - timeout / cancel 対応

また registry では tool 実行前に、

1. mode 制約
2. plan stage 制約
3. approval

を通す。

このため tool 層は、単なる syscall wrapper ではなく
**runtime policy enforcement point** でもある。

---

## 8. Session / persistence / resume

現在の session は `state_root/sessions/<uuid>/` 配下に置かれる。

主な内容:

- `session.json`
- `logs/llm-io.jsonl`
- `plans/plan-<timestamp>.md`

補助構成:

- `src/session/discovery.rs`
  - session directory 走査
  - symlink / oversized json / broken json の除外
- `src/session/sessions_cli.rs`
  - `list/show/clean`
  - explicit resume id の path confinement
- `src/lib.rs`
  - 既存 `.anvil/` があれば `logs/sessions/plans` を symlink

設計としては、
**repo 内に session 状態をべったり書く**のではなく、
**state root へ隔離し、必要時だけ repo から辿れるようにする**
形になっている。

---

## 9. 現在の代表フロー

```text
main
  -> run_cli
    -> Config / state root / session / logging / model select
      -> Agent::new
        -> oneshot / resume / REPL
          -> handle_user_message
            -> run_actor_loop
              -> build_request_messages
              -> Ollama reply
              -> parse tool calls (native or XML fallback)
              -> tool execute
              -> repo / plan / quality / test checks
              -> continue or finish
```

この図から分かる通り、現在の Anvil の中心は
`turn.rs` であり、
他モジュールはその周辺を支える形でぶら下がっている。

---

## 10. 主な根拠ファイル

- `src/main.rs`
- `src/lib.rs`
- `src/cli.rs`
- `src/config.rs`
- `src/model_registry.rs`
- `src/session/store.rs`
- `src/session/discovery.rs`
- `src/session/sessions_cli.rs`
- `src/session/compact.rs`
- `src/modes/plan_act.rs`
- `src/system_prompt.rs`
- `src/agent/loop_run.rs`
- `src/agent/loop_run/commands.rs`
- `src/agent/loop_run/turn.rs`
- `src/agent/loop_run/lifecycle.rs`
- `src/ollama/client.rs`
- `src/ollama/transport.rs`
- `src/ollama/parsing.rs`
- `src/ollama/xml_fallback.rs`
- `src/tools/registry.rs`
- `src/tools/bash.rs`
- `src/tools/read.rs`
- `src/tools/write.rs`
- `src/tools/edit.rs`
