# Target Metric Current State And Issue Inventory

作成日: 2026-06-27

対象:

- `postcheck_stability_score`
- `phase_completion_score`
- `runtime_friction_score`
- `finalization_score`
- `execution_contract_adherence_score` の low-subscore cap

根拠にした直近 eval:

- MVP: `/private/tmp/anvilminimal-eval-mvp-smoke-speed-cloud-3x-mvp-20260627-live/summary.eval.tsv`
- MVP report: `/private/tmp/anvilminimal-eval-mvp-smoke-speed-cloud-3x-mvp-20260627-live/report.md`
- anvildev: `/private/tmp/anvilminimal-eval-mvp-smoke-speed-cloud-3x-anvildev-20260627-live/summary.eval.tsv`
- anvildev report: `/private/tmp/anvilminimal-eval-mvp-smoke-speed-cloud-3x-anvildev-20260627-live/report.md`

条件:

- suite: `mvp-smoke.yaml`
- model profile: `speed-cloud`
- local LLM: unused
- runs: 3
- modes: `minimal-loop,step-plan,plan-run,ultra-plan-run`

## 現状サマリ

MVP は全体で `123/144` 成功、anvildev は `96/144` 成功だった。

| binary | total | minimal-loop | step-plan | plan-run | ultra-plan-run |
|---|---:|---:|---:|---:|---:|
| MVP | 123/144 | 33/36 | 35/36 | 31/36 | 24/36 |
| anvildev | 96/144 | 33/36 | 26/36 | 27/36 | 10/36 |

MVP の失敗 21 件を層別すると以下。

| layer | count | failure kinds | 主な解釈 |
|---|---:|---|---|
| planning | 10 | `planner_schema_error`, `verify_command_policy_error`, `planner_lint_error` | ultra phase/step plan 生成、verify policy、schema/lint が中心 |
| bridge | 5 | `step_verify_failure`, `postcheck_failure`, `deferred_verify_requirement_pending` | plan は生成できたが、成果物/verify/postcheck 契約に接続できていない |
| execution | 4 | `tool_validation_error`, `verify_repair_progress_unchanged` | tool args、repair loop、step runtime の問題 |
| provider | 2 | `provider_http_status` | provider API/status の問題 |

## 重要な観測

### 1. 静的 plan score は plan-run 失敗を十分に分離していない

MVP の `plan-run` では、失敗行の静的 plan 指標が成功行と同等、または失敗行の方が高い。

| metric | success avg | failure avg | gap |
|---|---:|---:|---:|
| `plan_quality_score` | 86.8 | 87.6 | -0.8 |
| `executable_plan_score` | 81.9 | 80.6 | +1.3 |
| `verify_strength_score` | 68.5 | 71.4 | -2.9 |
| `execution_shape_readiness_score` | 73.7 | 69.6 | +4.1 |

問題点:

- `step-plan` 品質だけでは、plan が実際に postcheck まで閉じるかを予測しきれていない。
- 特に `verify_strength_score` は「強そうな verify」を評価しているが、「policy に通り、postcheck と同じ意味を検証できる verify」までは十分に見ていない。
- `execution_shape_readiness_score` は anvildev の方が MVP より高いが、成功率は低い。単体の目的指標にすると誤誘導になる。

対応方向:

- 静的 plan 指標を増やすより、runtime/bridge 指標を主軸にする。
- `verify_strength_score` とは別に、verify の policy 適合と postcheck 意味一致を見る指標を分ける。

### 2. `postcheck_stability_score` は plan-run の失敗分離に強い

MVP の `plan-run` では以下。

| metric | success avg | failure avg | gap |
|---|---:|---:|---:|
| `postcheck_stability_score` | 96.0 | 25.0 | +71.0 |
| `dependency_contract_score` | 100.0 | 70.0 | +30.0 |
| `config_contract_score` | 75.0 | 50.0 | +25.0 |
| `verify_contract_score` | 94.4 | 88.0 | +6.4 |
| `execution_contract_adherence_score` | 96.0 | 80.2 | +15.9 |

問題点:

- `postcheck_stability_score` は強く分離しているが、`execution_contract_adherence_score` に平均化されると失敗シグナルが薄まる。
- 例: postcheck が低くても dependency/config/verify が高いと aggregate が高止まりし、bridge failure の危険を過小評価する。
- `verify_contract_score` は postcheck failure に対する分離力が弱い。verify command が存在することと、postcheck が安定して通ることを混同している。

対応方向:

- `execution_contract_adherence_score` は weighted average だけでなく low-subscore cap を入れる。
- `postcheck_stability_score` が低い場合は、aggregate を強く cap する。
- postcheck の低下理由を `dependency_mutation`, `lockfile_mutation`, `package_manager_mismatch`, `dependency_resolution_failure`, `config_compatibility_failure`, `compile_failure` のカテゴリで残す。

cap 案:

- `min_subscore < 40` の場合、`execution_contract_adherence_score <= 55`
- `min_subscore < 60` の場合、`execution_contract_adherence_score <= 70`
- `postcheck_stability_score < 60` の場合、`execution_contract_adherence_score <= 65`
- `verify_contract_score < 70` の場合、`execution_contract_adherence_score <= 85`

この cap は個別シナリオではなく「低い契約 subscore を aggregate が隠さない」という汎用ルールである。

### 3. `phase_completion_score` は ultra-plan-run の成否を強く分離している

MVP の `ultra-plan-run` では以下。

| metric | success avg | failure avg | gap |
|---|---:|---:|---:|
| `phase_completion_score` | 100.0 | 40.0 | +60.0 |
| `ultra_runtime_health_score` | 100.0 | 40.0 | +60.0 |
| `verify_strength_score` | 77.8 | 54.7 | +23.1 |
| `plan_run_runtime_health_score` | 55.1 | 48.7 | +6.4 |

問題点:

- ultra の失敗は、単一 step の runtime friction よりも phase の進行状況で説明しやすい。
- 現状の `phase_completion_score` は最終的な進捗率としては有効だが、どの phase stage で落ちたかの診断粒度がまだ粗い。
- `planner_schema_error` や `verify_command_policy_error` が phase scaffold / phase step plan に混ざるため、phase 内の planning failure と execution failure を分けて見る必要がある。

対応方向:

- `phase_completion_score` は維持し、ultra の主目的指標にする。
- 追加で phase 内訳を出す。
  - `phase_plan_validity_score`
  - `phase_scaffold_success_score`
  - `phase_step_execution_score`
  - `phase_profile_check_score`
  - `phase_finalization_score`
- `ultra_runtime_health_score` は phase completion の mirror になりすぎないよう、build repair / diagnostic progress / final profile check を独立して反映する。

### 4. `runtime_friction_score` は minimal-loop / plan-run では有効だが ultra では扱いに注意が必要

MVP の mode 別差分:

| mode | success avg | failure avg | gap |
|---|---:|---:|---:|
| minimal-loop | 94.7 | 33.3 | +61.3 |
| plan-run | 63.5 | 29.6 | +33.9 |
| ultra-plan-run | 0.3 | 8.2 | -7.9 |

問題点:

- minimal-loop と plan-run では、tool validation、no tool response、inspection stagnation、repair stagnation をよく捕まえている。
- ultra-plan-run では phase orchestration が主で、通常の minimal loop friction と意味がずれる。成功 ultra でも score が低く出るため、目的指標として使うと誤誘導になる。
- `tool_validation_error` は `tool_policy_compatibility_score` にも出るが、tool args の荒さや recovery の有無がまだ十分に分離されていない。

対応方向:

- `runtime_friction_score` は minimal-loop / plan-run の主目的指標とする。
- ultra-plan-run では直接目的指標にせず、phase 内 step runtime の補助指標として使う。
- tool 系は分解する。
  - `tool_call_validity_score`: raw tool call name / arguments shape が runtime policy に合うか
  - `tool_arg_recovery_score`: malformed args や missing args から回復できたか
  - `repair_stagnation_score`: verify repair が同じ失敗を繰り返していないか

### 5. `finalization_score` は完了判断の失敗を強く分離している

MVP の mode 別差分:

| mode | success avg | failure avg | gap |
|---|---:|---:|---:|
| minimal-loop | 100.0 | 20.0 | +80.0 |
| plan-run | 50.0 | 20.0 | +30.0 |
| ultra-plan-run | 50.0 | 29.0 | +21.0 |

問題点:

- minimal-loop では非常に有効。
- plan-run / ultra-plan-run では、step finalization と plan finalization が混ざって平均化されるため、成功時も 50 付近に留まる。
- required artifacts が揃った後に final response できない問題、deferred verify が残る問題、postcheck failure の問題が同じ低 score に見える。

対応方向:

- `finalization_score` を層別する。
  - `step_finalization_score`
  - `plan_finalization_score`
  - `deferred_verify_finalization_score`
  - `postcheck_finalization_score`
- plan-run の成功条件は step 完了ではなく plan-level final contract と postcheck なので、step finalization だけで aggregate しない。

## 改善すべき指標値の優先順位

### P0: `execution_contract_adherence_score` の low-subscore cap

目的:

- 低い bridge subscore を平均が隠さないようにする。

受け入れ観点:

- `postcheck_stability_score < 60` の run は `execution_contract_adherence_score` が高止まりしない。
- `dependency_contract_score`, `config_contract_score`, `verify_contract_score`, `postcheck_stability_score` の最低値が aggregate に反映される。
- plan-run の failure avg が成功 avg から明確に離れる。

### P1: `postcheck_stability_score` の内訳化

目的:

- postcheck failure を単に低 score とするだけでなく、何が不安定だったかを分類する。

受け入れ観点:

- dependency mutation / lockfile mutation / package manager mismatch / dependency resolution failure / config compatibility failure / compile failure のカテゴリが `extras_json` または report に出る。
- 個別エラーメッセージではなくカテゴリで集計できる。

### P2: `phase_completion_score` の stage 分解

目的:

- ultra-plan-run の失敗箇所を phase planning / scaffold / execution / profile check / finalization に分ける。

受け入れ観点:

- ultra-plan-run の failure row で、どの stage で score が落ちたかが report から分かる。
- `planner_schema_error` と `tool_validation_error` が同じ phase failure として潰れない。

### P3: `runtime_friction_score` の mode-aware 化

目的:

- minimal-loop / plan-run では主指標、ultra-plan-run では phase step の補助指標として扱う。

受け入れ観点:

- ultra-plan-run 成功時に `runtime_friction_score` が低いことを単独で悪化判定しない。
- plan-run では tool validation / repair stagnation / repeated inspection が分離できる。

### P4: `finalization_score` の層別化

目的:

- step finalization と plan/postcheck finalization を分ける。

受け入れ観点:

- `required_artifacts_satisfied` 後に deferred verify が残る failure を別カテゴリで説明できる。
- postcheck failure と no-final-response failure を同じ score 低下として扱わない。

## 現時点の問題点一覧

| ID | 問題 | 影響 | 対応方向 |
|---|---|---|---|
| M013-01 | 静的 plan score が plan-run 失敗を分離しない | 高 score YAML でも runtime/postcheck で失敗する | bridge/runtime 指標を主軸化 |
| M013-02 | `execution_contract_adherence_score` が低い subscore を平均で隠す | postcheck failure の危険を過小評価する | low-subscore cap |
| M013-03 | `postcheck_stability_score` の低下理由が report 上で粗い | 改善対象が dependency/config/build のどこか分かりにくい | stability reason categories を出す |
| M013-04 | `phase_completion_score` は有効だが stage 内訳が不足 | ultra failure の修正箇所が見えにくい | phase stage scores を追加 |
| M013-05 | `runtime_friction_score` は ultra では意味がずれる | ultra 成功を低評価する可能性がある | mode-aware aggregation |
| M013-06 | `finalization_score` が step/plan/postcheck を混ぜている | 完了判断の失敗原因が潰れる | finalization subscore 分解 |
| M013-07 | `tool_validation_error` の回復性が独立評価されていない | tool args が荒い provider の弱点を見落とす | tool call validity/recovery score |
| M013-08 | provider HTTP failure が capability failure と混ざりやすい | API 一時問題と agent 問題を混同する | provider 層として score 集計から分離 |

## 次フェーズで実装すべきこと

1. `execution_contract_adherence_score` に low-subscore cap を実装する。
2. `postcheck_stability_score` の reason categories を TSV/report/extras に出す。
3. `phase_completion_score` を stage subscore に分ける。
4. `runtime_friction_score` の mode-aware aggregation を report 側に反映する。
5. `finalization_score` を step/plan/deferred verify/postcheck に分ける。
6. 既存の MVP/anvildev 3-run summary に後付け再計算し、成功/失敗分離が改善するか確認する。
7. blind suite でも同じ分離傾向が出るか確認し、個別ケースへの過適応を防ぐ。

## 受け入れ条件案

- MVP `mvp-smoke.yaml` 3-run の plan-run failure で、`execution_contract_adherence_score` failure avg が success avg より 25pt 以上低い。
- `postcheck_stability_score < 60` の run が aggregate で 80 以上にならない。
- ultra-plan-run failure の 90%以上で、phase stage subscore から planning/scaffold/execution/profile/finalization のどこで落ちたか分かる。
- minimal-loop / plan-run の failure で `runtime_friction_score` または `finalization_score` のどちらかが明確に低下する。
- blind suite で、known suite と同じ指標が同方向に機能する。
- 指標名や判定条件に、特定 scenario 名、特定成果物名、単発ログ文面を直接埋め込まない。
