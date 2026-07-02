# UltraPlan Generation Source Parity Eval Result

作成日: 2026-06-29

## 実行結果サマリ

Phase 021-1 の実装後、MVP 版のみで targeted eval を実行した。

実行条件:

- suite: `mvp-smoke`
- scenario: `nextjs-space-invaders-large`
- mode: `ultra-plan-run`
- model profile: `openai-main-gemini-plan`
- main: `openai:gpt-5.4-mini`
- planner: `gemini:gemini-3.5-flash`
- binary: `mvp/anvilminimal/target/release/anvilminimal`
- run root: `/private/tmp/anvilminimal-021-1-ultra-targeted-net`

結果:

| 観点 | 結果 |
| --- | --- |
| process success | false |
| valid_plan_generated | true |
| UltraPlan generation attempt | 1 |
| UltraPlan generation retry | 0 |
| UltraPlan deterministic fallback | not used |
| UltraPlan metadata normalization | goal |
| UltraPlan phase count | 3 |
| failure stage | phase 2 execute |
| failure kind after classifier correction | step_verify_failure |
| failed verify | npm run build |

## 期待通り改善した点

### 1. deterministic fallback plan ではなく planner-generated UltraPlan になった

保存された UltraPlan は `UltraPlan::deterministic` の `scaffold / implement / verify` ではなく、Gemini planner が生成した profile-aware phase plan だった。

これは 021-1 の主目的である「invalid planner output を薄い deterministic fallback として通常成功扱いしない」方向に合っている。

### 2. source parity event が出ている

`anvil-events.jsonl` に以下が出ている。

- `ultra_plan_generation_attempt`
- `ultra_plan_raw_output_shape`
- `ultra_plan_generation_metadata_normalized`
- `ultra_plan_generation_succeeded`

これにより、UltraPlan generation の成否と retry の有無を runtime failure から分離して確認できる。

### 3. metadata echo 揺れを過剰拒否していない

planner は goal を少し言い換えたが、request context の goal に正規化して実行している。これは移植元と同じ方向である。

## 失敗した点

失敗は UltraPlan generation ではなく、phase 2 の step verify repair で発生した。

stderr の直接原因:

```text
step verify-build-success failed verification after bounded repair: command failed: npm run build
Type error: Cannot find module or type declarations for side-effect import of './globals.css'.
```

つまり、021-1 で対象外にした以下の領域が残っている。

- phase-aware verification
- runtime bridge
- step verify repair
- Next.js CSS side-effect import / declaration repair

## 指標上の注意

targeted eval の summary は旧分類では `planner_lint_error` に見えていた。これは phase 2 の StepPlan 生成中に retry された planner error が残っており、後段の runtime failure より優先されていたため。

今回、classifier を補正し、後段の以下 event を優先できるようにした。

- `step_verify_failure`
- `ultra_phase_failed`

補正後の分類は `step_verify_failure`。

## 結論

021-1 の範囲では改善が確認できた。

- UltraPlan generation は source parity に近づいた。
- deterministic fallback を通常成功 plan として実行する経路は止めた。
- invalid / tool-call / metadata normalization / retry diagnostics をテストで確認した。

ただし `/ultra-plan-run` 全体の成功率改善にはまだ直結していない。次に改善すべき主因は UltraPlan generation ではなく、生成された良い UltraPlan を phase/step runtime で成功まで運ぶ bridge 側である。
