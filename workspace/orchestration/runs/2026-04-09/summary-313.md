# オーケストレーション完了報告 — Issue #313

## 対象Issue

| Issue | タイトル | ステータス |
|-------|---------|-----------|
| #313 | bug: large-file の file.edit recovery が mid-file context を返せず shell.exec drift / partial に落ちる | 完了 |

## 実行フェーズ結果

| Phase | 内容 | ステータス |
|-------|------|-----------|
| 1 | 分析・計画 | 完了 |
| 2 | Worktree準備 | 完了（feature/issue-313-edit-recovery-mid-context） |
| 2.5 | Opus根本原因分析 | 完了 |
| 3 | 開発（/bug-fix） | 完了（1コミット、3ファイル変更） |
| 4 | 設計突合 | スキップ（単一Issue） |
| 5 | 品質確認 | 完了（全Pass） |
| 6 | PR・マージ | 完了（PR #314） |

## 品質チェック

| チェック項目 | 結果 |
|-------------|------|
| cargo build | Pass |
| cargo clippy --all-targets | Pass |
| cargo test | Pass |
| cargo fmt --check | Pass（既存差分のみ） |

## 変更概要（+366, -13）

| ファイル | 変更内容 |
|---------|---------|
| `src/tooling/mod.rs` | `extract_edit_context()` にtoken-based fallback追加。`extract_tokens()`, `find_best_token_match()` |
| `src/app/agentic.rs` | Recovery hintにinline context参照追加、`file.read` offset guidance |
| `tests/edit_recovery_mid_context.rs` | 7回帰テスト |

## 成果物

- PR: https://github.com/Kewton/Anvil/pull/314
