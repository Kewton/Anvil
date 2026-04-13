# Orchestration Plan — 2026-04-11 (Issue #353)

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #353 | Phase2: make no-progress detector tunable | BUG |

## 実行計画

1. Phase 2: Worktree準備（feature/issue-353-no-progress-detector-tunable）
2. Phase 2.5: スキップ（Issue本文に詳細分析済み）
3. Phase 3: /bug-fix 353
4. Phase 5: 品質確認
5. Phase 6: PR作成・developマージ
6. Phase 8: 完了報告

## 背景

PR #352 の subagent no-progress detector が cycle 11 で以下の regression を引き起こした:
- worker_observed: 1/4 (cycle 10) → 0/4 (cycle 11)
- cycle 6 baseline (detector 無し): 4/4 worker success
- 3 file.read の閾値が qwen3.5:122b の exploration pattern に厳しすぎる

## 修正方針（Issue 本文より）

### 即時対策
1. `REPEATED_READ_LOOP_THRESHOLD` を 3 → 5 に緩和
2. env var `ANVIL_FIXSLICE_NO_PROGRESS_DETECTOR` 追加（値: on/off/任意の数値）
3. config key `fixslice_no_progress_detector` 追加

### 恒久対策
- detector trigger を「同 path file.read 反復」だけでなく「反復 + reasoning advancement なし」の2条件に強化
- content-advancing が続いていれば abort しない

### 予防（回帰テスト）
- detector が 5 read までは抑制される
- env var で off にできる
- threshold override が機能する
