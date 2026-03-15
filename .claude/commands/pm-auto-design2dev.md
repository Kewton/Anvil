---
model: sonnet
description: "作業計画から実装完了まで完全自動化（作業計画→TDD実装）"
---

# PM自動 設計→開発スキル

## 概要
作業計画立案から実装完了までの全工程（作業計画立案 → TDD実装）を**完全自動化**するプロジェクトマネージャースキルです。ユーザーはIssue番号を指定するだけで、計画から開発完了まで自律的に実行します。

Issue内容が十分に整備されている場合に使用します。Issue補完が必要な場合は `/pm-auto-issue2dev` を使用してください。

**アーキテクチャ**: 2つの既存コマンドを順次実行し、各フェーズの成果物を次フェーズに引き継ぎます。

## 使用方法
- `/pm-auto-design2dev [Issue番号]`
- 「Issue #XXXを作業計画から開発まで自動実行してください」

## 実行内容

あなたはプロジェクトマネージャーとして、作業計画から開発までの全工程を統括します。以下のフェーズを順次実行し、各フェーズの完了を確認しながら進めてください。

### パラメータ

- **issue_number**: 開発対象のIssue番号（必須）

### サブエージェントモデル指定

各サブコマンド内で個別にモデル指定されています（TDD系=opus、報告系=sonnet継承）。

---

## 実行フェーズ

### Phase 0: 初期設定とTodoリスト作成

まず、TodoWriteツールで作業計画を作成してください：

```
- [ ] Phase 1: 作業計画立案
- [ ] Phase 2: TDD自動開発
- [ ] Phase 3: 完了報告
```

---

### Phase 1: 作業計画立案

#### 1-1. 作業計画作成

`/work-plan` コマンドを実行：

```
/work-plan {issue_number}
```

**このフェーズで行われること**:
- Issue内容に基づいたタスク分解
- 依存関係の整理
- 実装順序の決定

#### 1-2. 完了確認

- 作業計画書が生成されていること

**出力ファイル**: `dev-reports/issue/{issue_number}/work-plan.md`

---

### Phase 2: TDD自動開発

#### 2-1. TDD実装実行

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

#### 2-2. 完了確認

- `cargo build` エラー0件
- `cargo clippy --all-targets` 警告0件
- `cargo test` 全テストパス
- 進捗レポートが生成されていること

**出力ファイル**: `dev-reports/issue/{issue_number}/pm-auto-dev/iteration-1/progress-report.md`

---

### Phase 3: 完了報告

#### 3-1. 最終検証

```bash
cargo build
cargo clippy --all-targets
cargo test
cargo fmt --check
```

#### 3-2. 成果物サマリー

完了時に以下を報告：

```markdown
## PM Auto Design2Dev 完了報告

### Issue #{issue_number}

#### 実行フェーズ結果

| Phase | 内容 | ステータス |
|-------|------|-----------|
| 1 | 作業計画立案 | 完了 |
| 2 | TDD自動開発 | 完了 |

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

- Phase 1: 作業計画書が作成されている
- Phase 2: TDD自動開発完了（テスト全パス、clippy警告0件）
- Phase 3: 完了報告

---

## 使用例

```
User: /pm-auto-design2dev 2

PM Auto Design2Dev:

Phase 1/3: 作業計画立案
  - タスク分解: 6タスク
  - 依存関係: 整理済み
  - 作業計画完了

Phase 2/3: TDD自動開発
  - TDD実装: 完了
  - 受入テスト: 完了 (6/6 passed)
  - リファクタリング: 完了
  - TDD自動開発完了

Phase 3/3: 完了報告
  - cargo build: Pass
  - cargo clippy: Pass
  - cargo test: Pass (108/108)
  - cargo fmt: Pass

Issue #2 の作業計画から開発まで完了しました！

次のアクション:
- /create-pr でPR作成
```

---

## 関連コマンド

- `/work-plan`: 作業計画立案
- `/pm-auto-dev`: TDD自動開発
- `/create-pr`: PR作成
- `/pm-auto-issue2dev`: Issue補完から開発まで一括実行（Issue補完あり版）
