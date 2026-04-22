# Orchestration Plan — 2026-04-21（#430）

## 対象 Issue

| Issue | Title | 種別 | Priority |
|---|---|---|---|
| #430 | ux: fixed footer with status bar (mode / token usage / log level) | FEATURE | - |

## 依存関係分析

### 影響ファイル
- `src/agent/loop_run/commands.rs`（フッター初期化・更新）
- `src/agent/loop_run/turn.rs`（ターン毎のフッター更新）
- 新規: `src/tui/footer.rs`（DECSTBM + ANSI フッター実装）

### 依存関係判定: **単独**
- 単一Issueのため依存関係なし

## 種別分類

- **FEATURE_ISSUES**: #430 → `/pm-auto-issue2dev` で処理

## Worktree識別子

| Issue | Branch | Path | CommandMate ID |
|---|---|---|---|
| #430 | feature/issue-430-fixed-footer | ../Anvil-feature-issue-430-fixed-footer | anvil-feature-issue-430-fixed-footer |
