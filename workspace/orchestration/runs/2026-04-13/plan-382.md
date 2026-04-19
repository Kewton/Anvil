# オーケストレーション計画: Issue #382

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #382 | task semantics gate: detect implementation-required tasks before accepting read-only completion | FEATURE |

## 依存関係

- 前提: #385（準備リファクタリング）→ マージ済み ✔
- 後続: なし

## 実行計画

1. Worktree作成: `feature/issue-382-task-semantics-gate`
2. `/pm-auto-issue2dev 382` で開発実行
3. 品質チェック（cargo fmt/clippy/test）
4. PR作成・developマージ

## マージ順序

1. #382（単独）
