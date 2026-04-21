# Orchestration Plan — 2026-04-12 (Issue #292)

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #292 | feat: add model-aware slice selection and delegation policy for local models | FEATURE |

## 実行計画

1. Phase 2: Worktree準備（feature/issue-292-model-aware-delegation）
2. Phase 2.5: スキップ（機能Issue）
3. Phase 3: /pm-auto-issue2dev 292
4. Phase 5: 品質確認
5. Phase 6: PR作成・developマージ
6. Phase 8: 完了報告

## 要件（Issue本文より）

1. delegation の発火条件が明示されること
2. model size だけでなく stagnation / repeated read / no progress も考慮
3. delegation 成否の telemetry を記録
4. 親が worker に渡す scope は bounded
5. large model path を不必要に劣化させない

## 受入条件

- model-aware delegation policy が追加される
- medium/small model path で slice-first の挙動を取れる
- delegation telemetry が追加される
- regression / policy test が追加される

## 関連ファイル
- `src/agent/model_classifier.rs` — モデル分類・ToolProtocolMode判定
- `src/app/agentic.rs` — 親エージェントルーティング
- `src/agent/subagent.rs` — fix_slice subagent
- `src/contracts/mod.rs` — AgentTelemetry
