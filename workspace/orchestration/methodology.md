# オーケストレーション方法論

## 1. アーキテクチャ概要

```
┌─────────────────────────────────────────────────────────┐
│  anvil-develop (Orchestrator)                           │
│                                                         │
│  役割:                                                   │
│  - タスク分配・進捗監視・結果収集・統合判断              │
│  - コード変更は一切行わない                              │
│  - commandmatedev CLI でワーカーを制御                   │
│                                                         │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐              │
│  │ send     │  │ wait     │  │ capture  │              │
│  │ respond  │  │ ls       │  │ auto-yes │              │
│  └──────────┘  └──────────┘  └──────────┘              │
└────────┬──────────────┬──────────────┬──────────────────┘
         │              │              │
    ┌────▼────┐    ┌────▼────┐    ┌────▼────┐
    │ Worker  │    │ Worker  │    │ Worker  │
    │ WT-A    │    │ WT-B    │    │ WT-C    │
    │ Issue#N │    │ Issue#M │    │ Issue#K │
    │         │    │         │    │         │
    │ Claude  │    │ Claude  │    │ Claude  │
    │ Session │    │ Session │    │ Session │
    └─────────┘    └─────────┘    └─────────┘
```

### 原則

1. **オーケストレーターはコードに触れない** - 制御と判断のみ
2. **各ワーカーは独立したworktree** - コンフリクトなし
3. **既存スラッシュコマンドを活用** - ワーカーに送信するメッセージとして使う
4. **段階的な自律度** - auto-yes / prompt対応 を使い分け

---

## 2. 並列実行モデル

### 2.1 フェーズ分割と並列化ポイント

既存の `/pm-auto-issue2dev` は1Issue内で **直列** 実行する：

```
Issue Review → Design → Design Review → Work Plan → TDD → PR
```

オーケストレーターはこれを **Issue横断で並列化** する：

```
                   Issue #24                    Issue #25
                   ────────                    ────────
Phase A (準備):    worktree-setup              worktree-setup       ← 並列
Phase B (設計):    issue-review + design       issue-review + design ← 並列
Phase C (実装):    tdd-impl                    tdd-impl              ← 並列
Phase D (検証):    acceptance-test             acceptance-test       ← 並列
Phase E (PR):      create-pr → develop         create-pr → develop   ← 並列→直列マージ
Phase F (UAT):     develop上で /uat 実施                              ← 直列（オーケストレーター）
Phase G (修正):    UAT不合格 → featureで修正 → Phase E に戻る         ← 条件分岐ループ
Phase H (完了):    UAT全合格 → 終了                                   ← ゴール
```

### 2.2 同期ポイント（バリア）

並列実行には **同期が必要なタイミング** がある：

| バリア | タイミング | 理由 |
|--------|----------|------|
| B1 | 設計完了後 | Issue間の依存関係・設計矛盾を検出 |
| C1 | 実装完了後 | 共通モジュールへの変更がコンフリクトしないか確認 |
| E1 | PRマージ時 | 1つずつ順次マージ（コンフリクト検出） |
| F1 | UAT実施 | developブランチ上で受入テスト（全Issueマージ後） |
| G1 | UAT不合格時 | featureブランチに戻って修正→再PR→再UATのループ |

### 2.3 依存関係のあるIssueの扱い

```
パターン1: 独立（Issue #24 と #25）
  → 完全並列。バリアは統合テスト時のみ

パターン2: 弱依存（共通モジュールを触る可能性）
  → 設計フェーズ完了後にオーケストレーターが設計書を突合
  → コンフリクト可能性がある場合、実装順を調整

パターン3: 強依存（A の成果物が B の入力）
  → A を先行、A完了後に B を開始
  → /issues-exec-plan で事前に検出
```

---

## 3. オーケストレーターの制御フロー

### 3.1 基本パターン: Fire-and-Wait

```bash
# 1. 複数ワーカーにタスクを一斉送信
commandmatedev send <wt-1> "<task>" --auto-yes --duration 1h
commandmatedev send <wt-2> "<task>" --auto-yes --duration 1h

# 2. 全ワーカーの完了を待機
commandmatedev wait <wt-1> --timeout 1800
commandmatedev wait <wt-2> --timeout 1800

# 3. 結果を収集して判断
commandmatedev capture <wt-1>
commandmatedev capture <wt-2>
```

### 3.2 段階的パターン: Phase-by-Phase

```bash
# Phase A: 準備（並列）
commandmatedev send <wt-1> "/worktree-setup 24" --auto-yes
commandmatedev send <wt-2> "/worktree-setup 25" --auto-yes
commandmatedev wait <wt-1> --timeout 300
commandmatedev wait <wt-2> --timeout 300

# Phase B: 設計（並列）
commandmatedev send <wt-1> "/pm-auto-issue2dev 24"
commandmatedev send <wt-2> "/pm-auto-issue2dev 25"

# 監視ループ（プロンプト対応）
while true; do
  commandmatedev wait <wt-1> --timeout 60 --on-prompt agent
  # exit 10 → プロンプト検出 → respond
  # exit 0 → 完了 → break
done
```

### 3.3 監視パターン: Polling Dashboard

```bash
# 定期的なステータス確認
commandmatedev ls --json | jq '.[] | select(.repositoryName=="Anvil") | {id, name, status}'
```

---

## 4. 既存コマンドとの統合マッピング

### ワーカーに送信するコマンド

| フェーズ | 送信コマンド | auto-yes | 備考 |
|---------|------------|----------|------|
| 準備 | `/worktree-setup {N}` | Yes | worktreeが既存なら不要 |
| 全自動 | `/pm-auto-issue2dev {N}` | Conditional | 設計フェーズは手動確認推奨 |
| 設計のみ | `/design-policy {N}` | Yes | 軽量タスク |
| 実装のみ | `/pm-auto-dev {N}` | Yes | TDD + テスト + リファクタ |
| PR作成 | `/create-pr` | No | マージ先・タイトル確認が必要 |
| 品質確認 | `cargo clippy && cargo test` | Yes | 結果確認のみ |

### オーケストレーター側で実行するコマンド

| タスク | コマンド | タイミング |
|--------|---------|----------|
| 実行計画策定 | `/issues-exec-plan` | 最初に1回 |
| 進捗確認 | `commandmatedev ls` | 随時 |
| 統合テスト | develop上で `cargo test` | 全PRマージ後 |

---

## 5. 自律度レベル

タスクの性質に応じて自律度を使い分ける：

### Level 1: Full Auto（品質チェック・定型作業）

```bash
commandmatedev send <wt> "cargo clippy --all-targets && cargo test" --auto-yes --duration 1h
```

- ファイル変更なし、リスク低
- 結果を capture で収集するだけ

### Level 2: Semi Auto（TDD実装・リファクタリング）

```bash
commandmatedev send <wt> "/pm-auto-dev 24" --auto-yes --duration 3h --stop-pattern "error|failed|FAIL"
```

- ファイル変更あり、エラー時は停止
- stop-pattern でエラー検知→オーケストレーターが判断

### Level 3: Supervised（設計・PR作成）

```bash
commandmatedev send <wt> "/design-policy 24"
# プロンプトを監視
commandmatedev wait <wt> --timeout 600 --on-prompt agent
# プロンプト内容を確認して応答
commandmatedev respond <wt> "yes"
```

- 重要な判断はオーケストレーターが介入

---

## 6. エラーハンドリング

### 6.1 ワーカーのビルドエラー

```bash
# capture で出力確認
commandmatedev capture <wt> --json
# 必要なら追加指示
commandmatedev send <wt> "ビルドエラーを修正して再度テストを実行してください"
```

### 6.2 タイムアウト

```bash
EXIT=$(commandmatedev wait <wt> --timeout 1800; echo $?)
if [ "$EXIT" -eq 124 ]; then
  # stall しているか確認
  commandmatedev capture <wt>
  # 必要なら中断指示
fi
```

### 6.3 コンフリクト（統合時）

```
1. 各ワーカーのPRをdevelopにマージ試行
2. コンフリクト発生時:
   a. オーケストレーターがコンフリクト内容を分析
   b. 影響が小さい方のワーカーに修正を指示
   c. 再テスト → 再マージ
```

---

## 7. PR → マージ → UAT → 修正ループ

開発完了後のライフサイクル全体を管理する。これがオーケストレーションの最も重要なフェーズ。

### 7.1 全体フロー

```
┌──────────────────────────────────────────────────────┐
│                                                      │
│  [Phase E] PR作成・マージ                             │
│  ┌─────────┐  ┌─────────┐                            │
│  │ WT-24   │  │ WT-25   │  ← 各ワーカーで /create-pr │
│  │ PR #XX  │  │ PR #YY  │                            │
│  └────┬────┘  └────┬────┘                            │
│       │            │                                 │
│       ▼            ▼                                 │
│  develop に順次マージ（1つずつ、テスト確認後）         │
│                                                      │
│  [Phase F] UAT（受入テスト）                          │
│  ┌──────────────────────┐                            │
│  │ develop (orchestrator)│                           │
│  │ /uat 24 25           │  ← オーケストレーターが実行 │
│  └──────────┬───────────┘                            │
│             │                                        │
│        ┌────┴────┐                                   │
│        │ 判定    │                                   │
│   ┌────┴───┐ ┌───┴────┐                             │
│   │全PASS  │ │FAIL有  │                              │
│   │        │ │        │                              │
│   ▼        │ ▼        │                              │
│  完了      │ [Phase G] 修正ループ                     │
│            │ ┌─────────────────────┐                 │
│            │ │ FAILのIssueの       │                 │
│            │ │ featureブランチで修正│                  │
│            │ │ → 再PR → 再マージ   │                  │
│            │ │ → 再UAT             │                 │
│            │ └─────────┬───────────┘                 │
│            │           │                             │
│            │           └──→ Phase F に戻る            │
│            │                                         │
└──────────────────────────────────────────────────────┘
```

### 7.2 Phase E: PR作成・マージ

#### E-1: 各ワーカーにPR作成を指示

```bash
commandmatedev send <wt-24> "/create-pr" --auto-yes --duration 1h
commandmatedev send <wt-25> "/create-pr" --auto-yes --duration 1h
```

- `/create-pr` は develop 向けPRを作成（品質チェック付き）
- PR作成後、CIの通過を確認

#### E-2: 順次マージ（直列）

**重要**: 1つずつマージし、各マージ後にビルド確認する。

```bash
# 1つ目のPRをマージ
gh pr merge <PR_NUMBER_1> --merge --repo Kewton/Anvil

# developを更新・確認
git pull origin develop
cargo build && cargo clippy --all-targets && cargo test

# 問題なければ2つ目をマージ
gh pr merge <PR_NUMBER_2> --merge --repo Kewton/Anvil
git pull origin develop
cargo build && cargo clippy --all-targets && cargo test
```

#### E-3: コンフリクト発生時

```bash
# ワーカーに rebase を指示
commandmatedev send <wt-N> \
  "developの最新を取り込んでコンフリクトを解消してください:
  git fetch origin develop && git rebase origin/develop
  解消後に cargo build && cargo test で確認してください" \
  --auto-yes --duration 1h

commandmatedev wait <wt-N> --timeout 1800

# ワーカーで force push → PR更新 → 再マージ
commandmatedev send <wt-N> "git push --force-with-lease" --auto-yes
```

### 7.3 Phase F: UAT（受入テスト）

**実行場所**: developブランチ（オーケストレーター自身）

```bash
# developが最新であることを確認
git pull origin develop

# 全Issueの受入テストを一括実行
/uat 24 25
```

**`/uat` が行うこと**:
1. 各Issueの受け入れ基準からテスト計画を作成
2. テスト計画を2回レビュー
3. ユーザーにテスト計画を確認
4. Anvilバイナリを実際に起動してE2Eテスト実行
5. HTMLレポート生成（`sandbox/{issue_num}/` 配下）
6. GitHubのIssueコメントに結果を記録
7. 結果サマリーを報告

**UAT結果の判定**:
- 全Issue全テストPASS → **Phase H（完了）** へ
- 1つでもFAIL → **Phase G（修正ループ）** へ

### 7.4 Phase G: UAT不合格 → 修正ループ

#### G-1: 不合格内容の分析

UATレポートからFAIL項目を特定：

```bash
# レポートを確認
cat sandbox/24/latest/report.html  # ブラウザで確認推奨
cat sandbox/25/latest/report.html
```

#### G-2: featureブランチで修正を指示

FAIL したIssueのワーカーに修正を指示する。UAT結果を修正指示に含める：

```bash
commandmatedev send <wt-24> \
  "受入テスト（UAT）で以下のテスト項目がFAILしました。修正してください。

  FAIL項目:
  - AT-24-2: Exploreサブエージェントが読み取り専用ツールのみ使用する
    期待: file.read, file.search のみ許可
    実際: file.write も許可されていた
    原因: ツール制限のフィルタリングが未実装

  修正後、以下を実行して確認してください:
  1. cargo clippy --all-targets（警告0件）
  2. cargo test（全パス）
  3. 修正内容をコミット" \
  --auto-yes --duration 2h

commandmatedev wait <wt-24> --timeout 7200
```

#### G-3: 再PR・再マージ

修正完了後、既存PRが更新される（同じブランチなので追加コミットが反映）。
PRがcloseされている場合は新規PR作成：

```bash
# 既存PRの状態を確認
gh pr list --head feature/issue-24-subagent --state all --json number,state

# openなら追加pushで自動更新、closedなら再作成
commandmatedev send <wt-24> "/create-pr" --auto-yes --duration 1h
```

#### G-4: 再マージ

```bash
gh pr merge <PR_NUMBER> --merge --repo Kewton/Anvil
git pull origin develop
cargo build && cargo clippy --all-targets && cargo test
```

#### G-5: 再UAT → Phase F に戻る

```bash
# 修正が入ったIssueのみ再テスト or 全Issue再テスト
/uat 24     # FAIL した Issue のみ
# or
/uat 24 25  # 回帰確認も含めて全Issue
```

→ 全PASS なら Phase H へ。FAIL が残れば Phase G を再度実行。

### 7.5 Phase H: 完了

全IssueのUATが合格したら完了。

確認事項:
- [ ] 全UATテスト PASS
- [ ] HTMLレポートが `sandbox/` に保存されている
- [ ] GitHub Issueコメントに最終結果が記録されている
- [ ] developブランチで `cargo build && cargo clippy && cargo test` が全パス

---

## 8. 修正ループの最大回数と中断判断

| 回数 | アクション |
|------|----------|
| 1回目 | 通常の修正指示 |
| 2回目 | 修正指示 + 前回との差分を詳しく伝える |
| 3回目 | オーケストレーターが問題を分析し、修正方針を具体的に指示 |
| 4回目以降 | ユーザーに判断を仰ぐ（手動介入 or Issue分割 or スコープ縮小） |

---

## 9. 成果物と報告

オーケストレーターは以下を管理：

```
workspace/orchestration/
├── methodology.md          # この方法論
├── runs/
│   └── YYYY-MM-DD/
│       ├── plan.md          # 実行計画（/issues-exec-plan の出力）
│       ├── status.md        # 各ワーカーの進捗
│       ├── issue-24/
│       │   ├── design.md    # 設計確認結果
│       │   └── result.md    # 実行結果
│       ├── issue-25/
│       │   ├── design.md
│       │   └── result.md
│       └── integration.md   # 統合テスト結果
sandbox/
├── 24/
│   ├── YYYY-MM-DD_001/
│   │   ├── report.html      # UATレポート（第1回）
│   │   └── AT-*/             # テストエビデンス
│   ├── YYYY-MM-DD_002/       # 修正後の再テスト（第2回）
│   ├── latest -> ...
│   └── history.html          # 全ランの履歴
├── 25/
│   └── ...
└── uat-summary.html          # 全体サマリー
```
