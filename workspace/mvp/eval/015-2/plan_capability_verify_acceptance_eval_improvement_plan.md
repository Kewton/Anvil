# Plan capability / verify / acceptance eval improvement plan

作成日: 2026-06-28

## 目的

`anvilminimal` の評価方法を、従来の `process success + postcheck success` から、以下の契約連鎖を機械的に検証する方式へ引き上げる。

```text
user prompt / profile
  -> YAML plan capability contract
  -> verify capability coverage
  -> final output adherence
  -> acceptance_success
```

今回の Space Invaders 受入失敗では、生成された YAML plan が Canvas / 60fps / keyboard / enemies / bullets / collision / score などを要求していたにもかかわらず、成果物は静的タイトル画面に近く、従来の build / launch / artifact existence では検出が不十分だった。

この計画では「最低限の smoke」ではなく、あるべき評価方法として、**計画が約束した機能を verify が検証し、成果物が満たしているか**を主要評価軸にする。

## 非目的

- MVP runtime / planner の改善をこの計画で直接行わない。
- 特定 scenario id や Space Invaders 専用ルールで評価を作らない。
- 人間レビューを評価フローに組み込まない。
- LLM judge を主 oracle にしない。
- eval を通しやすくするために postcheck / verify / acceptance を弱めない。
- pre-run score に実行後 success/failure を混ぜない。
- eval-only hidden contract を通常の `anvilminimal` 実行 prompt に注入しない。
- 移植元 Anvil に存在しない semantic oracle を「移植漏れ」と誤分類しない。

## 現状整理

### 実装済み

`015-1` と直近対応で以下が入っている。

- `acceptance_success`
- `acceptance_false_positive`
- `source_semantic_success`
- `source_semantic_score`
- `plan_output_adherence_success`
- `plan_output_adherence_score`
- `plan_output_failure_kind`
- `verify_adequacy_score`
- `semantic_verify_coverage_score`
- `behavior_oracle_declared_score`
- `contentless_verify_penalty`
- acceptance fixture / static semantic oracle / plan-output adherence regression tests

### 直近再スコア結果

対象:

- `/private/tmp/anvilminimal-eval-015-1-recheck-net/summary.plan-output.rescored.eval.tsv`

結果:

| 指標 | 結果 |
|---|---:|
| legacy success | 41/48 |
| acceptance success | 25/36 |
| acceptance false positive | 4 |
| plan-output 判定対象 | 7 |
| plan-output success | 6/7 |

plan-output 契約違反として検出されたケース:

| mode | scenario | main | planner | score | missing |
|---|---|---|---|---:|---|
| plan-run | nextjs-space-invaders-large | gemini:gemini-3.1-flash-lite | openai:gpt-5.4-mini | 50.0 | player_entity |

### 残る問題

1. `plan_output_adherence_score` は「成果物が plan の capability を満たしたか」を見るが、`verify` がその capability を検証しているかはまだ主指標になっていない。
2. `verify_adequacy_score` は scenario / prompt contract 由来の検証十分性を見るが、YAML plan が明示した機能要求との対応が弱い。
3. `npm run build` や `test -f` は build / existence gate として有用だが、interactive behavior の検証としては不十分である。
4. plan が高度な機能を約束しても、verify が build-only なら、実行後 false positive の温床になる。
5. acceptance report は failure kind を出せるが、`required / verified / implemented / missing` の capability 対応表がまだ不足している。
6. plan 自体が prompt の主要 capability を書き漏らした場合、plan-output adherence だけでは「弱い plan に忠実な弱い実装」を見逃す。
7. `step-plan` は成果物がまだ無いため、verify artifact の中身を読めない。plan-run 後の実ファイル検査と同じ指標として扱うと、pre-run score に post-run 情報が混ざる。
8. verify coverage をすぐ `acceptance_success` の hard gate にすると、成果物は良いが verify が弱いケースを failure とする false negative が増える可能性がある。
9. minimal-loop には YAML plan が無い場合があるため、plan capability 系指標を必須化すると通常実行の評価が不当に不利になる。
10. deterministic browser oracle は人間レビュー不要だが、実装コストと環境依存があるため、source/plan/verify oracle と gate の順序を分ける必要がある。

## レビュー結果と反映事項

| 観点 | 指摘 | 反映 |
|---|---|---|
| 目的達成 | plan-output adherence は「plan に書かれたこと」を見るだけで、plan が prompt を取りこぼした場合を検出できない。 | `prompt_plan_capability_coverage_score` と `prompt_plan_gap_kind` を追加し、prompt-derived capability と plan-derived capability の差分を評価対象にした。 |
| 指標の純度 | `step-plan` 評価で verify artifact の中身を読もうとすると、成果物が存在しないため post-run 情報が混ざりやすい。 | verify coverage を `plan_verify_declared_coverage_score` と `executed_verify_coverage_score` に分割した。 |
| false negative | verify coverage を即 hard gate にすると、実装は良いが verify が弱いケースを失敗扱いし過ぎる。 | Phase 9 までは acceptance confidence / diagnostic を主用途にし、hard gate 化は calibration 後に判断する方針を明記した。 |
| minimal-loop 互換 | YAML plan が無い minimal-loop に plan-based 指標を強制すると不公平になる。 | plan absent の場合は `not_applicable` とし、prompt/source semantic/postcheck 側で acceptance を評価する方針を追加した。 |
| 過適応防止 | 現在の Next.js game 失敗だけに寄るリスクがある。 | CLI/library/docs/data-transform と adversarial lexical fixtures を Phase 10 とテスト計画に追加した。 |
| 実装影響 | acceptance report の詳細が増えすぎると使いにくい。 | 主指標を限定し、capability 対応表は details/report に寄せる方針を維持した。 |
| 指標定義 | `plan_capability_contract_score` の算出責務が Phase 1 に明示されておらず、TSV 列だけ増えるリスクがある。 | Phase 1 に score 算出と evidence completeness の作業・受入条件を追加した。 |
| schema 整合 | `acceptance_confidence_score` はあるが、cap / reason の出力先が曖昧だった。 | `acceptance_confidence_reason` を summary/report 対象に追加した。 |
| 安全性 | verify artifact を読む処理で workspace 外パスや巨大ファイルを読めると eval が不安定になる。 | path confinement、skip dirs、最大 read bytes、binary skip を Phase 2 / リスク対策に追加した。 |
| source parity | anvildev 比較が Phase 8 だけに寄っており、評価指標の差分が実装前に整理されない。 | Phase 0 に source/anvildev eval boundary の確認を追加した。 |
| oracle 判定 | 静的 source / plan-output oracle だけで hard failure にすると false negative が残る可能性がある。 | deterministic browser oracle を `inconclusive` の確認先として扱う方針を追加した。 |

## 設計思想

### S1: 契約連鎖を分離して可視化する

評価は以下を別々に持つ。

| layer | 役割 | 代表指標 |
|---|---|---|
| Plan contract | YAML plan が何を約束したか | `plan_capability_contract_score` |
| Verify coverage | verify が plan capability を検証しているか | `plan_verify_coverage_score` |
| Output adherence | 成果物が plan capability を満たしているか | `plan_output_adherence_score` |
| Acceptance | 最終的に成功扱いできるか | `acceptance_success` |

### S2: score は増やすだけでなく関係を明確にする

主指標は増やしすぎない。

第一級指標:

- `acceptance_success`
- `prompt_plan_capability_coverage_score`
- `plan_output_adherence_score`
- `plan_verify_coverage_score`
- `verify_adequacy_score`
- `acceptance_false_positive`

補助指標:

- `plan_required_capability_count`
- `plan_verified_capability_count`
- `plan_output_missing_capability_count`
- `prompt_plan_missing_capability_count`
- `plan_verify_declared_coverage_score`
- `executed_verify_coverage_score`
- `contentless_verify_penalty`
- `prompt_plan_gap_kind`
- `plan_verify_gap_kind`
- `acceptance_confidence_reason`

### S3: scenario id 固有分岐は禁止する

許容するのは一般化した domain capability のみ。

例:

- `render_loop_or_canvas`
- `keyboard_or_player_control`
- `player_entity`
- `adversary_entity`
- `projectile_or_shooting`
- `collision_or_failure_rule`
- `score_or_progression`
- `audio_feedback`
- `visual_effects`
- `cli_entrypoint`
- `library_functionality`
- `deterministic_test`
- `docs_requested_content`
- `data_transform_contract`

### S4: build は必要条件であり十分条件ではない

`npm run build`, `cargo test`, `tsc`, `next build` は有用だが、domain behavior の検証とは分ける。

例:

| verify | build gate | semantic gate |
|---|---:|---:|
| `npm run build` | high | low |
| `test -f src/app/page.tsx` | low | none |
| `node smoke-check.js` with source assertions | medium | medium-high |
| browser interaction oracle | medium | high |

### S5: 人間レビューを避け、deterministic oracle を優先する

この計画では人間レビューを評価フローに組み込まない。hard gate は以下の deterministic oracle に限定する。

- source semantic oracle
- plan-output adherence oracle
- plan-verify coverage oracle
- deterministic postcheck
- deterministic browser / interaction oracle

### S6: prompt-plan-output の三者を分ける

評価対象は plan-output の二者関係だけではない。

| 関係 | 見るもの | 代表指標 |
|---|---|---|
| prompt -> plan | plan がユーザー要求を拾ったか | `prompt_plan_capability_coverage_score` |
| plan -> verify | verify が plan の要求を検証するか | `plan_verify_coverage_score` |
| plan -> output | 成果物が plan の要求を満たすか | `plan_output_adherence_score` |
| prompt -> output | 成果物がユーザー要求を満たすか | `source_semantic_score` / `prompt_contract_success` |

plan が弱い場合、plan-output adherence だけでは弱い実装を高く評価し得る。そのため、prompt-derived capability と plan-derived capability の差分を必ず出す。

### S7: pre-run score と post-run score を混ぜない

`step-plan` の時点では成果物も verify artifact もまだ存在しない。したがって verify coverage は以下に分ける。

| 指標 | 入力 | 用途 |
|---|---|---|
| `plan_verify_declared_coverage_score` | YAML 上の verify command / expected verify artifact 名 | step-plan / pre-run predictor |
| `executed_verify_coverage_score` | 実行後 workdir の verify artifact 内容 / postcheck events | plan-run / ultra-plan-run post-run diagnosis |

`plan_verify_coverage_score` は mode に応じて上記を束ねる表示用指標とするが、details には source を必ず残す。

### S8: plan absent を failure にしない

minimal-loop など YAML plan が無い実行では、plan capability 系指標は `not_applicable` とする。acceptance は prompt/source semantic/postcheck/behavior oracle で評価する。plan-based 指標が空であることを、失敗や低スコアとして扱わない。

### S9: static inconclusive を deterministic behavior oracle へ送る

source semantic / plan-output / plan-verify は deterministic static oracle だが、構文や表現の違いで false negative が起き得る。強い実装 evidence が一部あるが capability の一部だけが missing の場合は、直ちに human review へ送らず、`semantic_inconclusive_needs_behavior_oracle` として deterministic browser / interaction oracle の対象にする。

この計画では browser oracle を人間レビューの代替として扱う。ブラウザ環境が利用できない場合は `browser_oracle_unavailable` として confidence を下げ、成功率とは別に report する。

## 追加する主要指標

### 1. `plan_capability_contract_score`

目的:

YAML plan が prompt/profile に対して、機能契約を明示しているかを見る。

入力:

- scenario prompt
- scenario profile
- StepPlan / Ultra phase plan YAML

算出:

```text
plan_capability_contract_score =
  prompt-derived capability coverage
  + profile-derived capability coverage
  + explicit expected artifact / verify linkage
  - vague promise penalty
```

例:

- prompt が game を要求し、plan が `canvas`, `keyboard`, `enemy`, `score` を明示している: 高い
- prompt が game を要求しているが、plan が `Create Next.js app` だけ: 低い

### 2. `prompt_plan_capability_coverage_score`

目的:

plan が prompt / profile の主要 capability を拾えているかを見る。

入力:

- scenario prompt
- scenario profile
- StepPlan / Ultra phase plan YAML

算出:

```text
prompt_required = capabilities inferred from prompt/profile
plan_declared = capabilities inferred from YAML plan
prompt_plan_capability_coverage_score = 100 * intersection(prompt_required, plan_declared) / prompt_required
```

gap kind:

- `prompt_capability_missing_from_plan`
- `profile_capability_missing_from_plan`
- `plan_too_generic_for_prompt`
- `prompt_capability_not_applicable`

重要:

この指標は `plan_output_adherence_score` の前提である。plan が prompt を取りこぼしている場合、成果物が plan に忠実でもユーザー要求を満たすとは限らない。

### 3. `plan_verify_coverage_score`

目的:

plan が要求した capability を verify が検証しているかを見る。

入力:

- extracted plan capability contract
- plan `verify` commands
- verify artifact contents if referenced, such as `smoke-check.js`

算出:

```text
required = plan_required_capabilities
verified = capabilities_detected_from_verify_commands_and_verify_artifacts
plan_verify_coverage_score = 100 * verified / required
```

コマンド別の扱い:

| verify type | capability coverage |
|---|---:|
| file existence only | 0-10 |
| build / compile only | 10-25 |
| grep/source assertion | 30-60 |
| dedicated static smoke script | 60-85 |
| deterministic browser interaction | 80-100 |

重要:

`npm run build` は `render_loop_or_canvas` や `keyboard_or_player_control` を検証しない。compile gate としては加点するが、plan capability coverage には大きく加点しない。

### 4. `plan_verify_gap_kind`

目的:

verify の不足理由を診断可能にする。

候補:

- `build_only_verify_for_behavior_contract`
- `contentless_verify_for_capability_contract`
- `missing_verify_artifact`
- `verify_artifact_not_referenced`
- `semantic_capability_unverified`
- `browser_required_but_not_declared`
- `no_plan_capability_contract`
- `semantic_inconclusive_needs_behavior_oracle`
- `browser_oracle_unavailable`

### 5. `acceptance_confidence_score`

目的:

`acceptance_success=true` の信頼度を表す。

算出案:

```text
acceptance_confidence_score =
  0.25 * plan_output_adherence_score
  + 0.25 * plan_verify_coverage_score
  + 0.20 * verify_adequacy_score
  + 0.10 * prompt_plan_capability_coverage_score
  + 0.10 * build_success_score
  + 0.10 * launch_success_score
```

cap:

- `plan_output_adherence_score < 70` なら confidence は最大 70
- `plan_verify_coverage_score < 40` なら confidence は最大 75
- `prompt_plan_capability_coverage_score < 70` なら confidence は最大 80
- `acceptance_success=false` なら confidence は最大 50

## 既存指標との関係

| 指標 | 位置づけ | 修正方針 |
|---|---|---|
| `success` | legacy process/postcheck | 後方互換として残す。成功率の主指標にしない。 |
| `acceptance_success` | 最終成功判定 | 主指標にする。 |
| `prompt_plan_capability_coverage_score` | plan が prompt/profile の主要要求を拾ったか | plan が弱い場合の早期診断に使う。 |
| `plan_output_adherence_score` | 実装成果物の plan 準拠 | 主指標にする。 |
| `plan_verify_declared_coverage_score` | YAML 上の verify 宣言 coverage | step-plan / pre-run predictor に使う。 |
| `executed_verify_coverage_score` | 実行後 verify artifact / postcheck evidence coverage | plan-run / ultra-run の post-run diagnosis に使う。 |
| `verify_adequacy_score` | verify 全体の十分性 | `plan_verify_coverage_score` で cap する。 |
| `verify_strength_score` | verify command の一般的な強さ | domain capability coverage とは分ける。 |
| `semantic_verify_coverage_score` | scenario contract の検証 coverage | plan capability coverage と統合しすぎない。 |
| `plan_run_readiness_score` | plan-run しやすさ | product acceptance の代替にしない。 |
| `execution_contract_adherence_score` | 実行中の契約遵守 | plan-output / verify coverage と併読する。 |

## 対策方針

### A. Plan capability extraction を正式化する

現状:

- `plan_output_adherence.py` 内で capability 抽出している。

改善:

- `plan_capability_contract.py` として独立させる。
- prompt-derived capability と plan-derived capability を両方出す。
- `prompt_plan_capability_coverage_score` と missing capability を出す。
- StepPlan と UltraPlan の両方に対応する。
- `details` に required capability の根拠テキストを残す。

出力例:

```json
{
  "plan_required_capabilities": [
    "render_loop_or_canvas",
    "keyboard_or_player_control",
    "adversary_entity"
  ],
  "capability_sources": {
    "render_loop_or_canvas": ["instruction: HTML5 Canvas 60fps"],
    "keyboard_or_player_control": ["instruction: keyboard controls"]
  },
  "prompt_plan_missing_capabilities": ["projectile_or_shooting"]
}
```

### B. Verify capability coverage を追加する

新規:

- `mvp/anvilminimal/scripts/eval_lib/plan_verify_coverage.py`

役割:

- verify command と verify artifact を解析する。
- `smoke-check.js` や `*.test.*` を読み、capability assertion を検出する。
- build-only / contentless / semantic / browser を分類する。
- pre-run では YAML 上の宣言情報のみを見る。
- post-run では実行後 workdir の verify artifact 内容と postcheck events を見る。

追加 TSV:

- `plan_verify_declared_coverage_score`
- `executed_verify_coverage_score`
- `plan_verify_coverage_score`
- `plan_verified_capability_count`
- `plan_unverified_capability_count`
- `plan_verify_gap_kind`
- `plan_verify_oracle_version`

### C. `verify_adequacy_score` を plan coverage で cap する

方針:

```text
if plan_verify_coverage_score < 40:
  verify_adequacy_score <= 60
if plan_verify_coverage_score < 20 and plan has behavior capabilities:
  verify_adequacy_score <= 45
if prompt_plan_capability_coverage_score < 70:
  verify_adequacy_score <= 80
```

理由:

interactive game の plan で verify が build-only の場合、verify は強く見えても acceptance には弱い。

### D. Acceptance success の条件を明確化する

現在:

```text
process_success
artifact_success
build_success
launch_success
source_semantic_success
plan_output_adherence_success
postcheck_ok
```

追加方針:

```text
if plan has behavior capabilities:
  require plan_verify_coverage_score >= threshold
  or mark acceptance_confidence low
```

注意:

初期導入では `acceptance_success` を直ちに hard fail するかは段階導入にする。まず `acceptance_confidence_score` と `plan_verify_gap_kind` を出し、既存 run で false positive / false negative を確認する。

hard gate 化の暫定方針:

- `plan_output_adherence_success=false` は既存どおり acceptance failure とする。
- `plan_verify_coverage_score` は Phase 9 までは acceptance confidence と diagnostic に使う。
- `prompt_plan_capability_coverage_score` は Phase 9 までは plan quality / readiness diagnostic に使う。
- static oracle が inconclusive の場合は hard failure にせず、behavior oracle requirement / low confidence として扱う。
- hard gate 化は false negative の分析後に限定する。

現時点で hard gate として扱う deterministic oracle は、すでに導入済みの process / artifact / build / launch / source semantic / plan-output adherence / postcheck に限定する。新規の prompt-plan coverage と plan-verify coverage は Phase 9 の gate policy までは hard gate にしない。

### E. Report を required / verified / implemented にする

report に以下の表を追加する。

```text
Capability Contract Outcomes
| mode | scenario | prompt_required | plan_required | verified | implemented | missing_plan | missing_output | missing_verify |
```

目的:

- 計画の不備か。
- verify の不備か。
- 実装の不備か。
- つなぎの不備か。

を機械的に切り分ける。

### F. StepPlan score へ plan verify coverage を反映する

step-plan は成果物を作らないため `acceptance_success` は対象外。ただし、生成された YAML が「実行して成功しやすいか」を見るため、以下を step-plan score に入れる。

- `plan_capability_contract_score`
- `prompt_plan_capability_coverage_score`
- `plan_verify_declared_coverage_score`
- `plan_verify_gap_kind`

`overall_score` はむやみに増やさず、report で分けて見る。

### G. UltraPlan では phase 単位と final 単位を分ける

UltraPlan は phase ごとの contract と、最終成果物 contract を分ける。

追加:

- `ultra_phase_plan_verify_min_score`
- `ultra_phase_plan_output_min_score`
- `ultra_final_capability_adherence_score`

狙い:

- phase ごとには通っているが final app が弱いケースを検出する。
- final phase が report-only で実装不足を隠すケースを検出する。

### H. TUI 経路も同じ acceptance に接続する

TUI slash command で生成された plan / artifacts / events を同じ acceptance pipeline に流す。

必要:

- run_dir / workdir / plan_artifacts の回収。
- `acceptance_success`, `plan_output_adherence_score`, `plan_verify_coverage_score` の出力。
- TUI 自体の表示や ESC は別テストで扱う。

## Phase 計画

### Phase 0: Baseline 再固定

目的:

直近の 015-1 再スコア結果を、015-2 の before として固定する。

作業:

1. `/private/tmp/anvilminimal-eval-015-1-recheck-net/summary.plan-output.rescored.eval.tsv` を分析する。
2. `legacy_success`, `acceptance_success`, `acceptance_false_positive`, `plan_output_adherence_score` を記録する。
3. Space Invaders false positive の plan / output / missing capability を記録する。
4. 誤検出した `entry point` -> `points` 問題が修正済みであることを記録する。
5. anvildev / MVP の既存評価境界を確認し、移植元に存在する評価と 015-2 で新設する eval-only oracle を分類する。

成果物:

- `workspace/mvp/eval/015-2/baseline_plan_output_acceptance_analysis.md`

受入条件:

- before の数値が固定されている。
- false positive の分類が `semantic`, `plan-output`, `process` に分かれている。
- source parity / eval-only addition / intentional difference が分かれている。
- 以後の改善比較に使える。

### Phase 1: Plan capability contract module

目的:

plan capability extraction を `plan_output_adherence` から独立させる。

対象:

- `mvp/anvilminimal/scripts/eval_lib/plan_capability_contract.py`
- `mvp/anvilminimal/scripts/eval_lib/plan_output_adherence.py`
- `mvp/anvilminimal/tests/eval/test_plan_capability_contract.py`

作業:

1. StepPlan / UltraPlan から capability を抽出する。
2. prompt-derived capability と plan-derived capability を分ける。
3. capability source snippet を保存する。
4. `prompt_plan_capability_coverage_score` を算出する。
5. `plan_capability_contract_score` を算出する。
6. evidence completeness を算出する。
7. scenario id に依存しないことをテストする。
8. 日本語 prompt / 日本語 instruction を fixture に入れる。

受入条件:

- `canvas`, `keyboard`, `enemy`, `bullet`, `collision`, `score` を一般 capability として抽出できる。
- `entry point` を `points` と誤検出しない。
- prompt が game を要求しているのに plan が Next.js skeleton だけなら `prompt_plan_capability_coverage_score` が低い。
- plan が prompt capability を拾い、expected paths / verify と接続している場合は `plan_capability_contract_score` が高い。
- plan が機能契約を持たない場合は `not_applicable` になる。
- pytest が通る。

### Phase 2: Plan verify coverage metric

目的:

verify が plan capability を検証しているかを測る。

対象:

- `mvp/anvilminimal/scripts/eval_lib/plan_verify_coverage.py`
- `mvp/anvilminimal/scripts/eval_lib/plan_scoring.py`
- `mvp/anvilminimal/tests/eval/test_plan_verify_coverage.py`

作業:

1. verify command を分類する。
2. verify artifact を読み込む。
3. build-only / contentless / source assertion / smoke script / browser declaration を分類する。
4. capability ごとに verified / unverified を出す。
5. pre-run 用の `plan_verify_declared_coverage_score` を算出する。
6. post-run 用の `executed_verify_coverage_score` を算出する。
7. mode に応じた表示用 `plan_verify_coverage_score` を算出する。
8. verify artifact 読み込みは workspace-relative path confinement、skip dirs、最大 read bytes、binary skip を適用する。

受入条件:

- `npm run build` だけでは interactive game capability coverage が低い。
- `node smoke-check.js` が capability assertion を含む場合は coverage が上がる。
- `test -f` / `cat` / `readFileSync` だけでは coverage が上がらない。
- `step-plan` では workdir の未生成ファイルを読まない。
- workspace 外パス、巨大ファイル、binary file は安全に skip される。
- required capability と verified capability の対応表が details に出る。
- pytest が通る。

### Phase 3: Verify adequacy cap

目的:

`verify_adequacy_score` が plan capability coverage を無視して高くならないようにする。

対象:

- `mvp/anvilminimal/scripts/eval_lib/verify_adequacy.py`
- `mvp/anvilminimal/scripts/eval_lib/plan_scoring.py`
- `mvp/anvilminimal/tests/eval/test_verify_adequacy.py`

作業:

1. `plan_verify_coverage_score` を `verify_adequacy_score` の cap 入力にする。
2. build-only behavior plan の cap を追加する。
3. contentless verify の cap を追加する。
4. cap reason を `verify_adequacy_cap_reason` として出す。

受入条件:

- game capability がある plan で build-only の場合、verify adequacy が高止まりしない。
- non-interactive CLI / library では過度に penalize しない。
- cap reason が report に出る。
- pytest が通る。

### Phase 4: Summary schema and report

目的:

新指標を eval output と report に出す。

対象:

- `mvp/anvilminimal/scripts/eval_lib/run_summary.py`
- `mvp/anvilminimal/scripts/eval_lib/report.py`
- `mvp/anvilminimal/eval/plan_score_schema.yaml`
- `mvp/anvilminimal/eval/README.md`
- `mvp/anvilminimal/tests/eval/test_summary_schema.py`
- `mvp/anvilminimal/tests/eval/test_eval_event_report.py`

追加列:

- `plan_capability_contract_score`
- `plan_capability_oracle_version`
- `prompt_plan_capability_coverage_score`
- `prompt_plan_missing_capability_count`
- `plan_required_capability_count`
- `plan_verify_declared_coverage_score`
- `executed_verify_coverage_score`
- `plan_verify_coverage_score`
- `plan_verified_capability_count`
- `plan_unverified_capability_count`
- `prompt_plan_gap_kind`
- `plan_verify_gap_kind`
- `plan_verify_oracle_version`
- `verify_adequacy_cap_reason`
- `acceptance_confidence_score`
- `acceptance_confidence_reason`

受入条件:

- `summary.eval.tsv` の read/write が通る。
- compare report に新指標の avg が出る。
- acceptance report に capability contract outcome が出る。
- pytest が通る。

### Phase 5: Eval-run integration

目的:

通常 eval と rescore の両方で新指標を算出する。

対象:

- `mvp/anvilminimal/scripts/eval-run.py`
- `mvp/anvilminimal/scripts/eval-rescore-runtime.py`

作業:

1. plan score 時に capability contract / verify coverage を算出する。
2. acceptance outcome に verify coverage を渡す。
3. minimal-loop など plan absent の場合は plan-based 指標を `not_applicable` にする。
4. rescore で既存 run root から再計算できるようにする。
5. details を `extras_json` に保存する。

受入条件:

- dry-run が通る。
- 既存 run root で rescore できる。
- plan absent が failure / zero score として扱われない。
- `extras_json.acceptance_details` に required / verified / implemented / missing が残る。
- pytest が通る。

### Phase 6: Acceptance confidence

目的:

`acceptance_success=true` の信頼度を定量化する。

対象:

- `mvp/anvilminimal/scripts/eval_lib/acceptance_outcome.py`
- `mvp/anvilminimal/tests/eval/test_acceptance_outcome.py`

作業:

1. `acceptance_confidence_score` を追加する。
2. prompt-plan / plan-output / verify coverage / verify adequacy / build / launch から算出する。
3. `acceptance_confidence_reason` を追加する。
4. hard fail と low confidence を分離する。
5. 初期導入では report diagnostic とし、hard gate 化は Phase 9 後に判断する。

受入条件:

- build-only success は confidence が高くならない。
- prompt-plan coverage が低い成功は confidence が高くならない。
- plan-output と verify coverage が高い成功は confidence が高い。
- acceptance false positive は confidence が低い。
- confidence cap reason が report に出る。
- pytest が通る。

### Phase 7: Suite update

目的:

smoke / blind / acceptance suite に capability-aware contract を入れる。

対象:

- `mvp/anvilminimal/eval/suites/mvp-smoke.yaml`
- `mvp/anvilminimal/eval/suites/mvp-blind.yaml`
- `mvp/anvilminimal/eval/suites/mvp-acceptance.yaml`
- `mvp/anvilminimal/eval/suites/mvp-tui-acceptance.yaml`

作業:

1. interactive game / web app / CLI / library / docs / data transform の contract を整理する。
2. expected artifacts と capability contract を分ける。
3. postcheck を build / launch / semantic verify に分ける。
4. blind suite に同じロジックが効くことを確認する。

受入条件:

- scenario id 固有の評価ロジックが不要。
- suite 側は declarative contract のみ。
- dry-run が通る。
- pytest が通る。

### Phase 8: Re-evaluation and calibration

目的:

新指標が false positive を減らし、成功率解釈を改善するか確認する。

実行:

```bash
python3 scripts/eval-run.py \
  --suite eval/suites/mvp-acceptance.yaml \
  --model-profile speed-cloud \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --runs 1 \
  --parallel 4 \
  --run-root /private/tmp/anvilminimal-eval-015-2-acceptance
```

加えて、ローカル LLM 未使用の speed-cloud で MVP / anvildev 比較を行う。

受入条件:

- `acceptance_success` と `legacy_success` の差が説明できる。
- `prompt_plan_capability_coverage_score` が弱い plan を検出する。
- `plan_verify_coverage_score` が false positive と相関する。
- `plan_output_adherence_score` が成果物未達を検出する。
- build-only / contentless verify の false positive が低 confidence になる。
- `prompt-plan`, `plan-verify`, `plan-output`, `postcheck` のどこが問題か report で分類できる。

### Phase 9: Gate policy decision

目的:

新指標を acceptance hard gate にするか、diagnostic に留めるかを決める。

判断基準:

| 条件 | 方針 |
|---|---|
| false positive を安定して検出し、false negative が少ない | hard gate 候補 |
| false negative が残るが診断価値が高い | diagnostic |
| scenario に寄りすぎる | rollback / redesign |

受入条件:

- hard gate 化する指標としない指標が明記されている。
- threshold の根拠が run result に基づいている。
- `acceptance_success` の定義変更が README に反映されている。

### Phase 10: Regression guard

目的:

今後、評価が再び build-only success に戻らないようにする。

対象:

- acceptance fixture
- plan-output fixture
- plan-verify fixture
- report fixture

受入条件:

- 静的タイトルのみの Next.js game は acceptance false。
- plan が collision / score を要求して verify が build-only の場合、plan verify coverage が低い。
- plan が source assertion smoke-check を持つ場合、coverage が上がる。
- CLI の `entry point` を game `points` と誤検出しない。
- `python3 -m pytest mvp/anvilminimal/tests/eval -q` が通る。

## テスト計画

### Unit tests

- `test_plan_capability_contract.py`
- `test_plan_verify_coverage.py`
- `test_plan_output_adherence.py`
- `test_verify_adequacy.py`
- `test_acceptance_outcome.py`
- `test_summary_schema.py`
- `test_eval_event_report.py`
- `test_eval_rescore_runtime.py`

### Fixture tests

追加 fixture:

- game static title only
- game prompt rich / plan skeleton only
- game build-only verify
- game source semantic smoke verify
- game browser-declared verify
- step-plan verify artifact declared but not yet materialized
- plan-run verify artifact materialized and parsed
- CLI entry point
- library function with deterministic test
- docs requested content
- data transform with input/output assertion

### Dry-run tests

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

### Rescore tests

```bash
python3 scripts/eval-rescore-runtime.py \
  --run-root /private/tmp/anvilminimal-eval-015-1-recheck-net \
  --suite eval/suites/mvp-acceptance.yaml \
  --out-summary /private/tmp/anvilminimal-eval-015-1-recheck-net/summary.015-2.rescored.eval.tsv
```

### Live eval

ローカル LLM 未使用、speed-cloud:

```bash
python3 scripts/eval-run.py \
  --suite eval/suites/mvp-acceptance.yaml \
  --model-profile speed-cloud \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --runs 1 \
  --parallel 4 \
  --run-root /private/tmp/anvilminimal-eval-015-2-acceptance
```

## 完了条件

### 評価方法としての完了条件

- `plan_capability_contract_score` が plan の機能契約明示度を表す。
- `prompt_plan_capability_coverage_score` が prompt/profile 要求の plan 取り込み度を表す。
- `plan_verify_coverage_score` が verify の機能検証 coverage を表す。
- `plan_verify_declared_coverage_score` と `executed_verify_coverage_score` が pre-run / post-run で分離されている。
- `plan_output_adherence_score` が成果物の plan 準拠を表す。
- `acceptance_success` が legacy success とは別に、成果物品質を含む成功判定になる。
- `acceptance_confidence_reason` が confidence 低下の根拠を説明する。
- build-only / contentless verify が高い acceptance confidence を取らない。
- false positive が `postcheck_too_weak_for_plan_contract` / `postcheck_too_weak_for_semantic_contract` として分類される。
- YAML plan が無い実行は plan-based 指標が `not_applicable` になり、不当に失敗扱いされない。
- static oracle の不確実な failure は `semantic_inconclusive_needs_behavior_oracle` として hard failure から分離される。

### テスト完了条件

- eval unit tests が全て通る。
- dry-run が通る。
- 既存 run root の rescore が通る。
- at least one live speed-cloud eval が完了する。
- false positive / false negative の件数と根拠が report に出る。
- rescore 後の `extras_json` に capability source / missing / verified details が残る。

### 過適応防止条件

- scenario id 固有分岐がない。
- Space Invaders 固有名詞だけで判定しない。
- capability は game / web / CLI / library / docs / data-transform に汎化されている。
- blind suite で metric が破綻しない。
- `entry point` -> `points` のような lexical false positive を fixture で防ぐ。
- prompt の capability 抽出が日本語/英語どちらか一方に過度依存しない。
- thresholds は Phase 8 の実測結果に基づき、固定値の根拠を report に残す。

## リスクと対策

| リスク | 内容 | 対策 |
|---|---|---|
| lexical false positive | `entry point` を `points` と誤検出するような問題 | source snippet 付き details、fixture regression、word-boundary 厳格化 |
| lexical false negative | 座標比較の collision など、単語がなくても実装されているケースを落とす | 構造パターン検出、inconclusive 分類、browser oracle へ送る |
| score 過多 | 指標が増えて判断不能になる | 主指標を 5つに限定し、補助指標は details/report に回す |
| eval 過適応 | 現 suite だけに最適化される | blind suite / anvildev 比較 / scenario id 禁止 |
| runtime 混入 | eval-only oracle が runtime prompt に入る | eval harness に閉じる。通常 CLI/TUI 実行には注入しない |
| success 低下の誤読 | 評価厳格化で成功率が下がる | `legacy_success` と `acceptance_success` を併記し、評価改善による再分類として扱う |
| verify 強制しすぎ | 小規模タスクまで過剰な smoke-check を要求する | capability count / task category / size に応じた threshold を使う |
| prompt extraction miss | prompt-derived capability 抽出が不十分で plan の取りこぼしを見逃す | 日本語/英語 fixture、source snippet details、inconclusive 分類を追加する |
| premature hard gate | verify coverage を早く hard gate にして false negative を増やす | Phase 9 まで confidence/diagnostic に留め、実測後に gate policy を決める |
| minimal-loop disadvantage | plan が無い実行を plan-based metric で不利にする | plan absent は `not_applicable` とし、prompt/source semantic/postcheck で評価する |
| pre/post leakage | step-plan 評価に実行後 artifact 情報が混ざる | declared coverage と executed coverage を分離し、mode ごとの入力境界をテストする |
| unsafe artifact read | verify artifact parser が workspace 外や巨大ファイルを読む | workspace-relative confinement、skip dirs、read byte limit、binary skip を必須にする |
| static oracle false negative | 実装はあるが静的パターンが拾えず failure になる | inconclusive 分類と deterministic browser oracle への送付で hard failure と分離する |

## 優先順位

1. Phase 0: baseline 再固定
2. Phase 1: plan capability contract module
3. Phase 2: plan verify coverage metric
4. Phase 3: verify adequacy cap
5. Phase 4: summary/report/schema
6. Phase 5: eval-run/rescore integration
7. Phase 6: acceptance confidence
8. Phase 7: suite update
9. Phase 8: re-evaluation
10. Phase 9: gate policy
11. Phase 10: regression guard

## 成功時の評価の見方

今後、成功率を見るときは以下の順に確認する。

1. `acceptance_success`
2. `acceptance_confidence_score`
3. `prompt_plan_capability_coverage_score`
4. `plan_output_adherence_score`
5. `plan_verify_coverage_score`
6. `verify_adequacy_score`
7. `legacy_success`

解釈:

- `legacy_success=true`, `acceptance_success=false`: 従来 eval の false positive。
- `prompt_plan_capability_coverage_score` 低: plan が prompt/profile の主要要求を取りこぼしている。
- `plan_output_adherence_score` 低: 実装が plan を満たしていない。
- `plan_verify_coverage_score` 低: verify が plan の機能要求を検証していない。
- `plan_output_adherence_score` 高, `plan_verify_coverage_score` 低: 成果物は良さそうだが、検証が弱い。
- `plan_verify_coverage_score` 高, `plan_output_adherence_score` 低: verify が実行されていない、verify artifact が弱い、または oracle gap。
- `plan_verify_declared_coverage_score` 高, `executed_verify_coverage_score` 低: plan は verify artifact を意図したが、実行時に artifact が作られていない、または内容が弱い。
- `acceptance_confidence_score` 低: 成功扱いでも信頼度が低い。
