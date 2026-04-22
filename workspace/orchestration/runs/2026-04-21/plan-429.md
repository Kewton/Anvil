# Orchestration Plan — 2026-04-21（#429）

## 対象 Issue

| Issue | Title | 種別 | Priority |
|---|---|---|---|
| #429 | ux: ESC key interrupt to stop agent mid-run | FEATURE | - |

## 依存関係分析

### 影響ファイル
- `src/agent/loop_run/turn.rs`（割り込みチェック箇所）
- `src/agent/loop_run/commands.rs`（run loop）
- `Cargo.toml`（crossterm は既存）

### 依存関係判定: **単独**
- 単一Issueのため依存関係なし

## 種別分類

- **FEATURE_ISSUES**: #429 → `/pm-auto-issue2dev` で処理

## Worktree識別子

| Issue | Branch | Path | CommandMate ID |
|---|---|---|---|
| #429 | feature/issue-429-esc-interrupt | ../Anvil-feature-issue-429-esc-interrupt | anvil-feature-issue-429-esc-interrupt |
