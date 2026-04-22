# Orchestration Plan — 2026-04-19

## 対象 Issue

| Issue | Title | 種別 | Labels |
|---|---|---|---|
| #402 | harness: move `.anvil/logs` out of workdir to survive scaffold wipes | **FEATURE** | (none) |

単独 issue のため並列性なし。`--phase` 未指定のため PR マージまで実行。

## 種別分類

- **BUG_ISSUES**: (なし、Phase 2.5 スキップ)
- **FEATURE_ISSUES**: #402 → `/pm-auto-issue2dev` で処理

## 依存関係

- 前提依存なし
- 内部影響ファイル(推定): `src/logging.rs`、`src/config.rs`、`src/session/store.rs`
- 並列実行: 単独 issue のため適用外

## マージ推奨順序

1. #402 単独(他 issue との競合なし)

## 各 Phase の扱い

| Phase | 扱い |
|---|---|
| 1 | plan.md(本文書)作成 |
| 2 | `feature/issue-402-log-persistence` worktree 作成、CommandMate 登録 |
| 2.5 | **スキップ**(bug ラベルなし) |
| 3 | `/pm-auto-issue2dev 402` 送信、起動シーケンス適用、成果物確認 |
| 4 | **簡易化**(単独 issue、設計書の存在確認のみ) |
| 5 | worker に fmt/clippy/test 実行依頼 |
| 6 | `/pr-merge-pipeline 402` で PR 作成→マージ |
| 7 | **スキップ**(`--full` 未指定) |
| 8 | 完了報告 `summary.md` 出力 |

## Worktree 識別子

- ブランチ: `feature/issue-402-log-persistence`
- Path: `../Anvil-feature-issue-402-log-persistence`
- CommandMate ID(推定): `anvil-feature-issue-402-log-persistence`

## Acceptance 要件(issue 本文より)

- [ ] workdir 内で `rm -rf .anvil/` されても次の log write が失敗しない
- [ ] `anvil logs path [--session ID]` で log パスを表示できる
- [ ] heavy 5-run の全 run で llm-io.jsonl が保全される
