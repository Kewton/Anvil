# Orchestration Plan — 2026-04-12 (Issue #364)

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #364 | Phase3: classifier threshold blocks delegation telemetry | BUG |

## 実行計画

1. Phase 2: Worktree準備（feature/issue-364-classifier-threshold）
2. Phase 2.5: スキップ（Issue本文に詳細分析済み）
3. Phase 3: /bug-fix 364
4. Phase 5: 品質確認
5. Phase 6: PR作成・developマージ
6. Phase 8: 完了報告

## 修正方針（Issue 本文より）

### 即時対策
1. model_classifier の size boundary 修正
   - Small: <= 10B, Medium: 10B-50B, Large: > 50B
   - qwen3.5:35b / gemma4:31b → Medium (delegation 対象)
   - qwen3.5:122b → Large (据え置き)
2. model_aware_delegation_produced_mutation の runtime call site 追加
3. 既存テストの threshold を新分類に合わせて更新

## 関連ファイル
- src/agent/model_classifier.rs — ModelSizeClass 分類
- src/app/agentic.rs — proactive delegation branch
- src/contracts/mod.rs — telemetry counters
- tests/model_aware_delegation.rs — regression tests
