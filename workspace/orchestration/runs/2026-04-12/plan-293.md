# Orchestration Plan — 2026-04-12 (Issue #293)

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #293 | change: make sidecar compaction advisory for local-model execution paths | FEATURE |

## 実行計画

1. Phase 2: Worktree準備（feature/issue-293-advisory-sidecar-compaction）
2. Phase 2.5: スキップ（機能Issue）
3. Phase 3: /pm-auto-issue2dev 293
4. Phase 5: 品質確認
5. Phase 6: PR作成・developマージ
6. Phase 8: 完了報告

## 受入条件

- structured state が authoritative であることがコード上で明確になる
- sidecar summary なしでも主要 session flow が維持される
- compaction quality / post-compact recovery を測る telemetry または test が追加される

## 関連ファイル
- src/session/mod.rs — セッション永続化・LLM要約コンパクション
- src/provider/ollama.rs — sidecar_summarize
- src/app/agentic.rs — agenticループ・コンパクション呼び出し
- src/contracts/mod.rs — telemetry
