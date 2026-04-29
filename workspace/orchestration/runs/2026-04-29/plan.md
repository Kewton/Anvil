# Orchestration Plan — 2026-04-29 (Epic C / Track 3)

## 対象 Issue

| GitHub# | 論理# | タイトル |
|---|---|---|
| #462 | #12 | CaseRecord を抽出する |
| #463 | #13 | Case retrieval を実装する |
| #464 | #14 | failed case / anti-pattern を保存する |

親 Epic: #446 (Epic C: Case Memory & CBR)

## 依存関係グラフ

```
#462 (CaseRecord 型 + 抽出パイプライン)
  ↓
#463 (retrieval — relevance score, prompt 注入)
  ↓
#464 (anti-pattern — failed action 抽出, avoid precaution 注入)
```

`workspace/v0.1.1/feature.md` Track 3 の指定に従い完全直列実行（Epic A と同様）。

## 前提

- #1 (#450 FeedbackFrame): merged ✅
- #2 (#451 active_precautions): merged ✅
- #7 (#456 AnvilScore): merged ✅

すべて develop に統合済みなので #462 は即時着手可能。

## 実行戦略 — Option A: 直列実行

```
Phase C-1: #462 → PR → develop merge → develop pull
Phase C-2: #463 → PR → develop merge → develop pull
Phase C-3: #464 → PR → develop merge → develop pull
Phase D:   統合検証 (fmt/clippy/test/build) + summary.md
```

各 Issue について:
1. develop pull
2. worktree 作成
3. ワーカーに `/pm-auto-issue2dev <N>` を送信（起動シーケンス）
4. 完了後、品質チェック → 直接 push + PR 作成（worker 経由 /create-pr は不安定なため）
5. CI watch → green 確認 → squash merge
6. develop pull → 次へ

## ワーカー管理ポリシー

- 起動シーケンス: hello → 状態確認 → /pm-auto-issue2dev → "a" probe で確実に processing 化
- "API Error: Extra usage..." 検出時: "a" 送信で再開
- worker idle 検出時: 60秒 idle 確認 + "a" probe（Claude / Codex 両方）
- 5 回 probe 失敗時: 明示的な「Phase 5 (TDD実装) へ skip」指示にエスカレーション
- commit 完了後: オーケストレーター側で push + gh pr create + CI watch + squash merge を直接実行

## 想定リスク

- API Error 頻発（Epic A/B で経験済み）→ "a" 送信で対処
- CaseRecord は新規ファイル中心（衝突少）
- retrieval は session module の参照系で読み取りメイン → 衝突少
- anti-pattern は #462 の case 構造と #464 の anti-pattern 構造が並存 → mod レベルで分離可能

## 失敗時の方針

- Worker stuck: "a" probe + commit 指示 escalation
- 品質 NG 3 回連続: ユーザーに報告して中断
- マージコンフリクト: 手動 resolve、複雑な場合はユーザー判断仰ぐ

## 記録

- このプラン: `workspace/orchestration/runs/2026-04-29/plan.md`
- 完了報告: `workspace/orchestration/runs/2026-04-29/summary.md`
