# metric revision and MVP vs anvildev comparison

## 実施内容

以下の方針で eval 指標を見直した。

- `plan_quality_score` は「計画として整っているか」の補助指標に留める。
- plan-run 成功予測用に `execution_shape_readiness_score` と `plan_run_predictive_score` を追加する。
- 実行後診断用に `runtime_friction_score`, `artifact_progress_score`, `finalization_score`, `tool_policy_compatibility_score`, `plan_run_runtime_health_score` を追加する。
- `finalization_score` は outcome に近いため、事前予測の `plan_run_predictive_score` には混ぜない。
- anvildev は今回 `ANVIL_EVAL_EVENTS` を出していないため、runtime event 系は算出不能として空欄扱いにする。

## 追加した主指標

### execution_shape_readiness_score

YAML だけから実行摩擦を推定する事前指標。

主に以下を見る。

- first artifact owner が早いか
- artifact を持たない `inspect` / `report` wrapper step が多すぎないか
- empty `expected_paths` step が多すぎないか
- verify が artifact owner から離れすぎていないか
- read-before-write を誘発する instruction になっていないか
- terminal report step が no-tool finalization を誘発しないか

### plan_run_predictive_score

plan-run を走らせる前の成功予測指標。

```
30% execution_shape_readiness_score
25% executable_plan_score
15% artifact_ownership_score
15% verify_strength_score
10% constraint_coverage_score
 5% lint_repair_score
```

### plan_run_runtime_health_score

実行後の診断指標。事前予測ではなく、失敗理由の切り分けに使う。

```
35% runtime_friction_score
25% artifact_progress_score
20% tool_policy_compatibility_score
20% finalization_score
```

## 実行条件

- suite: `mvp/anvilminimal/eval/suites/mvp-smoke.yaml`
- model profile: `speed-cloud`
- modes: `step-plan,plan-run`
- runs: `1`
- local LLM: 未使用
- MVP binary: `mvp/anvilminimal/target/release/anvilminimal`
- anvildev binary: `anvildev --engine minimal`

Run roots:

- MVP: `/private/tmp/anvilminimal-eval-009-mvp-step-plan-run`
- anvildev: `/private/tmp/anvilminimal-eval-009-anvildev-step-plan-run-v2`
- anvildev corrected summary: `/private/tmp/anvilminimal-eval-009-anvildev-step-plan-run-v2/summary.eval.corrected.tsv`
- compare: `/private/tmp/anvilminimal-eval-009-mvp-vs-anvildev-corrected-compare.md`

## mode summary

| impl | mode | success | plan quality avg | executable avg | shape readiness avg | predictive avg | runtime health avg | overall avg | elapsed avg sec |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| MVP | step-plan | 11/12 | 87.4 | 84.2 | 65.5 | 80.4 |  | 81.0 | 12.8 |
| MVP | plan-run | 0/12 | 84.3 | 79.8 | 51.8 | 72.3 | 38.2 | 52.0 | 55.5 |
| anvildev | step-plan | 8/12 | 68.2 | 64.4 | 90.4 | 69.2 |  | 48.9 | 11.8 |
| anvildev | plan-run | 8/12 | 69.8 | 64.5 | 90.8 | 69.9 |  | 58.2 | 20.8 |

## provider pair summary

| impl | mode | main | planner | success | predictive avg | runtime health avg | elapsed avg sec |
|---|---|---|---|---:|---:|---:|---:|
| MVP | step-plan | gemini:gemini-3.1-flash-lite | openai:gpt-5.4-mini | 5/6 | 72.1 |  | 5.4 |
| MVP | step-plan | openai:gpt-5.4-mini | gemini:gemini-3.5-flash | 6/6 | 87.3 |  | 20.2 |
| MVP | plan-run | gemini:gemini-3.1-flash-lite | openai:gpt-5.4-mini | 0/6 | 71.1 | 41.2 | 36.1 |
| MVP | plan-run | openai:gpt-5.4-mini | gemini:gemini-3.5-flash | 0/6 | 73.5 | 35.1 | 74.9 |
| anvildev | step-plan | gemini:gemini-3.1-flash-lite | openai:gpt-5.4-mini | 3/6 | 66.5 |  | 6.4 |
| anvildev | step-plan | openai:gpt-5.4-mini | gemini:gemini-3.5-flash | 5/6 | 70.8 |  | 17.2 |
| anvildev | plan-run | gemini:gemini-3.1-flash-lite | openai:gpt-5.4-mini | 3/6 | 65.0 |  | 13.5 |
| anvildev | plan-run | openai:gpt-5.4-mini | gemini:gemini-3.5-flash | 5/6 | 72.9 |  | 28.2 |

## scenario success

| scenario | mode | MVP | anvildev |
|---|---|---:|---:|
| docs-heading-update-small | step-plan | 1/2 | 2/2 |
| docs-heading-update-small | plan-run | 0/2 | 2/2 |
| fix-js-date-helper-small | step-plan | 2/2 | 1/2 |
| fix-js-date-helper-small | plan-run | 0/2 | 2/2 |
| nextjs-space-invaders-large | step-plan | 2/2 | 0/2 |
| nextjs-space-invaders-large | plan-run | 0/2 | 0/2 |
| python-markdown-linter-medium | step-plan | 2/2 | 2/2 |
| python-markdown-linter-medium | plan-run | 0/2 | 1/2 |
| repair-exhausted-report-large | step-plan | 2/2 | 1/2 |
| repair-exhausted-report-large | plan-run | 0/2 | 1/2 |
| rust-cli-medium | step-plan | 2/2 | 2/2 |
| rust-cli-medium | plan-run | 0/2 | 2/2 |

## failure summary

MVP failure kinds:

- `tool_validation_error`: 5
- `verify_command_policy_error`: 3
- `planner_lint_error`: 2
- `max_iterations`: 1
- `tool_execution_error`: 1
- `postcheck_failure`: 1

anvildev failure kinds:

- `unclassified_process_failure`: 8

anvildev は eval events が無いため、failure classification は MVP より粗い。

## 読み取り

### step-plan は MVP の方が成功率と plan quality が高い

MVP step-plan は 11/12 成功、anvildev は 8/12 成功だった。

MVP は `plan_quality_score`, `executable_plan_score`, `plan_run_predictive_score` がいずれも anvildev より高い。一方で `execution_shape_readiness_score` は anvildev の方が高い。これは anvildev の plan が短く直接的な形になりやすく、MVP は構造的には整っているが wrapper / verify separation が入りやすいことを示す。

### plan-run は anvildev の方が大きく上回る

MVP plan-run は 0/12、anvildev は 8/12 成功だった。

この差は、step-plan の静的品質だけでは説明できない。MVP は step-plan quality が高いにもかかわらず、plan-run runtime で tool validation / verify command policy / max iteration に落ちている。

### 新指標は「MVP 内の plan-run 失敗診断」には有効

MVP plan-run の `plan_run_runtime_health_score` は平均 38.2 と低い。特に `runtime_friction_score` 13.5、`artifact_progress_score` 43.8、`finalization_score` 30.0 が低く、失敗が runtime 摩擦側にあることを示している。

一方、anvildev では runtime event が無いため runtime health 系は比較不能。このため、MVP vs anvildev の横比較では `plan_run_predictive_score` と成功率の関係だけを見る必要がある。

### plan_run_predictive_score はまだ十分に成否を分離しない

MVP plan-run は全滅だが predictive avg は 72.3 と高い。anvildev plan-run 成功 avg は 69.9。つまり、この run では `plan_run_predictive_score` が成功率に単調対応していない。

原因は、現在の predictive score が YAML 形状中心であり、runtime/provider policy 差、MVP の tool argument validation、verify command policy、postcheck failure を事前に十分に見ていないため。

## 採用方針

採用する。

ただし、役割を分ける。

| 指標 | 採用 | 用途 |
|---|---|---|
| `plan_quality_score` | yes | 計画構造の補助指標 |
| `execution_shape_readiness_score` | yes | YAML が runtime に渡しやすい形かを見る事前指標 |
| `plan_run_predictive_score` | yes, 要改善 | plan-run 事前成功予測。ただし現状は provider/runtime policy を十分に含んでいない |
| `runtime_friction_score` | yes | MVP runtime の探索停滞、no-tool、検証エラー診断 |
| `artifact_progress_score` | yes | artifact 作成進捗診断 |
| `finalization_score` | yes | artifact 作成後に完了できたかの診断 |
| `tool_policy_compatibility_score` | yes | tool_validation_error / tool_execute error 診断 |
| `plan_run_runtime_health_score` | yes | 実行後診断の主指標。事前予測には使わない |

## 次の改善方向

1. `plan_run_predictive_score` に runtime/provider policy risk を入れる。
   - verify command allowlist risk
   - tool argument validation risk
   - postcheck dependency/setup risk
   - large profile completion risk

2. anvildev にも runtime events 相当を出すか、anvildev 用の stderr/stdout classifier を作る。
   - 現状は runtime health 横比較ができない。

3. MVP plan-run の failure kind を優先して修正する。
   - `tool_validation_error`: 5
   - `verify_command_policy_error`: 3
   - `planner_lint_error`: 2

