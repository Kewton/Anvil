---
model: sonnet
description: "Issue開発を完全自動化（TDD→テスト→リファクタリング→報告）"
---

# PM自動開発スキル

## 概要
Issue開発（TDD実装 → 受入テスト → リファクタリング → 進捗報告）を**完全自動化**するプロジェクトマネージャースキルです。

**アーキテクチャ**: サブエージェント方式を採用し、各フェーズを専門エージェントに委譲します。

## 使用方法
- `/pm-auto-dev [Issue番号]`
- `/pm-auto-dev [Issue番号] --max-iterations=5`

## 実行内容

あなたはプロジェクトマネージャーとして、Issue開発を統括します。

### パラメータ
- **issue_number**: 開発対象のIssue番号（必須）
- **max_iterations**: 最大イテレーション回数（デフォルト: 3）

### サブエージェントモデル指定

| エージェント | モデル | 理由 |
|-------------|--------|------|
| tdd-impl-agent | **opus** | コード生成にOpus必要 |
| acceptance-test-agent | **opus** | テスト品質にOpus必要 |
| refactoring-agent | **opus** | コード改善にOpus必要 |
| progress-report-agent | sonnet | テンプレート埋め込み程度 |

---

## 実行フェーズ

### Phase 0: 初期設定

TodoWriteツールで作業計画を作成：

```
- [ ] Phase 1: Issue情報収集
- [ ] Phase 2: TDD実装 (イテレーション 0/3)
- [ ] Phase 3: 受入テスト
- [ ] Phase 4: リファクタリング
- [ ] Phase 5: ドキュメント最新化
- [ ] Phase 6: 進捗報告
```

### Phase 1: Issue情報収集

```bash
gh issue view {issue_number} --json number,title,body,labels,assignees
```

ディレクトリ構造作成：
```bash
BRANCH=$(git branch --show-current)
ISSUE_NUM=$(echo "$BRANCH" | grep -oE '[0-9]+')
BASE_DIR="dev-reports/issue/${ISSUE_NUM}/pm-auto-dev/iteration-1"
mkdir -p "$BASE_DIR"
```

### Phase 2: TDD実装（イテレーション可能）

TDDコンテキストファイル作成後、サブエージェント呼び出し：
```
Use tdd-impl-agent (model: opus) to implement Issue #{issue_number} with TDD approach.
Context file: dev-reports/issue/{issue_number}/pm-auto-dev/iteration-1/tdd-context.json
Output file: dev-reports/issue/{issue_number}/pm-auto-dev/iteration-1/tdd-result.json
```

### Phase 3: 受入テスト

```
Use acceptance-test-agent (model: opus) to verify Issue #{issue_number}.
Context file: dev-reports/issue/{issue_number}/pm-auto-dev/iteration-1/acceptance-context.json
Output file: dev-reports/issue/{issue_number}/pm-auto-dev/iteration-1/acceptance-result.json
```

### Phase 4: リファクタリング

```
Use refactoring-agent (model: opus) to improve code quality for Issue #{issue_number}.
Context file: dev-reports/issue/{issue_number}/pm-auto-dev/iteration-1/refactor-context.json
Output file: dev-reports/issue/{issue_number}/pm-auto-dev/iteration-1/refactor-result.json
```

### Phase 5: ドキュメント最新化

- **README.md**: 機能一覧の更新
- **CLAUDE.md**: モジュール構成の更新

### Phase 6: 進捗報告

```
Use progress-report-agent to generate progress report for Issue #{issue_number}.
Context file: dev-reports/issue/{issue_number}/pm-auto-dev/iteration-1/progress-context.json
Output file: dev-reports/issue/{issue_number}/pm-auto-dev/iteration-1/progress-report.md
```

## ファイル構造

```
dev-reports/issue/{issue_number}/
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

## 完了条件

- Phase 2: TDD実装成功（全テストパス、clippy警告0件）
- Phase 3: 受入テスト成功
- Phase 4: リファクタリング完了
- Phase 5: ドキュメント最新化完了
- Phase 6: 進捗レポート作成完了
