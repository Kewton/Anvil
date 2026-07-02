# Plan capability / verify / acceptance eval work breakdown

作成日: 2026-06-28

対象計画:

- `workspace/mvp/eval/015-2/plan_capability_verify_acceptance_eval_improvement_plan.md`

## 実施方針

この作業は eval 方法の改善であり、MVP runtime / planner の生成ロジック改善ではない。

実装では以下を守る。

- eval harness に閉じる。
- scenario id 固有分岐を入れない。
- 人間レビューを評価フローに組み込まない。
- LLM judge を hard gate にしない。
- `legacy_success` と `acceptance_success` を分離したままにする。
- `step-plan` の pre-run score に post-run artifact / success / stderr を混ぜない。
- YAML plan が無い minimal-loop は plan-based 指標を `not_applicable` とする。
- hard gate 化は Phase 9 まで保留し、まず diagnostic / confidence として出す。

## レビュー反映事項

| 観点 | 指摘 | 反映 |
|---|---|---|
| 指標定義 | `plan_capability_contract_score` の算出作業が曖昧だった。 | P1-W03 に score / evidence completeness 算出を追加した。 |
| schema 整合 | `acceptance_confidence_score` の理由列がなく、低 confidence の根拠が TSV/report で追いにくい。 | Phase 4 / Phase 6 / チェックリストに `acceptance_confidence_reason` を追加した。 |
| 安全性 | verify artifact parser が workspace 外や巨大ファイルを読むリスクがある。 | P2-W03 に path confinement / skip dirs / read limit / binary skip を追加した。 |
| source parity | anvildev との差分確認が Phase 8 だけで遅い。 | Phase 0 に source/anvildev eval boundary の事前確認を追加した。 |
| pre/post 純度 | step-plan と plan-run の verify coverage 入力境界をテストで固定する必要がある。 | P2-W03 / P2-W04 / P10-W01 に declared/executed fixture を明記した。 |
| oracle 判定 | 静的 oracle の false negative を hard failure と誤分類するリスクがある。 | P6-W01 / P10-W01 に inconclusive と deterministic browser oracle unavailable の扱いを追加した。 |

## 変更予定ファイル

### 新規

- `mvp/anvilminimal/scripts/eval_lib/plan_capability_contract.py`
- `mvp/anvilminimal/scripts/eval_lib/plan_verify_coverage.py`
- `mvp/anvilminimal/tests/eval/test_plan_capability_contract.py`
- `mvp/anvilminimal/tests/eval/test_plan_verify_coverage.py`
- `mvp/anvilminimal/tests/eval/test_eval_rescore_runtime.py`
- `mvp/anvilminimal/eval/fixtures/acceptance_oracle/game_prompt_plan_gap/...`
- `mvp/anvilminimal/eval/fixtures/acceptance_oracle/game_build_only_verify/...`
- `mvp/anvilminimal/eval/fixtures/acceptance_oracle/game_static_smoke_verify/...`
- `mvp/anvilminimal/eval/fixtures/acceptance_oracle/step_plan_declared_verify_artifact/...`
- `workspace/mvp/eval/015-2/baseline_plan_output_acceptance_analysis.md`
- `workspace/mvp/eval/015-2/015_2_recalibration_results.md`
- `workspace/mvp/eval/015-2/gate_policy_decision.md`

### 既存更新

- `mvp/anvilminimal/scripts/eval_lib/plan_output_adherence.py`
- `mvp/anvilminimal/scripts/eval_lib/plan_scoring.py`
- `mvp/anvilminimal/scripts/eval_lib/verify_adequacy.py`
- `mvp/anvilminimal/scripts/eval_lib/acceptance_outcome.py`
- `mvp/anvilminimal/scripts/eval_lib/report.py`
- `mvp/anvilminimal/scripts/eval_lib/run_summary.py`
- `mvp/anvilminimal/scripts/eval-run.py`
- `mvp/anvilminimal/scripts/eval-rescore-runtime.py`
- `mvp/anvilminimal/eval/plan_score_schema.yaml`
- `mvp/anvilminimal/eval/README.md`
- `mvp/anvilminimal/eval/suites/mvp-smoke.yaml`
- `mvp/anvilminimal/eval/suites/mvp-blind.yaml`
- `mvp/anvilminimal/eval/suites/mvp-acceptance.yaml`
- `mvp/anvilminimal/eval/suites/mvp-tui-acceptance.yaml`
- `mvp/anvilminimal/tests/eval/test_plan_output_adherence.py`
- `mvp/anvilminimal/tests/eval/test_verify_adequacy.py`
- `mvp/anvilminimal/tests/eval/test_acceptance_outcome.py`
- `mvp/anvilminimal/tests/eval/test_summary_schema.py`
- `mvp/anvilminimal/tests/eval/test_eval_event_report.py`

## Phase 0: Baseline 再固定

目的:

015-2 実装前の評価結果を、比較可能な baseline として固定する。

### P0-W01 再スコア結果の集計

対象:

- `/private/tmp/anvilminimal-eval-015-1-recheck-net/summary.plan-output.rescored.eval.tsv`

作業:

1. `legacy_success`, `acceptance_success`, `acceptance_false_positive` を集計する。
2. mode 別の acceptance success を集計する。
3. `acceptance_failure_kind` と `oracle_gap_kind` を集計する。
4. `plan_output_adherence_success=false` の行を抽出する。
5. `extras_json.acceptance_details.plan_output` から required / missing capability を記録する。

成果物:

- `workspace/mvp/eval/015-2/baseline_plan_output_acceptance_analysis.md`

受入条件:

- before 数値が表で固定されている。
- plan-output failure の根拠 capability が記録されている。
- semantic failure / plan-output failure / process failure が分かれている。

### P0-W02 既知 oracle 誤検出の固定

対象:

- `mvp/anvilminimal/tests/eval/test_plan_output_adherence.py`
- `mvp/anvilminimal/eval/fixtures/acceptance_oracle`

作業:

1. `entry point` が `points` と誤検出されない fixture を確認する。
2. 類似の lexical false positive 候補を追加する。
   - `checkpoint`
   - `endpoint`
   - `pointer`
   - `scoreboard` は score として扱う。
3. plan-output extractor の details に source snippet が出ることを確認する。

受入条件:

- CLI / API / docs 系で game progression capability が誤抽出されない。
- game 文脈では score / points / scoreboard が抽出される。
- pytest が通る。

### P0-W03 Source / anvildev eval boundary audit

対象:

- `src/agent/minimal_step_runner/verify.rs`
- `src/agent/minimal_step_runner/profiles`
- `mvp/anvilminimal/src/planner/verify.rs`
- `mvp/anvilminimal/scripts/eval_lib`

作業:

1. 移植元 anvildev の verify / profile / semantic acceptance の範囲を確認する。
2. MVP 側の eval-only oracle と runtime logic を分離して表にする。
3. 015-2 で新設する指標が移植漏れ対策なのか、評価方法の追加なのかを分類する。
4. anvildev 比較で使える列と使えない列を整理する。

成果物:

- `workspace/mvp/eval/015-2/baseline_plan_output_acceptance_analysis.md` に追記

受入条件:

- source parity / eval-only addition / intentional difference が分かれている。
- 015-2 指標を移植元に無いから移植不備と誤分類しない。

## Phase 1: Plan capability contract module

目的:

plan capability extraction を独立 module 化し、prompt -> plan gap を評価できるようにする。

### P1-W01 module scaffolding

対象:

- `mvp/anvilminimal/scripts/eval_lib/plan_capability_contract.py`

作業:

1. `CapabilityRule` 相当の構造を定義する。
2. domain を定義する。
   - interactive-game
   - interactive-web-app
   - cli-tool
   - library-with-tests
   - docs-content
   - data-transform
3. capability extraction entrypoint を実装する。
   - `extract_prompt_capabilities(scenario)`
   - `extract_plan_capabilities(plan_paths)`
   - `score_prompt_plan_capability_coverage(scenario, plan_paths)`
4. result に `oracle_version` を含める。
5. result に source snippets を含める。

受入条件:

- module import が通る。
- 空 plan / missing plan で `not_applicable` を返す。
- scenario id を参照しない。

### P1-W02 StepPlan / UltraPlan parsing

対象:

- `plan_capability_contract.py`
- `mvp/anvilminimal/tests/eval/test_plan_capability_contract.py`

作業:

1. StepPlan の `goal`, `steps[].instruction`, `expected_paths`, `verify` を読む。
2. UltraPlan の `goal`, `profile`, `phases[].prompt`, `phases[].intent`, `phases[].verify` を読む。
3. JSON / YAML fallback parser の失敗時は parse failure details を返す。
4. plan phase ごとの capability と aggregate capability を分ける。

受入条件:

- StepPlan で game capability を抽出できる。
- UltraPlan で phase capability を抽出できる。
- parse failure が exception で eval 全体を落とさない。

### P1-W03 prompt-plan coverage scoring

対象:

- `plan_capability_contract.py`
- `test_plan_capability_contract.py`

作業:

1. prompt-derived capability と plan-derived capability の intersection を算出する。
2. `prompt_plan_capability_coverage_score` を算出する。
3. `prompt_plan_gap_kind` を算出する。
4. `prompt_plan_missing_capability_count` を算出する。
5. `plan_capability_contract_score` を算出する。
6. plan capability と expected_paths / verify の接続度を evidence completeness として算出する。
7. profile-derived capability は prompt-derived capability と別 details に残す。

受入条件:

- game prompt + Next.js skeleton plan は低 coverage。
- game prompt + Canvas/keyboard/enemy/bullet/collision/score plan は高 coverage。
- capability が expected_paths / verify と接続している plan は `plan_capability_contract_score` が高い。
- docs / CLI / library では game capability を要求しない。
- 日本語 prompt でも主要 game capability を抽出できる。

### P1-W04 plan_output_adherence integration

対象:

- `mvp/anvilminimal/scripts/eval_lib/plan_output_adherence.py`
- `test_plan_output_adherence.py`

作業:

1. duplicated capability rules を `plan_capability_contract.py` へ移す。
2. plan-output adherence は plan-derived capabilities を入力として使う。
3. result details に `plan_capability_oracle_version` を含める。
4. existing tests を新 module 経由へ更新する。

受入条件:

- 既存 plan-output tests が通る。
- details の required capability と source snippet が維持される。

## Phase 2: Plan verify coverage metric

目的:

verify が plan capability をどれだけ検証しているかを測定する。

### P2-W01 module scaffolding

対象:

- `mvp/anvilminimal/scripts/eval_lib/plan_verify_coverage.py`

作業:

1. `score_plan_verify_coverage(...)` を実装する。
2. 入力を定義する。
   - plan paths
   - workdir
   - scenario
   - mode
   - postcheck events
   - plan capability result
3. output fields を定義する。
   - `plan_verify_declared_coverage_score`
   - `executed_verify_coverage_score`
   - `plan_verify_coverage_score`
   - `plan_verified_capability_count`
   - `plan_unverified_capability_count`
   - `plan_verify_gap_kind`
   - `plan_verify_oracle_version`
   - details

受入条件:

- plan absent で `not_applicable`。
- required capability が空なら `not_applicable`。
- exception で eval 全体を落とさない。

### P2-W02 verify command classification

対象:

- `plan_verify_coverage.py`
- `test_plan_verify_coverage.py`

作業:

1. verify command を分類する。
   - build / compile
   - file existence
   - contentless read
   - grep / source assertion
   - test runner
   - browser declaration
   - dedicated smoke artifact
2. `test -f`, `cat`, `readFileSync`, `existsSync` を contentless として分類する。
3. `npm run build`, `next build`, `cargo test` は build/test gate とし、behavior coverage には低加点にする。
4. `node smoke-check.js` など artifact 実行 command を検出する。

受入条件:

- build-only interactive game verify は low coverage。
- contentless verify は low coverage。
- test runner は library / CLI では有効、interactive game semantic では限定加点。

### P2-W03 verify artifact parsing

対象:

- `plan_verify_coverage.py`
- fixtures
- `test_plan_verify_coverage.py`

作業:

1. verify command から artifact path を抽出する。
   - `node smoke-check.js`
   - `python smoke_check.py`
   - `cargo test`
   - `npm test`
2. post-run mode では workdir 内の artifact を読む。
3. pre-run / step-plan mode では artifact 内容を読まず、declared coverage のみにする。
4. source assertion patterns を capability に map する。
   - `requestAnimationFrame` / `canvas`
   - `keydown` / `keyup`
   - `enemy` / `invader`
   - `bullet` / `projectile`
   - coordinate comparison / collision
   - `score` / `lives`
5. artifact path は workspace-relative に閉じる。
6. `node_modules`, `.next`, `.git`, `target`, `dist`, `build` は skip する。
7. 1ファイルあたりの読み取り上限を設ける。
8. binary file は skip する。

受入条件:

- step-plan で未生成 smoke-check を読まない。
- plan-run で生成済み smoke-check を読み coverage が上がる。
- assertion が file existence だけなら coverage は上がらない。
- workspace 外パス、巨大ファイル、binary file は安全に skip される。

### P2-W04 coverage scoring

対象:

- `plan_verify_coverage.py`
- `test_plan_verify_coverage.py`

作業:

1. required capability ごとに verified / unverified を出す。
2. declared coverage と executed coverage を別々に算出する。
3. mode に応じて display score を選ぶ。
   - `step-plan`: declared
   - `plan-run`: executed if available, otherwise declared
   - `ultra-plan-run`: phase min / final aggregate
   - `minimal-loop`: not_applicable
4. gap kind を算出する。

受入条件:

- details に required / verified / unverified / evidence が出る。
- coverage score が 0/blank/not_applicable を正しく使い分ける。

## Phase 3: Verify adequacy cap

目的:

verify adequacy が plan capability coverage を無視して高くならないようにする。

### P3-W01 cap logic

対象:

- `mvp/anvilminimal/scripts/eval_lib/verify_adequacy.py`
- `mvp/anvilminimal/scripts/eval_lib/plan_scoring.py`

作業:

1. `score_verify_adequacy(...)` に optional plan coverage input を追加する。
2. cap を適用する。
   - `plan_verify_coverage_score < 40`: max 60
   - behavior plan and `plan_verify_coverage_score < 20`: max 45
   - `prompt_plan_capability_coverage_score < 70`: max 80
3. cap reason を返す。
4. existing behavior との backward compatibility を保つ。

受入条件:

- plan coverage input が無い場合、既存 scorer は壊れない。
- build-only game verify は cap される。
- CLI / library の deterministic test は過度に cap されない。

### P3-W02 tests

対象:

- `test_verify_adequacy.py`

作業:

1. build-only interactive game fixture を追加する。
2. semantic smoke script fixture を追加する。
3. CLI/library fixture を追加する。
4. cap reason assertion を追加する。

受入条件:

- pytest が通る。
- cap reason が reportable field として残る。

## Phase 4: Summary schema and report

目的:

新指標を TSV / report / compare に出す。

### P4-W01 summary schema

対象:

- `run_summary.py`
- `plan_score_schema.yaml`
- `test_summary_schema.py`

作業:

1. 新列を `SUMMARY_HEADER` に追加する。
2. `empty_summary_row` に default を追加する。
3. fixture summary の read compatibility を確認する。
4. `read_summary` が古い fixture を不正扱いしないか確認する。
5. `acceptance_confidence_reason` を TSV に追加する。

受入条件:

- summary write/read round trip が通る。
- schema yaml と header が一致する。
- confidence 低下理由が TSV から読める。

### P4-W02 report

対象:

- `report.py`
- `test_eval_event_report.py`

作業:

1. `Acceptance Outcomes` に confidence / plan verify / prompt-plan を追加する。
2. `Capability Contract Outcomes` セクションを追加する。
3. compare report に新指標 avg を追加する。
4. gap kind count を出す。
5. confidence reason count を出す。

受入条件:

- report に required / verified / implemented / missing_plan / missing_output / missing_verify が出る。
- compare に新指標 avg が出る。
- confidence 低下理由が report に出る。
- report fixture tests が通る。

### P4-W03 README

対象:

- `mvp/anvilminimal/eval/README.md`

作業:

1. 新指標の意味を追記する。
2. `success` と `acceptance_success` の見方を更新する。
3. hard gate 化保留の方針を明記する。
4. not_applicable / blank / zero の違いを追記する。

受入条件:

- README だけで指標の読み方が分かる。

## Phase 5: Eval-run integration

目的:

通常 eval と rescore に新指標を接続する。

### P5-W01 eval-run integration

対象:

- `eval-run.py`

作業:

1. plan score loop 内で plan capability contract を算出する。
2. plan verify coverage を算出する。
3. row に新指標を埋める。
4. `acceptance_outcome` に新指標 details を渡す。
5. `events.jsonl` に `plan_capability_contract_evaluated` と `plan_verify_coverage_evaluated` を出す。

受入条件:

- dry-run が通る。
- step-plan で post-run artifact を読まない。
- minimal-loop plan absent が not_applicable になる。

### P5-W02 rescore integration

対象:

- `eval-rescore-runtime.py`
- `test_eval_rescore_runtime.py`

作業:

1. run root の `plan_artifacts` と `workdir` から新指標を再計算する。
2. `extras_json.acceptance_details` に capability details を保存する。
3. 古い run root で欠損ファイルがある場合は `not_available` とする。
4. rescore unit test を追加する。

受入条件:

- 015-1 run root を rescore できる。
- rescore summary に新列が出る。
- details が JSON として parse できる。

## Phase 6: Acceptance confidence

目的:

acceptance success の信頼度を出し、build-only success の過大評価を抑える。

### P6-W01 scoring

対象:

- `acceptance_outcome.py`

作業:

1. `acceptance_confidence_score` を算出する。
2. cap を適用する。
   - `acceptance_success=false`: max 50
   - `plan_output_adherence_score < 70`: max 70
   - `plan_verify_coverage_score < 40`: max 75
   - `prompt_plan_capability_coverage_score < 70`: max 80
3. `acceptance_confidence_reason` を details に出す。
4. `acceptance_confidence_reason` を TSV/report に出せる形にする。
5. static oracle が一部 evidence を持つが missing capability を残す場合、`semantic_inconclusive_needs_behavior_oracle` として low confidence にする。
6. deterministic browser oracle が利用できない場合は `browser_oracle_unavailable` として report する。

受入条件:

- false positive は低 confidence。
- plan/output/verify が高い成功は高 confidence。
- confidence 低下理由が report で確認できる。
- inconclusive が hard failure と区別される。

### P6-W02 tests

対象:

- `test_acceptance_outcome.py`

作業:

1. build-only success fixture を追加する。
2. prompt-plan gap fixture を追加する。
3. strong plan / strong verify / strong output fixture を追加する。

受入条件:

- confidence が expected range に入る。
- hard gate とは分離されている。

## Phase 7: Suite update

目的:

suite に capability-aware declarative contract を追加する。

### P7-W01 suite contract audit

対象:

- `mvp-smoke.yaml`
- `mvp-blind.yaml`
- `mvp-acceptance.yaml`
- `mvp-tui-acceptance.yaml`

作業:

1. scenario ごとの category / profile / prompt を確認する。
2. expected artifacts と capability contract を分ける。
3. interactive game / web app / CLI / library / docs / data transform の representative capability を整理する。
4. scenario id 固有の hidden logic が不要な contract にする。

受入条件:

- suite 側は declarative contract のみ。
- contract が scenario id 分岐を要求しない。

### P7-W02 suite dry-run

対象:

- `eval-run.py`
- updated suites

作業:

1. `mvp-acceptance` dry-run。
2. `mvp-blind` dry-run。
3. `mvp-tui-acceptance` dry-run。

受入条件:

- dry-run が全て通る。
- suite parse error が無い。

## Phase 8: Re-evaluation and calibration

目的:

新評価方法の有効性を実測する。

### P8-W01 MVP speed-cloud eval

作業:

1. ローカル LLM 未使用で `mvp-acceptance` を実行する。
2. `minimal-loop,step-plan,plan-run,ultra-plan-run` を対象にする。
3. report を生成する。
4. false positive / false negative を分類する。

受入条件:

- run root が記録されている。
- acceptance / confidence / prompt-plan / plan-verify / plan-output の集計がある。

### P8-W02 anvildev comparison

作業:

1. 同条件で anvildev を実行する。
2. `--engine minimal` が必要な経路に注意する。
3. MVP と anvildev を compare report で比較する。

受入条件:

- MVP/anvildev の差が指標ごとに説明されている。
- provider/network failure は agent capability と分けて扱われている。

### P8-W03 calibration report

成果物:

- `workspace/mvp/eval/015-2/015_2_recalibration_results.md`

内容:

1. before / after 成功率。
2. false positive 件数。
3. confidence 分布。
4. gap kind 分布。
5. hard gate 候補と diagnostic 維持候補。

## Phase 9: Gate policy decision

目的:

どの指標を acceptance hard gate にするか決める。

### P9-W01 threshold analysis

作業:

1. Phase 8 の results から threshold 候補を出す。
2. false positive / false negative を確認する。
3. blind suite への影響を確認する。
4. mode 別の threshold 必要性を確認する。

受入条件:

- threshold が実測に基づいている。
- scenario id 固有 threshold がない。

### P9-W02 policy document

成果物:

- `workspace/mvp/eval/015-2/gate_policy_decision.md`

内容:

1. hard gate 化する指標。
2. diagnostic に留める指標。
3. threshold と根拠。
4. false negative リスク。
5. rollback 条件。

## Phase 10: Regression guard

目的:

評価方法が build-only success に戻らないようにする。

### P10-W01 regression fixtures

対象:

- `mvp/anvilminimal/eval/fixtures/acceptance_oracle`

追加:

1. game static title only。
2. game prompt rich / plan skeleton only。
3. game build-only verify。
4. game semantic smoke verify。
5. step-plan verify artifact declared but not materialized。
6. plan-run verify artifact materialized。
7. CLI entry point adversarial lexical fixture。
8. docs / library / data transform non-game fixtures。
9. static inconclusive fixture。
10. browser oracle unavailable fixture。

受入条件:

- fixtures が test から参照される。
- expected result が JSON/YAML で明示されている。

### P10-W02 final test matrix

実行:

```bash
python3 -m pytest mvp/anvilminimal/tests/eval -q
python3 -m compileall -q mvp/anvilminimal/scripts/eval_lib mvp/anvilminimal/scripts/eval-run.py mvp/anvilminimal/scripts/eval-rescore-runtime.py
```

追加 dry-run:

```bash
python3 scripts/eval-run.py \
  --suite eval/suites/mvp-acceptance.yaml \
  --model-profile speed-cloud \
  --modes step-plan,plan-run \
  --scenario nextjs-space-invaders-large \
  --runs 1 \
  --run-root /private/tmp/anvilminimal-eval-015-2-dry \
  --dry-run
```

受入条件:

- pytest が通る。
- compileall が通る。
- dry-run が通る。
- rescore が通る。

## 実装順序

推奨順:

1. Phase 0
2. Phase 1
3. Phase 2
4. Phase 3
5. Phase 4
6. Phase 5
7. Phase 6
8. Phase 7
9. Phase 8
10. Phase 9
11. Phase 10

理由:

- まず baseline を固定する。
- 次に plan capability contract を独立させる。
- verify coverage は capability contract に依存する。
- summary/report/schema は指標が確定してから広げる。
- hard gate は実測後に判断する。

## 完了判定チェックリスト

- [ ] `baseline_plan_output_acceptance_analysis.md` が作成されている。
- [ ] `plan_capability_contract.py` が追加されている。
- [ ] `plan_verify_coverage.py` が追加されている。
- [ ] `prompt_plan_capability_coverage_score` が TSV に出る。
- [ ] `plan_verify_declared_coverage_score` が TSV に出る。
- [ ] `executed_verify_coverage_score` が TSV に出る。
- [ ] `plan_verify_coverage_score` が TSV に出る。
- [ ] `acceptance_confidence_score` が TSV に出る。
- [ ] `acceptance_confidence_reason` が TSV / report に出る。
- [ ] `verify_adequacy_cap_reason` が TSV に出る。
- [ ] `Capability Contract Outcomes` が report に出る。
- [ ] `step-plan` が post-run artifact に依存しない。
- [ ] `minimal-loop` plan absent が failure にならない。
- [ ] static inconclusive が hard failure と区別される。
- [ ] build-only game verify が high confidence にならない。
- [ ] prompt rich / plan skeleton が low prompt-plan coverage になる。
- [ ] CLI `entry point` が score/progression と誤検出されない。
- [ ] `python3 -m pytest mvp/anvilminimal/tests/eval -q` が通る。
- [ ] `python3 -m compileall -q ...` が通る。
- [ ] dry-run が通る。
- [ ] rescore が通る。
- [ ] live speed-cloud eval 結果が `015_2_recalibration_results.md` に整理されている。
- [ ] `gate_policy_decision.md` が作成されている。
