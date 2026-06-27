# MVP smoke measurement 2026-06-27

## 実行条件

- suite: `mvp/anvilminimal/eval/suites/mvp-smoke.yaml`
- model profile: `speed-cloud`
- binary: `mvp/anvilminimal/target/release/anvilminimal`
- modes: `step-plan,plan-run,ultra-plan-run`
- runs: `1`
- parallel: `3`
- timeout: `420 sec`
- local LLM: 未使用
- run root: `/private/tmp/anvilminimal-eval-measure-mvp-smoke-1run-net`
- report: `/private/tmp/anvilminimal-eval-measure-mvp-smoke-1run-net/report.md`
- baseline compare: `/private/tmp/anvilminimal-eval-measure-mvp-smoke-1run-compare-baseline.md`

## 結果サマリ

| mode | success | p50_exec_sec | p90_exec_sec | avg_score |
|---|---:|---:|---:|---:|
| step-plan | 12/12 | 10.6 | 34.0 | 86.9 |
| plan-run | 10/12 | 24.5 | 51.7 | 82.3 |
| ultra-plan-run | 8/12 | 69.7 | 103.0 | 71.6 |
| total | 30/36 | 33.7 | 103.0 | 80.3 |

## 前回 baseline との差分

baseline: `/private/tmp/anvilminimal-eval-013-live-mvp-rerun-net/summary.eval.tsv`

| metric | baseline | current | delta |
|---|---:|---:|---:|
| success_rate | 72.2 | 83.3 | +11.1 |
| valid_plan_generated_rate | 91.7 | 97.2 | +5.6 |
| overall_score_avg | 73.0 | 80.3 | +7.3 |
| executable_plan_score_avg | 76.5 | 81.6 | +5.0 |
| execution_shape_readiness_score_avg | 71.2 | 72.7 | +1.6 |
| plan_run_predictive_score_avg | 79.1 | 81.0 | +1.9 |
| plan_run_runtime_health_score_avg | 68.5 | 72.0 | +3.4 |
| ultra_runtime_health_score_avg | 71.5 | 73.8 | +2.4 |
| phase_completion_score_avg | 74.5 | 76.7 | +2.1 |
| postcheck_stability_score_avg | 93.8 | 94.7 | +1.0 |
| finalization_score_avg | 74.6 | 77.6 | +3.0 |

実行時間は p50 が `22.0 sec` から `33.7 sec` へ増加した。成功率とスコアは改善しているが、追加 retry / 修復観測により速度は悪化している可能性がある。

## 残存失敗

| scenario | mode | main | planner | layer | stage | kind | score |
|---|---|---|---|---|---|---|---:|
| docs-heading-update-small | plan-run | gemini | openai | planning | verify_policy | verify_command_policy_error | 4.8 |
| python-markdown-linter-medium | ultra-plan-run | openai | gemini | planning | schema | planner_schema_error | 23.0 |
| python-markdown-linter-medium | ultra-plan-run | gemini | openai | planning | schema | planner_schema_error | 34.8 |
| nextjs-space-invaders-large | plan-run | openai | gemini | postcheck |  |  | 57.0 |
| nextjs-space-invaders-large | ultra-plan-run | openai | gemini | planning | verify_policy | verify_command_policy_error | 28.4 |
| nextjs-space-invaders-large | ultra-plan-run | gemini | openai | planning | lint | planner_lint_error | 27.0 |

## 解釈

- `step-plan` は 12/12 で、計画生成単体は安定している。
- `plan-run` の残り失敗は 2件で、1件は verify policy、1件は postcheck。計画生成そのものより、計画を実行可能な verify / postcheck 契約へ落とす部分が残課題。
- `ultra-plan-run` は 4件失敗で、planner schema / lint / verify policy が中心。ultra phase ごとの plan validation と scaffold 境界は改善したが、phase 内 plan の schema 修復と verify 方針の安定性がまだ弱い。
- `Plan Run Predictiveness` は correlation `-0.4` で、step-plan の predictive score は plan-run 成否をまだ十分に分離できていない。高スコア false positive が 2件あり、実行時契約と verify policy 由来の失敗を事前指標へさらに反映する必要がある。

## 次の改善候補

1. verify policy エラーの内容を scenario 固有ではなく verify command の一般ルールとして分類し、planner retry hint と scorer の両方へ反映する。
2. ultra phase 内の schema repair / lint retry を phase 単位で観測し、どの phase が失敗を作ったかを report で明示する。
3. predictive score は step-plan の静的品質だけでなく、verify policy compatibility と postcheck contract risk をより強く加味する。
