# Plan quality to plan-run success gap countermeasure plan

作成日: 2026-06-27

## 目的

`step-plan` で「YAML として良い計画」と評価されたものが、`plan-run` で実際に成功するとは限らない問題を改善する。

今回の smoke 計測では `step-plan` は `12/12` 成功したが、`plan-run` は `10/12`、`ultra-plan-run` は `8/12` に留まった。また `Plan Run Predictiveness` は correlation `-0.4`、false positive `2`、false negative `1` であり、現在の静的な plan score は plan-run 成否を十分に予測できていない。

この計画では、YAML の見た目や構造の採点を増やすのではなく、StepPlan が plan-run runtime に渡った時に実行可能かを判定する「接続契約」を明示する。

## 非目的

- scenario id / suite 名 / prompt 固有文言による分岐は実装しない。
- score を上げるためだけに重みを調整しない。
- verify policy / lint / postcheck を弱めない。
- high score plan を無条件に実行拒否する gate を作らない。
- anvildev の大きな planner/runtime 構造を丸ごと戻さない。

## 現状

最新計測:

- run root: `/private/tmp/anvilminimal-eval-measure-mvp-smoke-1run-net`
- report: `/private/tmp/anvilminimal-eval-measure-mvp-smoke-1run-net/report.md`
- summary: `/private/tmp/anvilminimal-eval-measure-mvp-smoke-1run-net/summary.eval.tsv`

mode 別結果:

| mode | success | avg_score | p50_exec_sec |
|---|---:|---:|---:|
| step-plan | 12/12 | 86.9 | 10.6 |
| plan-run | 10/12 | 82.3 | 24.5 |
| ultra-plan-run | 8/12 | 71.6 | 69.7 |

predictiveness:

| metric | value |
|---|---:|
| paired_rows | 12 |
| correlation | -0.4 |
| false_positive | 2 |
| false_negative | 1 |

残存失敗:

| scenario | mode | layer | stage | kind |
|---|---|---|---|---|
| docs-heading-update-small | plan-run | planning | verify_policy | verify_command_policy_error |
| nextjs-space-invaders-large | plan-run | postcheck |  |  |
| python-markdown-linter-medium | ultra-plan-run | planning | schema | planner_schema_error |
| python-markdown-linter-medium | ultra-plan-run | planning | schema | planner_schema_error |
| nextjs-space-invaders-large | ultra-plan-run | planning | verify_policy | verify_command_policy_error |
| nextjs-space-invaders-large | ultra-plan-run | planning | lint | planner_lint_error |

## 問題の整理

### 問題1: plan score が YAML の静的品質に寄っている

`plan_quality_score`, `executable_plan_score`, `execution_shape_readiness_score` は、steps の粒度、expected paths、verify の強さ、artifact ownership を見る。しかし plan-run の成否には、以下のような runtime 接続条件が効く。

- verify command が planner policy と runtime policy の両方で許容されるか
- dependency setup と verify の順序が実行時にも成立するか
- expected artifact と verify/postcheck が同じ成果物契約を見ているか
- step 実行 prompt が overall goal / expected_result / verify / completion contract を十分に渡しているか
- finalization が step 単位と plan 単位の両方で閉じるか

これらは YAML としての整形品質だけでは分離できない。

### 問題2: false positive の診断が plan 側へ戻りきっていない

今回の false positive は、step-plan predictive score が高いにもかかわらず plan-run が失敗している。失敗後の event には verify policy / postcheck / finalization / execution contract の情報があるが、それが次回 plan 評価や planner repair hint に十分戻っていない。

### 問題3: ultra-plan-run は phase-local contract が弱い

`ultra-plan-run` は phase ごとに StepPlan を作るため、単一の plan score では以下を見落としやすい。

- どの phase の plan が schema/lint/verify policy に落ちたか
- phase plan は valid だが phase execution が進まないケース
- phase completion と final postcheck の接続ずれ

### 問題4: 指標が多く、成否予測の主軸が見えにくい

現在は個別 score が多い。原因調査には有効だが、plan-run 成功予測としては主軸が曖昧になっている。

必要なのは、既存 score を置き換えることではなく、以下を階層化すること。

1. YAML 静的品質
2. plan-run readiness
3. runtime execution health
4. postcheck/finalization stability

## 根本原因

MVP 移植では StepPlan の生成品質を先に復元し、その後に step runtime / verify / postcheck / ultra phase を順次補強してきた。そのため、plan 生成と plan 実行の間にある contract handoff が後追いになっている。

具体的には以下。

1. plan 評価が「計画として妥当か」を中心にしており、「この runtime に渡した時に成功しやすいか」の評価が薄い。
2. plan-run runtime の失敗理由が、plan generation retry へ戻る構造がまだ限定的。
3. verify policy は厳格化されたが、planner がその policy に合う verify を生成するための pre-run readiness 診断が不足している。
4. postcheck は最終判定として強いが、plan 側で postcheck 成功条件を予測する contract が弱い。
5. ultra phase は phase 単位の plan validity / scaffold / execution / verify / finalization を見る必要があるが、単一 plan score では粒度が粗い。

## 対策方針

### 方針A: plan-run readiness を静的 plan score から分離する

新しい主指標として `plan_run_readiness_score` を導入する。

これは YAML の美しさではなく、plan-run runtime に投入した時の失敗予測を扱う。

subscore:

| subscore | 観点 | 失敗との関係 |
|---|---|---|
| `verify_policy_readiness_score` | verify command が runtime policy と互換か | `verify_command_policy_error` を事前予測 |
| `contract_handoff_score` | goal / expected_result / verify / expected_paths / profile contract が step 実行へ渡るか | 実行モデルの契約逸脱を予測 |
| `postcheck_predictability_score` | expected artifacts / dependency / config / verify が postcheck と矛盾しないか | `postcheck_failure` を事前予測 |
| `dependency_ordering_score` | setup / manifest / install / build / test の順序が自然か | build/test verify の失敗を予測 |
| `finalization_readiness_score` | step 完了条件と plan 完了条件が明確か | max_iterations / missing final response を予測 |

初期段階では runtime の挙動を変えず、report / TSV へ追加して成否分離力を見る。十分に分離できることを確認してから planner repair hint へ使う。

### 方針B: readiness 診断を planner repair hint に戻す

plan-run 実行前に readiness 診断を行い、blocking issue がある場合は既存の schema/lint/quality retry と同じ枠で planner に修正指示を返す。

重要な制約:

- lint/verify policy を弱めない。
- readiness は invalid plan を通す抜け道にしない。
- 最初は warning/report のみにし、hard gate は既存 lint/verify policy 違反に限定する。
- repair hint は rule 名、違反箇所、期待される一般形だけを渡す。

### 方針C: plan-run outcome から missed signal を抽出する

plan-run 失敗時に、静的 score が高かったにもかかわらず失敗した理由を `missed_predictive_signal` として event / summary に記録する。

例:

- high plan score + verify policy failure
  - missed signal: `verify_policy_not_reflected_in_predictive_score`
- high plan score + postcheck failure
  - missed signal: `postcheck_contract_not_reflected_in_readiness`
- high plan score + max_iterations
  - missed signal: `finalization_readiness_not_reflected`

これは後続の score 改善に使うが、runtime の成功判定には使わない。

### 方針D: ultra は phase 単位で readiness を集計する

`ultra-plan-run` では、各 phase の StepPlan に対して `plan_run_readiness_score` を算出し、phase 全体では以下で集計する。

- min score: 最も弱い phase を検出
- avg score: 全体傾向
- failing phase id: 失敗 phase の特定
- stage: schema / lint / verify_policy / execution / postcheck

最終 score は min を重視する。ultra は 1 phase でも詰まると完走できないため。

## 実装候補

### 変更対象

Planner / runtime:

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/planner/lint.rs`
- `mvp/anvilminimal/src/planner/verify.rs`
- `mvp/anvilminimal/src/planner/step_plan.rs`

Eval:

- `mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py`
- `mvp/anvilminimal/scripts/eval_lib/run_summary.py`
- `mvp/anvilminimal/scripts/eval_lib/report.py`
- `mvp/anvilminimal/eval/plan_score_schema.yaml`
- `mvp/anvilminimal/eval/README.md`

Tests:

- `mvp/anvilminimal/tests/eval/test_runtime_scoring.py`
- `mvp/anvilminimal/tests/eval/test_summary_schema.py`
- Rust tests for verify policy diagnostics

### 新規 event 候補

| event | timing | payload |
|---|---|---|
| `plan_run_readiness_evaluated` | plan-run 実行前 | total score, subscores, blocking issues |
| `plan_run_readiness_repair_requested` | planner retry 前 | issue kind, step id, hint |
| `plan_run_missed_predictive_signal` | plan-run 失敗後 | high-score metric, failure kind, missed signal |
| `ultra_phase_readiness_evaluated` | phase plan 作成後 | phase id, readiness, blocking issues |

## Phase 計画

### Phase 0: baseline 固定

作業:

- 現在の run root から false positive / false negative を固定する。
- high score failure の score と failure layer を表にする。
- baseline として `plan_run_predictiveness correlation -0.4` を記録する。

完了条件:

- `step-plan` score が高いのに `plan-run` が失敗したケースを再現できる。
- 失敗理由が planning / runtime / bridge / postcheck のどこかに分類されている。

### Phase 1: readiness subscore 設計

作業:

- `plan_run_readiness_score` と 5つの subscore を schema に追加する。
- score の意味を `README` に記載する。
- score は 0-100 とし、低い subscore が total を cap する設計にする。

完了条件:

- 既存 score と役割が重複しすぎていない。
- score の説明が scenario 固有ではない。

### Phase 2: static readiness scorer 実装

作業:

- StepPlan YAML と profile contract から readiness を算出する。
- verify command policy 診断を再利用する。
- dependency ordering / finalization / postcheck predictability を rule 化する。

完了条件:

- 既存 plan score は大きく変えず、新規 readiness score が追加される。
- valid plan を不当に invalid 扱いしない。

### Phase 3: event / summary / report 連携

作業:

- readiness event を出力する。
- `summary.eval.tsv` へ新規列を追加する。
- report に readiness diagnostics と false positive table を追加する。

完了条件:

- plan-run 失敗時に、どの readiness subscore が低かったか見える。
- step-plan / plan-run paired table で readiness と成功率を比較できる。

### Phase 4: planner repair hint 連携

作業:

- readiness blocking issue を planner retry hint に変換する。
- hard fail は既存 lint/verify policy だけに限定する。
- warning は score/report に残し、runtime を止めない。

完了条件:

- verify policy 違反は planner retry へ具体的に返る。
- postcheck risk は planner に expected artifact / verify coupling の改善として返る。
- repair 後も lint/verify policy は弱化されない。

### Phase 5: ultra phase readiness

作業:

- phase-local StepPlan に readiness score を付ける。
- ultra report に phase min/avg/failing stage を表示する。
- phase scaffold/schema/lint/verify/execution/finalization を分離する。

完了条件:

- ultra 失敗時に詰まった phase と readiness subscore が分かる。
- phase ごとの score を使って全体の false positive を説明できる。

### Phase 6: plan-run outcome calibration

作業:

- `plan_run_missed_predictive_signal` を追加する。
- high score failure と low score success を分類する。
- readiness score と plan-run 成否の相関を report へ出す。

完了条件:

- 既存 `Plan Run Predictiveness` に加えて readiness correlation が出る。
- false positive / false negative が failure kind 付きで追跡できる。

### Phase 7: validation

作業:

- unit tests
- smoke eval
- blind eval
- anvildev comparison
- YAML 定性レビュー

完了条件:

- `plan_run_readiness_score` が既存 predictive score より plan-run 成否を分離する。
- smoke だけでなく blind でも同じ傾向が出る。
- 成功率改善が verify 弱化によるものではない。
- p50/p90 の速度悪化が許容範囲か説明できる。

## テスト計画

### Unit tests

Python:

```bash
python3 -m pytest mvp/anvilminimal/tests/eval/test_runtime_scoring.py -q
python3 -m pytest mvp/anvilminimal/tests/eval/test_summary_schema.py -q
python3 -m pytest mvp/anvilminimal/tests/eval/test_eval_event_report.py -q
```

Rust:

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::verify
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::runner
```

### Eval

MVP smoke:

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --model-profiles mvp/anvilminimal/eval/model_profiles.yaml \
  --model-profile speed-cloud \
  --modes step-plan,plan-run,ultra-plan-run \
  --runs 1 \
  --parallel 3 \
  --timeout-sec 420 \
  --binary mvp/anvilminimal/target/release/anvilminimal \
  --binary-kind anvilminimal \
  --run-root /private/tmp/anvilminimal-eval-015-mvp-smoke
```

Blind:

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-blind.yaml \
  --model-profiles mvp/anvilminimal/eval/model_profiles.yaml \
  --model-profile speed-cloud \
  --modes step-plan,plan-run \
  --runs 1 \
  --parallel 3 \
  --timeout-sec 420 \
  --binary mvp/anvilminimal/target/release/anvilminimal \
  --binary-kind anvilminimal \
  --run-root /private/tmp/anvilminimal-eval-015-mvp-blind
```

anvildev comparison:

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --model-profiles mvp/anvilminimal/eval/model_profiles.yaml \
  --model-profile speed-cloud \
  --modes step-plan,plan-run \
  --runs 1 \
  --parallel 3 \
  --timeout-sec 420 \
  --binary anvildev \
  --binary-kind anvildev \
  --engine minimal \
  --run-root /private/tmp/anvilminimal-eval-015-anvildev-smoke
```

## 成功基準

必須:

- `step-plan` の成功率を落とさない。
- `plan-run` の success rate が baseline から悪化しない。
- `verify_command_policy_error` と `postcheck_failure` の false positive が説明可能になる。
- `plan_run_readiness_score` の success/failure 分離が既存 predictive score より良い。
- report から high-score failure の原因が planning / bridge / runtime / postcheck のどこか分かる。

目標:

- smoke `plan-run` 成功率 `10/12` 以上を維持し、`11/12` 以上を目指す。
- ultra `phase_completion_score` の failure avg を改善する。
- `Plan Run Predictiveness` correlation を 0 以上へ戻す。
- false positive を 2件から 1件以下へ減らす。

過適応防止:

- blind eval で readiness score の分離傾向が崩れない。
- scenario 固有ファイル名や prompt 文言を条件にしない。
- verify_strength / tool_policy_compatibility が悪化しない。
- postcheck を弱めて成功にしていない。
