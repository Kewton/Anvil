# Orchestration Plan — 2026-04-28 (Epic B)

## 対象 Issue

| GitHub# | 論理# | サブトラック | タイトル |
|---|---|---|---|
| #456 | #7 | 2a | AnvilScore を導入する |
| #457 | #8 | 2a | auto_test を FeedbackFrame / AnvilScore に接続する |
| #458 | #9 | 2b | Temporary Test Workspace を追加する |
| #459 | #10 | 2b | Tester Skill v1 — smoke test を生成する |
| #460 | #11 | 2b | generated test の promote / discard フローを追加する |
| #461 | #23 | 2c | generated code / generated test の sandbox policy を強化する |

## 依存関係グラフ

```
sub-track 2a:  #456 (AnvilScore type)
                  ↓
                #457 (auto_test → FeedbackFrame/AnvilScore integration)

sub-track 2b:  #458 (Temporary Test Workspace)
                  ↓
                #459 (Tester Skill v1)
                  ↓
                #460 (promote/discard /tests command)

sub-track 2c:  #461 (sandbox policy hardening; independent)
```

## 実行戦略 — Option B: 段階的並列

```
Step 1: #456 + #458 + #461 を並列実行 (3 worker 同時)
Step 2: #456 merge 後 → #457 を開始
Step 3: #458 merge 後 → #459 を開始
Step 4: #459 merge 後 → #460 を開始
```

理論上の最短実行サイクル数: 3 (Step 1 と Step 2/3/4 が重複可能)。

## ワーカー管理ポリシー

- 並列起動時の起動シーケンス: hello → 状態確認 → /pm-auto-issue2dev <N>
- "API Error: Extra usage..." 検出時: "a" 送信で再開（Epic A の経験に基づく）
- worker idle 検出時: 90秒 idle 確認 + "a" probe（最大 5 回）
- 5 回 probe 失敗時: 明示的な commit 指示メッセージにエスカレーション
- commit 完了後: オーケストレーター側で push + gh pr create + CI watch + squash merge を直接実行

## ファイル衝突リスク

- #457 (auto_test) と #461 (sandbox policy) はいずれも `src/tools/bash.rs` 周辺に影響する可能性
  - #461 を Step 1 で先に着手し、merge 後に develop 経由で #457 が引き取る
  - もし両者の merge 順で衝突すれば、後発側の worker に rebase / 修正を依頼

## 失敗時の方針

- Worker stuck: 起動シーケンス + "a" probe + commit 指示エスカレーション
- 品質 NG 3 回連続: ユーザーに報告して中断
- マージコンフリクト: 直列に切り替え、後発 PR に rebase 依頼
- レートリミット: "a" 送信で再開

## 記録

- このプラン: `workspace/orchestration/runs/2026-04-28/plan.md`
- 完了報告: `workspace/orchestration/runs/2026-04-28/summary.md`
