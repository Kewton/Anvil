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

## 多角レビュー結果と反映方針

レビュー観点:

| 観点 | レビュー結果 | 反映 |
|---|---|---|
| 目的適合 | 対象指標は成功率そのものではなく、失敗箇所を計画系/実行系/つなぎ系へ分解するための診断指標として扱うべき。 | 指標ごとの責務と non-goal を明記し、単一 overall score 最適化を避ける。 |
| 層分離 | `postcheck_stability_score` は runtime 後の outcome 指標であり、step-plan の静的品質には混ぜるべきではない。 | postcheck は bridge/runtime 評価に限定し、step-plan score には入れない方針を明記。 |
| 統計妥当性 | 現状値は `mvp-smoke.yaml` 3-run の観測に基づくため、閾値は暫定。blind suite と再計測で確認が必要。 | cap の数値は候補扱いにし、acceptance に blind / post-hoc rescore / 複数 run 確認を追加。 |
| 過適応リスク | 個別 scenario、特定成果物名、単発ログ文面を metric 条件に入れると、今後の改善が評価合わせになる。 | reason category は汎用カテゴリだけに限定し、特定 scenario 名を判定条件に使わない制約を追加。 |
| ソフトウェア複雑性 | 新しい top-level score を増やしすぎると読み手が追えなくなる。 | 既存 score は維持し、まず subscore/detail/reason を追加する。top-level 追加は最小限にする。 |
| 互換性 | TSV schema/report 更新は既存 eval consumer とテストに影響する。 | 空値許容、schema test、report test、旧 summary の読み込み確認を受け入れ条件に追加。 |
| anvildev 比較 | anvildev は runtime events が少なく `unclassified_process_failure` が多いため、MVP と同じ粒度で層別できない。 | source 比較は成功率/成果物/静的 plan score 中心とし、runtime subscore は MVP 主軸で扱う。 |
| provider 変動 | `provider_http_status` や transient network は agent capability と分けないと誤判定になる。 | provider layer は指標改善の母集団から除外し、別枠で集計する方針を追加。 |

## 追加レビュー結果と反映

レビューで確認した修正点:

| 観点 | 指摘 | 反映 |
|---|---|---|
| 指標責務 | `execution_contract_adherence_score` の cap に `phase_completion_score` / `runtime_friction_score` / `finalization_score` まで混ぜると、bridge 指標が runtime/ultra 指標を吸収して責務が崩れる。 | ECA の cap 入力は `dependency_contract_score`, `config_contract_score`, `verify_contract_score`, `postcheck_stability_score` に限定する。phase/runtime/finalization は横並びの主指標として扱う。 |
| reason taxonomy | postcheck reason の名前が文書内で揺れている。 | 汎用カテゴリ一覧を固定し、TSV/report/extras では同じ vocabulary を使う。 |
| 後付け再計測 | `summary.eval.tsv` だけでは reason や stage を復元できない場合がある。 | post-hoc rescore は run root の events/logs を入力にし、summary だけで復元できない値は `not_available` と明示する。 |
| anvildev 実行 | anvildev 本体には `--engine minimal` が必要だが、`eval-run.py` は `--binary-kind anvildev` で内部的に付与する。作業手順に `eval-run.py --engine minimal` と書くと無効な引数になる。 | work breakdown の実行コマンドは `--binary anvildev --binary-kind anvildev` に統一し、`--engine minimal` は eval runner 内部付与であると明記する。 |
| テスト実体 | 候補テスト名に実在しない `test_eval_report.py` が含まれていた。 | 既存の `test_eval_event_report.py`, `test_plan_quality_report.py`, `test_summary_schema.py`, `test_runtime_scoring.py` を中心にする。 |

## 指標責務と non-goal

| 指標 | 主な責務 | 使う layer | non-goal |
|---|---|---|---|
| `postcheck_stability_score` | deterministic postcheck が安定して通るか、実行後に依存/設定/lockfile/compile の不安定さが出ていないかを測る。 | bridge/runtime | step-plan の静的美しさや prompt coverage は評価しない。 |
| `phase_completion_score` | ultra の phase が start/scaffold/execute/profile/finalize のどこまで進んだかを測る。 | planning/bridge/execution | 個別 phase の成果物品質やゲーム品質は直接評価しない。 |
| `runtime_friction_score` | tool validation、execution error、no-tool response、探索停滞、repair 停滞を測る。 | execution | postcheck failure や provider HTTP failure を直接説明する主指標にはしない。 |
| `finalization_score` | required artifacts、deferred verify、final response、plan-level contract の完了判断を測る。 | bridge/execution | 実装品質の良し悪しを単独では評価しない。 |
| `execution_contract_adherence_score` | plan contract と実成果物/postcheck の一致を aggregate する。 | bridge | 低い subscore を平均で隠して高評価にすることはしない。 |

設計上の制約:

- `postcheck_stability_score` は事後 outcome 指標なので、planner prompt の直接最適化には使わない。
- `phase_completion_score` は ultra 専用の主指標とし、minimal-loop / plan-run の aggregate には混ぜない。
- `runtime_friction_score` は mode-aware に扱い、ultra-plan-run では phase step の補助情報としてだけ使う。
- `execution_contract_adherence_score` の cap は、個別ケース対策ではなく「低い契約 subscore を aggregate が隠さない」ための一般ルールとして実装する。
- `execution_contract_adherence_score` の cap 入力は bridge 契約 subscore に限定し、phase/runtime/finalization の低下は別指標で見せる。
- provider/API 失敗は capability score から除外し、別の reliability bucket として報告する。

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
- postcheck の低下理由を、以下の汎用カテゴリで残す。
  - `artifact_missing`
  - `dependency_mutation`
  - `lockfile_mutation`
  - `package_manager_mismatch`
  - `dependency_resolution_failure`
  - `dependency_manifest_incoherent`
  - `config_compatibility_failure`
  - `build_or_test_command_failed`
  - `compile_or_type_failure`
  - `dev_server_readiness_failed`
  - `runtime_output_mismatch`
  - `postcheck_not_applicable`

cap 案:

- `min_subscore < 40` の場合、`execution_contract_adherence_score <= 55`
- `min_subscore < 60` の場合、`execution_contract_adherence_score <= 70`
- `postcheck_stability_score < 60` の場合、`execution_contract_adherence_score <= 65`
- `verify_contract_score < 70` の場合、`execution_contract_adherence_score <= 85`

この cap は個別シナリオではなく「低い契約 subscore を aggregate が隠さない」という汎用ルールである。ただし、上記の数値は現時点では暫定値であり、acceptance gate にする前に既存 run の後付け再計算と blind suite で確認する。

cap 適用順序:

1. `dependency_contract_score`, `config_contract_score`, `verify_contract_score`, `postcheck_stability_score` のうち、空でない subscore だけを対象に weighted average を算出する。
2. 空でない subscore の最小値を `min_contract_subscore` として記録する。
3. `postcheck_stability_score` が存在し、かつ 60 未満なら postcheck cap を最優先で適用する。
4. `min_contract_subscore` による cap を適用する。
5. `verify_contract_score` による cap は、verify command が存在する run のみ適用する。
6. cap 前後の値を `execution_contract_adherence_raw_score` と `execution_contract_adherence_score` として区別できるようにする。

cap 対象外:

- `phase_completion_score`
- `runtime_friction_score`
- `finalization_score`
- `tool_policy_compatibility_score`
- `plan_run_runtime_health_score`
- `ultra_runtime_health_score`

これらは ECA の cap 理由ではなく、ECA と並べて failure layer を説明する主指標または補助指標として扱う。

cap が満たすべき性質:

- monotonic: subscore が改善したのに capped score が下がらない。
- sparse-safe: anvildev や step-plan のように subscore が空の row では不当に 0 扱いしない。
- explainable: cap 理由を `execution_contract_cap_reason` として report/TSV/extras のいずれかに出す。
- non-overfit: scenario id、prompt 固有語、特定成果物名を条件に使わない。

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
| M013-09 | `execution_contract_adherence_score` の raw 値と capped 値が区別されていない | cap 導入後に、平均値の変化が実改善か評価ルール変更か分かりにくい | raw/capped 両方を保存 |
| M013-10 | cap 理由が出ないと score 低下の説明性が落ちる | report を見ても dependency/config/verify/postcheck のどこが原因か分からない | `execution_contract_cap_reason` を出す |
| M013-11 | runtime subscore が空の anvildev row と 0 点 row を区別しないと比較が歪む | source 比較で anvildev が不当に低評価または高評価になる | empty-safe 集計を徹底 |
| M013-12 | `phase_completion_score` と `ultra_runtime_health_score` が mirror になりやすい | 似た指標が複数あり、改善対象が曖昧になる | ultra runtime は phase 以外の repair/profile/build 要素も含める |
| M013-13 | `postcheck_stability_score` は outcome 指標なので planner 評価へ混ぜるとリークになる | step-plan の評価が実行後情報に依存してしまう | step-plan score からは分離 |
| M013-14 | 直近の根拠は smoke 3-run だけで統計的に弱い | 閾値が known suite に過適応する | blind 3-run と post-hoc rescore で確認 |
| M013-15 | provider/API failure が目的指標改善の母集団に混ざる | agent 改善では直らない外部要因で score が揺れる | provider failure は別 bucket とし、cap/平均の主分析から除外 |
| M013-16 | 指標が増えすぎると運用上読めない | report が複雑化し、どの改善を優先すべきか分からない | top-level は増やしすぎず、subscore/detail を基本にする |
| M013-17 | 後付け再計測で summary だけを入力にすると、新しい reason/stage 指標を復元できない | 既存 run の比較で `0` と `not_available` を混同する | run root の events/logs を入力にし、復元不能値は `not_available` と明示 |
| M013-18 | anvildev eval の CLI dialect を誤ると比較自体が失敗する | `eval-run.py --engine minimal` のような無効引数や、anvildev 直接実行時の engine 未指定が起きる | eval runner では `--binary-kind anvildev` を使い、内部で `--engine minimal` を付与することを手順に明記 |

## 次フェーズで実装すべきこと

1. `execution_contract_adherence_score` に low-subscore cap を実装する。
2. cap 前後の値と cap 理由を保存する。
3. `postcheck_stability_score` の reason categories を TSV/report/extras に出す。
4. `phase_completion_score` を stage subscore に分ける。
5. `ultra_runtime_health_score` が phase completion の単純 mirror にならないよう、build/profile/repair 要素を明示する。
6. `runtime_friction_score` の mode-aware aggregation を report 側に反映する。
7. `finalization_score` を step/plan/deferred verify/postcheck に分ける。
8. provider/API failure を capability analysis の母集団から除外する。
9. 既存の MVP/anvildev 3-run summary に後付け再計算し、成功/失敗分離が改善するか確認する。
10. blind suite でも同じ分離傾向が出るか確認し、個別ケースへの過適応を防ぐ。

## 受け入れ条件案

必須:

- MVP `mvp-smoke.yaml` 3-run の plan-run failure で、capped `execution_contract_adherence_score` failure avg が success avg より 25pt 以上低いことを診断有効性の目標にする。
- `postcheck_stability_score < 60` の run が capped `execution_contract_adherence_score` で 80 以上にならない。
- `execution_contract_adherence_raw_score` と capped `execution_contract_adherence_score` の両方が TSV/report から確認できる。
- cap が発生した row では `execution_contract_cap_reason` が確認できる。
- ultra-plan-run failure の 90%以上で、phase stage subscore から planning/scaffold/execution/profile/finalization のどこで落ちたか分かる。
- minimal-loop / plan-run の failure で `runtime_friction_score` または `finalization_score` のどちらかが成功平均より 20pt 以上低い。
- provider/API failure は capability failure の成功率・平均 score 改善判定から除外され、別 bucket に集計される。
- blind suite で、known suite と同じ指標が同方向に機能する。
- 指標名や判定条件に、特定 scenario 名、特定成果物名、単発ログ文面を直接埋め込まない。

判定補足:

- 25pt 以上の差分は既知 suite に対する目標値であり、実装を smoke suite に過適応させるための絶対条件ではない。
- 目標未達の場合は失敗扱いで黙って進めず、`target_metric_validation_results.md` に「未達の mode / failure layer / reason / 追加で必要な raw event」を記録する。
- blind suite でも逆方向に出る場合は、指標定義を採用せず再設計する。

推奨:

- `postcheck_stability_score` の低下理由が少なくとも 1 つの汎用カテゴリとして report に出る。
- anvildev row の空 runtime subscore は空値として扱われ、0 点として平均に混ざらない。
- `ultra_runtime_health_score` は `phase_completion_score` と完全一致するだけでなく、build/profile/repair の差も表現できる。
- 後付け再計測で復元できない値は `not_available` と表示し、0 点として扱わない。

## テスト計画案

Unit:

- low-subscore cap の単体テスト。
  - postcheck 25 / dependency 100 / config 100 / verify 100 で aggregate が高止まりしない。
  - min subscore が改善した場合に capped score が下がらない。
  - subscore が空の row では cap が不当に 0 扱いしない。
- postcheck reason category の単体テスト。
  - dependency mutation、lockfile mutation、package manager mismatch、dependency resolution failure、config compatibility failure、compile failure をカテゴリとして検出する。
  - 特定 scenario 名や成果物名がなくても分類できる。
- phase stage subscore の単体テスト。
  - scaffold 失敗、step execution 失敗、profile check 失敗、finalization 失敗を別 score として表現する。
- finalization subscore の単体テスト。
  - step finalization、plan finalization、deferred verify、postcheck finalization を分離する。

Contract:

- `summary.eval.tsv` header schema test に新規 column を追加する。
- `report.md` 生成テストで raw/capped/cap reason/postcheck reason/phase stage が出ることを確認する。
- 旧 summary fixture を読んでも missing column で落ちないことを確認する。
- `not_available` と空文字と 0 点が平均計算で混同されないことを確認する。

Post-hoc validation:

- 直近 MVP 3-run summary に後付け再計算し、plan-run の success/failure 分離が改善することを確認する。
- 直近 anvildev 3-run summary に後付け再計算し、空 runtime subscore が 0 扱いされないことを確認する。
- `mvp-blind.yaml` でも同じ方向に分離することを確認する。

Regression:

- `python3 -m pytest mvp/anvilminimal/tests/eval`
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`
- speed-cloud smoke の少なくとも `step-plan,plan-run` を 1-run 実行し、TSV/report が生成されること。
