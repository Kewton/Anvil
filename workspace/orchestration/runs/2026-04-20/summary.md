# オーケストレーション完了報告 — 2026-04-20

## 対象 Issue

| Issue | タイトル | ステータス |
|---|---|---|
| #403 | harness: bench.sh script for smoke / 5-run / matrix execution | 完了（PR #416 マージ済み） |

## 実行フェーズ結果

| Phase | 内容 | ステータス |
|---|---|---|
| 1 | 依存関係分析・実行計画 | 完了（単独 issue） |
| 2 | Worktree 準備 | 完了（`feature/issue-403-bench-script`） |
| 2.5 | 根本原因分析 | スキップ（bug ラベルなし） |
| 3 | 並列開発 | 完了（`/pm-auto-issue2dev 403`） |
| 4 | 設計突合 | 完了（単独 issue のため簡易化） |
| 5 | 品質確認 | 完了（fmt / clippy / test / shellcheck 全 Pass） |
| 6 | PR 作成・マージ | 完了（PR #416 squash-merge = `f0e3abf`） |
| 7 | UAT | スキップ（`--full` 未指定） |
| 8 | 完了報告 | 本文書 |

## コミット

| SHA | Subject |
|---|---|
| f0e3abf | feat(issue-403): add bench.sh harness for smoke/5-run/matrix benchmarks (#416) |

スカッシュ前のブランチコミット: `04d0dda feat(issue-403): add bench.sh harness ...`

## 成果物

- `scripts/bench.sh` — bash 3.2+ 互換、`--model` / `--models` / `--runs` / `--dry-run` / `--help`
- `benchmarks/heavy-space-invaders.yaml` — プロンプト + args の外部 YAML
- `.github/workflows/ci.yml` — shellcheck ジョブ追加（apt-get 明示インストール）
- dev-reports: Issue レビュー 8 stage / 設計レビュー 4 stage / pm-auto-dev iteration-1（TDD / Codex review / acceptance / refactor / progress）

## 品質チェック（develop HEAD = f0e3abf）

| チェック | 結果 |
|---|---|
| cargo fmt --check | Pass |
| cargo clippy --all-targets | Pass（警告 0） |
| cargo test --all | Pass |
| shellcheck -S warning scripts/bench.sh | Pass |
| CI (ubuntu-latest) | Format / Clippy / Test / Build / ShellCheck 全 Pass |
| `scripts/bench.sh --help` | Pass（exit 0、first-write mention 含む） |

## Acceptance 要件（Issue #403 本文基準）

| # | 内容 | 結果 |
|---|---|---|
| 1 | `scripts/bench.sh --help` で用例表示 | Pass |
| 2 | 既存 5-run 同様の出力（summary.tsv + run-N.stdout.log + session snapshot） | Pass（`.anvil/benchmarks/{timestamp-PID}/` 配下に保存） |
| 3 | matrix 実行で複数 model 結果が 1 回で取れる | Pass（`--models 'a,b' --runs N` で逐次実行） |

## Codex レビュー修正（PR に同梱）

- CB-001: `ANVIL_BIN` env 優先保持（`_anvil_bin_env`）
- CB-002: `BENCH_ROOT` を `timestamp-$$` で一意化
- CB-003: `validate_model` に制御文字拒否、`slugify` で `[A-Za-z0-9._-]` 正規化
- CB-004: CI で shellcheck を apt-get で明示インストール

## 備考・運用メモ

1. **worker 起動時に Sonnet 4.6 を preset**: Issue #402 の対応で学んだ 1M context 課金エラーを回避するため、`/model sonnet` を事前送信 → `/pm-auto-issue2dev 403` 送信のシーケンスで起動（問題なく完走）。
2. **AC-001 の軽微修正ループ**: `--help` 出力に `first-write` トークンを含めるよう 1 往復追加指示（acceptance-test-agent の提案通り実装）。
3. **worker の未コミット期間**: 実装完了後も自発的にコミットせず idle になる現行パターンは #402 と同じ。commit 指示の 1 ラウンドで即対応。
4. **PR #416 は 1 発 CI 通過**: develop 側の `model_registry.rs` 既存バグは #402 で解消済み。
