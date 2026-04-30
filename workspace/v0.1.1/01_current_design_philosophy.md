# 現在の設計思想

## 1. 要約

現在の Anvil は、**汎用エージェントを広く抽象化する**よりも、
**ローカル環境で Ollama を相手に安定して前進させる**ことを優先した実装である。

設計の重心は次の 6 点にある。

1. **local-first / Ollama 専用**
2. **単純な操作モデルを優先**
3. **ローカル LLM の逸脱を runtime 制御で矯正**
4. **terminal UX もプロダクト機能として扱う**
5. **安全境界は「信頼」ではなく「制約」で作る**
6. **成功判定は会話完了ではなく repo 進捗寄り**

---

## 2. local-first / Ollama 専用

Anvil の入口は最初から `local-first coding agent for Ollama` と定義されている。  
`src/cli.rs` と `src/config.rs` は Ollama host・model・context budget を中心に構成され、
`src/safety/host_validation.rs` では接続先も localhost のみに制限している。

さらに `src/ollama/client.rs` / `src/ollama/transport.rs` は
`/api/tags`・`/api/generate`・`/api/chat` を直接扱っており、
現在の実装には provider abstraction の層はない。

**意味すること**

- 「どの provider にも対応する」より「Ollama で壊れにくい」を優先している
- ローカル環境の RAM / インストール済み model に合わせた model 選択を前提にしている
- ネットワーク越しの外部サービスより、同一マシン内の実運用を前提にしている

---

## 3. 単純な操作モデルを優先

ユーザーに見せる操作モデルは、依然として **Plan / Act** の二状態が中心である。

- `src/modes/plan_act.rs`
  - `ExecutionMode::{Plan, Act}`
  - `PlanStage::{Stage1, Stage2, Stage3, Ready}`
- `src/agent/loop_run/commands.rs`
  - `enter_plan_mode`
  - `approve_plan_mode`
  - `maybe_auto_plan_prompt`
- `src/system_prompt.rs`
  - Plan mode は read-heavy / plan-file only
  - Act mode は accepted plan に沿って実行

単純なのは UI と mental model であり、
内部の制御はむしろかなり厚い。
つまり現在の Anvil は、
**表の操作感は単純、裏の回復ロジックは高度**
という設計になっている。

---

## 4. ローカル LLM の逸脱を runtime 制御で矯正

現行アーキテクチャの一番大きい特徴はここにある。

### 4.1 protocol を理想化しすぎない

- `src/ollama/transport.rs`
  - native tool calling は allowlist 方式
- `src/agent/prompting.rs`
  - `ToolProtocol::{Native, TaggedXml}`
- `src/ollama/xml_fallback.rs`
  - `<think>` 除去
  - XML / function tag 由来の tool call 回収
  - 引数 alias 正規化
- `src/agent/loop_run/turn.rs`
  - parser failure / transport failure 時に native tools を session 単位で downgrade

つまり「function calling が正しく出るはず」とは置いていない。  
**native が壊れたら fallback に落とす**思想が、現実の local model 前提で組み込まれている。

### 4.2 行動を促す guard が多い

`src/agent/loop_run/turn.rs` には、

- no-tool / empty response の再試行
- focused edit 回復
- bash loop 抑止
- plan exploration budget
- repeated plan exploration block
- transport retry
- deterministic scaffold / polish fallback

が入り、モデルを「自由に泳がせる」より
**失敗しやすい振る舞いを runtime が狭める**方向に寄っている。

### 4.3 成果物を repo 変化で見る

`src/agent/orchestration.rs` は repo snapshot を取り、
変更ファイルを

- implementation
- test
- setup
- other
- deleted

に分類する。

ここから分かる通り、
Anvil は単に assistant が返答したかではなく、
**repo にどんな進捗が出たか**を重要視している。

---

## 5. terminal UX もプロダクト機能として扱う

現在の Anvil は、CLI であっても
「LLM が遅い local 実行をどう気持ちよく扱うか」を明示的に設計している。

- `src/agent/loop_run/spinner.rs`
  - 推論 / tool 実行中の spinner
- `src/agent/loop_run/footer.rs`
  - fixed footer
  - token / mode / log / yes の表示
- `src/agent/loop_run/interrupt.rs`
  - ESC による割り込み
- `src/tui/markdown.rs`
  - assistant stream を SGR-only markdown として整形
  - `<think>` は表示しない

これは「出力が見やすい」以上の意味を持つ。  
local LLM は待ち時間と再試行が発生しやすいので、
**runtime の体感そのものを改善対象としている**。

---

## 6. 安全境界は「信頼」ではなく「制約」で作る

安全性は「モデルが賢く振る舞う」ことに依存していない。

### 6.1 path confinement

- `src/safety/path_guard.rs`
  - workspace 外への path escape を拒否
  - relative / absolute の両方を正規化

### 6.2 localhost restriction

- `src/safety/host_validation.rs`
  - Ollama host は localhost / 127.0.0.1 / ::1 のみ
  - credential 付き URL も拒否

### 6.3 tool 実行制約

- `src/tools/registry.rs`
  - Plan mode では plan file 以外への write/edit を禁止
  - stage ごとに編集可能 section を制限
  - Bash / Write / Edit は approval 対象
- `src/tools/bash.rs`
  - dangerous command 断片を block
  - offline 時の command policy

設計思想としては、
**危険な自由度を後で検知する**より
**最初から届かないようにする**方を選んでいる。

---

## 7. 非目標として見えるもの

現在コードから逆に分かる「まだ重心を置いていないもの」もある。

- provider を増やすこと
- 大規模な tool ecosystem を持つこと
- sub-agent / MCP を中心に据えた分散構成
- 完全な抽象化より先に広い拡張点を作ること

実装はむしろ、
**Ollama 専用・小さめの tool set・session と TUI を含む一体型 CLI**
としてまとまっている。

---

## 8. 主な根拠ファイル

- `src/cli.rs`
- `src/config.rs`
- `src/model_registry.rs`
- `src/system_prompt.rs`
- `src/modes/plan_act.rs`
- `src/agent/loop_run/commands.rs`
- `src/agent/loop_run/turn.rs`
- `src/agent/prompting.rs`
- `src/ollama/client.rs`
- `src/ollama/transport.rs`
- `src/ollama/xml_fallback.rs`
- `src/tools/registry.rs`
- `src/tools/bash.rs`
- `src/safety/path_guard.rs`
- `src/safety/host_validation.rs`
- `src/agent/loop_run/footer.rs`
- `src/agent/loop_run/interrupt.rs`
- `src/tui/markdown.rs`
