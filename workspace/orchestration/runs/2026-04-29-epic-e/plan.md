# Orchestration Plan — 2026-04-29 (Epic E / Track 5)

## 対象 Issue

| GitHub# | 論理# | タイトル |
|---|---|---|
| #468 | #18 | RepoGraph v1 を実装する |
| #469 | #19 | graph-aware repo context ranking を導入する |
| #470 | #20 | ANVIL.md instructions を path-aware にする |

親 Epic: #448 (Epic E: Repo Graph & Domain Context)

## 依存関係

```
#468 (RepoGraph v1) — 軽量 graph 基盤
  ↓
#469 (graph-aware ranking) — graph を使った context ranking
  ↓
#470 (path-aware ANVIL.md) — path-scoped instruction 注入
```

`feature.md` Track 5: 完全直列。他 Track と独立で、Epic A〜D 完了済の機能には影響しない（観測ベースの拡張）。

## 実行戦略 — Option A: 直列

```
Phase E-1: #468 → PR → merge
Phase E-2: #469 → PR → merge
Phase E-3: #470 → PR → merge
Phase F:   統合検証 + summary.md
```

各 Issue について:
1. develop pull
2. worktree 作成
3. ワーカーに `/pm-auto-issue2dev <N>` 送信（hello → 'a' probe で processing 化）
4. 完了後 commit 確認 → 直接 push + gh pr create
5. CI watch → green → squash merge → branch 削除
6. develop pull → 次へ

## ワーカー管理

- API Error 検出時: "a" 送信
- 60秒 idle 確認 + "a" probe (Claude / Codex 両方)
- 5 回 probe 失敗時: 「Phase 5 (TDD実装) へ skip」明示指示
- commit 後: オーケストレーター側で push + PR + CI watch + merge

## 失敗時の方針

- Worker stuck: probe + commit 指示 escalation
- 品質 NG 3 回連続: ユーザーに報告
- マージコンフリクト: 手動 resolve

## 記録

- このプラン: `workspace/orchestration/runs/2026-04-29-epic-e/plan.md`
- 完了報告: `workspace/orchestration/runs/2026-04-29-epic-e/summary.md`
