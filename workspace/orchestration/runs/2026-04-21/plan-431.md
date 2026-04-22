# Orchestration Plan — 2026-04-21（#431）

## 対象 Issue

| Issue | Title | 種別 | Priority |
|---|---|---|---|
| #431 | ux: markdown rendering for assistant response output | FEATURE | - |

## 依存関係分析

### 影響ファイル
- `src/agent/loop_run/turn.rs`（ストリーミング出力部分にmarkdownレンダリング統合）
- 新規: `src/tui/markdown.rs`（軽量markdownパーサー実装 or termimad クレート利用）

### 前提依存
- なし（単独Issue）

### 依存関係判定: **単独**
- 単一Issueのため依存関係なし

## 種別分類

- **FEATURE_ISSUES**: #431 → `/pm-auto-issue2dev` で処理

## Worktree識別子

| Issue | Branch | Path | CommandMate ID |
|---|---|---|---|
| #431 | feature/issue-431-markdown-render | ../Anvil-feature-issue-431-markdown-render | anvil-feature-issue-431-markdown-render |
