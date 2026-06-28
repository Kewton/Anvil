# 015-2 Baseline Plan Output / Acceptance Analysis

作成日: 2026-06-28

参照 run:

- `/private/tmp/anvilminimal-eval-015-1-recheck-net/summary.plan-output.rescored.eval.tsv`

## Baseline Summary

| metric | value |
|---|---:|
| rows | 48 |
| legacy success | 41/48 |
| acceptance success | 25/36 |
| acceptance false positive | 4 |
| step-plan plan-only rows | 12 |
| plan-output failure rows | 1 |

`step-plan` は成果物を生成しないため、acceptance success の母数から除外される。

## Mode Breakdown

| mode | acceptance success |
|---|---:|
| minimal-loop | 9/12 |
| plan-run | 10/12 |
| ultra-plan-run | 6/12 |
| step-plan | plan-only |

## Acceptance Failure Kind

| kind | count |
|---|---:|
| process_failure | 7 |
| missing_required_capabilities | 3 |
| plan_output_missing_required_capabilities | 1 |

## Oracle Gap Kind

| kind | count |
|---|---:|
| plan_only_mode | 12 |
| postcheck_too_weak_for_semantic_contract | 3 |
| postcheck_too_weak_for_plan_contract | 1 |

## Plan Output Failure

| mode | scenario | main | planner | score | failure |
|---|---|---|---|---:|---|
| plan-run | nextjs-space-invaders-large | gemini | openai | 50.0 | plan_output_missing_required_capabilities |

この失敗は「process/postcheck は成功したが、plan が要求した playable game capability を成果物が満たしていない」ケースである。015-2 ではさらに `prompt -> plan` と `plan -> verify` を追加し、次のどこで契約が切れたかを分離する。

```text
prompt/profile -> YAML plan -> verify coverage -> output adherence -> acceptance
```

## Known Lexical False Positive Guard

`entry point`, `checkpoint`, `endpoint`, `pointer` は `points` / score progression として扱わない。`score`, `points`, `scoreboard` は game/progression 文脈でのみ capability evidence として扱う。

## Source / anvildev Eval Boundary Audit

| area | source anvildev / src | MVP runtime | 015-2 eval method | classification |
|---|---|---|---|---|
| verify command allowlist | `src/agent/minimal_step_runner/verify.rs` が shell control syntax、長すぎる command、unsafe path、未許可 command を拒否する。 | `mvp/anvilminimal/src/planner/verify.rs` / lint が同等の deterministic verify policy を持つ。 | `plan_verify_coverage.py` は verify の意味的強さを評価する。 | eval-only addition |
| expected path confinement | source verify は relative path normalization と missing path 検出を行う。 | MVP plan lint/runtime が workspace-relative expected path を扱う。 | verify artifact read は workspace-relative confinement、skip dirs、read limit、binary skip を追加する。 | eval-only addition with source safety parity |
| source semantic acceptance | source runtime には product capability semantic oracle はない。 | MVP runtime には通常実行 prompt へ hidden oracle を注入しない。 | `source_semantic_oracle.py`, `plan_output_adherence.py`, `plan_capability_contract.py` が deterministic acceptance を評価する。 | eval-only addition |
| browser/interaction oracle | source runtime の minimal step verify では browser acceptance を実行しない。 | MVP runtime も通常実行では browser oracle を強制しない。 | static oracle が inconclusive の場合に deterministic browser oracle を確認先にする。 | eval-only addition |
| plan quality/readiness | source runtime は plan を実行するが、eval score は外側 harness の責務。 | MVP eval は plan/readiness/runtime metrics を外側で算出する。 | 015-2 新指標は source に無いから移植漏れではなく評価強化として扱う。 | intentional difference |

## 015-2 Before/After Comparison Use

この baseline は 015-2 実装後の再評価で以下を見るために固定する。

- `acceptance_false_positive` が減るか。
- `plan_output_missing_required_capabilities` が `prompt-plan` / `plan-verify` / `plan-output` のどこに分類されるか。
- build-only または contentless verify の成功が low confidence になるか。
- MVP と anvildev の比較で provider/network failure と agent/eval capability を分けられるか。
