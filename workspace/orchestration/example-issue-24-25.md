# 具体例: Issue #24 #25 を並列開発する

## Issue概要

| Issue | タイトル | 内容 | 影響モジュール |
|-------|---------|------|--------------|
| #24 | サブエージェント機構 | 並列調査・実行用のサブエージェント（Explore, Plan） | `src/agent/`, `src/tooling/`, `src/app/agentic.rs` |
| #25 | Hooks（ライフサイクルフック）機構 | ツール実行前後のカスタム処理 | `src/hooks/`(新規), `src/app/agentic.rs`, `src/config/` |

## 依存関係分析

```
#24 サブエージェント ──┐
                       ├── 共通: src/app/agentic.rs を両方が変更
#25 Hooks機構 ─────────┘
```

**判定: 弱依存**
- 両Issue とも `src/app/agentic.rs` を変更する
- ただし変更箇所は異なる（#24: サブエージェント起動、#25: フック呼び出し）
- 並列開発可能だが、統合時にコンフリクト注意

---

## 前提条件

### CommandMate サーバー

```bash
commandmatedev start --daemon
```

### Worktreeの確認

```bash
commandmatedev ls --branch feature/issue-2
```

既存worktreeがあれば再利用、なければ作成する。

---

## 実行手順

### Step 0: 実行計画の策定（オーケストレーター）

オーケストレーター（develop）自身で実行計画を確認：

```bash
# Issue #24, #25 の依存関係・優先度を確認
gh issue view 24 --json title,body,labels
gh issue view 25 --json title,body,labels
```

→ 独立性が高いことを確認。並列実行を決定。

---

### Step 1: Worktree準備（並列）

Issue用のworktreeが未作成の場合：

```bash
# 2つのworktreeを並列で準備
commandmatedev send anvil-develop "以下の2つのworktreeを作成してください:
1. git worktree add -b feature/issue-24-subagent ../Anvil-feature-issue-24-subagent develop
2. git worktree add -b feature/issue-25-hooks ../Anvil-feature-issue-25-hooks develop" --auto-yes --duration 1h
```

**注意**: worktreeの作成自体はオーケストレーターが行い、CommandMateに自動登録されるのを待つ。
登録確認：

```bash
commandmatedev ls --branch feature/issue-2
```

想定出力：
```
ID                                    NAME                                   STATUS  DEFAULT
anvil-feature-issue-24-subagent       feature/issue-24-subagent              idle    claude
anvil-feature-issue-25-hooks          feature/issue-25-hooks                 idle    claude
```

---

### Step 2: 設計フェーズ（並列）

```bash
# Issue #24 の設計を開始
commandmatedev send anvil-feature-issue-24-subagent \
  "/pm-auto-issue2dev 24" \
  --auto-yes --duration 3h

# Issue #25 の設計を開始
commandmatedev send anvil-feature-issue-25-hooks \
  "/pm-auto-issue2dev 25" \
  --auto-yes --duration 3h
```

### Step 2.1: 進捗監視

```bash
# 定期的にステータス確認
commandmatedev ls --branch feature/issue-2

# 詳細が必要な場合
commandmatedev capture anvil-feature-issue-24-subagent
commandmatedev capture anvil-feature-issue-25-hooks
```

### Step 2.2: プロンプト対応（必要時）

```bash
# プロンプト待ち
commandmatedev wait anvil-feature-issue-24-subagent --timeout 1800 --on-prompt agent
# exit code 10 の場合
commandmatedev respond anvil-feature-issue-24-subagent "yes"
```

---

### Step 3: 同期ポイント - 設計突合（オーケストレーター）

両方の設計が完了したら、オーケストレーターが設計書を比較：

```bash
# 各ワーカーの設計書を取得
commandmatedev capture anvil-feature-issue-24-subagent
commandmatedev capture anvil-feature-issue-25-hooks
```

確認ポイント：
1. **`src/app/agentic.rs` の変更が競合しないか**
   - #24: サブエージェント起動ロジック追加
   - #25: フック呼び出し（PreToolUse / PostToolUse）追加
   - → 異なる箇所なので並行可能と判断

2. **共通型定義の矛盾がないか**
   - #24: `agent.explore`, `agent.plan` ツール追加
   - #25: `hooks.json` 設定構造追加
   - → 独立した追加なので問題なし

---

### Step 4: 実装フェーズ（並列、auto-yes有効）

Step 2 で `/pm-auto-issue2dev` を使っている場合は、設計→実装まで自動で進む。

個別に実装だけ走らせる場合：

```bash
commandmatedev send anvil-feature-issue-24-subagent \
  "/pm-auto-dev 24" \
  --auto-yes --duration 3h --stop-pattern "error|FAILED"

commandmatedev send anvil-feature-issue-25-hooks \
  "/pm-auto-dev 25" \
  --auto-yes --duration 3h --stop-pattern "error|FAILED"
```

### Step 4.1: 完了待機

```bash
# 両方の完了を待つ（最大3時間）
commandmatedev wait anvil-feature-issue-24-subagent --timeout 10800
echo "Issue #24 exit: $?"

commandmatedev wait anvil-feature-issue-25-hooks --timeout 10800
echo "Issue #25 exit: $?"
```

---

### Step 5: 品質確認（並列）

```bash
# 各ワーカーに品質チェックを指示
commandmatedev send anvil-feature-issue-24-subagent \
  "以下の品質チェックを実行し結果を報告してください:
  cargo fmt --check
  cargo clippy --all-targets
  cargo test" \
  --auto-yes --duration 1h

commandmatedev send anvil-feature-issue-25-hooks \
  "以下の品質チェックを実行し結果を報告してください:
  cargo fmt --check
  cargo clippy --all-targets
  cargo test" \
  --auto-yes --duration 1h

# 完了待ち
commandmatedev wait anvil-feature-issue-24-subagent --timeout 600
commandmatedev wait anvil-feature-issue-25-hooks --timeout 600

# 結果収集
commandmatedev capture anvil-feature-issue-24-subagent
commandmatedev capture anvil-feature-issue-25-hooks
```

---

### Step 6: PR作成（並列）

```bash
commandmatedev send anvil-feature-issue-24-subagent \
  "/create-pr"

commandmatedev send anvil-feature-issue-25-hooks \
  "/create-pr"

# PR作成はプロンプトが出る可能性があるので監視
commandmatedev wait anvil-feature-issue-24-subagent --timeout 300 --on-prompt agent
commandmatedev wait anvil-feature-issue-25-hooks --timeout 300 --on-prompt agent
```

---

### Step 7: PRマージ（直列 - オーケストレーター）

**1つずつ順番にマージし、都度ビルド確認する。**

```bash
# PR番号を取得
PR_24=$(gh pr list --repo Kewton/Anvil --head feature/issue-24-subagent --json number -q '.[0].number')
PR_25=$(gh pr list --repo Kewton/Anvil --head feature/issue-25-hooks --json number -q '.[0].number')

# 1つ目をマージ（#24: agentic.rs の変更が大きい方を先に）
gh pr merge "$PR_24" --merge --repo Kewton/Anvil

# developを更新して確認
git pull origin develop
cargo build && cargo clippy --all-targets && cargo test

# 問題なければ2つ目をマージ
gh pr merge "$PR_25" --merge --repo Kewton/Anvil
git pull origin develop
cargo build && cargo clippy --all-targets && cargo test
```

**コンフリクト発生時**:
```bash
# 2つ目のPRでコンフリクトが発生した場合
commandmatedev send anvil-feature-issue-25-hooks \
  "developブランチの最新を取り込み、コンフリクトを解消してください:
  git fetch origin develop && git rebase origin/develop
  解消後に cargo build && cargo test で確認し、git push --force-with-lease してください" \
  --auto-yes --duration 1h

commandmatedev wait anvil-feature-issue-25-hooks --timeout 1800

# 再度マージ
gh pr merge "$PR_25" --merge --repo Kewton/Anvil
git pull origin develop
```

---

### Step 8: UAT（受入テスト）- developブランチで実行

**実行場所**: オーケストレーター（developブランチ）自身

```bash
# developが最新で、全PRがマージ済みであることを確認
git pull origin develop
git log --oneline -5

# 両Issueの受入テストを一括実行
/uat 24 25
```

`/uat` は以下を自動実行する:
1. Issue #24, #25 の受け入れ基準からテスト計画を作成
2. テスト計画を2回レビュー（サブエージェント）
3. ユーザーにテスト計画を確認
4. Anvilバイナリ（`./target/release/anvil`）を実際に起動してE2Eテスト
5. HTMLレポート生成
6. GitHub Issueコメントに結果記録

**結果の確認**:
```
sandbox/
├── 24/
│   ├── 2026-03-19_001/report.html   ← ブラウザで確認
│   └── history.html
├── 25/
│   ├── 2026-03-19_001/report.html
│   └── history.html
└── uat-summary.html                  ← 全体サマリー
```

---

### Step 9: UAT結果による分岐

#### 9a: 全PASS → 完了

```
受入テスト完了（第1回）

  Issue #24: 4/4 PASS (100%) → ACCEPTED
  Issue #25: 5/5 PASS (100%) → ACCEPTED

  全体: 9/9 PASS (100%)
```

→ **完了**。Step 10 へ。

#### 9b: FAIL あり → 修正ループ

```
受入テスト完了（第1回）

  Issue #24: 3/4 PASS ( 75%) → REJECTED
    FAIL: AT-24-2 Exploreサブエージェントのツール制限
  Issue #25: 5/5 PASS (100%) → ACCEPTED

  全体: 8/9 PASS (89%)
```

→ FAIL した Issue #24 を修正する。

```bash
# FAIL した Issue のワーカーに修正指示
commandmatedev send anvil-feature-issue-24-subagent \
  "受入テスト（UAT）で以下がFAILしました。修正してください:

  FAIL: AT-24-2 Exploreサブエージェントのツール制限
  - 期待: Exploreサブエージェントは file.read, file.search のみ使用可能
  - 実際: file.write も許可されていた
  - レポート: sandbox/24/2026-03-19_001/report.html 参照

  修正後:
  1. cargo clippy --all-targets (警告0件)
  2. cargo test (全パス)
  3. 修正をコミット
  4. git push" \
  --auto-yes --duration 2h

commandmatedev wait anvil-feature-issue-24-subagent --timeout 7200
```

修正完了後、PRが自動更新される（同じブランチへのpush）。
closeされていれば再作成：

```bash
commandmatedev send anvil-feature-issue-24-subagent "/create-pr" --auto-yes --duration 1h
```

#### 再マージ → 再UAT

```bash
# 再マージ
PR_24_NEW=$(gh pr list --repo Kewton/Anvil --head feature/issue-24-subagent --json number -q '.[0].number')
gh pr merge "$PR_24_NEW" --merge --repo Kewton/Anvil
git pull origin develop

# 再UAT（修正したIssueのみ、または回帰確認で全Issue）
/uat 24
# or
/uat 24 25
```

→ 全PASS なら完了。FAIL が残れば再度修正ループ。

**最大ループ回数の目安**:
| 回数 | アクション |
|------|----------|
| 1回目 | UATレポートの FAIL 項目をワーカーに伝えて修正 |
| 2回目 | 前回との差分も含めて詳しく修正指示 |
| 3回目 | オーケストレーターが問題を分析し具体的な修正方針を指示 |
| 4回目 | ユーザーに判断を仰ぐ |

---

### Step 10: 完了確認

```bash
# 最終状態の確認
cargo build && cargo clippy --all-targets && cargo test && cargo fmt --check

# UAT結果の確認
ls sandbox/24/latest/report.html
ls sandbox/25/latest/report.html

# GitHub Issue コメントに結果が記録されていることを確認
gh issue view 24 --repo Kewton/Anvil --comments | tail -20
gh issue view 25 --repo Kewton/Anvil --comments | tail -20
```

---

## タイムライン見積もり

```
Time  │ Issue #24 (WT-A)         │ Issue #25 (WT-B)         │ Orchestrator
──────┼──────────────────────────┼──────────────────────────┼──────────────────────
 0:00 │ worktree-setup           │ worktree-setup           │ send × 2
 0:05 │ Issue Review             │ Issue Review             │ wait
 0:20 │ Design Policy            │ Design Policy            │ wait
 0:40 │ Design Review            │ Design Review            │ wait
 1:00 │ ── barrier ──            │ ── barrier ──            │ 設計突合チェック
 1:10 │ Work Plan                │ Work Plan                │ wait
 1:20 │ TDD Implementation       │ TDD Implementation       │ wait (long)
 2:30 │ Acceptance Test          │ Acceptance Test          │ wait
 3:00 │ Refactoring              │ Refactoring              │ wait
 3:15 │ PR Creation              │ PR Creation              │ wait
 3:30 │                          │                          │ PRマージ（順次）
 3:45 │                          │                          │ /uat 24 25（受入テスト）
 4:30 │                          │                          │ UAT結果判定
      │                          │                          │
      │ ── UAT PASS の場合 ──    │                          │
 4:30 │                          │                          │ 完了
      │                          │                          │
      │ ── UAT FAIL の場合 ──    │                          │
 4:30 │ 修正                     │                          │ 修正指示 (send)
 5:00 │ 修正完了・push           │                          │ wait → 再マージ
 5:15 │                          │                          │ /uat 24（再テスト）
 5:45 │                          │                          │ 完了
```

**直列実行時**: 約 9-10時間（1Issue 4-5時間 × 2）
**並列実行時（UAT 1回合格）**: 約 4.5時間
**並列実行時（UAT 修正1回）**: 約 5.5-6時間
**短縮率**: 約 40-55%
