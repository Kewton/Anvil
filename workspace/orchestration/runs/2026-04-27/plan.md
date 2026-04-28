# Orchestration Plan — 2026-04-27

## 対象 Issue

| GitHub# | 論理# | 種別 | タイトル |
|---|---|---|---|
| #443 | - | bug | rustyline 14 umaskレースで readline_history_roundtrip がフレイキーに失敗 |
| #450 | #1 | feature | FeedbackFrame を導入する |
| #451 | #2 | feature | WorkingMemory に active_precautions を追加する |
| #452 | #3 | feature | Reminder Sidecar を実装する |
| #453 | #4 | feature | Act mode prompt に ACTIVE PRECAUTIONS を注入する |
| #454 | #5 | feature | /precautions コマンドを追加する |
| #455 | #6 | feature | runtime recovery を Precaution 更新に接続する |

## 依存関係グラフ

```
#443 (independent — CI flaky test)

#450 (FeedbackFrame)
  ├─→ #451 (active_precautions in WorkingMemory)
  │     ├─→ #452 (Reminder Sidecar; uses FeedbackFrame + active_precautions)
  │     │     └─→ #455 (recovery → Reminder)
  │     ├─→ #453 (Act mode prompt injection)
  │     └─→ #454 (/precautions command)
  └─→ #455 (FeedbackFrame consumer)
```

`workspace/v0.1.1/feature.md` の Track 1 に従い直列実行: `#1 → #2 → #3 → #4 → #5 → #6`

## 実行戦略 — Option A: 直列実行

衝突回避を最優先。全 Issue を順次実行し、各 PR マージ後に develop を pull してから次の worktree を作成する。

### Phase A (#443)
1. `anvil-develop` で Opus 4.6 による根本原因分析（`/cause-analysis 443` 相当）
2. 分析結果を Issue #443 本文に追記
3. worktree `feature/issue-443-readline-history-flaky` 作成
4. ワーカーに `/bug-fix 443` を送信（起動シーケンス遵守）
5. 完了後、品質チェック → PR 作成 → develop へマージ
6. develop を pull

### Phase B (#450 → #455 直列)
各 Issue について:
1. develop を pull
2. worktree 作成
3. ワーカーに `/pm-auto-issue2dev <N>` を送信（起動シーケンス遵守）
4. 完了後、品質チェック → PR 作成 → develop へマージ
5. develop を pull → 次へ

### Phase C
- develop での統合 build / clippy / test
- summary.md 出力

## 工程数

- Phase A: 1 サイクル
- Phase B: 6 サイクル
- 合計 7 サイクル（直列）

## 失敗時の方針

- Worker が `ready` のまま停止: 起動シーケンス（hello → running 確認 → コマンド送信 → "a" 送信）でリカバリー
- 品質 NG 3 回連続: ユーザーに報告して中断
- マージコンフリクト: ユーザーに報告して中断
- レートリミット: "a" 送信で再開

## 記録

- このプラン: `workspace/orchestration/runs/2026-04-27/plan.md`
- 完了報告: `workspace/orchestration/runs/2026-04-27/summary.md`
