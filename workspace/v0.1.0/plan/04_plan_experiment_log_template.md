# Plan Experiment Log Template

## 目的

`Plan` 改善の各実験を、毎回同じ粒度で記録するためのテンプレート。

このテンプレートは、次の比較をしやすくするために使う。

- 何を変えたか
- どのモデルで効いたか
- どこで止まったか
- 採用したか、戻したか

## 実験基本情報

- 実験ID:
- 日付:
- ブランチ:
- ベースコミット:
- 対象タスク:
- 対象改善案:
- 対応ファイル:

## 変更概要

### 目的

-

### 実装内容

-

### 想定している効果

-

### 想定している副作用

-

## ローカル確認

- `cargo test -q`:
- `cargo build --release`:
- その他確認:

## E2E 実行条件

- 実行方式:
  - sequential
- 実行バイナリ:
  - `target/release/anvil`
- 共通プロンプト:

```text
README.md を改善する3段階の作業です。まず変更計画を書き、その後見出しを1つ追加し、最後に内容を確認してください。
```

- sidecar:
- state root:
- run root:
- timeout:

## モデル別結果

### gemma4:31b

- classifier:
- fallback_used:
- plan_first_write:
- plan_approved:
- act_first_repo_edit:
- turn_completed:
- README 更新:
- 停止位置:
- 観測メモ:

### qwen3.5:27b

- classifier:
- fallback_used:
- plan_first_write:
- plan_approved:
- act_first_repo_edit:
- turn_completed:
- README 更新:
- 停止位置:
- 観測メモ:

### qwen3.5:122b

- classifier:
- fallback_used:
- plan_first_write:
- plan_approved:
- act_first_repo_edit:
- turn_completed:
- README 更新:
- 停止位置:
- 観測メモ:

### qwen3.6:35b-a3b

- classifier:
- fallback_used:
- plan_first_write:
- plan_approved:
- act_first_repo_edit:
- turn_completed:
- README 更新:
- 停止位置:
- 観測メモ:

### qwen3.6:27b-coding-nvfp4

- classifier:
- fallback_used:
- plan_first_write:
- plan_approved:
- act_first_repo_edit:
- turn_completed:
- README 更新:
- 停止位置:
- 観測メモ:

## 追加マイルストーン

必要に応じて、今回の改善案で追加した event を記録する。

- `agent.plan.repeated_exploration_detected`:
- `agent.plan.guard_blocked`:
- `agent.plan.stage_changed`:
- `agent.plan.stage_budget_exhausted`:
- `agent.plan.write_args_salvaged`:
- その他:

## 生成物の品質メモ

### Plan品質

- 構造:
- 具体性:
- repo 固有性:
- shallow / generic な傾向:

### Act成果物品質

- 最終READMEまたは成果物の質:
- 最小要件止まりか:
- 追加改善が入ったか:

## 比較対象

- 比較対象コミット:
- 比較対象実験ID:
- 良くなった点:
- 悪化した点:
- 変化なし:

## 判断

- 採用 / 不採用:
- 理由:

## 次アクション

- コミットするか:
- 戻すか:
- 次に試す改善案:

## 補足ログパス

- classifier / plan logs:
- 代表 session.json:
- 代表 plan file:
- 代表 README:

## 短い総括

-
