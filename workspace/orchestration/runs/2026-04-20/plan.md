# Orchestration Plan — 2026-04-20

## 対象 Issue

| Issue | Title | 種別 | Labels |
|---|---|---|---|
| #403 | harness: bench.sh script for smoke / 5-run / matrix execution | **FEATURE** | (none) |

単独 issue のため並列性なし。`--phase` 未指定のため PR マージまで実行。

## 種別分類

- **BUG_ISSUES**: （なし、Phase 2.5 スキップ）
- **FEATURE_ISSUES**: #403 → `/pm-auto-issue2dev` で処理

## 依存関係

- 前提依存なし（シェルスクリプト追加のみ）
- 新規ファイル: `scripts/bench.sh`, `benchmarks/heavy-space-invaders.yaml` 等を想定
- 既存ソース（`src/`）には手を入れず、`.anvil/benchmarks/` 配下に成果物保存

## マージ推奨順序

1. #403 単独

## 各 Phase の扱い

| Phase | 扱い |
|---|---|
| 1 | plan.md 作成 |
| 2 | `feature/issue-403-bench-script` worktree 作成、CommandMate 登録 |
| 2.5 | **スキップ**（bug ラベルなし） |
| 3 | `/pm-auto-issue2dev 403` 送信 |
| 4 | **簡易化**（単独 issue、設計書確認のみ） |
| 5 | fmt/clippy/test は source 変更が無ければ影響なし。bench.sh の `--help` 動作確認 |
| 6 | `/pr-merge-pipeline 403` 相当で squash-merge |
| 7 | **スキップ**（`--full` 未指定） |
| 8 | `summary.md` 出力 |

## Worktree 識別子

- Branch: `feature/issue-403-bench-script`
- Path: `../Anvil-feature-issue-403-bench-script`
- CommandMate ID: `anvil-feature-issue-403-bench-script`

## Acceptance 要件（issue 本文より）

- [ ] `scripts/bench.sh --help` で用例表示
- [ ] 既存 5-run 同様の出力が得られる
- [ ] matrix 実行で複数 model 結果が 1 回で取れる
