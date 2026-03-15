---
model: sonnet
description: "Issue補完から実装完了まで完全自動化（Issue補完→作業計画→TDD実装）"
---

# PM自動 Issue→開発スキル

## 概要
Issue補完から実装完了までの全工程（Issue補完 → 作業計画立案 → TDD実装）を**完全自動化**するプロジェクトマネージャースキルです。ユーザーはIssue番号を指定するだけで、Issueの品質向上から開発完了まで自律的に実行します。

**アーキテクチャ**: 3つの既存コマンドを順次実行し、各フェーズの成果物を次フェーズに引き継ぎます。

## 使用方法
- `/pm-auto-issue2dev [Issue番号]`
- 「Issue #XXXをIssue補完から開発まで自動実行してください」

## 実行内容

あなたはプロジェクトマネージャーとして、Issue補完から開発までの全工程を統括します。以下のフェーズを順次実行し、各フェーズの完了を確認しながら進めてください。

### パラメータ

- **issue_number**: 開発対象のIssue番号（必須）

### サブエージェントモデル指定

各サブコマンド内で個別にモデル指定されています（Issue補完=opus、TDD系=opus、報告系=sonnet継承）。

---

## 実行フェーズ

### Phase 0: 初期設定とTodoリスト作成

まず、TodoWriteツールで作業計画を作成してください：

```
- [ ] Phase 1: Issue補完
- [ ] Phase 2: 作業計画立案
- [ ] Phase 3: TDD自動開発
- [ ] Phase 4: 完了報告
```

---

### Phase 1: Issue補完

#### 1-1. Issue補完実行

`/issue-enhance` コマンドを実行：

```
/issue-enhance {issue_number}
```

**このフェーズで行われること**:
- Issue種別の判定
- コードベース調査
- ユーザーへの質問（不足情報の収集）
- Issue本文の補完・更新

#### 1-2. 完了確認

- GitHubのIssueが更新されていること
- 必須セクションが充足していること

---

### Phase 2: 作業計画立案

#### 2-1. 作業計画作成

`/work-plan` コマンドを実行：

```
/work-plan {issue_number}
```

**このフェーズで行われること**:
- Issue内容に基づいたタスク分解
- 依存関係の整理
- 実装順序の決定

#### 2-2. 完了確認

- 作業計画書が生成されていること

**出力ファイル**: `dev-reports/issue/{issue_number}/work-plan.md`

---

### Phase 3: TDD自動開発

#### 3-1. TDD実装実行

`/pm-auto-dev` コマンドを実行：

```
/pm-auto-dev {issue_number}
```

**このフェーズで行われること**:
- TDD実装（Red-Green-Refactor）
- 受入テスト
- リファクタリング
- ドキュメント更新
- 進捗報告

#### 3-2. 完了確認

- `cargo build` エラー0件
- `cargo clippy --all-targets` 警告0件
- `cargo test` 全テストパス
- 進捗レポートが生成されていること

**出力ファイル**: `dev-reports/issue/{issue_number}/pm-auto-dev/iteration-1/progress-report.md`

---

### Phase 4: 完了報告

#### 4-1. 最終検証

```bash
cargo build
cargo clippy --all-targets
cargo test
cargo fmt --check
```

#### 4-2. 成果物サマリー

完了時に以下を報告：

```markdown
## PM Auto Issue2Dev 完了報告

### Issue #{issue_number}

#### 実行フェーズ結果

| Phase | 内容 | ステータス |
|-------|------|-----------|
| 1 | Issue補完 | 完了 |
| 2 | 作業計画立案 | 完了 |
| 3 | TDD自動開発 | 完了 |

#### 品質チェック

| チェック項目 | コマンド | 結果 |
|-------------|----------|------|
| ビルド | cargo build | Pass |
| Clippy | cargo clippy --all-targets | Pass |
| テスト | cargo test | Pass |
| フォーマット | cargo fmt --check | Pass |

#### 生成ファイル

- 作業計画: `dev-reports/issue/{issue_number}/work-plan.md`
- 進捗報告: `dev-reports/issue/{issue_number}/pm-auto-dev/iteration-1/progress-report.md`

#### 次のアクション

- [ ] コミット確認
- [ ] PR作成（`/create-pr`）
```

---

## ファイル構造

```
dev-reports/
└── issue/{issue_number}/
    ├── work-plan.md
    └── pm-auto-dev/
        └── iteration-1/
            ├── tdd-context.json
            ├── tdd-result.json
            ├── acceptance-context.json
            ├── acceptance-result.json
            ├── refactor-context.json
            ├── refactor-result.json
            ├── progress-context.json
            └── progress-report.md
```

---

## 完了条件

以下をすべて満たすこと：

- Phase 1: Issue補完完了（Issue本文が更新されている）
- Phase 2: 作業計画書が作成されている
- Phase 3: TDD自動開発完了（テスト全パス、clippy警告0件）
- Phase 4: 完了報告

---

## 使用例

```
User: /pm-auto-issue2dev 1

PM Auto Issue2Dev:

Phase 1/4: Issue補完
  - Issue種別: fix (バグ修正)
  - コードベース調査: 完了
  - Issue本文更新: 完了

Phase 2/4: 作業計画立案
  - タスク分解: 4タスク
  - 依存関係: 整理済み
  - 作業計画完了

Phase 3/4: TDD自動開発
  - TDD実装: 完了
  - 受入テスト: 完了 (4/4 passed)
  - リファクタリング: 完了
  - TDD自動開発完了

Phase 4/4: 完了報告
  - cargo build: Pass
  - cargo clippy: Pass
  - cargo test: Pass (108/108)
  - cargo fmt: Pass

Issue #1 のIssue補完から開発まで完了しました！

次のアクション:
- /create-pr でPR作成
```

---

## 関連コマンド

- `/issue-enhance`: Issue補完
- `/work-plan`: 作業計画立案
- `/pm-auto-dev`: TDD自動開発
- `/create-pr`: PR作成
- `/pm-auto-design2dev`: 作業計画から実装完了まで（Issue補完なし版）
