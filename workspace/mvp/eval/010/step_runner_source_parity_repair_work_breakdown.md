# Step runner source parity repair work breakdown

## Scope

`step_runner_migration_miss_root_cause_audit.md` と `step_plan_trend_source_parity_countermeasures.md` に基づき、MVP `anvilminimal` の step-plan / plan-run を移植元 `anvildev` の step runner contract に寄せる作業を具体化する。

主目的:

- StepPlan YAML の契約を実行モデルへ渡す
- `report` を通常成功の final summary ではなく blocker 用へ戻す
- fresh workspace の空 `inspect` / terminal `report` を retryable quality issue として扱う
- runtime prompt contract を eval で測定可能にする
- plan-run 成功率と step-plan 指標の相関を高める

非目的:

- source の duplicate expected_paths ownership をそのまま復活させない
- `create` / `edit` / `work` / `repair` kind を即時全面復元しない
- eval scenario 名や固定 artifact 名に依存した分岐を入れない
- `report` / `inspect` を hard forbid しない

## Phase Overview

| Phase | 目的 | 主対象 |
| --- | --- | --- |
| Phase 0 | source parity matrix と baseline 固定 | docs / tests |
| Phase 1 | step execution prompt contract 復元 | `planner/runner.rs` |
| Phase 2 | repair prompt contract 復元 | `planner/repair.rs`, `planner/runner.rs` |
| Phase 3 | report semantics を blocker 用へ修正 | `planner/runner.rs`, `planner/lint.rs` |
| Phase 4 | fresh workspace / wrapper step quality retry | `planner/lint.rs`, intent/context |
| Phase 5 | verify-only step へ prior artifact context を渡す | `planner/runner.rs` |
| Phase 6 | ultra phase prompt の source parity 補強 | `planner/runner.rs`, profile |
| Phase 7 | step outcome / eval event / prompt score 追加 | Rust eval event / Python eval |
| Phase 8 | validation parity と diagnostics 補強 | `planner/lint.rs`, `planner/verify.rs` |
| Phase 9 | 統合 eval と regression 判定 | eval scripts / docs |

## Review Findings Reflected In This Breakdown

今回のレビューで、以下の不足を完了条件とテスト計画へ反映した。

- 目的達成の判定が plan-run 成功率だけに寄ると不安定なため、`prompt_contract_score`、terminal report 発生率、fresh workspace inspect 発生率、failure kind 分布も gate にする
- fresh workspace 判定は現行 `PlanQualityContext` だけでは入力が足りないため、workspace snapshot class / task intent / seed file presence を context に追加する作業を Phase 4 に明示する
- prompt contract の修正は CLI の `plan-run` だけでなく、TUI slash、ultra phase run、repair retry 経路にも適用されたことを検証する
- `prompt_contract_score` は anvildev や旧イベントでは欠損し得るため、scorer は missing event を crash させず、比較時は `unknown` として扱う
- 完了条件は「実装した」ではなく、対応する unit / integration / eval schema tests が追加され通過することを必須にする
- eval への過剰適応を避けるため、scenario 名を含む fixture で条件分岐していないことをテストまたはレビューで確認する

## Phase 0: Source Parity Matrix And Baseline

### 目的

実装前に、source function と MVP counterpart と必要テストを固定し、ファイル単位の「確認済み」ではなく function 単位の traceability を残す。

### 作業

- `workspace/mvp/eval/010/step_runner_source_parity_matrix.md` を新規作成する
- 以下の source function について、MVP counterpart、parity status、差分理由、required tests を記載する
  - `src/agent/minimal_step_runner.rs::run_plan`
  - `src/agent/minimal_step_runner.rs::build_step_prompt`
  - `src/agent/minimal_step_runner/repair.rs::build_repair_prompt`
  - `src/agent/minimal_step_runner/profile.rs::build_profiled_phase_prompt`
  - `src/agent/minimal_step_runner.rs::validate_step_id`
  - `src/agent/minimal_step_runner.rs::validate_step_plan`
  - `src/agent/minimal_step_runner.rs::StepRunSummary`
  - `src/agent/minimal_step_runner.rs::StepOutcome`
- 直近 baseline を matrix に記録する
  - MVP: `/private/tmp/anvilminimal-eval-009-mvp-step-plan-run`
  - anvildev: `/private/tmp/anvilminimal-eval-009-anvildev-step-plan-run-v2`

### 対象ファイル

- 追加: `workspace/mvp/eval/010/step_runner_source_parity_matrix.md`
- 参照: `src/agent/minimal_step_runner.rs`
- 参照: `src/agent/minimal_step_runner/repair.rs`
- 参照: `src/agent/minimal_step_runner/profile.rs`
- 参照: `mvp/anvilminimal/src/planner/runner.rs`
- 参照: `mvp/anvilminimal/src/planner/repair.rs`
- 参照: `mvp/anvilminimal/src/planner/lint.rs`

### 受け入れ条件

- matrix に source function、MVP counterpart、status、required tests が全行入っている
- SR-GAP-01〜SR-GAP-10 が matrix のいずれかの行に紐づく
- 「意図的差分」と「移植不備」が分離されている
- 各行に `implementation phase`、`acceptance evidence`、`test command` が入っている
- Phase 1〜8 の完了時に matrix の status を更新する運用が明記されている

### テスト

- ドキュメント作成のみ
- 実装フェーズ前に `rg -n "build_step_prompt|build_repair_prompt|build_profiled_phase_prompt|validate_step_id|StepRunSummary" src/agent/minimal_step_runner.rs src/agent/minimal_step_runner mvp/anvilminimal/src/planner` で参照箇所を確認する
- matrix 内の source function 名が実コードに存在することを `rg` 結果で確認する

## Phase 1: Step Execution Prompt Contract

### 目的

MVP の `run_step()` が `step.instruction + expected_paths` だけを渡している不備を修正し、source の `build_step_prompt(plan, step)` 相当を復元する。

### 作業

- `mvp/anvilminimal/src/planner/runner.rs` に `build_step_prompt(plan: &StepPlan, step: &PlanStep, context: &StepPromptContext) -> String` 相当を追加する
- `run_step_plan_with_ui()` から `run_step()` へ `&StepPlan` または step prompt context を渡す
- `run_step()` は `prompt_with_required_paths()` ではなく `build_step_prompt()` の結果を `run_session_with_outcome_with_ui()` へ渡す
- prompt に以下を必ず含める
  - overall goal
  - required final artifacts
  - current step id
  - current step instruction
  - expected paths after this step
  - verification commands for this step
  - expected verification result
  - work only on this step
  - bounded repair policy
- `prompt_with_required_paths()` は minimal-loop 用の互換 helper として残すか、step runner 以外の用途に限定する

### 対象ファイル

- 変更: `mvp/anvilminimal/src/planner/runner.rs`
- 変更候補: `mvp/anvilminimal/src/planner/step_plan.rs`
- 追加/変更: `mvp/anvilminimal/tests/safety_parity_traceability.rs`

### 受け入れ条件

- `run_step()` が plan goal / step verify / expected_result を含む prompt を実行 client へ渡す
- verify が空の step では verify セクションを空欄ではなく「no step-local verify」相当に明示する
- expected_result が空の場合は source と同様に default expected result を補う
- prompt に eval scenario 固有名の分岐がない
- CLI `/plan-run`、TUI slash `/plan-run`、ultra phase 内の step 実行が同じ prompt contract を使う、または差分理由が matrix に記録されている
- prompt contract の欠落は unit test failure として検出される

### テスト

- Rust unit:
  - `step_execution_prompt_includes_overall_goal`
  - `step_execution_prompt_includes_step_id_instruction`
  - `step_execution_prompt_includes_verify_and_expected_result`
  - `step_execution_prompt_includes_required_final_artifacts`
  - `verify_empty_step_prompt_is_explicit_not_missing`
- Integration:
  - fake client の受信 prompt history を assert する
  - `/plan-run` の非 TUI 経路と TUI slash 経路が同じ prompt builder を通ることを assert する
  - ultra phase runner が step execution prompt contract を落とさないことを assert する

### 実行コマンド

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::runner
cargo test --manifest-path mvp/anvilminimal/Cargo.toml --test safety_parity_traceability
```

## Phase 2: Repair Prompt Contract

### 目的

verify failure / artifact missing / tool misuse 後の repair prompt に、source と同等の plan/step/verify/expected_result 文脈を渡す。

### 作業

- `mvp/anvilminimal/src/planner/repair.rs` の `RepairContext` に以下を追加する
  - overall goal
  - required final artifacts
  - current step instruction
  - expected paths after this step
  - verification commands
  - expected verification result
  - repair cycle index / max repair cycles
  - progress warning
- `mvp/anvilminimal/src/planner/runner.rs` の repair 呼び出しで `RepairContext` を plan/step から構成する
- `prompt_with_required_paths()` の二重適用を避け、repair prompt builder 内で expected paths を一元的に出力する
- TDD red step 保護を明文化する
  - red step では test failure が成果であり、実装へ進ませない

### 対象ファイル

- 変更: `mvp/anvilminimal/src/planner/repair.rs`
- 変更: `mvp/anvilminimal/src/planner/runner.rs`
- 追加/変更: `mvp/anvilminimal/tests/safety_parity_traceability.rs`

### 受け入れ条件

- repair prompt に overall goal、step instruction、verify commands、expected_result が含まれる
- verifier failure が actionable feedback として prompt に入る
- repair prompt が expected paths を重複して不自然に出さない
- TDD red step の expected failure を破壊しない
- repair retry の cycle index / max cycle が prompt に入り、無制限修復に見えない
- repair prompt の欠落は unit test failure として検出される

### テスト

- Rust unit:
  - `repair_prompt_includes_source_contract`
  - `repair_prompt_includes_verification_failure_feedback`
  - `repair_prompt_preserves_tdd_red_step`
  - `repair_prompt_does_not_duplicate_required_paths_block`
- Integration:
  - fake plan-run で verify failure 後の repair prompt を assert する
  - max repair cycle 到達時に repair exhausted report が step/verify 文脈を保持することを assert する

### 実行コマンド

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::repair
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::runner
```

## Phase 3: Report Semantics

### 目的

MVP planner prompt の `Use report only for final summary.` を撤廃し、source と同じく report は explicit blocker 用であり success ではない、という意味に戻す。

### 作業

- `mvp/anvilminimal/src/planner/runner.rs::plan_generation_system_prompt()` を修正する
  - report は dependency_missing / unavailable external service / user input required / unfixable blocker 用
  - normal success plan は verify または artifact owner step で終える
  - report is not success を明記する
- `mvp/anvilminimal/src/planner/lint.rs::step_plan_quality_report()` に retryable quality issue を追加する
  - `terminal_report_step`
  - `empty_terminal_report_step`
- blocker intent が明示されている場合は quality issue にしない
- hard lint ではなく quality retry とする

### 対象ファイル

- 変更: `mvp/anvilminimal/src/planner/runner.rs`
- 変更: `mvp/anvilminimal/src/planner/lint.rs`
- 追加/変更: `mvp/anvilminimal/tests/eval/test_plan_quality_report.py`
- 追加/変更: Rust planner lint tests

### 受け入れ条件

- planner system prompt に final summary 用 report の誘導がない
- terminal report は通常実装タスクで retryable quality issue になる
- dependency missing / blocker を目的に含む plan では report が許容される
- report を hard forbid しない
- plan generation retry prompt でも report を final summary として誘導しない
- terminal report 発生率を eval artifact から集計できる

### テスト

- Rust unit:
  - `planner_prompt_report_is_blocker_not_success`
  - `terminal_report_step_is_retryable_quality_for_implementation_task`
  - `blocker_report_step_is_allowed`
- Python eval scoring:
  - terminal report の shape penalty は維持する
  - blocker report fixture は false positive にしない
  - terminal report 発生率を summary/report で確認できる

### 実行コマンド

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::lint
python3 -m unittest mvp.anvilminimal.tests.eval.test_plan_quality_report
```

## Phase 4: Fresh Workspace And Wrapper Step Quality Retry

### 目的

新規作成タスクで、先頭の空 `inspect` や artifact/verify を持たない wrapper step を減らす。

### 作業

- fresh workspace 判定 helper を追加する
  - workspace snapshot に package/source/spec/seed/reference asset がない
  - prompt intent が create/scaffold/new app/new docs
  - required final artifacts が非空
- `PlanQualityContext` に fresh workspace 判定に必要な入力を追加する
  - task intent
  - workspace snapshot class
  - workspace has user seed/source/package/spec/reference files
  - workspace has only agent/eval metadata
- `step_plan_quality_report()` に retryable quality issue を追加する
  - `fresh_workspace_read_before_write`
  - `empty_wrapper_step_for_create_task`
  - `artifact_owner_without_local_verify`
- existing file fix / investigation / refactor では inspect を許容する
- quality retry prompt に、次回 plan でどう直すべきかを一般条件で伝える

### 対象ファイル

- 変更: `mvp/anvilminimal/src/planner/lint.rs`
- 変更候補: `mvp/anvilminimal/src/planner/intent.rs`
- 変更候補: `mvp/anvilminimal/src/planner/runner.rs`
- 追加/変更: Rust lint tests
- 追加/変更: `mvp/anvilminimal/tests/eval/test_plan_scoring.py`

### 受け入れ条件

- fresh workspace の定義がコード上で閉じている
- `PlanQualityContext` の入力が不足している場合は fresh workspace と推定せず、false positive を避ける
- new-code / scaffold task では先頭空 inspect が retryable quality issue になる
- fix-code / investigation task では inspect が許容される
- artifact owner step に軽い deterministic verify がない場合、quality issue になる
- scenario 名に依存しない
- scenario 名を変えた fixture でも同じ判定になる

### テスト

- Rust unit:
  - `fresh_workspace_inspect_is_retryable_quality_issue`
  - `existing_workspace_inspect_is_allowed`
  - `artifact_owner_without_local_verify_is_quality_issue`
  - `fresh_workspace_detection_ignores_agent_metadata_only`
  - `fresh_workspace_unknown_context_does_not_penalize_inspect`
  - `fresh_workspace_quality_is_not_scenario_name_based`
- Python eval scoring:
  - wrapper step penalty と quality issue の整合性 fixture

### 実行コマンド

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::lint
python3 -m unittest mvp.anvilminimal.tests.eval.test_plan_scoring
```

## Phase 5: Verify Step Prior Artifact Context

### 目的

MVP の exactly-once artifact ownership を維持したまま、verify-only step に検証対象 artifact context を渡す。

### 作業

- `StepPromptContext` に prior expected artifacts を追加する
- `run_step_plan_with_ui()` で step index ごとに、過去 step の expected_paths を累積する
- verify-only step の prompt に以下を含める
  - artifacts produced by previous steps
  - verify commands should validate these artifacts
  - do not claim success unless verify command passes or blocker is explicit
- expected_paths の duplicate ownership は復活させない
- eval event に prior artifact context の有無を保存する

### 対象ファイル

- 変更: `mvp/anvilminimal/src/planner/runner.rs`
- 変更: `mvp/anvilminimal/src/eval_events.rs`
- 追加/変更: `mvp/anvilminimal/tests/eval/test_eval_event_report.py`

### 受け入れ条件

- verify-only step の prompt に prior artifact context が入る
- artifact_ownership_score が低下しない設計である
- duplicate expected_paths を誘発しない
- prompt_contract_score で prior artifact context を測定できる
- verify-only step がない plan では prior artifact context 欠落を不当に減点しない

### テスト

- Rust unit:
  - `verify_step_prompt_includes_prior_artifact_context`
  - `prior_artifact_context_does_not_duplicate_expected_paths`
  - `plan_run_accumulates_prior_artifacts_by_step_order`
  - `prompt_contract_score_does_not_penalize_absent_verify_only_step`
- Python:
  - eval event fixture で prior artifact context の boolean を読み取れる

### 実行コマンド

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::runner
python3 -m unittest mvp.anvilminimal.tests.eval.test_eval_event_report
```

## Phase 6: Ultra Phase Prompt Contract

### 目的

`ultra_phase_prompt()` に source の `build_profiled_phase_prompt()` が持つ workspace snapshot / profile runtime contract を戻し、phase drift を減らす。

### 作業

- `mvp/anvilminimal/src/planner/runner.rs::ultra_phase_prompt()` を補強する
  - workspace snapshot
  - profile name
  - profile required files
  - profile verify commands
  - dev server / port / readiness contract
  - phase id / phase prompt / phase required paths
- source と違う箇所は parity matrix に「意図的差分」として記録する
- ultra prompt が肥大化しすぎる場合は、profile contract を deterministic compact format にする

### 対象ファイル

- 変更: `mvp/anvilminimal/src/planner/runner.rs`
- 変更候補: `mvp/anvilminimal/src/planner/profile.rs`
- 変更候補: `mvp/anvilminimal/src/planner/profiles/nextjs.rs`
- 追加/変更: `mvp/anvilminimal/tests/safety_parity_traceability.rs`

### 受け入れ条件

- ultra phase prompt に workspace snapshot と profile runtime contract が含まれる
- existing required final artifacts の継承テストが維持される
- phase prompt が source parity matrix の SR-GAP-04 を close できる
- prompt 長が過度に肥大化しないよう、snapshot / profile contract は deterministic compact format で出力される
- ultra plan run の phase scaffold と phase execution の両方で contract が維持される

### テスト

- Rust unit:
  - `ultra_phase_prompt_includes_snapshot_and_runtime_contract`
  - `ultra_phase_prompt_includes_profile_required_files`
  - `ultra_phase_prompt_preserves_required_final_artifacts`
  - `ultra_phase_prompt_uses_compact_workspace_snapshot`

### 実行コマンド

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::runner
cargo test --manifest-path mvp/anvilminimal/Cargo.toml --test safety_parity_traceability
```

## Phase 7: Step Outcome Events And Prompt Contract Score

### 目的

`execution_shape_readiness_score` では測れない runtime prompt contract を eval で測定可能にする。

### 作業

- Rust 側で step prompt contract event を保存する
  - step id
  - step kind
  - has_overall_goal
  - has_required_final_artifacts
  - has_expected_paths
  - has_verify_commands
  - has_expected_result
  - has_prior_artifact_context
  - has_bounded_repair_policy
  - prompt_contract_version
- `mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py` に `prompt_contract_score` を追加する
- `mvp/anvilminimal/scripts/eval-run.py` と summary に `prompt_contract_score` を追加する
- report に score avg を出す
- TSV / JSON schema test を更新する

### 対象ファイル

- 変更: `mvp/anvilminimal/src/eval_events.rs`
- 変更: `mvp/anvilminimal/src/planner/runner.rs`
- 変更: `mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py`
- 変更: `mvp/anvilminimal/scripts/eval_lib/run_summary.py`
- 変更: `mvp/anvilminimal/scripts/eval_lib/report.py`
- 変更: `mvp/anvilminimal/scripts/eval-run.py`
- 追加/変更: `mvp/anvilminimal/tests/eval/test_runtime_scoring.py`
- 追加/変更: `mvp/anvilminimal/tests/eval/test_summary_schema.py`
- 追加/変更: `mvp/anvilminimal/tests/eval/test_eval_event_report.py`

### 受け入れ条件

- `ANVIL_EVAL_EVENTS=1` で prompt contract event が保存される
- `summary.eval.tsv` に `prompt_contract_score` が出る
- `summary.eval.report.md` に prompt contract avg が出る
- prompt_contract_score は YAML 静的 score ではなく runtime event 由来である
- prompt 本文そのものは保存せず、boolean summary のみ保存する
- prompt contract event が欠損している anvildev / 旧run では scorer が crash せず `unknown` または空欄として扱う
- prompt contract が false の場合は、plan-run failure の原因候補として report に出る

### テスト

- Python:
  - prompt contract event fixture scoring
  - summary schema に新 column がある
  - event redaction が prompt 本文を保存しない
  - missing prompt contract event でも anvildev summary が生成できる
  - false prompt contract event が report の原因候補に出る
- Rust:
  - eval event payload が expected keys を持つ

### 実行コマンド

```bash
python3 -m unittest mvp.anvilminimal.tests.eval.test_runtime_scoring
python3 -m unittest mvp.anvilminimal.tests.eval.test_summary_schema
python3 -m unittest mvp.anvilminimal.tests.eval.test_eval_event_report
cargo test --manifest-path mvp/anvilminimal/Cargo.toml eval_events
```

## Phase 8: Validation Parity And Diagnostics

### 目的

source validation と verifier diagnostic の差分を、MVP の API を肥大化させずに補う。

### 作業

- `mvp/anvilminimal/src/planner/lint.rs` に source 相当の軽量 validation を追加する
  - step id 文字種/長さ
  - goal 長さ
  - instruction 長さ
  - obviously empty / placeholder instruction
- `mvp/anvilminimal/src/planner/verify.rs` の failure diagnostic を補強する
  - failed command
  - exit status
  - stderr excerpt
  - actionable next hint
- repair prompt に diagnostic excerpt を渡す
- shell control syntax policy との整合性を維持する

### 対象ファイル

- 変更: `mvp/anvilminimal/src/planner/lint.rs`
- 変更: `mvp/anvilminimal/src/planner/verify.rs`
- 変更: `mvp/anvilminimal/src/planner/repair.rs`
- 追加/変更: Rust lint/verify tests

### 受け入れ条件

- invalid step id が lint/quality で検出される
- 巨大/曖昧/placeholder plan が通りにくくなる
- verifier failure が repair prompt で actionable になる
- valid plan に対する false positive が増えない
- validation 追加により provider 生成の minor formatting を過剰に reject しない
- verifier diagnostic は secret / home path redaction を維持する

### テスト

- Rust unit:
  - `invalid_step_id_is_rejected`
  - `overlong_goal_is_rejected_or_quality_issue`
  - `placeholder_instruction_is_quality_issue`
  - `verify_failure_diagnostic_includes_stderr_excerpt`
  - `repair_prompt_includes_verify_diagnostic_excerpt`
  - `valid_step_id_examples_are_accepted`
  - `verify_diagnostic_redacts_sensitive_paths`

### 実行コマンド

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::lint
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::verify
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::repair
```

## Phase 9: Integration Eval And Regression Gate

### 目的

実装が step-plan 静的スコアだけでなく plan-run 成功率と runtime prompt contract を改善したか確認する。

### 事前ビルド

```bash
cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml
```

### MVP eval

スピード重視、ローカル LLM なし、step-plan / plan-run 対象:

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite cloud_trend \
  --model-profile speed-cloud \
  --modes step-plan,plan-run \
  --runs 1 \
  --parallel 6 \
  --context-budget 65536 \
  --binary mvp/anvilminimal/target/release/anvilminimal \
  --binary-kind anvilminimal \
  --run-root /private/tmp/anvilminimal-eval-010-mvp-step-plan-run
```

比較用に必要なら anvildev も実行する:

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite cloud_trend \
  --model-profile speed-cloud \
  --modes step-plan,plan-run \
  --runs 1 \
  --parallel 6 \
  --context-budget 65536 \
  --binary anvildev \
  --binary-kind anvildev \
  --run-root /private/tmp/anvilminimal-eval-010-anvildev-step-plan-run
```

### 回帰判定

必須:

- Rust unit/integration が通る
- Python eval tests が通る
- `summary.eval.tsv` に `prompt_contract_score` が出る
- plan-run failure kind の分類が粗くならない
- `verify_command_policy_error` / `planner_lint_error` が増えない
- terminal report 発生率が下がる
- prompt_contract_score が source parity を満たす
- fresh workspace inspect 発生率が下がる、または残存ケースが existing/fix/investigation と説明できる
- plan-run 失敗ケースに対して、prompt contract 欠落が原因ではないことを event evidence で示せる

期待:

- MVP plan-run success が baseline 0/12 から改善する
- `execution_shape_readiness_score` が baseline 65.5 より改善する
- `runtime_health_score` が改善する
- step-plan の `artifact_ownership_score` は高水準を維持する
- step-plan success / plan_quality / executable_plan が大きく悪化しない

数値 gate:

- `prompt_contract_score`: MVP run で平均 95 以上
- `terminal_report_step` 発生率: baseline より低下
- `fresh_workspace_read_before_write` 発生率: baseline より低下
- `artifact_ownership_score`: baseline 97.8 から 5pt 以上悪化しない
- `planner_lint_error` と `verify_command_policy_error`: 合計件数が baseline より増えない

反復:

- smoke では `--runs 1` を許容する
- 改善判定としてコミット前に同条件で少なくとも 2 回実行し、成功率と主要 score の方向性が逆転しないことを確認する

改善しない場合の切り分け:

- prompt_contract_score が低い
  - Phase 1/2/5/7 の実装漏れを疑う
- prompt_contract_score は高いが plan-run が失敗
  - tool policy / provider output / verify command policy / runtime repair を分類する
- terminal report が残る
  - Phase 3/4 の quality retry が効いていない
- fresh workspace inspect が残る
  - fresh workspace 判定または quality retry prompt が弱い

## Full Test Command Set

実装完了時に最低限実行する。

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml
python3 -m unittest discover -s mvp/anvilminimal/tests/eval -p 'test_*.py'
cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml
```

## Completion Checklist

- [x] Phase 0: source parity matrix 作成済み
- [x] Phase 1: step execution prompt が source contract を含む
- [x] Phase 2: repair prompt が source contract を含む
- [x] Phase 3: report semantics が blocker 用へ戻っている
- [x] Phase 4: fresh workspace / wrapper step quality retry が入っている
- [x] Phase 5: verify-only step に prior artifact context が渡る
- [x] Phase 6: ultra phase prompt に snapshot / runtime contract が入る
- [x] Phase 7: prompt_contract_score が eval summary に出る
- [x] Phase 8: validation / diagnostic parity が補強されている
- [x] Phase 9: MVP step-plan / plan-run eval を実行済み
- [x] Source parity matrix の SR-GAP-01〜SR-GAP-10 status が更新済み
- [x] 実装差分が eval scenario 固有条件に依存していないことを確認済み
- [x] 追加した完了条件に対応する unit / integration / Python tests がすべて存在し通過済み
- [x] 2回以上の cloud eval で主要指標の改善方向が確認済み

## Phase 010 Execution Result

Implemented files:

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/planner/repair.rs`
- `mvp/anvilminimal/src/planner/lint.rs`
- `mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py`
- `mvp/anvilminimal/scripts/eval_lib/run_summary.py`
- `mvp/anvilminimal/scripts/eval_lib/report.py`
- `mvp/anvilminimal/tests/eval/test_runtime_scoring.py`
- `mvp/anvilminimal/tests/eval/test_summary_schema.py`
- `workspace/mvp/eval/010/step_runner_source_parity_matrix.md`

Verification:

- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`: pass
- `python3 -m unittest discover -s mvp/anvilminimal/tests/eval -p 'test_*.py'`: pass
- `cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml`: pass

Eval:

- `/private/tmp/anvilminimal-eval-010-mvp-step-plan-run-1-net`
  - step-plan: 12/12 success
  - plan-run: 0/12 success
  - plan-run `prompt_contract_score`: 100.0
- `/private/tmp/anvilminimal-eval-010-mvp-step-plan-run-2-net`
  - step-plan: 12/12 success
  - plan-run: 0/12 success
  - plan-run `prompt_contract_score`: 100.0

Gate judgment:

- Source prompt contract gate: pass
- Step-plan stability gate: pass
- Artifact ownership gate: pass
- Plan-run success improvement gate: not achieved
- Remaining failure focus: `missing_tool_call` and `required_artifacts_missing`, which are outside the Phase 010 prompt-contract miss and should be handled in a separate execution finalization/artifact completion plan.

## Risk Controls

- `report` は blocker 用として残し、通常 success summary として誘導しない
- `inspect` は existing/fix/investigation では許容し、fresh create task のみ retryable quality issue にする
- expected_paths の duplicate ownership は戻さず、prior artifact context を prompt event と prompt builder で補う
- prompt_contract_score は prompt 本文ではなく boolean event で測り、機密/長文 prompt 保存を避ける
- plan score の改善だけで完了扱いにせず、plan-run success と failure kind 分布を併せて確認する
