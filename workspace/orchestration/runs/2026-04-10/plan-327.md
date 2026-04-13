# Orchestration Plan — 2026-04-10 (Issue #327)

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #327 | bug: pre-exit repair turn 応答が新規 pending plan を append しても loop が即 terminate し Phase2 B1 が partial で閉じる | BUG |

## 依存関係

単一Issueのため依存関係なし。

## 影響ファイル

- `src/app/agentic.rs` (consumed-repair break → post-repair decision)
- `src/app/mod.rs` (テレメトリフィールド追加)
- `src/app/execution_plan.rs` (repair mode での unchecked items ポリシー)
- `src/contracts/mod.rs` (テレメトリ型定義)

## 実行計画

1. Phase 2: Worktree準備（feature/issue-327-repair-turn-closure）
2. Phase 2.5: スキップ（Issue本文に詳細な根本原因分析済み）
3. Phase 3: /bug-fix 327 をワーカーに送信
4. Phase 5: 品質確認（cargo fmt/clippy/test）
5. Phase 6: PR作成・developマージ
6. Phase 8: 完了報告
