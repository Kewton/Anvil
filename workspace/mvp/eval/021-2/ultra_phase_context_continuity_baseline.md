# Ultra Phase Context Continuity Baseline

作成日: 2026-06-29

## Baseline

Phase 021-2 の対象は `/ultra-plan-run` の phase step execution における文脈継続だけである。

移植元 anvil は `run_ultra_plan` が受け取った `SessionSnapshot` を ultra 全体で使い回し、各 phase の step 実行にも同じ session を渡す。これにより phase 1 の tool call、assistant response、repair prompt、verify failure が phase 2 以降の execution model の message history に残る。

MVP は実装前、step-plan 実行関数の内部で毎回 `SessionSnapshot::new()` していた。そのため ultra の phase が変わるたびに execution model の会話履歴が切れ、ファイルシステム上の成果物だけを頼りに後続 phase が再推論していた。

## Source Parity Contract

021-2 で戻す契約は以下。

- ultra-run 経由の phase step execution は同じ caller-owned `SessionSnapshot` を使う。
- step-plan 単体実行は従来どおり独立 session を使う。
- planner の StepPlan 生成 session は共有しない。
- planner には bounded phase context を prompt として渡す。
- failure path でも changed paths、verify failure、repair target の partial outcome を捨てない。
- bounded context には raw stderr、prompt 全文、secret-like value を入れない。

## Scope Boundary

今回対象外にしたもの。

- UltraPlan 生成 prompt / retry / fail-fast の追加変更
- profile final repair の shared session 化
- profile verifier の invariant/final 分離
- repair exhaustion からの `/ultra-plan-run` recovery prompt 保存
- Next.js / Space Invaders 専用テンプレート
- source の `RepairJob` / scaffold pipeline 全体移植

## Baseline Gap

UCC-01〜UCC-13 のうち、今回直接扱うのは UCC-01〜UCC-06、UCC-08、UCC-11〜UCC-13。

特に重要な gap は、session ownership が caller ではなく step-plan runner 内部に閉じていたこと、かつ eval event で shared session の有無を観測できなかったことである。
