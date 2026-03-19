# Runbook: 並列オーケストレーション完全手順

コピペで即実行可能な手順書。developブランチのClaude Codeセッションから実行する。

全フェーズ: **準備 → 開発 → PR → マージ → UAT → (修正ループ) → 完了**

---

## 事前準備

```bash
# CommandMate サーバー稼働確認
commandmatedev ls
```

---

## Phase 1: Worktree準備

```bash
# 既存worktreeの確認
commandmatedev ls --branch feature/issue-

# worktreeが存在しない場合は作成（Issue番号・ブランチ名は適宜変更）
git worktree add -b feature/issue-24-subagent ../Anvil-feature-issue-24-subagent develop
git worktree add -b feature/issue-25-hooks ../Anvil-feature-issue-25-hooks develop

# CommandMateに認識させる
curl -s -X POST http://localhost:3000/api/repositories/sync

# 確認（idle で表示されればOK）
commandmatedev ls --branch feature/issue-2
```

---

## Phase 2: 並列開発

```bash
# 両Issueに全自動開発を一斉送信
commandmatedev send anvil-feature-issue-24-subagent \
  "/pm-auto-issue2dev 24" \
  --auto-yes --duration 3h

commandmatedev send anvil-feature-issue-25-hooks \
  "/pm-auto-issue2dev 25" \
  --auto-yes --duration 3h
```

### 進捗確認（随時）

```bash
# ステータス一覧
commandmatedev ls --branch feature/issue-2

# 詳細出力
commandmatedev capture anvil-feature-issue-24-subagent
commandmatedev capture anvil-feature-issue-25-hooks
```

### 完了待機

```bash
commandmatedev wait anvil-feature-issue-24-subagent --timeout 10800
echo "Issue #24 exit: $?"

commandmatedev wait anvil-feature-issue-25-hooks --timeout 10800
echo "Issue #25 exit: $?"
```

---

## Phase 3: 品質確認（並列）

```bash
QUALITY_CMD="以下を順に実行し結果を報告してください:
1. cargo fmt --check
2. cargo clippy --all-targets
3. cargo test
最後に Pass/Fail のサマリーを出力してください。"

commandmatedev send anvil-feature-issue-24-subagent "$QUALITY_CMD" --auto-yes --duration 1h
commandmatedev send anvil-feature-issue-25-hooks "$QUALITY_CMD" --auto-yes --duration 1h

commandmatedev wait anvil-feature-issue-24-subagent --timeout 600
commandmatedev wait anvil-feature-issue-25-hooks --timeout 600

commandmatedev capture anvil-feature-issue-24-subagent
commandmatedev capture anvil-feature-issue-25-hooks
```

品質チェックが FAIL の場合、ワーカーに修正を指示してから次へ進む。

---

## Phase 4: PR作成（並列）

```bash
commandmatedev send anvil-feature-issue-24-subagent "/create-pr" --auto-yes --duration 1h
commandmatedev send anvil-feature-issue-25-hooks "/create-pr" --auto-yes --duration 1h

commandmatedev wait anvil-feature-issue-24-subagent --timeout 600
commandmatedev wait anvil-feature-issue-25-hooks --timeout 600

# PR番号を取得
commandmatedev capture anvil-feature-issue-24-subagent
commandmatedev capture anvil-feature-issue-25-hooks
```

---

## Phase 5: PRマージ（直列）

**1つずつマージし、都度ビルド確認する。**

```bash
# PR番号を取得
PR_24=$(gh pr list --repo Kewton/Anvil --head feature/issue-24-subagent --json number -q '.[0].number')
PR_25=$(gh pr list --repo Kewton/Anvil --head feature/issue-25-hooks --json number -q '.[0].number')

echo "PR #24: $PR_24"
echo "PR #25: $PR_25"
```

### 1つ目をマージ

```bash
gh pr merge "$PR_24" --merge --repo Kewton/Anvil
git pull origin develop
cargo build && cargo clippy --all-targets && cargo test
```

### 2つ目をマージ

```bash
gh pr merge "$PR_25" --merge --repo Kewton/Anvil
git pull origin develop
cargo build && cargo clippy --all-targets && cargo test
```

### コンフリクト発生時

```bash
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

## Phase 6: UAT（受入テスト）

**実行場所: developブランチ（オーケストレーター自身）**

```bash
# developが最新であることを確認
git pull origin develop

# 受入テストを実行（スラッシュコマンド）
/uat 24 25
```

### UAT結果の確認

```bash
# HTMLレポートをブラウザで確認
open sandbox/24/latest/report.html
open sandbox/25/latest/report.html
open sandbox/uat-summary.html
```

---

## Phase 7: UAT結果による分岐

### 7a: 全PASS → 完了

全Issueの全テストが PASS の場合、**Phase 8（完了）** へ進む。

### 7b: FAIL あり → 修正ループ

FAIL した Issue の featureブランチで修正する。

#### 修正指示

```bash
# FAIL した Issue のワーカーに修正を指示（FAIL内容はUATレポートから転記）
commandmatedev send anvil-feature-issue-24-subagent \
  "受入テスト（UAT）で以下のテスト項目がFAILしました。修正してください:

  FAIL項目:
  - AT-24-X: （テスト項目名）
    期待: （期待結果）
    実際: （実際の結果）

  修正後:
  1. cargo clippy --all-targets (警告0件)
  2. cargo test (全パス)
  3. 修正をコミット
  4. git push" \
  --auto-yes --duration 2h

commandmatedev wait anvil-feature-issue-24-subagent --timeout 7200
```

#### 再PR（既存PRがcloseされている場合）

```bash
# 既存PRの状態確認
gh pr list --repo Kewton/Anvil --head feature/issue-24-subagent --state all --json number,state

# open なら push で自動更新済み。closed なら再作成:
commandmatedev send anvil-feature-issue-24-subagent "/create-pr" --auto-yes --duration 1h
commandmatedev wait anvil-feature-issue-24-subagent --timeout 600
```

#### 再マージ

```bash
PR_FIX=$(gh pr list --repo Kewton/Anvil --head feature/issue-24-subagent --json number -q '.[0].number')
gh pr merge "$PR_FIX" --merge --repo Kewton/Anvil
git pull origin develop
cargo build && cargo clippy --all-targets && cargo test
```

#### 再UAT

```bash
# 修正した Issue のみ再テスト
/uat 24

# または回帰確認も含めて全 Issue
/uat 24 25
```

→ 全 PASS なら **Phase 8（完了）** へ。FAIL が残れば修正ループを繰り返す。

**ループ上限の目安**: 3回まで自動、4回目以降はユーザー判断。

---

## Phase 8: 完了

```bash
# 最終品質確認
cargo build && cargo clippy --all-targets && cargo test && cargo fmt --check

# UATレポートの存在確認
ls sandbox/24/latest/report.html
ls sandbox/25/latest/report.html

# GitHub Issue コメントに結果が記録されていることを確認
gh issue view 24 --repo Kewton/Anvil --comments | tail -20
gh issue view 25 --repo Kewton/Anvil --comments | tail -20
```

---

## フロー全体図

```
Phase 1  準備        worktree作成（並列）
  │
Phase 2  開発        /pm-auto-issue2dev（並列）
  │
Phase 3  品質確認    cargo clippy / test（並列）
  │
Phase 4  PR作成      /create-pr（並列）
  │
Phase 5  マージ      develop に順次マージ（直列）
  │
Phase 6  UAT         /uat（developで実行）
  │
Phase 7  判定 ─── 全PASS ──→ Phase 8 完了
  │
  └── FAIL ──→ featureで修正 → 再PR → 再マージ → Phase 6 に戻る
```

---

## クイックリファレンス

| 操作 | コマンド |
|------|---------|
| 送信 | `commandmatedev send <id> "<msg>" [--auto-yes] [--duration Nh]` |
| 待機 | `commandmatedev wait <id> [--timeout N] [--on-prompt agent]` |
| 応答 | `commandmatedev respond <id> "<answer>"` |
| 出力 | `commandmatedev capture <id> [--json]` |
| 一覧 | `commandmatedev ls [--branch <prefix>] [--json] [--quiet]` |
| auto-yes | `commandmatedev auto-yes <id> --enable\|--disable [--duration Nh]` |

| スラッシュコマンド | 実行場所 | 用途 |
|------------------|---------|------|
| `/pm-auto-issue2dev N` | ワーカー（send経由） | 設計〜実装の全自動化 |
| `/create-pr` | ワーカー（send経由） | PR作成 |
| `/uat N [M ...]` | オーケストレーター（develop） | 受入テスト |
| `/worktree-cleanup` | ワーカー（send経由） | 後片付け |
