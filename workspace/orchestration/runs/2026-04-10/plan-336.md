# Orchestration Plan — 2026-04-10 (Issue #336)

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #336 | Phase2: late-stage closure loop keeps appending last plan item after Issue334 | BUG |

## 実行計画

1. Phase 2: Worktree準備（feature/issue-336-late-stage-closure-guard）
2. Phase 2.5: スキップ（Issue本文に詳細分析済み）
3. Phase 3: /bug-fix 336
4. Phase 5: 品質確認
5. Phase 6: PR作成・developマージ
6. Phase 8: 完了報告

## 修正方針
1. remaining==1 の structural closure guard を追加（unchecked plan expansion 禁止）
2. touched_files ベースの late-append item reconcile
3. Superseded-skip dedup を closure mode では narrow 化
4. 回帰テスト: late-stage closure loop パターンのカバー
