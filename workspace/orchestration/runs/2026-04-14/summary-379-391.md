# オーケストレーション完了報告: Issue #379 + #391

## 対象Issue

| Issue | タイトル | ステータス | PR |
|-------|---------|-----------|-----|
| #391 | local-model drift: repeated ANVIL_PLAN restatement before first tool call | 完了 | #392 |
| #379 | [Feature] 操作性改善・TUI改善 (PR #1: Phase 0 + P1) | 完了 | #393 |

## 実行フェーズ結果

| Phase | 内容 | ステータス |
|-------|------|-----------|
| 1 | 依存関係分析 | 完了 |
| 2 | Worktree準備 | 完了 |
| 3 | 並列開発 | 完了（#379 は緊急停止後再開、#391 は新規） |
| 5 | 品質確認 | 完了（全Pass） |
| 6 | PR・マージ | 完了（PR #392, #393） |
| 8 | 完了報告 | 完了 |

## 品質チェック（develop最終）

| チェック項目 | 結果 |
|-------------|------|
| cargo build | Pass |
| cargo clippy --all-targets | Pass (0 warnings) |
| cargo test | **2384 tests passed, 0 failed** |
| cargo fmt --check | Pass |

## 変更内容

### #391: plan stall detection
- **新規**: `src/app/plan_stall_tracker.rs` (134行) — ANVIL_PLAN 繰り返し検出
- **新規**: `tests/plan_stall_detection.rs` (235行)
- 変更: `src/app/agentic.rs` (+84行), `src/agent/mod.rs` (PROMPT_TOOL_RULES強化), `src/app/mutation_barrier.rs` (CB-001修正)

### #379: TUI improvements (PR #1)
- **新規**: `src/tui/keyboard.rs` — crossterm-based 非ブロッキングキーボード監視
- **新規**: `src/tui/stream.rs` — mpsc-based ストリーミングイベントチャネル
- **新規**: `tests/tui_keyboard_watcher.rs`, `tests/tui_render_stream.rs`
- 変更: `src/app/cli.rs`, `src/contracts/mod.rs`, `src/session/mod.rs`, `src/tui/mod.rs`, `tests/tui_console.rs`, `Cargo.toml` (crossterm追加)
- **スコープ**: Phase 0 (mpsc基盤) + Phase 1 (P1 ESC interrupt) のみ。P2-P6 は follow-up PR

## CI結果

| PR | Build | Clippy | Format | Test |
|----|-------|--------|--------|------|
| #392 (#391) | Pass 53s | Pass 33s | Pass 7s | Pass 1m18s |
| #393 (#379) | Pass 2m16s | Pass 1m3s | Pass 9s | Pass 2m14s |

## タイムライン

- #391 PR: https://github.com/Kewton/Anvil/pull/392 → merged 2026-04-14T10:17:30Z
- #379 PR: https://github.com/Kewton/Anvil/pull/393 → merged 2026-04-14T10:26:11Z

## 備考

- **#379**: 当初の設計事前決定事項 D5（段階的実装）を採用、PR #1 で Phase 0 + P1 のみ実装。P2-P6 は follow-up PR で対応予定
- **#379 緊急停止**: 開発途中で緊急停止 → #391 並行開始 → #379 再開 → 両並列で完了
- **マージ順序**: #391 → #379（#379は #391 を develop から取り込んでマージ）
