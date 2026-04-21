# オーケストレーション計画: Issue #391

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #391 | local-model drift: repeated ANVIL_PLAN restatement before first tool call stalls implementation | FEATURE |

## 依存関係

- 依存なし（単独Issue）
- 関連: #382 (task-semantics gate), #383 (no-tool-call), #380 (mode split)
- #379 (TUI改善) とは独立（並行作業可）

## スコープ

- PROMPT_TOOL_RULES 強化: ANVIL_PLAN 後の ANVIL_TOOL 即時発火要求
- task-semantics retry guidance 強化（説明禁止、file.write/file.edit 要求）
- 繰り返し ANVIL_PLAN / setup prose の検出
- 長時間 plan-only stream の stalled-turn guard

## 実行計画

1. Worktree作成: `feature/issue-391-anvil-plan-stall`
2. `/pm-auto-issue2dev 391` で開発実行
3. 品質チェック・コミット
4. PR作成・developマージ
