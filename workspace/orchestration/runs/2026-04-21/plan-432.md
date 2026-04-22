# Orchestration Plan — 2026-04-21（#432）

## 対象 Issue

| Issue | Title | 種別 | Priority |
|---|---|---|---|
| #432 | ux: terminal resize handling (SIGWINCH) for TUI layout | FEATURE | - |

## 依存関係分析

### 影響ファイル
- `Cargo.toml`（signal_hook 追加）
- `src/agent/loop_run/footer.rs`（#430 の footer — SIGWINCH→再描画）
- `src/agent/loop_run/turn.rs`（プログレス行幅の動的計算）

### 前提依存
- #430（固定フッター）: develop に MERGED 済み ✓

### 依存関係判定: **単独**
- 単一Issueのため依存関係なし

## 種別分類

- **FEATURE_ISSUES**: #432 → `/pm-auto-issue2dev` で処理

## Worktree識別子

| Issue | Branch | Path | CommandMate ID |
|---|---|---|---|
| #432 | feature/issue-432-sigwinch-resize | ../Anvil-feature-issue-432-sigwinch-resize | anvil-feature-issue-432-sigwinch-resize |
