# Step-plan improvement trend and source parity investigation

## Scope

直近の eval 結果を受けて、MVP `anvilminimal` の step-plan 改善に向けた傾向、問題箇所、移植元 `anvildev` との差分、移植漏れ/移植不備候補、対策案を整理する。

対象の直近 eval:

- MVP: `/private/tmp/anvilminimal-eval-009-mvp-step-plan-run`
- 移植元: `/private/tmp/anvilminimal-eval-009-anvildev-step-plan-run-v2`
- 移植元 corrected summary: `/private/tmp/anvilminimal-eval-009-anvildev-step-plan-run-v2/summary.eval.corrected.tsv`

## 結論

MVP の step-plan は静的な YAML 品質では移植元より高いが、plan-run が扱いやすい実行形状では移植元より低い。

直近の主要差分:

| 対象 | step-plan 成功 | plan_quality avg | executable_plan avg | verify_strength avg | artifact_ownership avg | execution_shape_readiness avg |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| MVP | 11/12 | 87.4 | 84.2 | 73.2 | 97.8 | 65.5 |
| anvildev | 8/12 | 68.2 | 64.4 | 42.8 | 36.9 | 90.4 |

MVP は「artifact を exactly once で所有する」「schema/lint を通す」「強めの verify を置く」方向は改善している。一方で、実行形状としては以下が弱い。

- 空の `inspect` step が先頭に入りやすい
- 空の `report` step が末尾に入りやすい
- verify が artifact owner step から分離され、実行モデルへの文脈が薄い
- final summary step が追加されることで、成果物作成後の finalization risk が上がる
- step 実行時に、移植元が渡している `overall goal` / `verify` / `expected_result` が MVP では実行モデルへ十分渡っていない

このため、step-plan 自体の成功率が高くても plan-run 成功率に直結していない。

## レビュー反映事項

目的達成、完了条件、テスト計画の観点で再レビューし、以下を本計画の gate として追加する。

- 目的達成は plan-run 成功率だけでなく、`prompt_contract_score`、terminal report 発生率、fresh workspace inspect 発生率、failure kind 分布で確認する
- fresh workspace 判定には、現行 `PlanQualityContext` の profile / required artifacts だけでは入力不足があるため、task intent / workspace snapshot class / seed file presence を追加する
- `execution_shape_readiness_score` は YAML 形状の指標であり、runtime prompt contract の代替にしない
- prompt contract event が欠損する anvildev / 旧run でも eval summary が壊れないことをテストする
- 各対策は unit / integration / eval schema test が追加され通過するまで完了扱いにしない
- scenario 名や固定 artifact 名に依存した条件分岐が入っていないことを fixture で確認する

## 直近 eval の傾向

MVP step-plan:

- 11/12 成功
- 失敗 1 件は `verify_command_policy_error`
- 静的スコアは高い
- `execution_shape_readiness_score` は平均 65.5 で、移植元の 90.4 より低い

MVP plan-run:

- 0/12 成功
- 主な失敗分類:
  - `tool_validation_error`
  - `verify_command_policy_error`
  - `planner_lint_error`
  - `max_iterations`
  - `tool_execution_error`
  - `postcheck_failure`

重要な傾向:

- plan-run 失敗は step-plan の YAML schema だけでは説明できない
- plan-run 失敗ケースでも `plan_quality_score` / `executable_plan_score` は高い
- 後付けの execution shape / runtime health 系の方が失敗傾向を説明しやすい

## 実 YAML で見えた問題

### MVP の典型形

`fix-js-date-helper-small` の MVP step-plan では以下の形になっていた。

- `inspect-workspace`
- `implement-date-helper`
- `add-smoke-check`
- `verify-artifacts`
- `report-completion`

問題:

- 先頭 inspect は新規/空 workspace では摩擦になりやすい
- 末尾 report は artifact も verify も持たない
- final step が verification ではなく report になる
- verify step は expected_paths を持たず、artifact owner と切り離される

`nextjs-space-invaders-large` でも同様に、`inspect-workspace` と `report-completion` が入り、実装 step に profile contract が長文で付与されていた。

### 移植元の典型形

移植元 `anvildev` の Rust CLI plan は以下のような形だった。

- `create-cargo-toml`
  - expected_paths: `Cargo.toml`
  - verify: `cat Cargo.toml`
- `create-main-rs`
  - expected_paths: `src/main.rs`
  - verify: `cargo check`
- `verify-tests`
  - expected_paths: `Cargo.toml`, `src/main.rs`
  - verify: `cargo test`

特徴:

- 先頭から artifact owner step がある
- 空の final report がない
- 各 artifact 作成 step に軽い verify がある
- 最終 step が verification で終わる

ただし移植元は duplicate expected path ownership を許しており、`artifact_ownership_score` は低い。これは MVP の設計思想である exactly-once ownership と衝突するため、単純に移植元へ戻すのではなく、verify step へ「検証対象 artifact context」を渡す実装で補うべき。

## 問題箇所

### P1. MVP の step 実行プロンプトが移植元より情報不足

MVP:

- `mvp/anvilminimal/src/planner/runner.rs:266`
- `run_step()` は `step.instruction` と `expected_paths` だけを `run_session_with_outcome_with_ui()` に渡している
- step の `verify` と `expected_result` は実行モデルへの prompt に入っていない
- `overall goal` も入っていない

移植元:

- `src/agent/minimal_step_runner.rs:382`
- `build_step_prompt()` を通して実行する
- `src/agent/minimal_step_runner.rs:742`
- prompt に以下が含まれる:
  - Overall goal
  - Current step id
  - Current step instruction
  - Expected paths after this step
  - Verification commands for this step
  - Expected verification result
  - step 境界と repair 方針

判定:

- これは移植漏れ/移植不備候補として扱うべき
- step-plan YAML の良し悪し以前に、YAML の契約が executor へ渡っていない

### P2. MVP の repair prompt も移植元より文脈不足

MVP:

- `mvp/anvilminimal/src/planner/repair.rs`
- failure、missing paths、changed files は渡す
- overall goal、step instruction、verify commands、expected_result は基本的に渡らない

移植元:

- `src/agent/minimal_step_runner/repair.rs`
- `build_repair_prompt()` は plan/step/report/progress を受け取り、overall goal、required artifacts、verify、expected_result を含める

判定:

- これも移植不備候補
- max_iterations / repair exhausted / tool misuse の回復力に影響する

### P3. `report` step の意味が移植元と MVP でズレている

MVP:

- `mvp/anvilminimal/src/planner/runner.rs:960`
- system prompt が `Use report only for final summary.` と指示している

移植元:

- `src/agent/minimal_step_runner.rs:705`
- report は explicit blocker 用であり、通常成功の final summary ではない
- plan generation prompt でも `report steps are for explicit blockers ... report is not success` と扱う

判定:

- 明確な移植不備
- MVP が terminal `report-completion` を誘発している直接要因

### P4. `inspect` step が空 workspace でも入りやすい

MVP:

- `mvp/anvilminimal/src/planner/runner.rs:956`
- inspect は expected_paths / verify を持たないルール
- その結果、空 workspace でも `inspect-workspace` が先頭に入りやすく、execution shape が下がる

移植元:

- inspect は許容されるが、実 YAML では `cat` など軽い verify を伴うケースがある
- validator も inspect verify を完全禁止していない

判定:

- 単純に inspect verify を戻す必要はない
- 新規作成タスクでは inspect を省く/短くする誘導を入れる方が設計思想に合う

### P5. create/edit/work/repair が MVP では `implement` に畳まれている

MVP:

- `mvp/anvilminimal/src/planner/step_plan.rs`
- generated kind の `work/create/edit/repair` を `implement` に正規化する

移植元:

- `StepKind` と YAML は `create` / `edit` / `work` / `repair` を保持する

判定:

- 致命的ではないが、LLM と eval の両方にとって責任境界が少し曖昧になる
- ただし種類を増やすと MVP API の複雑性が上がるため、まずは prompt/lint/runner 側で改善する

### P6. quality retry が execution shape を見ていない

MVP:

- `step_plan_quality_report()` は verify strength や profile verify は見る
- しかし `execution_shape_readiness` の構成要素、特に terminal report / empty wrapper / write-first は planner retry の条件になっていない

判定:

- eval 指標へ過剰適応するのではなく、一般的な実行契約として以下を quality issue 化すべき:
  - implementation task の final step が空 report
  - fresh workspace + required artifacts で先頭 inspect
  - verify step が prior artifacts と結びつかない
  - artifact creation step に軽い deterministic verify がない

## 根本原因

根本原因は、MVP の step-plan 改善が A/B/C のうち A に寄りすぎていること。

- A. プランを作る
  - schema、profile guidance、preferred verify は改善済み
- B. 作成したプランをチェックする
  - safety lint は強い
  - execution shape check は eval 側にあり、planner 側の自己チェックには不足
- C. チェック結果を反映する
  - lint/quality retry はあるが、execution shape の retry 条件が弱い
  - executor への契約伝達が移植元より弱いため、良い YAML でも実行に失敗しやすい

## 移植漏れ/移植不備の判定

| ID | 判定 | 内容 | 影響 |
| --- | --- | --- | --- |
| SP-GAP-01 | 移植不備 | step 実行 prompt に overall goal / verify / expected_result がない | plan-run 成功率低下、verify 未意識、finalization 失敗 |
| SP-GAP-02 | 移植不備 | repair prompt に plan/step/verify/expected_result の文脈が薄い | repair 成功率低下、max_iterations への回復不足 |
| SP-GAP-03 | 移植不備 | report step を final summary として誘導 | terminal report step 増加、execution shape 低下 |
| SP-GAP-04 | 差分あり | inspect を空 step として誘導しやすい | fresh workspace で read-before-write 摩擦 |
| SP-GAP-05 | 差分あり | create/edit/work/repair を implement に畳む | 責任境界の明瞭さ低下。ただし即時復元は不要 |
| SP-GAP-06 | 未反映 | execution shape が planner retry 条件になっていない | 高スコア YAML と plan-run 成功率の乖離 |

## 対策案

### 優先度 1: executor contract を移植元に寄せる

目的:

- YAML の契約を実行モデルへ渡す
- step-plan の静的品質を plan-run 成功率へ接続する

対応:

- MVP に `build_step_prompt(plan, step)` 相当を追加する
- `run_step()` の入力を `step` だけでなく `plan` も受け取る
- prompt に以下を含める:
  - Overall goal
  - Current step id
  - Current step instruction
  - Expected paths after this step
  - Verification commands for this step
  - Expected verification result
  - Work only on this step
  - verify が失敗した場合の bounded repair 方針

受け入れ条件:

- 既存 unit test が通る
- 新規 test で、step prompt に verify / expected_result / overall goal が含まれることを確認
- plan-run の runtime failure が少なくとも悪化しない
- CLI `/plan-run`、TUI slash `/plan-run`、ultra phase 内の step 実行が同じ prompt contract を使う
- fake client の prompt history で、実際に execution client へ渡った prompt を検査できる
- 欠落時は test failure になり、目視レビューだけに依存しない

### 優先度 2: report step の意味を source parity に戻す

目的:

- final summary 用の空 report step を減らす
- final step を verify または artifact owner に寄せる

対応:

- system prompt の `Use report only for final summary.` をやめる
- report は explicit blocker / dependency_missing / unfixable blocker 用に限定する
- normal success plan は report step で終わらせない
- lint/quality report に `terminal_report_step` を retryable quality として追加する

受け入れ条件:

- generated plan の末尾が空 report になった場合は retry される
- blocker を明示した goal では report step を許容する
- plan generation retry prompt でも report を final summary として誘導しない
- terminal report 発生率を eval artifact から集計できる

### 優先度 3: fresh workspace の inspect を抑制する

目的:

- 新規作成タスクで無駄な inspect/read-first を減らす

判定条件:

- `fresh workspace` は以下をすべて満たす場合に限定する
  - workspace snapshot に user seed/source/package/config/reference document がない
  - `.git`、agent/session 管理ディレクトリ、eval harness の空ディレクトリだけが存在する、または実質的に空
  - prompt intent が create/scaffold/new app/new docs のいずれかに分類できる
  - required final artifacts が非空
- 既存の package manifest、source file、README/spec/design document、seed input、reference asset がある場合は fresh workspace としない
- この判定は hard lint ではなく retryable quality issue として扱う

対応:

- `PlanQualityContext` に fresh workspace 判定に必要な以下の入力を追加する
  - task intent
  - workspace snapshot class
  - user seed/source/package/spec/reference file の有無
  - agent/eval metadata だけの workspace かどうか
- required artifacts があり、workspace に seed input がない場合:
  - 先頭 inspect を原則避ける
  - 先頭 step は setup/implement で artifact を所有する
- 既存ファイル修正や investigation では inspect を許容する

受け入れ条件:

- new-code / docs creation / scaffold 系では first artifact owner が index 0 または 1
- fix-code / investigation 系では inspect を過剰に禁止しない
- context 入力が不足している場合は fresh workspace と推定せず、false positive を避ける
- scenario 名を変えた fixture でも同じ判定になる

### 優先度 4: verify と artifact の結合を duplicate ownership なしで補う

目的:

- source の「verify step に expected_paths を再掲する」効果を、MVP の exactly-once ownership と両立させる

対応:

- step 実行 prompt の verify step に、prior expected artifacts を context として渡す
- quality check は以下を評価する:
  - verify command が prior artifacts を直接参照する
  - build/test のように project-level artifact を検証する
  - content assertion が対象 artifact を検証する
- verify step の expected_paths 再掲は復活させない

受け入れ条件:

- artifact_ownership_score を維持
- YAML だけを読む `execution_shape_readiness_score` では runtime prompt context を評価しない
- 代わりに `prompt_contract_score` または `ANVIL_EVAL_EVENTS` の step prompt event で、verify step に prior artifact context が渡ったことを測定できる
- plan-run で verify-only step が実行モデルに検証対象を伝えられる
- verify-only step が存在しない plan では prior artifact context 欠落を不当に減点しない

### 優先度 5: quality retry に execution shape issue を追加する

目的:

- eval 専用ではなく、一般的な「実行しやすい計画」の自己チェックを planner に戻す

追加する retryable quality 候補:

- `terminal_report_step`
- `empty_wrapper_step_for_create_task`
- `fresh_workspace_read_before_write`
- `detached_verify_without_prior_artifact_context`
- `artifact_owner_without_local_verify`

注意:

- eval scenario 名や固定 artifact 名に依存しない
- profile contract / required artifacts / workspace snapshot から判断する
- quality retry で直らない場合は、planner retry prompt の問題として failure event に残す

## テスト方針

### Unit tests

- `build_step_prompt` が source parity の情報を含む
- final report step が通常実装タスクで retryable quality になる
- blocker/report が必要なケースは許容される
- fresh workspace + required artifact で先頭 inspect が retryable quality になる
- existing file fix / investigation では inspect が許容される
- verify step が prior artifacts context を受け取れる
- prompt contract event は prompt 本文を保存せず boolean summary だけを保存する
- anvildev / 旧run の prompt contract event 欠損でも scorer が crash しない
- scenario 名を変更した fixture でも quality issue 判定が変わらない

### Eval

最低限:

- MVP step-plan cloud, local LLM なし
- MVP plan-run cloud, local LLM なし
- step-plan と plan-run を同じ scenario/model matrix で比較
- `ANVIL_EVAL_EVENTS=1` で step prompt contract event を保存する
- `prompt_contract_score` を追加し、runtime prompt に以下が入ったかを測る
  - overall goal
  - current step id
  - step instruction
  - expected paths
  - verification commands
  - expected verification result
  - prior artifact context for verify-only step
- 同条件で少なくとも2回実行し、主要指標の改善方向が逆転しないことを確認する

見る指標:

- step-plan success
- plan-run success
- execution_shape_readiness_score
- plan_run_predictive_score
- runtime_friction_score
- artifact_progress_score
- finalization_score
- tool_policy_compatibility_score
- prompt_contract_score

期待:

- `execution_shape_readiness_score` が anvildev に近づく
- `prompt_contract_score` が source parity を満たす
- terminal report / empty wrapper の発生率が下がる
- plan-run 成功率が上がる
- verify_command_policy_error / planner_lint_error が増えない
- step-plan success / plan_quality / executable_plan / artifact_ownership が大きく悪化しない

数値 gate:

- `prompt_contract_score` は MVP run 平均 95 以上
- `artifact_ownership_score` は baseline 97.8 から 5pt 以上悪化しない
- `planner_lint_error` と `verify_command_policy_error` の合計件数は baseline より増やさない
- terminal report 発生率と fresh workspace inspect 発生率は baseline より下げる

## 過剰適応リスク

リスク:

- eval の 6 scenario にだけ合わせた prompt になる
- final report を完全禁止して investigation/reporting task を壊す
- inspect を禁止しすぎて既存コード修正の精度が落ちる
- source の duplicate expected_paths をそのまま戻して ownership 設計を壊す

回避:

- scenario 固有名ではなく task intent / required artifacts / workspace seed の一般条件で判定する
- report は blocker 用として残す
- inspect は existing/fix/investigation では残す
- verify target context は prompt 側で補い、schema の expected_paths duplicate は戻さない

## 次アクション

1. SP-GAP-01/02 を先に直す
   - step/repair prompt の source parity
2. SP-GAP-03 を直す
   - report semantics を source parity に戻す
3. SP-GAP-04/06 を quality retry に入れる
   - fresh workspace / terminal report / empty wrapper
4. eval を再実行して、step-plan score と plan-run success の相関を見る

この順番が妥当な理由:

- YAML scoring をいじる前に、実行モデルへ渡す契約の移植不備を直す必要がある
- report/inspect の prompt 修正は低コストで、過剰適応になりにくい
- execution shape quality retry は効果が大きいが、意図判定を誤ると副作用があるため、source parity 修正後に入れる
