# Orchestration Plan — 2026-04-11 (Issue #345)

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #345 | Phase2: fix_slice worker contract is brittle | BUG |

## 実行計画

1. Phase 2: Worktree準備（feature/issue-345-fix-slice-worker-contract）
2. Phase 2.5: スキップ（Issue本文に詳細分析済み）
3. Phase 3: /bug-fix 345
4. Phase 5: 品質確認
5. Phase 6: PR作成・developマージ
6. Phase 8: 完了報告

## 修正方針（Issue 本文より）

### 即時対策
1. worker-required 条件での escalation bypass（A1型）を failure class として明示
2. `no_proposal` をさらに分解（parse failure / path confusion / empty final / tool-only completion）
3. Phase 2 条件で fix_slice escalation 後の親 file.edit 抑制（worker path 優先）

### 恒久対策
1. escalation を advisory のままにせず worker-required モードで bounded rewrite を強く誘導
2. ANVIL_FINAL 一発厳格 JSON 依存を緩和し proposal salvage 余地を持たせる
3. FIXSLICE_MAX_ITERATIONS=3 の固定値を phase/task 複雑度で調整可能に
4. state を明確化: escalated_but_not_invoked / invoked_but_no_proposal / proposal_parsed_but_invalid / rewrite_failed

### 回帰テスト
- escalation 発火後の worker 未起動 A1 型が明確な failure class で落ちる
- path confusion からの proposal salvage 動作確認
- iteration budget 増加時の worker_observed=true 達成率
