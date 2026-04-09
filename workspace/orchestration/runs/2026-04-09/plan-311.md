# Orchestration Plan — Issue #311 (2026-04-09)

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #311 | bug: superseded-only terminal plan が final gate / completion_kind と exit semantics を分断し complete_unverified + exit 2 を生む | BUG |

## 依存関係

単一Issue。#309の修正済みHEAD (95a673e) 上で作業。

## 影響ファイル

- `src/contracts/mod.rs` — success predicate統一、supersede/dedup修正
- `src/app/mod.rs` — `has_tool_execution_failure()` 修正

## 実行計画

1. Phase 2: Worktree準備（feature/issue-311-superseded-plan-exit-fix）
2. Phase 2.5: Opus 4.6 根本原因分析（Issue本文に詳細分析あり）
3. Phase 3: /bug-fix 311 をワーカーに送信
4. Phase 5: 品質確認
5. Phase 6: PR作成・developマージ
6. Phase 8: 完了報告
