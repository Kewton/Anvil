# Orchestration Plan — 2026-04-11 (Issue #343)

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #343 | Phase2: fix_slice failure degrades into repair salvage | BUG |

## 実行計画

1. Phase 2: Worktree準備（feature/issue-343-fixslice-failure-control）
2. Phase 2.5: スキップ（Issue本文に詳細分析済み）
3. Phase 3: /bug-fix 343
4. Phase 5: 品質確認
5. Phase 6: PR作成・developマージ
6. Phase 8: 完了報告

## 修正方針（Issue 本文より）

### 即時対策
1. `handle_fixslice_result()` の失敗理由を telemetry に分離
   - `no_proposal` / `proposal_validation_failed` / `rewrite_failed` / `max_iterations_reached`
2. worker-required 実行中に parent-side mutation が先に出た場合、早期 worker failure 確定
3. `repair_turn_observed=true && worker_observed=false` に明示 failure reason

### 恒久対策
1. worker-required runtime mode で fix_slice failure 後の parent file.edit を worker 成功代替にしない
2. fix_slice 失敗後の control flow 見直し、repair turn への無制限流出を制限
3. worker invocation / worker failure / repair salvage を別軸で可視化

### 回帰テスト
- worker が proposal を返せない fixture で worker_observed=false + 早期 failure
- repair_turn_observed=true && mutation_observed=true && worker_observed=false の salvage 一貫記録
- worker-required 条件で completion_kind=partial を成功扱いしない
