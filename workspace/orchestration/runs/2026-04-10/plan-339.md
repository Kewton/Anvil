# Orchestration Plan — 2026-04-10 (Issue #339)

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #339 | Phase2: worker_observed is set by escalation before any mutation evidence exists | BUG |

## 実行計画

1. Phase 2: Worktree準備（feature/issue-339-worker-observed-success-side）
2. Phase 2.5: スキップ（Issue本文に詳細分析済み）
3. Phase 3: /bug-fix 339
4. Phase 5: 品質確認
5. Phase 6: PR作成・developマージ
6. Phase 8: 完了報告

## 修正方針
1. request-side (escalation emitted) と success-side (worker executed + mutation applied) telemetry を分離
2. worker_observed を **real post-execution worker event** でのみ set するように redefine
3. RequiresWorkerObservation validation を success-side field にひも付ける
4. 回帰テスト追加:
   - escalation emitted but no mutation → worker-required pack validation = mismatch
   - worker really executes and mutation succeeds → satisfied
