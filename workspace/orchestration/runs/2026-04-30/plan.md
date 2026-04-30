# オーケストレーション実行計画

## 対象Issue

| Issue | タイトル | 種別 | 依存 |
|-------|---------|------|------|
| #471 | structured evaluation log を追加する | FEATURE | なし |
| #472 | local model A/B evaluation harness を作る | FEATURE | なし |
| #473 | fine-tuning dataset export を追加する | FEATURE | #471（弱依存）|

## 依存関係グラフ

```
471 ─────────────────────→ merge
472 ─────────────────────→ merge
473 ──(設計で471参照)────→ merge
```

## 並列実行グループ

- **グループA（完全並列）**: #471, #472, #473
  - 473は471の設計書を Phase 4 設計突合で参照し整合性を確認

## マージ推奨順序

1. #471（基盤ログ）
2. #472（独立ハーネス）
3. #473（471に依存するexport）

## ブランチ名

- feature/issue-471-structured-eval-log
- feature/issue-472-ab-eval-harness
- feature/issue-473-dataset-export
