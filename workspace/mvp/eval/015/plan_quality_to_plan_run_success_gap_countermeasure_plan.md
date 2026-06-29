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
- suite の hidden postcheck oracle、実行後の成否、失敗ログを pre-run score の入力にしない。

## 指標設計の境界

今回追加する指標は、pre-run score と post-run diagnostic を分離する。

| 種別 | 目的 | 入力に使ってよい情報 | 入力に使ってはいけない情報 |
|---|---|---|---|
| pre-run readiness score | plan-run 前に実行摩擦を予測する | user prompt, StepPlan, profile contract, declared expected paths, declared verify, deterministic verify policy | scenario id, suite 名, hidden postcheck, 実行後 success/failure, stderr 固有文言 |
| post-run runtime diagnostic | 失敗後に原因を説明する | event stream, failure kind, stop reason, postcheck result, runtime counters | 同じ run の pre-run score 補正、成功判定の後付け変更 |
| calibration / analysis | 指標の予測力を評価する | paired step-plan/plan-run rows, false positive/negative, run summaries | runtime 本体の分岐条件、planner prompt の個別 scenario 条件 |

この境界により、`plan_run_readiness_score` は plan-run 前に算出できる一般指標として扱い、`missed_predictive_signal` は失敗後の分析 event として扱う。後者を同じ run の score に混ぜることは禁止する。

profile 固有の条件は、profile contract に宣言された抽象条件だけを使う。たとえば web app profile であれば「manifest」「dependency setup」「build/smoke verify」のような contract を使い、特定 framework の固定ファイル名や固定コマンドを scorer に直書きしない。

`declared expected paths` や `declared contract` は、StepPlan、profile contract、user prompt から導出された completion contract、または runtime に明示的に渡された eval-generated completion contract に限る。suite の hidden oracle としてしか存在しない expected artifacts / postcheck 条件は pre-run score に使わない。

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

| subscore | 観点 | 入力 | 失敗との関係 |
|---|---|---|---|
| `verify_policy_readiness_score` | verify command が runtime policy と互換か | StepPlan verify, deterministic verify policy | `verify_command_policy_error` を事前予測 |
| `contract_handoff_score` | StepPlan が宣言した contract と runner が渡す contract が一致するか | StepPlan, execution prompt contract, profile contract, source parity result | 実行モデルの契約逸脱を予測 |
| `postcheck_contract_alignment_score` | declared expected artifacts / dependency / config / verify が互いに矛盾しないか | declared contract のみ。hidden postcheck は使わない | declared contract 由来の postcheck risk を事前検出 |
| `dependency_ordering_score` | setup / manifest / dependency / build / test の順序が抽象 contract と合うか | profile contract, StepPlan order | build/test verify の失敗を予測 |
| `finalization_readiness_score` | step 完了条件と plan 完了条件が明確か | expected_result, verify, expected_paths, completion contract | max_iterations / missing final response を予測 |

初期段階では runtime の挙動を変えず、report / TSV へ追加して成否分離力を見る。十分に分離できることを確認してから planner repair hint へ使う。

`postcheck_contract_alignment_score` は、旧案の `postcheck_predictability_score` を置き換える。postcheck の hidden oracle を予測しようとすると評価過適応になるため、採点対象は「宣言済み contract 同士の整合性」に限定する。

`contract_handoff_score` は単一の LLM 出力採点ではない。内訳は以下に分ける。

- `declared_contract_completeness`: StepPlan 側に goal / expected_result / expected_paths / verify / profile contract が宣言されているかを見る pre-run score。
- `runner_handoff_integrity`: MVP runner がその contract を step prompt / execution context へ渡すことを source parity と event で確認する diagnostic。

同じ run の実行成否を使って `runner_handoff_integrity` を補正してはならない。

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

同じ run の `plan_run_readiness_score` へ反映することも禁止する。`missed_predictive_signal` は次回以降の scorer 改善候補を発見するための post-run analysis event であり、pre-run 指標ではない。

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

- `mvp/anvilminimal/scripts/eval_lib/plan_readiness.py`
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

pre-run readiness の主実装は eval 側の `plan_readiness.py` に分離する。`runtime_scoring.py` は既存 runtime event 由来の採点に責務を寄せ、必要な場合だけ wrapper として呼び出す。これにより、実行後 event を pre-run score に混ぜる事故を避ける。

verify policy は Rust 側の `planner/verify.rs` を source of truth とする。Python scorer が verify policy を mirror する場合は、Rust の診断結果から作った golden fixture / parity test を必須にする。Python 側だけで独自 policy を拡張しない。

## レビュー結果と対応

| 観点 | レビュー結果 | 対応 |
|---|---|---|
| 設計思想 | 小さい loop + deterministic verify の方針には合っている。ただし readiness が runtime の別系統 gate になると複雑化する。 | 初期実装は score/report 追加に限定し、hard gate は既存 lint/verify policy だけに限定する。 |
| 不安定性 | postcheck 予測に hidden oracle を使うと評価過適応と不安定化を招く。 | `postcheck_predictability_score` を `postcheck_contract_alignment_score` に改名し、declared contract の整合性だけを見る。 |
| 影響調査 | step-plan, plan-run, ultra-plan-run, eval report への影響は整理されているが、TUI/slash 経路と anvildev 比較を明示する必要がある。 | Phase 0 に impact matrix、Phase 7 に TUI/slash smoke と anvildev comparison を追加する。 |
| 他機能影響 | readiness score を planner retry に使うと、valid plan が過剰修正される可能性がある。 | Phase 4 では warning と repair hint に留め、valid plan を不当に invalid 扱いしない negative test を追加する。 |
| 複雑性 | score が増えすぎると主軸が不明確になる。 | 既存 score は診断用、主指標は `plan_run_readiness_score` とその 5 subscore に階層化する。 |
| 原因深掘り | 根本原因は contract handoff の後追い復元だが、移植元との差分確認が計画に不足していた。 | Phase 0 に source parity check を追加し、step prompt / verify / completion / ultra phase contract の差分を確認する。 |
| 移植漏れ | plan-run 実行 prompt と completion contract 周辺は過去に移植漏れが出ているため、同種漏れの再確認が必要。 | Phase 0 で `workspace/mvp/eval/010` / `011` の parity 文書と現行コードを照合する。 |
| 移植不備 | 指標改善だけで runtime 修正へ進むと、移植不備の温存につながる。 | Phase 2 以降の実装前に、readiness が示す問題が metric 不備か runtime parity 不備かを分類する。 |
| 責務分離 | `runtime_scoring.py` に pre-run scorer を追加すると post-run event と混ざりやすい。 | pre-run scorer は `plan_readiness.py` へ分離し、`runtime_scoring.py` は必要に応じて呼ぶだけにする。 |
| policy drift | verify policy を Rust と Python の両方に実装すると差分が出る。 | Rust `planner/verify.rs` を source of truth とし、Python mirror は golden parity test を必須にする。 |
| 不確実な対策 | provider/tool-call API 仕様は今回の計画対象ではない。API 挙動変更を仮定した対策は未検証のまま入れない。 | provider request/response 仕様や LLM API 呼び出しを変える場合のみ、OpenAI/Gemini/Ollama live smoke で仮説検証する。今回の文書修正では API 実行は不要。 |
| 評価過適応 | smoke 失敗の直接原因を scorer に直書きすると個別評価指標になる。 | scenario id、suite 名、hidden postcheck、固定 prompt 文言、実行後 failure を pre-run score 入力として禁止する。 |

## Phase 計画

### Phase 0: baseline 固定

作業:

- 現在の run root から false positive / false negative を固定する。
- high score failure の score と failure layer を表にする。
- baseline として `plan_run_predictiveness correlation -0.4` を記録する。
- source parity check と impact matrix を作る。
- pre-run score と post-run diagnostic の入力境界を test fixture 化する。
- verify policy source-of-truth と Python mirror の許容範囲を決める。
- StepPlan 側の contract 宣言不備と runner 側の handoff 不備を別カテゴリにする。

完了条件:

- `step-plan` score が高いのに `plan-run` が失敗したケースを再現できる。
- 失敗理由が planning / runtime / bridge / postcheck のどこかに分類されている。
- `workspace/mvp/eval/010` / `011` の移植差分整理と現行 MVP の step prompt / verify / completion / ultra phase contract が照合済み。
- scorer が scenario id, suite 名, hidden postcheck, 実行後 success/failure を pre-run 入力に使わないことを確認済み。
- `verify_policy_readiness_score` の Rust/Python policy drift 防止方針が決まっている。
- `contract_handoff_score` の内訳が declared contract と runner handoff に分離されている。

### Phase 1: readiness subscore 設計

作業:

- `plan_run_readiness_score` と 5つの subscore を schema に追加する。
- score の意味を `README` に記載する。
- score は 0-100 とし、低い subscore が total を cap する設計にする。
- `postcheck_contract_alignment_score` は declared contract の整合性だけを採点する。
- `missed_predictive_signal` は post-run diagnostic であり readiness score ではないことを schema に明記する。

完了条件:

- 既存 score と役割が重複しすぎていない。
- score の説明が scenario 固有ではない。
- pre-run score の入力一覧と禁止入力一覧が schema / README / tests に反映されている。

### Phase 2: static readiness scorer 実装

作業:

- StepPlan YAML と profile contract から readiness を算出する。
- verify command policy 診断を再利用する。
- dependency ordering / finalization / declared postcheck contract alignment を rule 化する。
- hidden postcheck ではなく declared contract alignment を rule 化する。
- scorer は `plan_readiness.py` に実装し、post-run event scoring と責務を分ける。
- verify policy mirror を実装する場合は Rust 診断 fixture との parity test を先に作る。

完了条件:

- 既存 plan score は大きく変えず、新規 readiness score が追加される。
- valid plan を不当に invalid 扱いしない。
- 同じ StepPlan に対して、実行前と実行後で readiness score が変化しない。
- scenario id を変更しても readiness score が変化しない。
- Rust verify policy と Python readiness policy の差分が test で検出できる。

### Phase 3: event / summary / report 連携

作業:

- readiness event を出力する。
- `summary.eval.tsv` へ新規列を追加する。
- report に readiness diagnostics と false positive table を追加する。
- post-run diagnostic と pre-run readiness の表を分ける。
- runtime event が不要な場合は eval-derived readiness record として summary/report にだけ出す。runtime に event を増やす場合は input provenance を明示する。

完了条件:

- plan-run 失敗時に、どの readiness subscore が低かったか見える。
- step-plan / plan-run paired table で readiness と成功率を比較できる。
- `missed_predictive_signal` は report の post-run analysis セクションにだけ出る。
- readiness が runtime event 由来か eval-derived 由来か report で分かる。

### Phase 4: planner repair hint 連携

作業:

- readiness blocking issue を planner retry hint に変換する。
- hard fail は既存 lint/verify policy だけに限定する。
- warning は score/report に残し、runtime を止めない。
- repair hint は rule 名、該当 step、一般的な期待形だけを渡す。scenario 固有の修正案は渡さない。

完了条件:

- verify policy 違反は planner retry へ具体的に返る。
- declared postcheck contract alignment の問題は planner に expected artifact / verify coupling の改善として返る。
- repair 後も lint/verify policy は弱化されない。
- readiness warning だけで valid plan が拒否されない。

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
- calibration は scorer 改善の評価に使い、同じ run の readiness score 補正には使わない。

完了条件:

- 既存 `Plan Run Predictiveness` に加えて readiness correlation が出る。
- false positive / false negative が failure kind 付きで追跡できる。
- label leakage がないことを unit test で確認する。

### Phase 7: validation

作業:

- unit tests
- smoke eval
- blind eval
- anvildev comparison
- YAML 定性レビュー
- TUI slash smoke
- post-hoc rescore による既存 run root 比較

完了条件:

- `plan_run_readiness_score` が既存 predictive score より plan-run 成否を分離する。
- smoke だけでなく blind でも同じ傾向が出る。
- 成功率改善が verify 弱化によるものではない。
- p50/p90 の速度悪化が許容範囲か説明できる。
- readiness score が高くなった理由を、scenario 固有条件ではなく contract 境界で説明できる。
- readiness score が改善しない場合は、重み調整で押し切らず、metric 不備 / runtime parity 不備 / eval 母集団不足のどれかに分類する。

## テスト計画

### Unit tests

Python:

```bash
python3 -m pytest mvp/anvilminimal/tests/eval/test_runtime_scoring.py -q
python3 -m pytest mvp/anvilminimal/tests/eval/test_summary_schema.py -q
python3 -m pytest mvp/anvilminimal/tests/eval/test_eval_event_report.py -q
```

追加する negative tests:

- scenario id / suite 名を変えても pre-run readiness score が変わらない。
- hidden postcheck result を与えても pre-run readiness score は参照しない。
- 同じ StepPlan では plan-run 成功後 / 失敗後でも readiness score が変わらない。
- `missed_predictive_signal` は summary/report の post-run diagnostic にのみ出る。
- readiness warning だけでは valid plan を invalid にしない。

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
- pre-run readiness score に label leakage がない。
- readiness 指標の改善が runtime の verify/postcheck 弱化によるものではない。
- Rust verify policy と Python readiness policy の drift がない、または drift が検出可能。

目標:

- smoke `plan-run` 成功率 `10/12` 以上を維持し、`11/12` 以上を目指す。
- ultra `phase_completion_score` の failure avg を改善する。
- `Plan Run Predictiveness` correlation を 0 以上へ戻す。
- false positive を 2件から 1件以下へ減らす。
- 分離力が改善しない場合でも、その原因を metric 不備 / runtime parity 不備 / eval 母集団不足に分類できる。

過適応防止:

- blind eval で readiness score の分離傾向が崩れない。
- scenario 固有ファイル名や prompt 文言を条件にしない。
- suite hidden oracle を条件にしない。
- 実行後の success/failure を pre-run score に混ぜない。
- profile 固有判定は profile contract 経由に限定する。
- verify_strength / tool_policy_compatibility が悪化しない。
- postcheck を弱めて成功にしていない。
