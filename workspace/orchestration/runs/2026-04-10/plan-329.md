# Orchestration Plan — 2026-04-10 (Issue #329)

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #329 | bug: current Phase2 short smoke pack can close complete_unverified with zero mutation, so worker-on gate no longer exercises worker path | BUG |

## 依存関係

単一Issueのため依存関係なし。

## 影響ファイル

- `commandindextest/` (prompts, pack manifests, harness)
- `anvil_test/benchmark_sources/` (frozen source revision)
- `src/contracts/mod.rs` (result fields: worker_observed, mutation_observed等)

## 実行計画

1. Phase 2: Worktree準備（feature/issue-329-pack-validation-gate）
2. Phase 2.5: スキップ（Issue本文に詳細分析済み）
3. Phase 3: /bug-fix 329 をワーカーに送信
4. Phase 5: 品質確認
5. Phase 6: PR作成・developマージ
6. Phase 8: 完了報告
