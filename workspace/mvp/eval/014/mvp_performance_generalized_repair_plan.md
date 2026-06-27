# MVP Performance Generalized Repair Plan

作成日: 2026-06-27

## 目的

MVP `anvilminimal` の実行性能を上げる。

直近 eval では MVP は anvildev より全体成功率が高い一方で、失敗は以下に集中している。

- planner 出力の schema / lint / verify policy
- ultra-plan-run の phase scaffold / phase execution / phase finalization
- plan-run 経由 step runtime の verify repair / finalization
- postcheck と実成果物の契約ずれ

この計画では、個別 scenario に合わせた分岐を追加するのではなく、既存の設計思想に沿って以下の一般化された境界を強化する。

- planner が出す plan contract
- plan/step/phase の lint と deterministic verify policy
- step runtime へ渡す execution contract
- postcheck で確認される成果物契約
- eval event による診断可能性

## 非目的

- 特定 scenario 名、prompt 断片、固定成果物名、固定ログ文言だけを条件にした修正はしない。
- verify policy を緩めて成功率を上げない。
- plan lint を弱めて invalid plan を通さない。
- provider HTTP failure を agent 能力改善として扱わない。
- eval score を直接上げるためだけの重み調整はしない。
- anvildev と完全同一の大きなアーキテクチャへ戻さない。

## 参照した直近結果

MVP live eval:

- run root: `/private/tmp/anvilminimal-eval-013-live-mvp-rerun-net`
- report: `/private/tmp/anvilminimal-eval-013-live-mvp-rerun-net/report.md`
- summary: `/private/tmp/anvilminimal-eval-013-live-mvp-rerun-net/summary.eval.tsv`

anvildev live eval:

- run root: `/private/tmp/anvilminimal-eval-013-live-anvildev-rerun-net`
- report: `/private/tmp/anvilminimal-eval-013-live-anvildev-rerun-net/report.md`
- summary: `/private/tmp/anvilminimal-eval-013-live-anvildev-rerun-net/summary.eval.tsv`

比較:

- `/private/tmp/anvilminimal-eval-013-live-rerun-compare.md`
- `workspace/mvp/eval/013/target_metric_validation_results.md`

## 現状サマリ

直近 1-run live eval:

| binary | total | step-plan | plan-run | ultra-plan-run |
|---|---:|---:|---:|---:|
| MVP | 26/36 | 11/12 | 8/12 | 7/12 |
| anvildev | 22/36 | 8/12 | 9/12 | 5/12 |

MVP は全体では anvildev を上回っているが、plan-run は anvildev より 1 件低い。

MVP failure kind:

| failure kind | count | layer | 解釈 |
|---|---:|---|---|
| `planner_schema_error` | 2 | planning | schema repair 後も valid plan へ収束しない |
| `verify_command_policy_error` | 3 | planning | verify command が deterministic policy に合わない |
| `planner_lint_error` | 1 | planning | lint repair 後も plan contract が不正 |
| `step_verify_failure` | 1 | bridge | step 実行後 verify repair が閉じない |
| `max_iterations` | 1 | runtime | plan-run step runtime が完了判断へ到達しない |
| `postcheck_failure` | 1 | postcheck | 成果物/依存/設定が postcheck 契約に合わない |
| `provider_http_status` | 1 | provider | provider/API 側失敗 |

重要な見立て:

- 10 件中 6 件が planning 系で、最優先の改善対象。
- ultra-plan-run は失敗時の `phase_completion_score` が低く、phase contract の入口で落ちている。
- plan-run の失敗は、計画品質だけでは予測しきれず、runtime/finalization/postcheck 指標と併読が必要。
- provider failure は能力改善の母集団から除外する。

## スコア分析

MVP が高い指標:

| metric | anvildev | MVP | delta |
|---|---:|---:|---:|
| success_rate | 61.1 | 72.2 | +11.1 |
| plan_quality_score_avg | 68.0 | 85.0 | +17.0 |
| executable_plan_score_avg | 63.1 | 76.5 | +13.5 |
| verify_strength_score_avg | 50.5 | 71.5 | +21.0 |
| artifact_ownership_score_avg | 31.1 | 97.8 | +66.7 |
| overall_score_avg | 56.3 | 73.0 | +16.6 |

MVP が低い指標:

| metric | anvildev | MVP | delta |
|---|---:|---:|---:|
| execution_shape_readiness_score_avg | 84.6 | 71.2 | -13.4 |
| lint_repair_score_avg | 100.0 | 79.2 | -20.8 |

MVP target metrics:

| mode | metric | success avg | failure avg | 解釈 |
|---|---|---:|---:|---|
| plan-run | `postcheck_stability_score` | 96.9 | 25.0 | postcheck failure を強く分離 |
| plan-run | `runtime_friction_score` | 65.2 | 37.0 | runtime 停滞を一定程度分離 |
| plan-run | `finalization_score` | 87.5 | 46.9 | 完了判断 failure を分離 |
| plan-run | `execution_contract_adherence_score` | 97.8 | 76.7 | bridge 契約は分離するが単独では不足 |
| ultra-plan-run | `phase_completion_score` | 100.0 | 30.0 | ultra failure を強く分離 |
| ultra-plan-run | `finalization_score` | 87.5 | 54.2 | phase 完了後の閉じ方を分離 |

改善の目的指標:

- 第一目的: capability failure を除いた成功率
- 第二目的: planning layer failure の削減
- 第三目的: `phase_completion_score`, `finalization_score`, `runtime_friction_score`, `postcheck_stability_score` の失敗分離維持
- 第四目的: anvildev 比で MVP の優位性が known suite だけでなく blind/holdout でも崩れないこと

## レビュー結果と反映

| 観点 | 指摘 | 反映 |
|---|---|---|
| 設計思想 | 計画は概ね MVP の小さい contract 境界に沿っているが、provider retry や postcheck repair を広げすぎると local-first/minimal の設計から外れる。 | provider は診断分離を主目的にし、retry 変更は明示 gate を置く。postcheck repair は bounded かつ contract-derived のみに制限する。 |
| 不安定性 | verify command repair が「安全そうな別コマンドへの置換」になると、verify 意味が変わり成功率だけ上がる。 | verify 修復は planner retry を優先し、deterministic rewrite は意味保存できる正規化か profile contract 由来の候補だけに限定する。 |
| 影響調査 | 各 Phase の対象コードは挙がっているが、CLI/TUI/slash/ultra/eval への横展開確認が不足していた。 | Phase 0/7/8/9 に影響 matrix、negative control、TUI smoke を追加し、実装前に source parity gate を通す。 |
| 他機能影響 | plan-run 改善が step-plan や minimal-loop 単体に副作用を出す可能性がある。 | Phase ごとに mode 別 regression acceptance を追加し、minimal-loop 単体成功率と step-plan YAML 品質の悪化を明示的に見る。 |
| 複雑性 | phase-local validation / repair / postcheck repair を別々に実装すると重複した mini runtime が増える。 | 既存の StepPlan parse/lint/verify/quality/retry と minimal loop feedback を共有し、新しい並列 pipeline を作らない制約を追加する。 |
| 原因深掘り | 失敗傾向は整理されているが、「なぜここまで残ったか」の根本原因が薄い。 | 根本原因仮説を追加し、source parity audit を実装前 gate に変更する。 |
| 移植漏れ | Source parity audit が Phase 7 で遅く、実装後に漏れが見つかる構造だった。 | 実施順序を `Phase 0 -> Phase 7 -> Phase 1...` に変更し、Phase 7 を gate として扱う。 |
| 不確実な方針 | provider retry と LLM API 挙動は実 API 依存で不確実。 | provider request/retry/tool-call 仕様を変える場合のみ、OpenAI/Gemini/Ollama の live smoke を事前仮説検証として必須にする。今回の計画レビューでは API 仕様変更を確定していないため live API は実行しない。 |

## 根本原因仮説

今回の失敗は単一の bug ではなく、MVP 化の過程で plan 生成、step 実行、ultra phase、postcheck の契約境界を段階的に復元してきた結果、境界間のつなぎにまだ弱い箇所が残っているものと見る。

主な根本原因:

1. source parity audit が機能単位ではなく failure 後追いで進んだため、step runner / ultra phase / repair prompt の差分発見が遅れた。
2. 初期の eval が YAML の静的品質を中心に見ており、plan-run 実行時の contract handoff や finalization の弱さを十分に捕捉できなかった。
3. planner の retry / lint / quality repair は存在するが、ultra phase-local plan と plan-run step repair で同じ contract として一貫適用する設計がまだ不十分。
4. postcheck failure を runtime の最後で検出しているが、postcheck reason を bounded repair context に戻す契約が弱い。
5. provider/API failure と agent capability failure の分離が後付けだったため、改善対象の優先順位が一時的にぶれた。

このため、以後は「個別失敗を潰す」ではなく、source parity gate と contract handoff の一般化を先に行う。

## 過適応防止ルール

実装時に守る制約:

1. scenario id を条件分岐に使わない。
2. profile 名は profile contract の一般ルールとして使い、個別 prompt 文言の一致では判定しない。
3. 固定ファイル名は plan expected_paths / eval-generated completion contract / profile contract から得られる場合だけ使う。
4. stderr の単一文言ではなく、failure kind / event kind / structured reason を使う。
5. planner の invalid output を通すために lint/verify policy を弱めない。
6. 成功率改善の前後で unit/contract/eval/blind eval を比較する。
7. score 改善だけでは完了にしない。実 run の成功率、成果物品質、failure layer の減少で確認する。

検証時に守る制約:

- `mvp-smoke.yaml` だけで合格にしない。
- blind suite と、既存 run root の post-hoc rescore の両方を使う。
- provider/API failure を除外した capability success と、除外前の raw success を両方報告する。
- 失敗が減った代わりに verify が弱くなっていないか確認する。

アーキテクチャ制約:

- StepPlan の parse/lint/verify/quality/retry は shared pipeline として扱い、ultra phase 用に別実装を作らない。
- minimal-loop 単体と plan-run step runtime の差分は、設定値・契約入力・停止条件として明示し、暗黙の prompt 差分にしない。
- postcheck repair は normal execution の代替 runtime にしない。postcheck reason による 1-2 回の bounded repair だけを許可する。
- provider retry は agent runtime の能力改善ではなく reliability policy として扱い、評価では raw success と provider-excluded success を分ける。
- TUI/CLI/slash で同じ plan-run/ultra-plan-run 実行経路を使うことを確認する。

runtime / eval 境界:

- suite `expected_artifacts` や postcheck oracle は eval harness または eval が生成した completion contract として扱い、通常 runtime に固定ロジックとして埋め込まない。
- runtime が repair に使ってよいのは、user prompt、StepPlan、completion contract、profile contract、実際の verification result から導出できる情報に限定する。
- eval-only oracle でしか分からない postcheck reason は、通常 runtime repair には渡さず、eval report の診断情報として扱う。

## 実施順序

Phase 番号は論点ごとの分類として残すが、実装順は以下を原則にする。

1. Phase 0: baseline 固定
2. Phase 7: source parity audit gate
3. Phase 1: planner schema/lint repair
4. Phase 2: verify command policy repair
5. Phase 3: ultra phase contract
6. Phase 4: step runtime repair feedback
7. Phase 5: postcheck-driven bounded repair
8. Phase 6: provider/API failure separation
9. Phase 8: eval / blind / qualitative validation
10. Phase 9: release readiness / TUI smoke

Phase 7 で採用/保留/非採用を決めていない source 差分は、Phase 1-5 の実装に入れない。

## 対象コード候補

Planner / plan execution:

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/planner/step_plan.rs`
- `mvp/anvilminimal/src/planner/lint.rs`
- `mvp/anvilminimal/src/planner/verify.rs`
- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`

Eval / diagnostics:

- `mvp/anvilminimal/scripts/eval_lib/failure_classification.py`
- `mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py`
- `mvp/anvilminimal/scripts/eval_lib/report.py`
- `mvp/anvilminimal/scripts/eval_lib/run_summary.py`
- `mvp/anvilminimal/eval/suites/*.yaml`

Tests:

- `mvp/anvilminimal/tests/eval/test_runtime_scoring.py`
- `mvp/anvilminimal/tests/eval/test_failure_classification.py`
- `mvp/anvilminimal/tests/eval/test_summary_schema.py`
- Rust unit tests in `planner/runner.rs`, `planner/lint.rs`, `planner/verify.rs`, `minimal_loop/loop_run.rs`
- integration tests under `mvp/anvilminimal/tests/`

## Phase 0: ベースライン固定と失敗再現

### 作業

1. 直近 live eval の失敗 10 件を run artifact から再確認する。
2. failure kind / layer / mode / provider / score / stop reason を固定表として記録する。
3. provider failure を capability 改善対象から除外する。
4. `mvp-smoke.yaml` 以外の blind/holdout suite 候補を確認する。
5. 以後の改善で比較する baseline report を `workspace/mvp/eval/014` に保存する。
6. 影響 matrix を作る。
   - CLI direct execution
   - TUI slash command
   - `/plan-steps`
   - `/plan-run`
   - `/ultra-plan-run`
   - minimal-loop single prompt
   - eval runner
   - anvildev comparison mode
7. negative control を定義する。
   - valid plan が repair により変更されない
   - provider failure が capability score を下げない
   - minimal-loop 単体の existing behavior が変わらない

### 受け入れ条件

- raw success と provider-excluded success を区別できる。
- 失敗 10 件の layer が planning / bridge / runtime / postcheck / provider に分類されている。
- 以後の Phase で参照する baseline summary/report が再現可能。
- 影響 matrix と negative control が `workspace/mvp/eval/014` に保存される。
- baseline には raw success、capability success、mode 別 failure layer、主要 target metric が含まれる。

### テスト

- `eval-report.py` が baseline run root を読める。
- `failure_classification.py` の既存 unit test が通る。

## Phase 1: Planner schema/lint repair の一般化

### 狙い

`planner_schema_error`, `planner_lint_error`, `verify_command_policy_error` を、lint/verify policy を弱めずに減らす。

### 作業

1. schema retry / lint retry / quality retry の retry context を統一する。
2. schema retry prompt に、直前 output の欠落フィールドと期待 shape を structured に渡す。
3. lint retry prompt に、カテゴリ別の hard constraints と修復例を入れる。
4. `last_valid_plan` が存在する場合の degraded fallback を見直す。
   - invalid latest output へ流れない。
   - valid だが quality degraded な plan は event に理由を残す。
5. retry exhaustion 時に、失敗カテゴリを schema/lint/verify policy へ正確に分類する。
6. retry context は JSON/schema/lint report など structured input を優先し、free-form 文字列だけに依存しない。
7. valid plan の再生成は避け、必要な場合も差分修正の理由を event に残す。

### 過適応防止

- 固定 scenario の goal を使った修復分岐は作らない。
- schema/lint rule は既存の `StepPlan` contract と profile contract から生成する。
- retry prompt には具体例を入れる場合も汎用例に限定する。
- quality retry で high-score になるような語句を誘導しない。契約違反の解消だけを求める。

### 受け入れ条件

- planner schema/lint/verify policy の unit test が追加される。
- retry 後に invalid plan が通過しない。
- live eval で planning layer failure が baseline より減る。
- `lint_repair_score` が改善しても `verify_strength_score` が低下しない。

### テスト

- Rust unit: missing top-level goal, numeric id, invalid expected_result, duplicate expected path, verify policy violation。
- Rust unit: last valid plan fallback が invalid latest output より優先される。
- Rust unit: valid plan は no-op で通過し、repair event が増えない。
- eval unit: planner failure kind が unclassified へ落ちない。

## Phase 2: Verify command policy repair の deterministic 化

### 狙い

verify command が強いが policy に違反して失敗するケースを、弱い verify に落とさず deterministic verify へ修復する。

### 作業

1. verify policy violation をカテゴリ化する。
   - shell control syntax
   - setup/install command
   - dev server command
   - destructive command
   - workspace 外参照
   - verify 対象 artifact 不明
2. カテゴリ別に安全な修復方針を定義する。
   - artifact existence
   - unit/build/test command
   - syntax/type check
   - postcheck delegation
3. 修復不能な場合は plan を通さず、理由を event に残す。
4. verify command の sanitizer を planner retry と plan execution の両方で同じ contract として使う。
5. sanitizer は原則として reject/diagnose を行い、rewrite は以下だけに限定する。
   - quoting/whitespace の正規化
   - workspace-relative path の正規化
   - profile contract から導出できる deterministic verify への置換
6. setup/install/dev-server は verify command へ置換せず、必要なら deferred verify / postcheck requirement として扱う。

### 過適応防止

- `README.md` や Next.js 固有ファイルなどの固定名を直接条件にしない。
- profile contract から導出できる expected artifacts だけを使う。
- unsafe command を安全に見せかける rewrite はしない。
- verify を弱い existence check へ単純退避しない。artifact existence は verify の一部に留める。

### 受け入れ条件

- `verify_command_policy_error` が baseline より減る。
- verify command が `test -f` だけに偏らない。
- postcheck がある scenario では、verify と postcheck の責務が重複しすぎない。
- verify 修復後も `verify_strength_score` と `tool_policy_compatibility_score` が同時に悪化しない。

### テスト

- Rust unit: shell control syntax を拒否または安全 repair。
- Rust unit: install/dev server を verify から除外。
- Rust unit: build/test verify は manifest/setup 前に配置されない。
- Rust unit: rewrite できない command は reject され、弱い command に silently downgrade されない。
- eval unit: `verify_strength_score` と `tool_policy_compatibility_score` が矛盾しない。

## Phase 3: Ultra phase contract の事前検証

### 狙い

ultra-plan-run の phase が実行開始後に schema/lint/policy で落ちる問題を、phase 実行前に検出し repair する。

### 作業

1. ultra phase scaffold 後に phase-local StepPlan validation を必ず通す。
2. phase-local plan に対して schema/lint/verify policy/quality check を適用する。
3. phase repair retry は global plan retry と同じ failure taxonomy を使う。
4. phase failure event に stage を必ず入れる。
   - `plan`
   - `scaffold`
   - `lint`
   - `verify_policy`
   - `execute`
   - `verify`
   - `finalize`
5. phase が失敗した時、後続 phase を進めるか中断するかの基準を明確化する。
6. phase-local validation は既存の StepPlan validation pipeline を呼び出す形にし、ultra 専用の別 validator を作らない。
7. phase prompt には source parity audit で採用した workspace snapshot / profile runtime contract / required final artifacts だけを渡す。

### 過適応防止

- ultra の profile 固有条件は profile contract と eval-generated completion contract から読み取る。
- phase 数や phase 名を特定 scenario に固定しない。
- `phase_completion_score` を上げるためだけに failed phase を success 扱いしない。
- phase を細かくしすぎて eval 上の partial completion だけを稼がない。

### 受け入れ条件

- ultra-plan-run failure で `phase_failure_stage` が必ず埋まる。
- `phase_completion_score` の低下理由が report で読める。
- ultra-plan-run の planning layer failure が baseline より減る。
- phase-local plan validation 追加後も、成功ケースの phase count や artifact ownership が悪化しない。

### テスト

- Rust unit: phase-local StepPlan の missing id/prompt/goal を検出。
- Rust unit: phase verify policy violation を phase 実行前に検出。
- Rust unit: phase-local validation が通常 StepPlan validation と同じ rule を使う。
- eval unit: phase stage breakdown が report に出る。

## Phase 4: Step runtime repair feedback の強化

### 狙い

plan-run 経由の `step_verify_failure` と `max_iterations` を減らす。

### 作業

1. step verify failure 時の repair prompt に以下を渡す。
   - overall goal
   - step id/kind/instruction
   - expected_result
   - expected_paths
   - verify command
   - verify stderr/stdout summary
   - changed paths
   - remaining artifacts
2. repair loop が同じ inspection/tool call を繰り返す場合、既存の progress feedback を step scope でも発火させる。
3. required artifacts が揃った後に finalization できない場合、bounded finalization prompt へ切り替える。
4. repair が進展しない場合、明確に `step_verify_failure` / `verify_repair_progress_unchanged` として落とす。
5. plan-run step scope と minimal-loop single prompt scope の違いを event に出す。
6. repair feedback は observation を追加するだけにし、既存会話履歴を壊さない。

### 過適応防止

- repair prompt は contract 情報を渡すだけで、特定 task の解答を埋め込まない。
- max_iterations を単純に増やして成功率を上げない。
- tool policy error を無視して retry しない。
- finalization prompt は artifact/verify/postcheck 契約が満たされた場合だけ使う。

### 受け入れ条件

- `step_verify_failure` と `max_iterations` の原因 event が report で追える。
- bounded repair の回数上限が保たれる。
- required artifacts satisfied 後の停止は finalization prompt によって回復できる。
- minimal-loop 単体 eval の成功率と stop reason 分布が悪化しない。

### テスト

- Rust unit: verify failure repair prompt に contract が含まれる。
- Rust unit: repair progress がない時に bounded に停止する。
- Rust unit: artifacts satisfied 後に finalization prompt へ移る。
- Rust unit: minimal-loop single prompt では plan-run 専用 contract が混入しない。
- eval unit: runtime friction / finalization reason が正しく出る。

## Phase 5: Postcheck-driven bounded repair

### 狙い

plan-run で postcheck failure になった時に、無制限再実行ではなく、postcheck reason に基づく限定 repair を実施する。

### 作業

1. postcheck failure reason を execution repair context に渡せるようにする。
2. repair 対象を postcheck reason から導出する。
   - dependency / lockfile / manifest
   - config
   - build/test command failure
   - dev server readiness
   - artifact missing
3. repair できる failure と、environment/provider failure を分ける。
4. repair 後に同じ postcheck を再実行し、改善がなければ停止する。
5. postcheck repair は原則 1 回、最大でも設定上限内に限定する。
6. postcheck reason が `postcheck_not_applicable`, provider, environment の場合は repair しない。
7. repair 後に postcheck の command/expected result を弱めない。
8. 実装前に postcheck reason を以下へ分類し、runtime repair に渡せるものだけ採用する。
   - runtime contract-derived
   - profile contract-derived
   - eval harness-only
   - provider/environment

### 過適応防止

- 特定 package/version 名へ固定しない。
- profile contract が要求する dependency/config だけを repair 対象にする。
- postcheck を弱めない。
- dependency を固定 version に寄せる場合は profile contract または既存 manifest の整合性に基づく。
- eval harness-only の postcheck oracle を通常 runtime logic に入れない。

### 受け入れ条件

- postcheck failure の reason が repair prompt に反映される。
- postcheck repair は bounded で、同一 failure の無限 retry がない。
- `postcheck_stability_score` の failure avg が改善する。
- postcheck repair によって passing postcheck の runtime が不必要に増えない。
- eval-only oracle が通常 runtime repair に漏れていない。

### テスト

- Rust/integration: dependency manifest mismatch の bounded repair。
- Rust/integration: config mismatch の bounded repair。
- Rust/integration: environment/provider failure では postcheck repair しない。
- eval unit: postcheck reason が repair/finalization score に反映される。

## Phase 6: Provider/API failure の分離と retry policy

### 狙い

provider failure を agent 能力 failure と混ぜない。retry policy は診断分離の次段階であり、この計画ではまず分類と report を優先する。

### 作業

1. provider HTTP 4xx/5xx/rate limit/transient network を分類する。
2. retry してよい failure と即停止すべき failure を分ける。ただし実装は classification/report を先行し、retry 変更は明示承認された場合だけ行う。
3. eval では provider-excluded success と raw success を両方出す。
4. provider failure が planner/runtime score を下げないよう確認する。
5. provider request/retry/tool-call payload を変更する場合は、変更前に OpenAI/Gemini/Ollama の live smoke または recorded fixture で仮説検証する。

### 過適応防止

- provider error を隠して success 扱いしない。
- 4xx schema/model error は retry でごまかさない。
- eval の raw success は必ず維持する。
- model 固有の一時的な HTTP 文言に依存した classification をしない。

### 受け入れ条件

- `provider_http_status` は provider layer として report に出る。
- capability score から除外した件数が見える。
- retry 変更を行う場合は bounded であり、行わない場合は classification/report のみで完了できる。
- provider request/retry/tool-call 仕様変更を行った場合、live API または recorded fixture による仮説検証結果が残る。

### テスト

- unit: provider failure classification。
- unit: capability excluded average。
- integration fixture: transient provider failure は retry 対象候補として分類される。
- optional live smoke: provider request/retry/tool-call 仕様を変更した場合のみ実施する。

## Phase 7: Source parity audit gate

### 狙い

移植元 anvil/anvildev の有効な安全装置や runtime contract を再確認し、MVP の独自簡素化で落としてはいけないものを洗い出す。

この Phase は実施順序上、Phase 1-5 の実装前に完了させる gate とする。

### 作業

1. 移植元と MVP の差分を観点別に確認する。
   - planner retry
   - verify policy
   - step execution prompt
   - repair feedback
   - finalization
   - ultra phase handoff
   - postcheck handling
   - eval event
   - provider native tool call / XML fallback
   - TUI slash command handoff
   - completion contract / required artifacts extraction
   - workspace path confinement
   - profile runtime contract
2. 差分を以下に分類する。
   - MVP 方針として意図的に捨てたもの
   - MVP に必要だが未移植のもの
   - MVP に移すと複雑性が過大なもの
3. 未移植候補は、実装前に影響範囲とテスト観点を追加する。
4. 既存 `workspace/mvp/eval/010` / `011` の source parity 文書で対応済みとした項目が、今回の失敗に再発していないか確認する。
5. 採用する差分は、MVP の小さい API へ落とす具体案を持つものに限定する。

### 過適応防止

- anvildev の大きな抽象をそのまま持ち込まない。
- MVP の小さい API と deterministic contract に落とせるものだけ採用する。

### 受け入れ条件

- parity matrix が `workspace/mvp/eval/014` に出る。
- 未移植候補が、採用/保留/非採用に分類される。
- 採用する差分には対応する test plan がある。
- Phase 1-5 の作業対象が parity matrix の採用項目に紐づく。
- 既存の移植済み項目に regression があれば、別 issue ではなく本計画内の前提 blocker として扱う。

### テスト

- parity audit 自体は文書成果物。
- 採用差分ごとに Phase 1-5 の unit/integration test に反映する。
- `rg` で source/MVP counterpart/test を対応付ける evidence を残す。

## Phase 8: Eval / blind / qualitative validation

### 狙い

known suite への過適応を避け、実改善かどうかを確認する。

### 作業

1. `mvp-smoke.yaml` で MVP/anvildev を再計測する。
2. blind suite で MVP を再計測する。
3. provider-excluded success と raw success を両方比較する。
4. score だけでなく、生成 YAML と成功成果物を人手確認する。
5. false positive / false negative を抽出する。
6. 失敗が減ったが verify が弱くなったケースがないか確認する。
7. Phase 1-5 で変更した contract ごとに regression sample を確認する。
8. raw success、provider-excluded success、capability score、qualitative review を同じ表でまとめる。

### 受け入れ条件

- MVP の capability success が baseline より改善する。
- blind suite で成功率または failure layer が悪化しない。
- `verify_strength_score`, `tool_policy_compatibility_score`, `postcheck_stability_score` が意図せず低下しない。
- plan quality と runtime success の相関が悪化しない。
- known suite で改善して blind suite で悪化した場合は、実装を成功扱いにしない。
- 成果物の定性確認で profile/prompt の主要制約違反が増えていない。

### テスト / 実行コマンド候補

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --model-profiles mvp/anvilminimal/eval/model_profiles.yaml \
  --model-profile speed-cloud \
  --modes step-plan,plan-run,ultra-plan-run \
  --runs 3 \
  --parallel 4 \
  --binary mvp/anvilminimal/target/release/anvilminimal \
  --binary-kind anvilminimal \
  --run-root /private/tmp/anvilminimal-eval-014-mvp-smoke
```

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --model-profiles mvp/anvilminimal/eval/model_profiles.yaml \
  --model-profile speed-cloud \
  --modes step-plan,plan-run,ultra-plan-run \
  --runs 3 \
  --parallel 4 \
  --binary anvildev \
  --binary-kind anvildev \
  --run-root /private/tmp/anvilminimal-eval-014-anvildev-smoke
```

blind suite は既存の blind eval 定義を確認してから同条件で実行する。

## Phase 9: Release readiness と通常利用確認

### 作業

1. `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` を通す。
2. `cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml` を通す。
3. symlink が `mvp/anvilminimal/target/release/anvilminimal` を向いていることを確認する。
4. TUI から `/ultra-plan-run` が起動し、少なくとも phase planning / phase execution の進捗が可視化されることを確認する。
5. 何も成果物を作らず終了した場合に、failure reason と eval event で診断できることを確認する。
6. `/plan-steps`, `/plan-run`, `/ultra-plan-run` が CLI/TUI で同じ contract を通ることを確認する。

### 受け入れ条件

- release build 済み binary に改善が反映されている。
- TUI 通常実行で silent exit しない。
- failure 時に user-facing error と structured event の両方が残る。
- TUI で provider/planner/runtime のどの layer で止まったか最低限分かる。

### テスト

- Rust unit/integration tests。
- eval tests。
- 手動 TUI smoke:

```bash
anvilminimal --yes --context-budget 65536 \
  --model qwen3.6:27b-coding-nvfp4 \
  --planner-model gemini-3.5-flash \
  --planner-provider gemini \
  --provider ollama
```

TUI 内:

```text
/ultra-plan-run --profile nextjs あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。
```

## 成功基準

最低基準:

- known suite で provider-excluded success が baseline より悪化しない。
- planning layer failure が baseline より減る。
- ultra-plan-run の `phase_completion_score` failure avg が改善する。
- plan-run の `finalization_score` failure avg が改善する。
- `verify_strength_score` と `tool_policy_compatibility_score` が悪化しない。

目標基準:

- MVP total capability success が `30/36` 以上。
- plan-run が `10/12` 以上。
- ultra-plan-run が `9/12` 以上。
- planning layer failure が 50% 以上減る。
- known suite と blind suite の傾向が一致する。

上記の数値目標は optimization target ではなく改善確認の目安とする。数値だけ達成しても、verify が弱くなる、postcheck が緩む、blind suite が悪化する場合は不合格とする。

品質基準:

- 成功時の成果物が postcheck だけでなく、prompt/profile の主要制約を満たす。
- generated YAML が過度に冗長化しない。
- repair loop が bounded で、停止理由が明確。
- anvildev より高い性能が、score と定性的確認の両方で説明できる。

## リスクと対策

| リスク | 影響 | 対策 |
|---|---|---|
| planner repair を強めすぎて、invalid plan を通してしまう | 実行時 failure 増加 | lint/verify policy は弱めず、repair 後も必ず再検証 |
| verify を安全側に寄せすぎて弱い verify になる | 見かけの成功率だけ上がる | `verify_strength_score`, postcheck, qualitative review で確認 |
| ultra phase repair が複雑化する | MVP の単純さを損なう | global retry と同じ contract/taxonomy を再利用 |
| postcheck repair が個別ケース化する | 評価過適応 | reason category と profile contract だけを使う |
| provider failure と capability failure が混ざる | 改善判断を誤る | raw/excluded success を併記 |
| known suite だけ改善する | 汎化しない | blind suite と成果物の人手確認を acceptance に入れる |
| source parity audit が後追いになる | 移植不備の再発 | Phase 7 を実装前 gate にする |
| postcheck repair が第二 runtime 化する | 複雑性増大、停止不能 | 1-2 回の bounded repair と同一 postcheck 再実行に限定 |
| provider retry が API 依存で不安定化する | flaky failure 増加 | classification/report 先行、retry 変更時のみ live/fixture 検証 |

## 作業具体化

具体的な編集対象、テスト名、eval コマンド、成果物パスは以下で管理する。

- `workspace/mvp/eval/014/mvp_performance_generalized_repair_work_breakdown.md`

実装時は本計画と work breakdown の両方を更新対象にし、Phase 完了条件やテスト計画に差分が出た場合は同時に反映する。
