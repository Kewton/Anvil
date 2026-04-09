# Orchestration Plan — 2026-04-09

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #309 | bug: active unfinished plan 中の shell.exec inspection drift が guidance をすり抜け partial + tool execution failed に落ちる | BUG |

## 依存関係

単一Issueのため依存関係なし。

## 影響ファイル

- `src/app/phase_estimator.rs`
- `src/app/read_transition_guard.rs`
- `src/tooling/shell_policy.rs`
- `src/app/agentic.rs`
- `src/app/mod.rs`

## 実行計画

1. Phase 2: Worktree準備（feature/issue-309-shell-inspection-drift）
2. Phase 2.5: Opus 4.6 根本原因分析（Issue本文に詳細分析あり、追加分析実施）
3. Phase 3: /bug-fix 309 をワーカーに送信
4. Phase 5: 品質確認（cargo fmt/clippy/test）
5. Phase 6: PR作成・developマージ
6. Phase 8: 完了報告

## マージ推奨順序

単一Issueのため順序制約なし。
