# Orchestration Plan — 2026-04-11 (Issue #349)

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #349 | Phase2: closure-mode loops after fix_slice failure | BUG |

## 実行計画

1. Phase 2: Worktree準備（feature/issue-349-closure-loop-guard）
2. Phase 2.5: スキップ（Issue本文に詳細分析済み）
3. Phase 3: /bug-fix 349
4. Phase 5: 品質確認
5. Phase 6: PR作成・developマージ
6. Phase 8: 完了報告

## 修正方針（Issue 本文より）

### 即時対策
1. closure mode で `agent.fix_slice` 失敗後、tool call 間隔が一定秒数以上空いたら early invalid termination
2. 親 message の近似重複が 2 回以上続いたら loop detector 発火

### 恒久対策
1. worker-required flow で `fix_slice` 失敗後の parent 分岐を `retry_once_with_new_target` または `abort_as_invalid` に限定
2. `worker_observed=false` かつ late-stage closure かつ pattern 収束しない場合の explicit runtime gate 追加

### 複合条件での発火
- `worker_observed=false`
- `fix_slice` failure 済み
- 近似重複メッセージ連発

### 回帰テスト
1. closure mode で無限 reasoning が止まること
2. cycle 6 相当の有効 run が regression しないこと
3. cycle 8/9 型の loop で controlled invalid termination
