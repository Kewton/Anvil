# 015-2 Recalibration Results

作成日: 2026-06-28

## Run Roots

| target | run root | report |
|---|---|---|
| MVP | `/private/tmp/anvilminimal-eval-015-2-mvp-live` | `/private/tmp/anvilminimal-eval-015-2-mvp-live/report.md` |
| anvildev | `/private/tmp/anvilminimal-eval-015-2-anvildev-live` | `/private/tmp/anvilminimal-eval-015-2-anvildev-live/report.md` |
| compare | `/private/tmp/anvilminimal-eval-015-2-compare-mvp-vs-anvildev.md` | same |

条件:

- suite: `mvp-acceptance`
- model profile: `speed-cloud`
- local LLM: 未使用
- modes: `minimal-loop,step-plan,plan-run,ultra-plan-run`
- runs: 1
- parallel: 4
- timeout: 1800 sec

## Headline

| target | legacy success | acceptance success | acceptance false positive |
|---|---:|---:|---:|
| MVP | 36/48 | 17/36 | 9 |
| anvildev | 34/48 | 22/36 | 5 |

解釈:

- legacy success では MVP が高い。
- 015-2 の acceptance では anvildev が高い。
- MVP は build/postcheck だけでは成功に見えるが、source semantic / plan-output / plan-verify の契約で落ちる false positive が多い。

## Mode Breakdown

### MVP

| mode | legacy success | acceptance success | false positive |
|---|---:|---:|---:|
| minimal-loop | 10/12 | 7/12 | 3 |
| step-plan | 10/12 | plan-only | 0 |
| plan-run | 10/12 | 7/12 | 3 |
| ultra-plan-run | 6/12 | 3/12 | 3 |

### anvildev

| mode | legacy success | acceptance success | false positive |
|---|---:|---:|---:|
| minimal-loop | 12/12 | 11/12 | 1 |
| step-plan | 7/12 | plan-only | 0 |
| plan-run | 10/12 | 9/12 | 1 |
| ultra-plan-run | 5/12 | 2/12 | 3 |

## Metric Comparison

`eval-compare.py` baseline=anvildev, experiment=MVP.

| metric | anvildev | MVP | delta |
|---|---:|---:|---:|
| success_rate | 70.8 | 75.0 | +4.2 |
| acceptance_success_rate | 61.1 | 47.2 | -13.9 |
| acceptance_false_positive_count | 5.0 | 9.0 | +4.0 |
| plan_output_adherence_score_avg | 73.5 | 83.9 | +10.4 |
| plan_capability_contract_score_avg | 85.1 | 82.4 | -2.7 |
| prompt_plan_capability_coverage_score_avg | 82.0 | 84.3 | +2.3 |
| plan_verify_coverage_score_avg | 70.0 | 83.2 | +13.2 |
| plan_verify_declared_coverage_score_avg | 62.7 | 77.0 | +14.3 |
| executed_verify_coverage_score_avg | 64.4 | 85.3 | +20.9 |
| acceptance_confidence_score_avg | 75.3 | 71.8 | -3.5 |
| valid_plan_generated_rate | 80.6 | 86.1 | +5.6 |
| plan_quality_score_avg | 68.1 | 85.9 | +17.7 |
| executable_plan_score_avg | 59.1 | 79.0 | +19.9 |
| verify_strength_score_avg | 49.7 | 72.4 | +22.7 |
| verify_adequacy_score_avg | 51.1 | 59.4 | +8.3 |
| overall_score_avg | 66.4 | 74.2 | +7.7 |

## Gap Distribution

### MVP

| gap | count |
|---|---:|
| no_plan_contract | 17 |
| prompt_capability_missing_from_plan | 13 |
| semantic_capability_unverified | 11 |
| browser_required_but_not_declared | 3 |
| build_only_verify_for_behavior_contract | 2 |

confidence reason:

| reason | count |
|---|---:|
| acceptance_success_false | 19 |
| plan_only_mode | 12 |
| prompt_plan_capability_coverage_below_70 | 6 |
| plan_output_adherence_below_70 | 6 |
| plan_verify_coverage_below_40 | 3 |
| browser_oracle_unavailable | 2 |

### anvildev

| gap | count |
|---|---:|
| no_plan_contract | 19 |
| prompt_capability_missing_from_plan | 12 |
| semantic_capability_unverified | 12 |
| contentless_verify_for_capability_contract | 3 |

confidence reason:

| reason | count |
|---|---:|
| acceptance_success_false | 14 |
| plan_only_mode | 12 |
| prompt_plan_capability_coverage_below_70 | 8 |
| plan_output_adherence_below_70 | 7 |
| plan_verify_coverage_below_40 | 6 |

## False Positive Details

### MVP

| mode | scenario | main | planner | failure | plan output | plan verify | prompt-plan | confidence reason |
|---|---|---|---|---|---:|---:|---:|---|
| minimal-loop | fix-js-date-helper-small | openai | gemini | missing_required_capabilities | n/a | n/a | n/a | acceptance_success_false |
| minimal-loop | fix-js-date-helper-small | gemini | openai | missing_required_capabilities | n/a | n/a | n/a | acceptance_success_false |
| ultra-plan-run | fix-js-date-helper-small | openai | gemini | plan_output_missing_required_capabilities | 50.0 | 100.0 | 50.0 | plan_output / prompt-plan low |
| ultra-plan-run | fix-js-date-helper-small | gemini | openai | plan_output_missing_required_capabilities | 75.0 | 100.0 | 100.0 | acceptance_success_false |
| ultra-plan-run | docs-heading-update-small | gemini | openai | plan_output_missing_required_capabilities | 33.3 | 66.7 | 100.0 | plan_output low |
| plan-run | python-markdown-linter-medium | openai | gemini | plan_output_missing_required_capabilities | 66.7 | 100.0 | 100.0 | plan_output low |
| minimal-loop | rust-cli-medium | gemini | openai | missing_required_capabilities | n/a | n/a | n/a | acceptance_success_false |
| plan-run | nextjs-space-invaders-large | gemini | openai | plan_output_missing_required_capabilities | 66.7 | 25.0 | 57.1 | plan-output / plan-verify / prompt-plan low |
| plan-run | repair-exhausted-report-large | gemini | openai | plan_output_missing_required_capabilities | 66.7 | 100.0 | 100.0 | plan_output low |

### anvildev

| mode | scenario | main | planner | failure | plan output | plan verify | prompt-plan | confidence reason |
|---|---|---|---|---|---:|---:|---:|---|
| minimal-loop | fix-js-date-helper-small | openai | gemini | missing_required_capabilities | n/a | n/a | n/a | acceptance_success_false |
| plan-run | fix-js-date-helper-small | gemini | openai | plan_output_missing_required_capabilities | 0.0 | 12.0 | 50.0 | plan-output / plan-verify / prompt-plan low |
| ultra-plan-run | docs-heading-update-small | openai | gemini | plan_output_missing_required_capabilities | 50.0 | 50.0 | 100.0 | plan_output low |
| ultra-plan-run | python-markdown-linter-medium | openai | gemini | plan_output_missing_required_capabilities | 66.7 | 66.7 | 50.0 | plan-output / prompt-plan low |
| ultra-plan-run | python-markdown-linter-medium | gemini | openai | plan_output_missing_required_capabilities | 80.0 | 80.0 | 100.0 | acceptance_success_false |

## Findings

1. 新評価は false positive を検出できている。
   - MVP の legacy success 36/48 のうち 9 件が acceptance false positive。
   - 以前の「ビルドは通るがゲームではない」型の問題は、`plan_output_adherence_score`, `plan_verify_coverage_score`, `acceptance_confidence_reason` で説明可能になった。

2. MVP は YAML/verify 指標が高いが、acceptance success は低い。
   - MVP は `plan_quality`, `verify_strength`, `plan_verify_coverage`, `plan_output_adherence` が anvildev より高い。
   - しかし acceptance success は低く、特に minimal-loop / plan-run / ultra-plan-run の成果物 semantic gap が残っている。

3. anvildev は legacy success と acceptance の差が小さい。
   - anvildev は step-plan 成功率と plan quality は低いが、minimal-loop / plan-run の acceptance が比較的高い。
   - source runtime 側の実行品質が高いケースと、planner/schema で落ちるケースが分かれている。

4. `plan_verify_coverage_score` は false positive 診断に有効だが、単独 hard gate にはまだ早い。
   - `nextjs-space-invaders-large` の MVP plan-run false positive は plan output 66.7 / verify 25.0 / prompt-plan 57.1 と明確に弱い。
   - 一方で plan output 75-80 / verify 80-100 でも acceptance false のケースがあり、source semantic / exact capability evidence の校正が必要。

5. `plan_output_adherence` は non-game にも効くが、厳しさの校正が必要。
   - docs / linter / repair report の plan-output failure は有用な可能性がある一方、plan が過剰に要求した capability を成果物が満たさないケースもある。
   - Phase 9 の方針通り、当面は plan-output hard gate を維持しつつ、新指標は confidence / diagnostic として観察する。

## Gate Policy Update

今回の実測では、新指標をすぐ hard gate 化しない方針を維持する。

| metric | decision | reason |
|---|---|---|
| `plan_output_adherence_success` | hard gate 維持 | false positive を実際に検出している。 |
| `prompt_plan_capability_coverage_score` | diagnostic | weak plan 検出には有効だが、単独失敗扱いは false negative リスクあり。 |
| `plan_verify_coverage_score` | diagnostic/confidence | build-only verify 検出に有効だが、成果物の良し悪しと完全には一致しない。 |
| `acceptance_confidence_score` | diagnostic | success の信頼度として有用。成功率の代替にはしない。 |

## Next

- MVP runtime 改善では、legacy success ではなく `acceptance_success` と `acceptance_false_positive` を主に見る。
- 特に MVP の false positive 9 件を、semantic missing / plan-output missing / verify weak に分けて改善する。
- non-game plan-output の厳しさは blind suite と source comparison で継続校正する。
