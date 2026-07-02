# step runner migration miss root cause audit

## Scope

直近で判明した重大な移植不備について、なぜ移植から漏れたのか、同種の漏れが他にないかを再調査する。

対象の重大不備:

- MVP の step 実行 prompt が `overall goal` / `verify` / `expected_result` を実行モデルへ渡していない
- MVP の planner prompt が `report` step を final summary 用に誘導している

参照した移植元:

- `src/agent/minimal_step_runner.rs`
- `src/agent/minimal_step_runner/repair.rs`
- `src/agent/minimal_step_runner/verify.rs`
- `src/agent/minimal_step_runner/plan_lint.rs`
- `src/agent/minimal_step_runner/profile.rs`
- `src/agent/minimal_step_runner/profiles/*`

参照した MVP:

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/planner/repair.rs`
- `mvp/anvilminimal/src/planner/verify.rs`
- `mvp/anvilminimal/src/planner/lint.rs`
- `mvp/anvilminimal/src/planner/profile.rs`
- `mvp/anvilminimal/src/planner/profiles/*`
- `mvp/anvilminimal/tests/*`
- `workspace/mvp/eval/002`, `006`, `007`, `008`, `009`, `010`

## 結論

これは軽微な prompt tuning 漏れではなく、step runner の移植契約を取り違えた不備である。

移植元では、StepPlan は単なる YAML ではなく、次の契約を step 実行ごとに実行モデルへ渡す。

- overall goal
- required final artifacts
- current step id
- current step instruction
- expected paths after this step
- verification commands for this step
- expected verification result
- step boundary / bounded repair policy

MVP ではこの契約が `step.instruction + expected_paths` に縮退している。

さらに、移植元では `report` は explicit blocker 用であり、通常成功の final summary ではない。MVP ではこれを final summary 用として planner prompt に書いてしまっている。

## レビュー反映事項

目的達成、完了条件、テスト計画の観点で再レビューし、以下を本監査の追跡条件として追加する。

- source parity は文書上の確認ではなく、function-level matrix、prompt contract tests、eval event evidence で完了判定する
- `build_step_prompt` / `build_repair_prompt` / `build_profiled_phase_prompt` の contract 欠落は、unit test と fake client prompt history test の両方で検出する
- `report` は hard forbid しないが、通常成功の final summary として誘導しないことを planner prompt / retry prompt / quality report の全経路で確認する
- fresh workspace 判定には `PlanQualityContext` の拡張が必要であり、入力不足時は保守的に false positive を避ける
- eval 側は prompt 本文を保存せず、boolean summary の prompt contract event だけを保存する
- anvildev / 旧run のように prompt contract event が欠損する場合でも summary 生成が壊れないことをテストする

## 直接の問題箇所

### 1. step 実行 prompt 契約の欠落

移植元:

- `src/agent/minimal_step_runner.rs:382`
  - `let prompt = build_step_prompt(&plan, step);`
- `src/agent/minimal_step_runner.rs:742`
  - `build_step_prompt()` が plan と step の契約をまとめて実行モデルへ渡す

MVP:

- `mvp/anvilminimal/src/planner/runner.rs:266`
  - `run_step()` は `PlanStep` だけを受け取る
- `mvp/anvilminimal/src/planner/runner.rs:273`
  - `prompt_with_required_paths(&step.instruction, &step.expected_paths)` のみ

欠落しているもの:

- `plan.goal`
- `step.id`
- `step.verify`
- `step.expected_result`
- required final artifacts の全体文脈
- verify failure 時の step-local repair 方針

### 2. repair prompt 契約の欠落

移植元:

- `src/agent/minimal_step_runner/repair.rs`
  - overall goal、required artifacts、step instruction、expected paths、verify commands、expected_result、repair cycle、verification failures、progress warning を含める

MVP:

- `mvp/anvilminimal/src/planner/repair.rs`
  - failure、missing paths、changed files、progress warning 中心
  - plan/step の契約文脈が薄い

欠落しているもの:

- overall goal
- step instruction
- verify commands
- expected_result
- TDD red step の保護
- "verifier failure は actionable feedback" という source の修復方針

### 3. report step semantics の drift

移植元:

- `src/agent/minimal_step_runner.rs:666`
  - `report steps are for explicit blockers such as dependency_missing ... report is not success`

MVP:

- `mvp/anvilminimal/src/planner/runner.rs:960`
  - `Use report only for final summary.`

問題:

- source と意味が逆転している
- LLM が `report-completion` を末尾に置く直接要因になっている
- terminal report step は execution shape / finalization の false positive を増やす

補足:

- source は planner prompt 上で report を blocker 用として強く誘導しているが、runtime validator がすべての report step を無条件に invalid としているわけではない
- MVP の修正対象は、report を hard forbid することではなく、通常成功の final summary step として生成させない prompt/quality contract を戻すこと
- dependency_missing / unavailable external service / user input required のような明示的 blocker では report step を許容する

## なぜ漏れたか

### RC-01: 「MVP API を切り直す」と「source contract を保つ」を分離できていなかった

MVP 実装では、既存 minimal 系を丸ごと移すのではなく、新しい小さい API として再設計した。

この方針自体は、コード量と移植性を考えると理解できる。しかし、再設計してよいのは public API や型の薄さであり、source が実行モデルへ渡していた契約まで削ってよいわけではなかった。

誤った単純化:

- `run_step(plan, step)` 相当を `run_step(step)` にした
- `build_step_prompt(plan, step)` を作らず、`prompt_with_required_paths(instruction, paths)` にした
- `report` の source semantics を確認せず、一般的な「最後に報告する step」として解釈した

### RC-02: 過去の棚卸しが「停止/成果物/verify safety」中心で、prompt contract parity を独立観点にしていなかった

`workspace/mvp/eval/002` では、SG-33 として次を挙げていた。

- plan generation、step prompt、repair prompt、ultra phase prompt へ required final artifacts を継承する

しかし、この項目は「required final artifacts の継承」に限定されていた。source の `build_step_prompt()` が持つ次の契約までは分解していない。

- overall goal
- verify commands
- expected_result
- step boundary
- repair policy

つまり `step prompt` という文字列は計画にあったが、中身の parity 定義が不足していた。

### RC-03: source function 対応表はあったが、function-level traceability がなかった

対応表には次があった。

- `src/agent/minimal_step_runner.rs`
- `mvp/anvilminimal/src/planner/{runner,step_plan,ultra_plan}.rs`

しかし以下を確認する traceability はなかった。

| source function | MVP counterpart | test |
| --- | --- | --- |
| `build_step_prompt(plan, step)` | missing | missing |
| `build_repair_prompt(plan, step, report, progress, ...)` | partial | partial |
| `build_profiled_phase_prompt(...)` | partial | partial |
| `StepRunSummary` / `StepOutcome` | missing/partial | missing |

このため、ファイル単位では「対応済み」に見えたが、関数単位では欠落していた。

### RC-04: テストが plan generation と YAML parse に寄り、executor prompt 内容を検査していなかった

既存テストは次をよく検査している。

- JSON StepPlan 生成
- schema retry
- verify policy retry
- YAML parse/render compatibility
- required final artifacts が prompt に含まれること
- fake plan-run が `plan-run complete: N steps` を返すこと

不足していたテスト:

- step 実行 prompt に `overall goal` が含まれる
- step 実行 prompt に `verify` が含まれる
- step 実行 prompt に `expected_result` が含まれる
- repair prompt に step/plan 契約が含まれる
- planner system prompt で report が blocker 用として扱われる
- terminal empty report が retryable quality になる
- fake plan-run が実行モデルに渡した prompt を assert する

結果として、`run_step()` が情報不足の prompt を渡していても、fake client が `done` を返すだけならテストは通った。

### RC-05: eval の読み方が step-plan success / score に引っ張られた

Phase 008/009 では、MVP step-plan は高成功率・高 plan_quality だった。

- step-plan: 11/12 success
- plan_quality avg: 87.4
- executable avg: 84.2

このため、調査の重心が次へ寄った。

- quality self-check
- verify strength
- execution shape score
- runtime/provider policy risk

これらは必要だったが、plan-run 失敗の根にある executor prompt 契約までは掘れていなかった。

### RC-06: anvildev 側の失敗が source parity 判断を鈍らせた

anvildev は step-plan 成功率や plan_quality が MVP より低く、safe allowlist / dependency ordering の policy mismatch で落ちるケースもあった。

そのため、過去の整理では「今回の主問題は source safeguard 取りこぼしとは断定しない」とした。

この判断は、planner 生成品質については一部妥当だったが、executor prompt contract については誤っていた。source の plan generation policy がそのまま良いかと、source の step execution contract が必要かは別問題だった。

### RC-07: `report` という言葉を一般UIの意味で解釈し、source内の意味を確認しなかった

`report` は一般的には「最後に結果を報告する」と読める。

しかし source では、`report` は成功サマリではなく、dependency missing / unfixable blocker のような明示的 blocker 用だった。

MVP prompt ではこの意味を逆にしてしまったため、LLM が空の `report-completion` を自然に作るようになった。

## 他に漏れがないかの追加調査

### 調査観点

以下の観点で、source と MVP を再比較した。

1. step execution contract
2. repair contract
3. report/inspect semantics
4. ultra phase contract
5. plan validation
6. step kind fidelity
7. plan-run result observability
8. tests / eval observability

## 追加で確認した漏れ・不備候補

| ID | 判定 | 内容 | source | MVP current | リスク |
| --- | --- | --- | --- | --- | --- |
| SR-GAP-01 | confirmed | step 実行 prompt に overall goal / verify / expected_result がない | `build_step_prompt(plan, step)` | `prompt_with_required_paths(instruction, paths)` | plan-run 失敗、verify 未意識、TDD red step破壊 |
| SR-GAP-02 | confirmed | repair prompt に plan/step/verify/expected_result 文脈が薄い | `build_repair_prompt(plan, step, ...)` | `build_repair_prompt_with_context(step_id, report, context)` | repair 成功率低下、max_iterations/Read停滞 |
| SR-GAP-03 | confirmed | report step semantics が逆転 | blocker用、successではない | final summary用 | terminal empty report 増加 |
| SR-GAP-04 | confirmed | ultra phase prompt に workspace snapshot / profile runtime contract が不足 | `build_profiled_phase_prompt()` | `ultra_phase_prompt()` は goal/profile/style/intent/required中心 | phase drift、profile違反の後倒し |
| SR-GAP-05 | confirmed | plan-run step outcome summary が弱い | `StepRunSummary`, `StepOutcome`, stop_reason | 文字列 `plan-run complete: N steps` 中心 | eval/人間が step単位原因を追いにくい |
| SR-GAP-06 | partial | source validation の長さ・ID文字種・shell syntax 制約が不完全 | `validate_goal`, `validate_instruction`, `validate_step_id` | duplicate/empty/shell prefix中心 | 巨大/曖昧/不正ID plan が残る |
| SR-GAP-07 | intentional-but-risky | `create/edit/work/repair` kind を `implement` に畳む | kind を保持 | generated kind を normalize | 責任境界が薄くなり execution shape が読みにくい |
| SR-GAP-08 | prompt-quality-gap | inspect/report の wrapper step を抑える prompt/quality retry が弱い | report は blocker 用、inspect は必要時だけ | final summary report や fresh workspace inspect を誘発 | 空 wrapper step が増える |
| SR-GAP-09 | partial | verifier diagnostic excerpt / source excerpt が planner verify に不足 | source excerpt / diagnostic line抽出 | command failure reason中心 | repair prompt の具体性不足 |
| SR-GAP-10 | test-gap | fake plan-run が executor prompt 内容を検査しない | sourceに同等比較テストはないが契約が実装内にある | prompt content assertなし | 同種欠落が再発する |

## 過去計画でなぜ検出できなかったか

### `workspace/mvp/eval/002`

検出できたこと:

- SG-33 で required final artifacts の plan/step/repair/phase 継承を挙げた
- SG-19/20/21 で repair/verify の弱さを挙げた
- SG-26 で step outcome の粗さを挙げた

検出できなかったこと:

- `build_step_prompt` の full contract
- `verify` / `expected_result` を executor prompt に渡す必要性
- `report` の source semantics

理由:

- `step prompt` を required artifacts 継承の一部として扱い、full prompt parity として分解していない
- SG が安全装置単位で、関数対応単位ではない

### `workspace/mvp/eval/006`

検出できたこと:

- LLM 境界を JSON only に戻す必要
- source-style parse / retry prompt / expected_result / kind contract の必要性

検出できなかったこと:

- 生成された plan を実行する prompt contract
- `report` step の blocker semantics

理由:

- 対象が `step-plan 生成` に限定され、`plan-run 実行` の call graph まで見ていない

### `workspace/mvp/eval/008`

検出できたこと:

- valid plan の品質不足
- B/C の self-check / retry が弱い
- plan-run predictiveness が未達

検出できなかったこと:

- plan-run false positive の根に executor prompt contract 欠落があること

理由:

- `plan-run に渡す step instruction 補強` という観点はあったが、結論では「主問題は source safeguard 取りこぼしとは断定しない」として深掘りを止めた
- anvildev の低スコア・policy mismatch を見て、source parity の必要性を過小評価した

### `workspace/mvp/eval/009`

検出できたこと:

- MVP plan-run が 0/12
- execution shape / runtime health 系が重要
- terminal report step が no-tool finalization risk を上げる

検出できなかったこと:

- terminal report step が、MVP prompt の `Use report only for final summary` から直接誘発されていること
- runtime health 低下が、executor prompt 情報不足と接続していること

理由:

- 指標再設計が中心で、source function audit へ戻っていない

## 漏れ防止の再発対策

### 1. function-level source parity matrix を Phase 0 deliverable として追加する

ファイル対応ではなく、関数単位で次を持つ。

| source function | MVP function | parity status | required tests | acceptance evidence |
| --- | --- | --- | --- | --- |
| `build_step_prompt` | TBD | missing | prompt includes goal/verify/expected_result | fake client prompt history + prompt_contract_score |
| `build_repair_prompt` | partial | partial | repair prompt includes plan/step contract | repair retry prompt history |
| `build_profiled_phase_prompt` | partial | partial | phase prompt includes snapshot/runtime contract | ultra phase prompt test |
| `validate_step_id` | partial | partial | id char/length test | lint pass/fail fixtures |
| `StepRunSummary` | partial | partial | step outcome emitted | eval event and summary row |

実装着手前に、この matrix を `workspace/mvp/eval/010` 配下へ固定し、各 Phase の完了条件は matrix の該当行を更新する形にする。
これにより、ファイル単位の「見た」ではなく、source function 単位の「対応/未対応/意図的差分」を追跡する。
matrix には `implementation phase`、`test command`、`status update rule` も含める。

### 2. prompt contract snapshot tests を追加する

LLM prompt は外部挙動なので、snapshot に近いテストを持つ。

必須:

- step execution prompt
- repair prompt
- ultra phase prompt
- planner system prompt
- quality retry prompt

### 3. fake client で「受け取った prompt」を assert する

`plan-run complete` の文字列だけでは不十分。

fake client の `messages` / prompt history を検査し、以下を固定する。

- `Verification commands for this step`
- `Expected verification result`
- `Overall goal`
- `Work only on this step`
- report は blocker 用であり final summary ではない
- CLI `/plan-run`、TUI slash `/plan-run`、ultra phase 内 step execution が同じ contract を使う

eval でも同じ観点を測るため、`ANVIL_EVAL_EVENTS=1` の step event に prompt contract の boolean summary を残す。
`execution_shape_readiness_score` は YAML 形状だけを見るため、runtime prompt contract の代替指標として使わない。
prompt 本文そのものは event に保存しない。event 欠損時は scorer が crash せず、`unknown` または空欄として扱う。

### 4. eval 指標と実装ゲートを接続する

`terminal_report_step` や `wrapper_steps_without_artifacts` は、eval scorer だけでなく planner quality report / retry へ接続する。

ただし、scenario 固有名ではなく以下で判定する。

- task intent
- required final artifacts
- existing workspace snapshot
- profile contract

現行 `PlanQualityContext` は workspace snapshot / task intent を十分に持っていないため、quality gate 実装時に context を拡張する。
入力が不足する場合は fresh workspace と断定せず、inspect を誤って penalize しない。

### 5. anvildev comparison の読み方を分離する

今後は以下を分ける。

- source policy が現在の eval に合わない
- source execution contract が MVP に必要

前者があるからといって、後者を捨てない。

## 直近で修正すべき順序

0. function-level source parity matrix を作成し、各 source function の MVP counterpart/test を確定する
1. `build_step_prompt(plan, step)` を MVP に追加し、`run_step()` に `plan` を渡す
2. repair prompt を source contract に寄せる
3. planner prompt の report semantics を blocker 用に戻す
4. terminal report / empty wrapper を quality retry へ接続する
5. ultra phase prompt に workspace snapshot / profile runtime contract を戻す
6. step outcome summary / eval event を step単位に増やす
7. validation parity の残りを確認する

## 受け入れ条件案

- Unit:
  - `step_execution_prompt_includes_source_contract`
  - `repair_prompt_includes_source_contract`
  - `planner_prompt_report_is_blocker_not_success`
  - `terminal_report_step_is_retryable_quality_for_implementation_task`
  - `ultra_phase_prompt_includes_snapshot_and_runtime_contract`
  - `invalid_step_id_is_rejected`
  - `fresh_workspace_unknown_context_does_not_penalize_inspect`
  - `prompt_contract_event_does_not_store_prompt_body`
- Integration:
  - fake plan-run で execution client が受けた prompt を assert
  - fake repair で verify failure 後の prompt を assert
  - `/plan-run` TUI slash smoke が同じ prompt contract を使う
  - ultra phase execution が同じ step prompt contract を使う
- Eval:
  - speed-cloud local LLMなし `step-plan,plan-run`
  - `prompt_contract_score` または同等の event summary で runtime prompt contract を測定
  - 同条件で少なくとも2回実行し、主要指標の改善方向が逆転しないことを確認
  - MVP plan-run の failure kind 分布が悪化しない
  - terminal report 発生率が低下
  - fresh workspace inspect 発生率が低下、または残存理由が existing/fix/investigation と説明できる
  - execution_shape_readiness_score が改善
  - plan-run success が改善、または失敗時に step prompt/repair prompt contract 起因ではないことが evidence で示せる

数値 gate:

- `prompt_contract_score` は MVP run 平均 95 以上
- `artifact_ownership_score` は baseline 97.8 から 5pt 以上悪化しない
- `planner_lint_error` と `verify_command_policy_error` の合計件数は baseline より増やさない
- terminal report 発生率と fresh workspace inspect 発生率は baseline より下げる

## 現時点の判断

今回の2点は、移植不備として確定扱いにする。

さらに、同じ原因から派生した可能性が高い追加不備として SR-GAP-04〜SR-GAP-10 を追跡対象にする。

最優先は scoring の再調整ではなく、source の step execution contract を MVP に戻すこと。
