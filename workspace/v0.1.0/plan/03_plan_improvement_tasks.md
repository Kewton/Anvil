# Plan Improvement Tasks

## 目的

`02_plan_improvement_priorities.md` の優先順位を、実際に着手できるタスク単位へ落とし込む。

このメモでは、各タスクについて次を整理する。

- 実装対象
- 変更箇所
- 確認方法
- 採用判断

## 前提

現時点の有効な改善は次のとおり。

- classifier fallback 改善
- `PlanProgress` expectation
- E2E milestone logging
- tool argument normalization
- stage-aware plan prompting
- classifier retry と error logging
- classifier failure 時の heuristic fallback

一方で、`Plan` の主ボトルネックは依然として

- repeated exploration
- weak tool-level forward control
- shallow but structurally valid plans

にある。

## Task 1: normalized repeated tool detection

### 目的

`Plan` 中の同一探索反復を、文字列差分ではなく意味単位で検出する。

### 実装内容

- `Read / Glob / Grep` 用の正規化関数を追加する
- `tool name + normalized args + current stage` を recent history に保持する
- 同一パターンがしきい値を超えたら repeated exploration とみなす

### 想定変更箇所

- `src/agent/loop_run/turn.rs`
- 必要なら `src/agent/recovery.rs`
- 必要なら milestone logging 周辺

### ログ追加候補

- `agent.plan.repeated_exploration_detected`
- `tool`
- `normalized_args`
- `stage`
- `count`

### 確認方法

- `cargo test -q`
- `cargo build --release`
- 同一の `README` 改善タスクで sequential E2E
- `Read README` 反復が減るかを見る

### 採用基準

- 少なくとも 1〜2 モデルで `plan_first_write` が前進する
- repeated exploration がログで明確に出る
- 正常系で過剰 block が増えない

## Task 2: tool-level block for repeated exploration

### 目的

recovery note ではなく、tool result として repeated exploration を止める。

### 実装内容

- repeated exploration 検出時に `ToolResult(error=true)` を返す
- エラーメッセージに current stage と next expected action を含める

例:

- `Error: repeated exploration blocked for Stage 1`
- `Update Goal, Constraints, and Deliverables in the plan file next`

### 想定変更箇所

- `src/agent/loop_run/turn.rs`
- `src/agent/recovery.rs`

### ログ追加候補

- `agent.plan.guard_blocked`
- `reason=repeated_exploration`
- `stage`

### 確認方法

- `cargo test -q`
- `cargo build --release`
- sequential E2E
- block 後に `Write/Edit(plan)` に切り替わるか確認

### 採用基準

- block が実発火する
- 少なくとも 1 モデルで `plan_first_write` または `plan_approved` が前進する
- user-facing 表示が分かりやすい

## Task 3: salvage strengthening for plan writes

### 目的

`Write/Edit(plan)` の軽い args 崩れや shallow output を吸収しやすくする。

### 実装内容

- `path/file/file_path` 系 alias の追加検討
- `content/text/body` 系 alias の追加検討
- nested wrapper の追加吸収
- shallow plan write に対して短い corrective retry を入れるか検討

### 想定変更箇所

- `src/ollama/xml_fallback.rs`
- `src/agent/loop_run/commands.rs`
- 必要なら `src/agent/loop_run/turn.rs`

### ログ追加候補

- `agent.plan.write_args_salvaged`
- `agent.plan.write_args_unsalvageable`

### 確認方法

- `cargo test -q`
- `cargo build --release`
- 既知の `qwen` 系揺れを含む E2E で確認

### 採用基準

- `Write/Edit(plan)` の失敗率が下がる
- 誤解釈で危険な action を通さない

## Task 4: repo-specific observation requirement

### 目的

repo 固有の観察を含まない generic plan が通りにくいようにする。

### 実装内容

- `Quality Bar` に repo-specific observation 条件を追加
- approval readiness 判定に、最低 1 つの具体観察を要求するか検討

例:

- 既存内容の弱点を1つ以上指摘している
- 改善案が reader / maintainer / current repo structure に結びついている

### 想定変更箇所

- `src/agent/loop_run/lifecycle.rs`
- `src/system_prompt.rs`
- `src/agent/loop_run/commands.rs`

### 確認方法

- `cargo test -q`
- `cargo build --release`
- 完走した plan の内容を目視確認

### 採用基準

- 完走時の plan 品質が上がる
- ただし approval 到達率が大きく悪化しない

## Task 5: classifier and transport observation refinement

### 目的

`Plan` 停滞と classifier / transport failure をより明確に分離して観測する。

### 実装内容

- classifier retry / fallback の structured logs を拡張
- stage 遷移と stage 停滞の milestone を追加

追加候補:

- `agent.plan.stage_changed`
- `agent.plan.stage_budget_exhausted`
- `agent.plan.recovery_note_added`

### 想定変更箇所

- `src/agent/loop_run/commands.rs`
- `src/agent/loop_run/turn.rs`
- 必要なら `src/agent/loop_run/lifecycle.rs`

### 確認方法

- `cargo test -q`
- `cargo build --release`
- E2E ログの比較しやすさを確認

### 採用基準

- どこで止まったかが今より明確になる
- `Plan` 問題と transport 問題を分離しやすくなる

## 推奨実施順

1. Task 1: normalized repeated tool detection
2. Task 2: tool-level block for repeated exploration
3. Task 3: salvage strengthening for plan writes
4. Task 5: classifier and transport observation refinement
5. Task 4: repo-specific observation requirement

## なぜこの順か

最初の3つは、`Plan` を前進させるための基盤である。

- 反復を見つける
- 反復を止める
- 壊れた write を救う

その後で、観測を強化して改善の効き先を見やすくする。  
最後に、repo 固有性を強めて plan 品質を底上げする。

repo 固有性の要求を早く入れすぎると、現状でも止まりやすい `Plan` がさらに止まりやすくなるため、最後に回すのがよい。

## 実験運用ルール

各タスクは単独で試す。

1. 1 タスクだけ実装
2. `cargo test -q`
3. `cargo build --release`
4. sequential E2E
5. 効果があればコミット
6. 効果が弱ければ戻す

## E2E の基本観測点

- `agent.classifier.result`
- `agent.classifier.error`
- `agent.classifier.fallback_used`
- `agent.milestone.plan_first_write`
- `agent.milestone.plan_approved`
- `agent.milestone.act_first_repo_edit`
- `agent.milestone.turn_completed`

追加したい観測点:

- `agent.plan.repeated_exploration_detected`
- `agent.plan.guard_blocked`
- `agent.plan.stage_changed`
- `agent.plan.write_args_salvaged`

## まとめ

`Plan` 改善は、まず

- repeated exploration
- weak runtime control
- fragile plan write

を潰すべきである。

その土台ができてから、

- repo 固有の深い観察
- 高品質な plan 条件

を強めるのがよい。
