# 不足コマンド・補完提案

## 現状のギャップ分析

既存のスラッシュコマンドは **単一Issue・単一worktree** を前提としている。
並列オーケストレーションに必要な以下の機能が不足している：

| カテゴリ | 不足機能 | 現状の代替手段 |
|---------|---------|--------------|
| ワーカー管理 | 複数worktreeへの一斉タスク送信 | 手動で commandmatedev send を複数回 |
| 進捗監視 | 複数ワーカーの統合ダッシュボード | commandmatedev ls を目視 |
| 同期制御 | バリア同期（全ワーカー完了待ち） | commandmatedev wait を順次実行 |
| 設計突合 | 複数Issueの設計書クロスチェック | 手動で設計書を読み比べ |
| 統合テスト | マージ後の回帰テスト自動化 | 手動で cargo test |
| 結果集約 | 全ワーカーの結果を統合レポート化 | 手動で capture を読む |

---

## 提案1: `/orchestrate` コマンド（新規）

複数Issueの並列オーケストレーションを1コマンドで実行する上位コマンド。

### 概要

```
/orchestrate 24 25
/orchestrate 24 25 --phase design    # 設計フェーズまで
/orchestrate 24 25 --phase impl      # 実装まで（設計は完了済み前提）
/orchestrate 24 25 --full            # PR作成まで全自動
```

### 実行フロー

```
/orchestrate 24 25
    │
    ├── Step 1: /issues-exec-plan で依存関係分析
    │           → 並列実行可能か判定
    │
    ├── Step 2: worktree確認・作成
    │           → commandmatedev ls で既存チェック
    │           → なければ git worktree add
    │
    ├── Step 3: 並列タスク送信
    │           → commandmatedev send <wt-24> "/pm-auto-issue2dev 24"
    │           → commandmatedev send <wt-25> "/pm-auto-issue2dev 25"
    │
    ├── Step 4: 監視ループ
    │           → commandmatedev wait + capture を繰り返し
    │           → プロンプト検出時は内容に応じて自動応答 or ユーザーに確認
    │
    ├── Step 5: 設計突合（バリア）
    │           → 両方の設計書を読み込んでクロスチェック
    │           → 問題があればワーカーに修正指示
    │
    ├── Step 6: 完了待ち + 品質チェック確認
    │
    └── Step 7: 統合レポート生成
```

### 出力

```
workspace/orchestration/runs/2026-03-18/
├── plan.md                 # 実行計画
├── status.md               # 最終ステータス
├── issue-24/
│   ├── worker-log.md       # ワーカーの出力ログ
│   └── quality-check.md    # 品質チェック結果
├── issue-25/
│   ├── worker-log.md
│   └── quality-check.md
├── design-crosscheck.md    # 設計突合結果
└── summary.md              # 統合サマリー
```

---

## 提案2: `/parallel-quality-check` コマンド（新規）

全Anvilワーカー（or 指定ワーカー）に品質チェックを一斉実行。

### 概要

```
/parallel-quality-check                      # 全Anvil worktree
/parallel-quality-check --branch feature/*   # featureブランチのみ
```

### 実行内容

```bash
# 1. 対象worktree一覧を取得
TARGETS=$(commandmatedev ls --branch feature/ --quiet)

# 2. 各ワーカーに品質チェックを送信
for WT in $TARGETS; do
  commandmatedev send "$WT" \
    "cargo fmt --check && cargo clippy --all-targets && cargo test" \
    --auto-yes --duration 1h
done

# 3. 結果を収集
for WT in $TARGETS; do
  commandmatedev wait "$WT" --timeout 600
  commandmatedev capture "$WT"
done
```

### 出力

```markdown
## 品質チェック結果 (2026-03-18)

| Worktree | fmt | clippy | test | 結果 |
|----------|-----|--------|------|------|
| feature/issue-24-subagent | Pass | Pass | Pass | OK |
| feature/issue-25-hooks | Pass | 2 warnings | Pass | NG |
```

---

## 提案3: `/design-crosscheck` コマンド（新規）

複数Issueの設計書をクロスチェックし、矛盾・競合を検出。

### 概要

```
/design-crosscheck 24 25
```

### 実行内容

1. 各Issueの設計方針書を読み込む
   ```
   dev-reports/design/issue-24-*-design-policy.md
   dev-reports/design/issue-25-*-design-policy.md
   ```

2. 以下の観点でクロスチェック：
   - **影響ファイルの重複**: 同じファイルを変更する場合のコンフリクトリスク
   - **型定義の整合性**: 共通型への変更が矛盾しないか
   - **アーキテクチャの一貫性**: 異なるIssueの設計方針が相反しないか
   - **モジュール境界**: 新規モジュールの責務が重複しないか

3. レポート出力

### 出力例

```markdown
## 設計クロスチェック結果

### コンフリクトリスク

| ファイル | Issue #24 の変更 | Issue #25 の変更 | リスク |
|---------|-----------------|-----------------|--------|
| src/app/agentic.rs | サブエージェント起動追加 | フック呼び出し追加 | 中（異なる箇所） |
| src/tooling/mod.rs | agent.* ツール追加 | 変更なし | 低 |
| src/config/mod.rs | 変更なし | hooks設定読み込み追加 | 低 |

### 推奨事項
- src/app/agentic.rs: #24 はループ本体、#25 はツール実行前後に挿入 → 競合低
- マージ順序: #24 → #25 を推奨（#25 の方がagentic.rsの変更が限定的）
```

---

## 提案4: `/worker-status` コマンド（新規）

CommandMate連携のリアルタイムステータス確認。

### 概要

```
/worker-status              # 全ワーカー一覧
/worker-status 24           # Issue #24 のワーカーのみ
```

### 実行内容

```bash
commandmatedev ls --json | jq '[.[] | select(.repositoryName=="Anvil")] |
  map({id, name, status: .sessionStatusByCli.claude})'
```

### 出力例

```markdown
## ワーカーステータス

| ID | Branch | Running | Processing | Waiting |
|----|--------|---------|------------|---------|
| anvil-feature-issue-24-subagent | feature/issue-24-subagent | Yes | Yes | No |
| anvil-feature-issue-25-hooks | feature/issue-25-hooks | Yes | No | Yes |
| anvil-develop | develop | Yes | Yes | No |
```

---

## 提案5: `/pr-merge-pipeline` コマンド（新規）

複数ワーカーのPR作成からdevelopへのマージ完了までを一貫して自動化するパイプライン。

### 概要

```
/pr-merge-pipeline 24 25                    # PR作成→CI→マージ（全自動）
/pr-merge-pipeline 24 25 --merge-order 24,25  # マージ順序を明示指定
/pr-merge-pipeline 24 25 --skip-pr           # PR作成済みの場合（マージのみ）
/pr-merge-pipeline 24 25 --dry-run           # 実行せずプランのみ表示
```

### 実行フロー

```
/pr-merge-pipeline 24 25
    │
    ├── Step 1: 事前チェック
    │   ├── commandmatedev ls で各Issue worktreeの存在・ステータス確認
    │   ├── 各ワーカーが ready であること（running なら wait）
    │   └── 既存PRの有無を確認（あればスキップ）
    │
    ├── Step 2: PR作成（並列）
    │   ├── commandmatedev send <wt-24> "/create-pr" --auto-yes
    │   ├── commandmatedev send <wt-25> "/create-pr" --auto-yes
    │   ├── commandmatedev wait <wt-24> --timeout 600
    │   ├── commandmatedev wait <wt-25> --timeout 600
    │   └── PR番号を取得・記録
    │
    ├── Step 3: CI通過待ち（並列）
    │   ├── gh pr checks <PR-24> --watch --fail-fast
    │   ├── gh pr checks <PR-25> --watch --fail-fast
    │   └── CI失敗時 → ワーカーに修正指示 → 再チェック
    │
    ├── Step 4: マージ順序の決定
    │   ├── --merge-order 指定あり → そのまま使用
    │   └── 指定なし → 影響ファイル分析で自動決定
    │       ├── 変更が大きいIssueを先にマージ（コンフリクト最小化）
    │       └── 共通ファイル変更数が少ないIssueを後にマージ
    │
    ├── Step 5: 順次マージ（直列、1つずつ）
    │   │
    │   ├── Issue #24 のマージ
    │   │   ├── gh pr merge <PR-24> --merge --repo Kewton/Anvil
    │   │   ├── git pull origin develop
    │   │   ├── cargo build && cargo clippy --all-targets && cargo test
    │   │   └── 失敗時 → revert して中断、ユーザーに報告
    │   │
    │   └── Issue #25 のマージ
    │       ├── マージ可能性チェック（gh pr view <PR-25> --json mergeable）
    │       ├── コンフリクト検出時 → Step 5a（自動解消）
    │       ├── gh pr merge <PR-25> --merge --repo Kewton/Anvil
    │       ├── git pull origin develop
    │       ├── cargo build && cargo clippy --all-targets && cargo test
    │       └── 失敗時 → revert して中断
    │
    ├── Step 5a: コンフリクト自動解消（必要時のみ）
    │   ├── commandmatedev send <wt-25> \
    │   │     "developの最新を取り込みコンフリクトを解消:
    │   │      git fetch origin develop && git rebase origin/develop
    │   │      解消後 cargo build && cargo test && git push --force-with-lease"
    │   │     --auto-yes --duration 1h
    │   ├── commandmatedev wait <wt-25> --timeout 3600
    │   └── 解消失敗時 → ユーザーに報告して中断
    │
    ├── Step 6: 最終統合検証
    │   ├── cargo build
    │   ├── cargo clippy --all-targets
    │   ├── cargo test
    │   ├── cargo fmt --check
    │   └── 全パス確認
    │
    └── Step 7: 結果レポート
        └── マージ結果サマリーを出力
```

### マージ順序の自動決定ロジック

`--merge-order` が未指定の場合、以下の基準で自動決定：

```
1. 各PRの変更ファイルを取得
   gh pr diff <PR-N> --stat

2. 共通変更ファイルの特定
   → 両PRが変更するファイル = コンフリクト候補

3. スコアリング
   score(issue) = 変更ファイル数 × 2 + 共通ファイル変更行数
   → スコアが高い（変更が大きい）Issue を先にマージ
   → 後続のIssueは少ない変更で rebase しやすい

4. 依存関係による上書き
   → Issue間に依存がある場合、依存元を先にマージ
```

### CI失敗時のリカバリ

```
CI失敗を検出
  │
  ├── ビルドエラー / clippy警告
  │   → commandmatedev send <wt> "CIが失敗しました。エラーを修正してpushしてください:
  │       {CI失敗ログの内容}" --auto-yes --duration 1h
  │   → commandmatedev wait <wt> --timeout 3600
  │   → 再度CI待ち
  │
  ├── テスト失敗
  │   → commandmatedev send <wt> "CIのテストが失敗しました。修正してください:
  │       {失敗テスト名と出力}" --auto-yes --duration 1h
  │   → commandmatedev wait <wt> --timeout 3600
  │   → 再度CI待ち
  │
  └── 3回連続失敗
      → ユーザーに報告して中断
```

### マージ後のビルド失敗時のリカバリ

```
マージ後のビルド/テスト失敗を検出
  │
  ├── 直前のマージが原因と判断
  │   → gh pr revert を検討（ユーザー確認）
  │   → または該当ワーカーに修正指示
  │
  └── 前のマージとの相互作用が原因
      → 後のIssueのワーカーに修正指示
      → "developの最新でビルドが失敗しています。以下のエラーを修正してください: ..."
```

### 出力

```markdown
## PR Merge Pipeline 結果

### 実行サマリー

| # | Issue | PR | CI | マージ | ビルド検証 |
|---|-------|----|----|--------|----------|
| 1 | #24 | #XX | Pass | 完了 | Pass |
| 2 | #25 | #YY | Pass (rebase 1回) | 完了 | Pass |

### 統合検証

| チェック | 結果 |
|---------|------|
| cargo build | Pass |
| cargo clippy --all-targets | Pass (0 warnings) |
| cargo test | Pass (XX tests) |
| cargo fmt --check | Pass |

### develop ブランチ状態

- コミット: {hash}
- マージ済みPR: #XX, #YY
- 次のアクション: /uat 24 25（受入テスト）
```

### 既存コマンドとの連携

```
/pm-auto-issue2dev (各ワーカー)   ← 開発完了
        │
        ▼
/pr-merge-pipeline 24 25          ← ★ このコマンド
        │
        ├── /create-pr (各ワーカーに送信)
        ├── CI待ち + 順次マージ
        └── 統合検証
        │
        ▼
/uat 24 25 (developで実行)        ← 受入テスト
        │
        ▼
/uat-fix-loop (FAIL時)            ← 修正ループ
```

---

## 提案6: `/uat-fix-loop` コマンド（新規）

UAT不合格 → featureブランチ修正 → 再PR → 再マージ → 再UATの一連の修正ループを自動化。

### 概要

```
/uat-fix-loop 24 25          # UAT結果をもとに修正ループを実行
/uat-fix-loop 24 --max-retry 3  # 最大リトライ回数を指定
```

### 実行フロー

```
1. 直前のUATレポートを読み込む
   sandbox/{issue_num}/latest/report.html → FAIL項目を抽出

2. FAIL した Issue の featureブランチ worktree を特定
   commandmatedev ls --branch feature/issue-{N} --quiet

3. 修正指示を送信
   commandmatedev send <wt> "UAT FAIL修正: {FAIL詳細}" --auto-yes

4. 修正完了を待機
   commandmatedev wait <wt> --timeout 7200

5. 再PR（既存PRがcloseなら再作成）
   gh pr list --head <branch> --state all → open/closed判定

6. 再マージ
   gh pr merge <PR> --merge
   git pull origin develop

7. 再UAT
   /uat {fail_issues}

8. 結果判定
   全PASS → 完了
   FAIL残り → retry_count < max_retry なら Step 3 に戻る
   retry超過 → ユーザーに判断を仰ぐ
```

### 出力

UATの run 履歴として `sandbox/{issue_num}/` に蓄積される（`/uat` の既存機能）。

---

## 既存コマンドの拡張提案

### `/issues-exec-plan` の拡張

現状: 実行計画をMarkdownとして出力するだけ
拡張: **並列実行可能なIssueグループを自動検出**

```markdown
### 並列実行グループ

| グループ | Issues | 理由 |
|---------|--------|------|
| A | #24, #25 | 依存関係なし、影響モジュール一部重複（低リスク） |
| B | #XX | グループAに依存 |
```

### `/progress-report` の拡張

現状: 単一Issueの進捗報告
拡張: **複数ワーカーの統合進捗ダッシュボード**

```bash
# 全ワーカーの capture を集約してレポート生成
for WT in $(commandmatedev ls --branch feature/ --quiet); do
  commandmatedev capture "$WT" --json >> /tmp/all-workers.json
done
# JSONを集約して統合レポート生成
```

### `/uat` の拡張（修正ループ統合）

現状: テスト実行とレポート生成のみ
拡張案: `--fix` オプション追加で修正ループを内蔵

```bash
/uat 24 25 --fix               # FAIL時に自動修正ループ（最大3回）
/uat 24 25 --fix --max-retry 5 # 最大5回
```

これにより `/uat-fix-loop` を独立コマンドにせず `/uat` に統合する選択肢もある。

---

## 優先度

| 提案 | 優先度 | 理由 |
|------|--------|------|
| `/orchestrate` | **高** | コアとなる上位コマンド。これがないと毎回手動でsend/wait |
| `/pr-merge-pipeline` | **高** | PR作成→CI→マージの一連が毎回必要。手動は煩雑でミスが起きやすい |
| `/uat-fix-loop` | **高** | UAT→修正→再テストのループが頻発する。手動は煩雑 |
| `/parallel-quality-check` | 中 | 頻繁に使うが `/pr-merge-pipeline` のStep内で品質チェックが含まれる |
| `/design-crosscheck` | 中 | 並列開発の安全性に寄与。`/orchestrate` のStep内に内蔵も可 |
| `/worker-status` | 低 | 便利だが commandmatedev ls --json で代替可 |
| `/issues-exec-plan` 拡張 | 低 | 既存コマンドの改善。必須ではない |

### コマンド間の関係と統合案

上記7提案は最終的に `/orchestrate` に段階的に統合できる：

```
/orchestrate 24 25 --full
    │
    ├── Phase 1-3: 準備・開発・品質チェック（/parallel-quality-check を内包）
    ├── Phase 4-5: PR・マージ（/pr-merge-pipeline を内包）
    ├── Phase 6-7: UAT・修正ループ（/uat + /uat-fix-loop を内包）
    └── Phase 8:   完了
```

ただし個別コマンドとしても使えるようにしておく（途中から再開するケースに対応）：

```
# 開発は完了済み、PRからやり直したい
/pr-merge-pipeline 24 25

# マージ済み、UATだけやり直したい
/uat 24 25

# UAT失敗、修正ループだけ回したい
/uat-fix-loop 24
```
