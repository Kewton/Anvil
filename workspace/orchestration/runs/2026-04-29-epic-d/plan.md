# Orchestration Plan — 2026-04-29 (Epic D / Track 4)

## 対象 Issue

| GitHub# | 論理# | タイトル |
|---|---|---|
| #465 | #15 | AgentSkill trait と skill registry を追加する |
| #466 | #16 | PrecautionSkill と VerifierSkill を registry に移す |
| #467 | #17 | skill permission / trust tier を導入する |

親 Epic: #447 (Epic D: Agentic Skills Layer)

## 依存関係

```
#465 (AgentSkill trait + registry) — 基盤
  ├─→ #466 (Precaution/Verifier を registry に移植)
  └─→ #467 (trust tier 追加)
```

`feature.md` 上は #466 のみ「#3 #7 #8 後」と書かれているが、論理的に #465 の registry に依存するため直列実行とする。

## 実行戦略 — Option A: 直列

```
Phase D-1: #465 → PR → merge → develop pull
Phase D-2: #466 → PR → merge → develop pull
Phase D-3: #467 → PR → merge → develop pull
Phase E:   統合検証 + summary.md
```

各 Issue について:
1. develop pull
2. worktree 作成
3. ワーカーに `/pm-auto-issue2dev <N>` 送信（hello → status check → 'a' probe）
4. 完了後、commit を確認 → 直接 push + gh pr create
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

- このプラン: `workspace/orchestration/runs/2026-04-29-epic-d/plan.md`
- 完了報告: `workspace/orchestration/runs/2026-04-29-epic-d/summary.md`
