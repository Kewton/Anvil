# オーケストレーション計画: Issue #379

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #379 | [Feature] 操作性改善・TUI改善 | FEATURE (enhancement) |

## 依存関係

- 依存なし（単独Issue）
- 領域: TUI/CLI改善（`src/tui/`, `src/app/cli.rs`, `src/spinner.rs`, `src/app/render.rs`）
- 最近の終了フロー refactor（#385/#382/#383/#381/#380）とは独立

## 設計事前決定事項

実装着手前に以下を確定（Issue本文参照）:
- D1: ターミナル制御クレート選定（crossterm/termion/ratatui/rustyline）
- D2: 状態管理アーキテクチャ
- D3: ストリーミングコールバック設計
- D4: 既存 --exec/--oneshot モードとの両立
- D5: 段階的実装 vs. 一括実装（P1のみで分割する選択肢）

## 実行計画

1. Worktree作成: `feature/issue-379-tui-improvements`
2. `/pm-auto-issue2dev 379` で開発実行
   - Phase 1: マルチステージIssueレビュー
   - Phase 2: 設計方針書作成（D1-D5の判断含む）
   - Phase 3: マルチステージ設計レビュー
   - Phase 4: 作業計画立案（D5判断後の分割含む可能性）
   - Phase 5: TDD自動開発
3. 品質チェック・コミット
4. PR作成・developマージ

## マージ順序

1. #379（単独）
