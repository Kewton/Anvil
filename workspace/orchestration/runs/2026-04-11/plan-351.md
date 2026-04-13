# Orchestration Plan — 2026-04-11 (Issue #351)

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #351 | Phase2: add subagent and tool-thrash detectors | BUG |

## 実行計画

1. Phase 2: Worktree準備（feature/issue-351-subagent-thrash-detectors）
2. Phase 2.5: スキップ（Issue本文に詳細分析済み）
3. Phase 3: /bug-fix 351
4. Phase 5: 品質確認
5. Phase 6: PR作成・developマージ
6. Phase 8: 完了報告

## 修正方針（Issue 本文より）

### 即時対策
1. **subagent no-progress detector** (src/agent/subagent.rs):
   - 同一 target path に対する file.read が N 回続いたら abort
   - 2 iteration 続けて proposal を出せず同系統の reasoning text を返したら abort
   - `fixslice_failure_reason` に `no_progress_loop` / `repeated_read_loop` を追加

2. **parent post-failure tool-active thrash detector** (src/app/agentic.rs):
   - worker_observed=false かつ fixslice_worker_failure_count>0 になった後、一定 turn の間に plan advancement (plan_update / changed_files / completed_final) が無ければ pack_gate_invalid で session_completed に切替
   - text-only 型は PR #350、tool-active 型は新 detector が担当

### 恒久対策
- subagent・parent 両方で「意味のある進捗」の統一メカニズム
- advancement signal の pack-aware 判定

### 回帰テスト
- subagent が同一 file.read を 3 回繰り返しても proposal を返さないケース → no_progress_loop
- worker 失敗後に parent が file.read / file.search を続けても plan が進まないケース → post_failure_thrash
- cycle 6 相当の正常成功が regression しないこと
- A2 系成功パスが regression しないこと
