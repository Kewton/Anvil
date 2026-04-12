# Orchestration Plan — 2026-04-12 (Issue #355)

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #355 | Phase2: fix escalated_not_invoked and early-exit after worker success | BUG |

## 実行計画

1. Phase 2: Worktree準備（feature/issue-355-escalation-routing-and-early-exit）
2. Phase 2.5: スキップ（Issue本文に詳細分析済み）
3. Phase 3: /bug-fix 355
4. Phase 5: 品質確認
5. Phase 6: PR作成・developマージ
6. Phase 8: 完了報告

## 修正方針（Issue 本文より）

### 即時対策 (1) escalated_not_invoked fix
- parent で escalation flag が立った直後、他のヒューリスティクスを一時 suspend して必ず agent.fix_slice を呼ぶ gated routing を追加
- 呼べない場合は fixslice_failure_reason に明示的な分類追加 (escalation_suppressed_by_<reason>)

### 即時対策 (2) post-success early-exit
- session ループで worker_observed=true AND pack_validation_result=satisfied を検知した直後、session_completed で break
- worker-required pack でない場合は適用しない

### 恒久対策
- pack expectation と session termination の結合を明示化（pack.termination_on_satisfied = true 等）
- escalation routing を pack-aware にする

### 回帰テスト
- escalation flagged and worker invoked under worker-required pack
- session terminates cleanly after worker success in worker-required pack
- elapsed_seconds <= pack_satisfied_turn_ms + 60s で post-success continuation を観測
- cycle 6 相当の正常 run が regression しない
