# Orchestration Plan - 2026-04-04

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #261 | design: plan-aware control の進捗/完了意味論を修正し batch-aware guidance へ移行 | FEATURE |

## 依存関係

単一Issueのため依存関係なし。

## 実行計画

1. Worktree準備: `feature/issue-261-plan-aware-batch-guidance`
2. ワーカーに `/pm-auto-issue2dev 261` を送信
3. 品質確認（cargo fmt/clippy/test）
4. PR作成・developマージ

## 影響ファイル

- `src/app/agentic.rs`
- `src/app/execution_plan.rs`
- `src/contracts/mod.rs`
- `src/agent/mod.rs`
- `src/app/phase_estimator.rs`
- `tests/agent_phase.rs`
- `tests/provider_integration.rs`
