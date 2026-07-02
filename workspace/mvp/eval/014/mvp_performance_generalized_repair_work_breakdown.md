# MVP Performance Generalized Repair Work Breakdown

作成日: 2026-06-27

対象計画:

- `workspace/mvp/eval/014/mvp_performance_generalized_repair_plan.md`

## 目的

MVP `anvilminimal` の性能改善を、評価過適応ではなく一般化された runtime / planner contract の改善として実施する。

主な改善対象:

- planner schema / lint / verify policy repair
- ultra-plan-run phase contract
- plan-run step verify repair / finalization
- postcheck contract handoff
- provider/API failure の診断分離

## 非目的

- scenario id / suite 名 / prompt 固有文言 / 固定成果物名による分岐を実装しない。
- verify / lint / postcheck を弱めて成功率を上げない。
- max_iterations を単純に増やして成功率を上げない。
- anvildev の大きな抽象を MVP へ丸ごと戻さない。
- provider retry を能力改善の主要施策にしない。

## 実施順序

Phase 番号は計画書の分類を維持するが、実装順は以下とする。

| order | phase | 目的 |
|---:|---|---|
| 1 | Phase 0 | baseline / impact matrix / negative control 固定 |
| 2 | Phase 7 | source parity audit gate |
| 3 | Phase 1 | planner schema/lint repair |
| 4 | Phase 2 | verify command policy repair |
| 5 | Phase 3 | ultra phase contract |
| 6 | Phase 4 | step runtime repair feedback |
| 7 | Phase 5 | postcheck-driven bounded repair |
| 8 | Phase 6 | provider/API failure separation |
| 9 | Phase 8 | eval / blind / qualitative validation |
| 10 | Phase 9 | release readiness / TUI smoke |

Phase 7 が完了するまで Phase 1-5 の runtime 実装修正に入らない。

## 共通ガード

全 Phase で守る。

- logic 内で `mvp-smoke`, scenario id, run id を参照しない。
- profile 固有の判断は profile contract 由来に限定する。
- fixed path は plan `expected_paths`, eval-generated completion contract, profile contract 由来だけ許可する。suite `expected_artifacts` は eval harness 側で completion contract へ変換された場合だけ runtime input として扱う。
- provider failure は raw success には残し、capability success では別枠にする。
- red baseline は実装前確認に限定し、default test suite に失敗テストを残さない。
- live API を使う検証は、provider request/retry/tool-call 仕様変更をした場合だけ必須にする。

## 作業成果物

この計画で追加・更新する文書:

- `workspace/mvp/eval/014/mvp_performance_generalized_repair_baseline.md`
- `workspace/mvp/eval/014/mvp_performance_source_parity_matrix.md`
- `workspace/mvp/eval/014/mvp_performance_generalized_repair_implementation_results.md`
- `workspace/mvp/eval/014/mvp_performance_generalized_repair_validation_results.md`

必要に応じて追加する test fixture:

- `mvp/anvilminimal/eval/fixtures/plans/`
- `mvp/anvilminimal/tests/eval/fixtures/`

## Phase 0: Baseline / Impact Matrix / Negative Control

### 対象

文書中心。

- `workspace/mvp/eval/014/mvp_performance_generalized_repair_baseline.md`
- `workspace/mvp/eval/014/mvp_performance_generalized_repair_plan.md`

参照 run root:

- `/private/tmp/anvilminimal-eval-013-live-mvp-rerun-net`
- `/private/tmp/anvilminimal-eval-013-live-anvildev-rerun-net`

### 作業

#### P0-W01 baseline 固定

直近 live eval から以下を抽出して文書化する。

- raw success
- provider-excluded capability success
- mode 別 success
- failure kind distribution
- failure layer distribution
- failed row の score
- stop reason / blocking reason
- representative stderr / event excerpt

出力:

- `workspace/mvp/eval/014/mvp_performance_generalized_repair_baseline.md`

#### P0-W02 impact matrix 作成

修正が影響し得る経路を matrix にする。

対象経路:

- CLI direct `--plan-steps`
- CLI direct `--plan-run`
- CLI direct `--ultra-plan-run`
- CLI `--run-plan`
- CLI `--run-ultra-plan`
- TUI slash `/plan-steps`
- TUI slash `/plan-run`
- TUI slash `/ultra-plan-run`
- minimal-loop `--prompt`
- eval runner with `--binary-kind anvilminimal`
- eval runner with `--binary-kind anvildev`

各経路について以下を記載する。

- entry point
- planner/execution client の使い分け
- StepPlan validation の有無
- completion contract の適用範囲
- TUI 表示影響
- regression test 候補

#### P0-W03 negative control 定義

以下を regression guard として固定する。

- valid StepPlan は repair で変更されない。
- invalid StepPlan は repair 後も lint を通らなければ実行されない。
- minimal-loop 単体は plan-run 用 prompt/contract を受け取らない。
- provider failure は capability score に混ざらない。
- postcheck が弱化されない。

#### P0-W04 baseline command 記録

再実行用 command を文書化する。

```bash
python3 mvp/anvilminimal/scripts/eval-report.py \
  --run-root /private/tmp/anvilminimal-eval-013-live-mvp-rerun-net
```

```bash
python3 mvp/anvilminimal/scripts/eval-report.py \
  --run-root /private/tmp/anvilminimal-eval-013-live-anvildev-rerun-net
```

```bash
python3 mvp/anvilminimal/scripts/eval-compare.py \
  --baseline /private/tmp/anvilminimal-eval-013-live-anvildev-rerun-net/summary.eval.tsv \
  --experiment /private/tmp/anvilminimal-eval-013-live-mvp-rerun-net/summary.eval.tsv \
  --out /private/tmp/anvilminimal-eval-014-baseline-compare.md
```

### 完了条件

- baseline 文書から、今回の改善前の failure layer と score が再現できる。
- impact matrix が CLI/TUI/eval/anvildev 経路を網羅している。
- negative control が後続 Phase のテスト計画へ紐づいている。

### テスト

```bash
python3 mvp/anvilminimal/scripts/eval-report.py \
  --run-root /private/tmp/anvilminimal-eval-013-live-mvp-rerun-net
```

```bash
python3 -m pytest mvp/anvilminimal/tests/eval/test_failure_classification.py -q
```

## Phase 7: Source Parity Audit Gate

### 対象

MVP:

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/planner/repair.rs`
- `mvp/anvilminimal/src/planner/lint.rs`
- `mvp/anvilminimal/src/planner/verify.rs`
- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`
- `mvp/anvilminimal/src/minimal_loop/completion.rs`
- `mvp/anvilminimal/src/tui/slash.rs`

移植元 / 既存 parity 文書:

- `src/agent/minimal_step_runner*`
- `src/agent/minimal_loop*`
- `workspace/mvp/eval/010/step_runner_migration_miss_root_cause_audit.md`
- `workspace/mvp/eval/010/step_runner_source_parity_repair_work_breakdown.md`
- `workspace/mvp/eval/011/plan_run_step_runtime_parity_repair_plan.md`

出力:

- `workspace/mvp/eval/014/mvp_performance_source_parity_matrix.md`

### 作業

#### P7-W01 source/MVP function matrix

以下の観点で source と MVP counterpart を対応付ける。

| 観点 | source 候補 | MVP 候補 |
|---|---|---|
| step prompt | `build_step_prompt` / minimal step runner | `planner/runner.rs::build_step_prompt` |
| repair prompt | source repair prompt | `planner/repair.rs::build_repair_prompt_with_context` |
| step verification | source verifier | `planner/verify.rs::verify_step` |
| plan lint | source plan validation | `planner/lint.rs::lint_step_plan_report` |
| ultra phase prompt | source profiled phase prompt | `planner/runner.rs::ultra_phase_prompt` |
| completion contract | source completion/final artifact handling | `minimal_loop/completion.rs`, `planner/runner.rs::verify_plan_final_contract` |
| provider fallback | source provider tool handling | `minimal_loop/loop_run.rs` provider fallback helpers |
| TUI slash handoff | source REPL slash | `tui/slash.rs::handle_command` |

#### P7-W02 gap classification

各差分を分類する。

- `adopt`: MVP に必要であり、小さい API に落とせる。
- `keep-different`: MVP 方針として意図的に違う。
- `defer`: 必要だが今回の scope では過大。
- `reject`: MVP 設計に反する。

`adopt` だけ Phase 1-5 の実装候補に入れる。

#### P7-W03 regression of previous parity repairs

`workspace/mvp/eval/010` / `011` で対応済みとした以下が再発していないか確認する。

- step prompt に overall goal / expected_result / verify / expected_paths がある。
- repair prompt に plan/step/verify/error context がある。
- ultra phase prompt に workspace snapshot / profile runtime contract がある。
- plan-level final contract と step-level obligation が分離している。
- TUI slash が CLI と同じ runner を通る。

#### P7-W04 evidence commands

調査コマンド例:

```bash
rg -n "build_step_prompt|build_repair_prompt|ultra_phase_prompt|verify_plan_final_contract|run_step_plan_with_ui|run_ultra_plan_with_ui" \
  src mvp/anvilminimal/src workspace/mvp/eval/010 workspace/mvp/eval/011
```

```bash
rg -n "handle_command|/plan-run|/ultra-plan-run|run_plan_file_with_ui|generate_and_run_ultra_plan_with_ui" \
  mvp/anvilminimal/src/tui mvp/anvilminimal/src/planner
```

### 完了条件

- source parity matrix が作成される。
- Phase 1-5 の実装対象が `adopt` 項目に紐づく。
- `keep-different` / `defer` / `reject` の理由が書かれている。
- 既存 parity repair の regression があれば、本計画内の blocker として記録される。

### テスト

文書フェーズのため自動テストは必須ではない。

確認コマンド:

```bash
rg -n "adopt|keep-different|defer|reject" \
  workspace/mvp/eval/014/mvp_performance_source_parity_matrix.md
```

## Phase 1: Planner Schema/Lint Repair

### 対象ファイル

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/planner/step_plan.rs`
- `mvp/anvilminimal/src/planner/lint.rs`
- `mvp/anvilminimal/tests/eval/test_failure_classification.py`

### 作業

#### P1-W01 structured retry context

`generate_step_plan_with_ui` 周辺で、schema/lint/quality retry に渡す情報を構造化する。

候補:

- `PlannerRetryContext`
- `PlannerRetryStage`
- `PlannerRetryIssue`

含める情報:

- stage: schema / lint / verify_policy / quality
- attempt
- previous error category
- missing required fields
- lint categories
- first-class hard constraints
- whether last valid plan exists

#### P1-W02 schema retry prompt の改善

`build_schema_retry_prompt` を以下の方針に寄せる。

- JSON object の expected shape を維持。
- missing top-level field / invalid field type / empty steps を明示。
- goal は渡すが、scenario 固有の解答は入れない。
- retry prompt は markdown fence を禁止。

#### P1-W03 lint retry prompt の改善

`build_lint_retry_prompt` / `lint_retry_hard_constraints` を整理する。

カテゴリ別に hard constraint を出す。

- `schema`
- `verify_policy`
- `ownership`
- `expected_paths`
- `ordering`
- `scaffold`

`verify_policy` が出た場合、Phase 2 の sanitizer と同じ vocabulary を使う。

#### P1-W04 last valid plan fallback

`last_valid_plan` の fallback ルールを明文化して実装する。

- latest output が invalid なら採用しない。
- valid plan があり、quality retry が degraded した場合は last valid plan を返す。
- degraded fallback は eval event に `planner_quality_retry_degraded` として残す。

#### P1-W05 failure classification

retry exhaustion 時に以下へ正しく落ちることを確認する。

- `planner_schema_error`
- `planner_lint_error`
- `verify_command_policy_error`
- `phase_scaffold_error`

### 実装上の注意

- retry count を単純に増やさない。
- lint を弱めない。
- quality issue を hard failure に昇格する場合は別判断とする。

### 完了条件

- invalid plan が実行されない。
- valid plan は no-op で通過し、repair event が増えない。
- `planner_schema_error` / `planner_lint_error` / `verify_command_policy_error` が unclassified へ落ちない。
- live eval で planning layer failure が baseline より増えない。

### テスト

Rust unit 候補:

- `schema_retry_prompt_reports_missing_goal`
- `schema_retry_prompt_reports_invalid_step_id_type`
- `lint_retry_prompt_includes_verify_policy_constraints`
- `last_valid_plan_survives_degraded_retry`
- `valid_plan_does_not_emit_repair_event`

Python eval unit:

```bash
python3 -m pytest \
  mvp/anvilminimal/tests/eval/test_failure_classification.py \
  mvp/anvilminimal/tests/eval/test_eval_event_report.py \
  -q
```

Rust:

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::runner
```

## Phase 2: Verify Command Policy Repair

### 対象ファイル

- `mvp/anvilminimal/src/planner/verify.rs`
- `mvp/anvilminimal/src/planner/lint.rs`
- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/minimal_loop/completion.rs`
- `mvp/anvilminimal/scripts/eval_lib/plan_scoring.py`

### 作業

#### P2-W01 verify policy violation taxonomy

`validate_verify_command` のエラーを分類可能にする。

候補カテゴリ:

- `shell_control_syntax`
- `setup_or_install_command`
- `dev_server_command`
- `destructive_command`
- `workspace_escape`
- `unknown_artifact_target`
- `dependency_order_violation`

エラー文字列だけでなく、可能なら enum/helper で扱う。

#### P2-W02 sanitizer / diagnosis helper

新 helper 候補:

- `diagnose_verify_command(command: &str) -> VerifyCommandDiagnosis`
- `normalize_verify_command(command: &str) -> Result<String, VerifyCommandError>`

許可する rewrite:

- whitespace 正規化
- quote 正規化
- workspace-relative path 正規化

許可しない rewrite:

- install/dev-server を test command に見せかける。
- shell control syntax を分解して一部だけ実行する。
- weak existence check へ無条件に downgrade する。

#### P2-W03 deferred verify / postcheck への委譲

setup/install/dev-server 系は verify command ではなく、completion contract / postcheck requirement として扱う。

作業:

- lint retry guidance に deferred verify への移動方針を書く。
- plan scoring 側も policy-compatible かつ verify-strong な plan を評価できるよう確認する。
- normal runtime は eval suite の postcheck だけに依存しない。

#### P2-W04 scorer consistency

`verify_strength_score` と `tool_policy_compatibility_score` が矛盾しないよう、scoring fixture を更新する。

### 完了条件

- `verify_command_policy_error` が単なる文字列ではなく category として診断できる。
- unsafe command が silently rewritten されない。
- verify が `test -f` だけへ偏らない。
- `verify_strength_score` と `tool_policy_compatibility_score` が同時に大きく悪化しない。

### テスト

Rust unit:

- `verify_command_diagnoses_shell_control_syntax`
- `verify_command_diagnoses_install_or_dev_server`
- `verify_command_rejects_workspace_escape`
- `verify_command_normalizes_safe_whitespace_only`
- `verify_command_does_not_silently_downgrade_to_existence_check`

Python:

```bash
python3 -m pytest \
  mvp/anvilminimal/tests/eval/test_plan_scoring.py \
  mvp/anvilminimal/tests/eval/test_summary_schema.py \
  -q
```

Rust:

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::verify planner::lint
```

## Phase 3: Ultra Phase Contract

### 対象ファイル

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/planner/ultra_plan.rs`
- `mvp/anvilminimal/src/planner/lint.rs`
- `mvp/anvilminimal/src/planner/profile.rs`
- `mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py`

### 作業

#### P3-W01 phase-local StepPlan validation pipeline

`run_ultra_plan_with_ui` の phase scaffold 後に、通常の StepPlan validation pipeline を呼ぶ。

対象:

- parse/schema validation
- `lint_step_plan_report`
- verify policy validation
- quality warning / retryable issue

ultra 専用 validator は作らない。

#### P3-W02 phase repair retry

phase-local plan が invalid な場合、global planner retry と同じ taxonomy で repair する。

event:

- `ultra_phase_plan_invalid`
- `ultra_phase_repair_retry`
- `ultra_phase_repair_degraded`
- `ultra_phase_failed`

stage:

- `plan`
- `scaffold`
- `lint`
- `verify_policy`
- `execute`
- `verify`
- `finalize`

#### P3-W03 phase continuation policy

phase failure 時の扱いを明確化する。

- fatal planning/verify policy failure: stop
- profile verification failure after bounded repair: stop
- recoverable phase profile drift: bounded profile repair
- provider failure: provider layer へ分類

#### P3-W04 phase prompt contract regression

`ultra_phase_prompt` が source parity gate で採用した要素を維持することをテストする。

- original goal
- profile
- phase id / prompt
- workspace snapshot
- profile runtime contract
- required final artifacts

### 完了条件

- ultra phase-local invalid plan が execution へ流れない。
- `phase_failure_stage` が失敗時に埋まる。
- `phase_completion_score` の低下理由が report で読める。
- phase count や artifact ownership が悪化しない。

### テスト

Rust unit:

- `ultra_phase_local_step_plan_runs_same_lint_pipeline`
- `ultra_phase_verify_policy_failure_stops_before_execute`
- `ultra_phase_failure_event_includes_stage`
- `ultra_phase_prompt_preserves_profile_runtime_contract`
- `ultra_phase_prompt_preserves_required_final_artifacts`

Python:

```bash
python3 -m pytest \
  mvp/anvilminimal/tests/eval/test_runtime_scoring.py \
  mvp/anvilminimal/tests/eval/test_eval_event_report.py \
  -q
```

Rust:

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::runner
```

## Phase 4: Step Runtime Repair Feedback

### 対象ファイル

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/planner/repair.rs`
- `mvp/anvilminimal/src/planner/verify.rs`
- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`
- `mvp/anvilminimal/src/minimal_loop/feedback.rs`

### 作業

#### P4-W01 repair context model

step verify failure 時に渡す context を struct 化する。

候補:

- `StepRepairContext`
- `StepExecutionContext`

含める情報:

- overall goal
- step id
- step kind
- instruction
- expected_result
- expected_paths
- verify commands
- verification stdout/stderr excerpt
- changed paths
- missing paths
- prior repair attempts

#### P4-W02 repair prompt update

`build_repair_prompt_with_context` を更新する。

制約:

- task の解答は入れない。
- verify を弱める指示は入れない。
- expected_paths は重複表示しない。
- overall final artifact と current step obligation を区別する。

#### P4-W03 plan-run step scope event

minimal-loop 単体と plan-run step の違いを event に出す。

event fields:

- `session_scope`
- `prompt_extracted_paths_enabled`
- `completion_contract_verification_enabled`
- `step_expected_paths`
- `plan_final_contract_deferred`

#### P4-W04 progress feedback for step scope

plan-run step scope でも以下の停滞を検出する。

- repeated read/glob/bash without edit
- verify failure after no changed paths
- artifacts satisfied but no finalization
- no tool call after tool-observation

#### P4-W05 bounded finalization prompt

artifact / verify / postcheck contract が満たされた場合だけ bounded finalization prompt へ移る。

### 完了条件

- `step_verify_failure` の repair prompt に contract が入る。
- repair は bounded で、max_iterations を単純に伸ばさない。
- minimal-loop 単体に plan-run 専用 contract が混入しない。
- `finalization_score` failure avg が改善するか、失敗時 reason が具体化する。

### テスト

Rust unit:

- `repair_prompt_includes_step_contract`
- `repair_prompt_includes_verify_diagnostic_excerpt`
- `repair_prompt_does_not_duplicate_expected_paths`
- `plan_run_step_scope_event_records_contract_options`
- `minimal_loop_prompt_scope_does_not_include_plan_run_contract`
- `artifacts_satisfied_switches_to_bounded_finalization_prompt`

Rust:

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::repair planner::runner minimal_loop::loop_run
```

Eval unit:

```bash
python3 -m pytest \
  mvp/anvilminimal/tests/eval/test_runtime_scoring.py \
  mvp/anvilminimal/tests/eval/test_eval_event_report.py \
  -q
```

## Phase 5: Postcheck-Driven Bounded Repair

### 対象ファイル

Runtime side:

- `mvp/anvilminimal/src/minimal_loop/completion.rs`
- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`
- `mvp/anvilminimal/src/planner/runner.rs`

Eval side:

- `mvp/anvilminimal/scripts/eval-run.py`
- `mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py`
- `mvp/anvilminimal/tests/eval/test_postcheck_dev_server.py`

### 重要な方針

eval suite の postcheck oracle を通常 runtime に直接注入しない。

実装前に以下を判定する。

- postcheck reason が completion contract / profile contract から runtime 内で導出できるか。
- eval harness 側の postcheck failure だけで分かる情報か。

runtime 内で導出できない場合、通常 agent runtime には入れず、eval harness の診断/optional rerun として扱う。

### 作業

#### P5-W01 feasibility gate

postcheck failure reason を以下に分類する。

- runtime contract-derived
- profile contract-derived
- eval harness-only
- provider/environment

`eval harness-only` は通常 runtime repair の入力にしない。

#### P5-W02 completion contract repair context

completion contract / profile contract 由来の failure だけ、bounded repair context に渡す。

対象:

- missing required artifact
- dependency manifest incoherence
- config mismatch
- build/test command failure from contract
- dev server readiness from profile contract

#### P5-W03 bounded repair policy

postcheck-derived repair は原則 1 回、最大でも config 上限内にする。

停止条件:

- same postcheck reason repeats
- no file changes
- provider/environment failure
- postcheck_not_applicable

#### P5-W04 eval harness reporting

通常 runtime に渡さない postcheck reason も、eval report では診断可能にする。

### 完了条件

- eval-only oracle が通常 runtime logic に漏れない。
- postcheck repair は bounded。
- postcheck command / expected result が弱化されない。
- `postcheck_stability_score` 低下時に reason が report で説明できる。

### テスト

Rust/integration:

- `completion_contract_failure_can_feed_bounded_repair_context`
- `eval_only_postcheck_is_not_injected_into_runtime`
- `postcheck_repair_stops_on_repeated_reason`
- `provider_failure_does_not_trigger_postcheck_repair`

Python:

```bash
python3 -m pytest \
  mvp/anvilminimal/tests/eval/test_postcheck_dev_server.py \
  mvp/anvilminimal/tests/eval/test_runtime_scoring.py \
  -q
```

## Phase 6: Provider/API Failure Separation

### 対象ファイル

- `mvp/anvilminimal/scripts/eval_lib/failure_classification.py`
- `mvp/anvilminimal/scripts/eval_lib/report.py`
- `mvp/anvilminimal/scripts/eval_lib/run_summary.py`
- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`
- provider modules under `mvp/anvilminimal/src/providers/` if needed

### 作業

#### P6-W01 provider failure taxonomy

failure classification を整理する。

分類:

- `provider_http_status`
- `provider_rate_limit`
- `provider_transient_network`
- `provider_schema_or_payload`
- `provider_tool_call_parse`
- `provider_model_not_found`

#### P6-W02 capability excluded aggregation

report に以下を出す。

- raw success
- provider-excluded success
- excluded count
- excluded failure kinds

#### P6-W03 retry change gate

この Phase では原則 retry 実装を変更しない。

変更する場合だけ以下を必須にする。

- recorded fixture
- provider smoke
- bounded retry count
- raw success への影響説明

#### P6-W04 live provider hypothesis validation

provider request/retry/tool-call payload を変更した場合のみ実行する。

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-provider-smoke.yaml \
  --model-profiles mvp/anvilminimal/eval/model_profiles.yaml \
  --model-profile speed-cloud \
  --runs 1 \
  --parallel 2 \
  --binary mvp/anvilminimal/target/release/anvilminimal \
  --binary-kind anvilminimal \
  --run-root /private/tmp/anvilminimal-eval-014-provider-smoke
```

### 完了条件

- provider failure が planning/runtime/postcheck score を下げない。
- provider-excluded success と raw success が report に併記される。
- retry を変更しない場合も classification/report 改善として完了できる。
- retry を変更した場合は live/fixture evidence が残る。

### テスト

```bash
python3 -m pytest \
  mvp/anvilminimal/tests/eval/test_failure_classification.py \
  mvp/anvilminimal/tests/eval/test_eval_event_report.py \
  mvp/anvilminimal/tests/eval/test_summary_schema.py \
  -q
```

## Phase 8: Eval / Blind / Qualitative Validation

### 対象

- `mvp/anvilminimal/eval/suites/mvp-smoke.yaml`
- `mvp/anvilminimal/eval/suites/mvp-blind.yaml`
- `mvp/anvilminimal/scripts/eval-run.py`
- `mvp/anvilminimal/scripts/eval-report.py`
- `mvp/anvilminimal/scripts/eval-compare.py`
- `workspace/mvp/eval/014/mvp_performance_generalized_repair_validation_results.md`

### 作業

#### P8-W01 unit / integration tests

実装後に必ず実行する。

```bash
python3 -m pytest mvp/anvilminimal/tests/eval -q
```

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml
```

#### P8-W02 release build

```bash
cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml
```

#### P8-W03 known suite eval

MVP:

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

anvildev:

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

#### P8-W04 blind suite eval

MVP:

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-blind.yaml \
  --model-profiles mvp/anvilminimal/eval/model_profiles.yaml \
  --model-profile speed-cloud \
  --modes step-plan,plan-run,ultra-plan-run \
  --runs 3 \
  --parallel 4 \
  --binary mvp/anvilminimal/target/release/anvilminimal \
  --binary-kind anvilminimal \
  --run-root /private/tmp/anvilminimal-eval-014-mvp-blind
```

必要に応じて anvildev blind も比較する。

#### P8-W05 report / compare

```bash
python3 mvp/anvilminimal/scripts/eval-report.py \
  --run-root /private/tmp/anvilminimal-eval-014-mvp-smoke
```

```bash
python3 mvp/anvilminimal/scripts/eval-report.py \
  --run-root /private/tmp/anvilminimal-eval-014-mvp-blind
```

```bash
python3 mvp/anvilminimal/scripts/eval-compare.py \
  --baseline /private/tmp/anvilminimal-eval-013-live-mvp-rerun-net/summary.eval.tsv \
  --experiment /private/tmp/anvilminimal-eval-014-mvp-smoke/summary.eval.tsv \
  --out /private/tmp/anvilminimal-eval-014-mvp-smoke-compare.md
```

#### P8-W06 qualitative review

成功/失敗から代表 YAML と成果物を確認する。

確認観点:

- plan が prompt/profile の主要制約を拾っている。
- verify が policy compatible かつ十分に強い。
- artifact ownership が exactly once に近い。
- step/phase が過度に細分化されていない。
- successful output が postcheck だけでなく実用上も成立している。
- failed output の failure layer が納得できる。

出力:

- `workspace/mvp/eval/014/mvp_performance_generalized_repair_validation_results.md`

### 完了条件

- known suite で provider-excluded success が baseline より悪化しない。
- blind suite で failure layer が悪化しない。
- `verify_strength_score`, `tool_policy_compatibility_score`, `postcheck_stability_score` が意図せず低下しない。
- qualitative review で明らかな評価過適応が見つからない。

## Phase 9: Release Readiness / TUI Smoke

### 対象ファイル

- `mvp/anvilminimal/src/tui/slash.rs`
- `mvp/anvilminimal/src/tui/repl.rs`
- `mvp/anvilminimal/tests/tui_repl.rs`
- `mvp/anvilminimal/tests/tui_pty.rs`

### 作業

#### P9-W01 symlink / binary check

```bash
which anvilminimal
```

```bash
ls -l "$(which anvilminimal)"
```

確認:

- `mvp/anvilminimal/target/release/anvilminimal` を向いている場合は `cargo build --release` だけで反映される。
- 別 path を向いている場合は install/symlink 更新手順を文書化する。

#### P9-W02 CLI smoke

API key が利用可能な場合:

```bash
mvp/anvilminimal/target/release/anvilminimal \
  --yes \
  --context-budget 65536 \
  --model gpt-5.4-mini \
  --provider openai \
  --planner-model gemini-3.5-flash \
  --planner-provider gemini \
  --plan-steps "Create a small README.md with one heading."
```

network/API を使わない routing smoke は、unit test または dry-run eval で確認する。

#### P9-W03 TUI slash smoke

手動または PTY test で確認する。

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

確認:

- ASCII logo / prompt が表示される。
- ESC が効く。
- phase planning / execution progress が見える。
- failure 時に silent exit しない。
- 成果物なし終了時に planner/runtime/provider どの layer か分かる。

#### P9-W04 final status document

以下を `workspace/mvp/eval/014/mvp_performance_generalized_repair_validation_results.md` に追記する。

- tests
- build
- eval
- blind eval
- qualitative review
- TUI smoke
- known residual risk

### 完了条件

- release build が成功する。
- `anvilminimal` symlink/install 状態が確認済み。
- CLI/TUI/slash が同じ runner contract を通る。
- TUI で silent exit しない。
- CLI smoke は実在する action selector `--plan-steps`, `--plan-run`, `--ultra-plan-run`, `--run-plan`, `--run-ultra-plan` だけを使う。

## Phase Completion Checklist

| Phase | 完了証跡 |
|---|---|
| Phase 0 | `mvp_performance_generalized_repair_baseline.md` |
| Phase 7 | `mvp_performance_source_parity_matrix.md` |
| Phase 1 | planner repair unit tests + eval classification tests |
| Phase 2 | verify policy diagnosis tests + scoring consistency |
| Phase 3 | ultra phase-local validation tests + phase event tests |
| Phase 4 | step repair context tests + minimal-loop regression |
| Phase 5 | postcheck repair feasibility + bounded repair tests |
| Phase 6 | provider classification/report tests |
| Phase 8 | known/blind eval + qualitative review |
| Phase 9 | release build + TUI smoke |

## Overall Acceptance

最低条件:

- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` が通る。
- `python3 -m pytest mvp/anvilminimal/tests/eval -q` が通る。
- `cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml` が通る。
- known suite で provider-excluded success が baseline より悪化しない。
- blind suite で failure layer が悪化しない。
- planning layer failure が baseline より減る、または減らない理由が source parity / provider / eval variance として説明できる。
- verify / lint / postcheck を弱めた変更がない。

目標:

- MVP total capability success `30/36` 以上。
- plan-run `10/12` 以上。
- ultra-plan-run `9/12` 以上。
- planning layer failure 50%以上削減。
- smoke と blind の改善方向が一致。

## Implementation Notes

- Phase 0 / 7 は先に文書だけで完了させる。
- Phase 1-5 は小さく commit 可能な単位で進める。
- Phase 6 は retry 実装を含めない場合でも完了可能。
- Phase 8 の live eval は network/API を使うため、実行時は必要に応じて escalated permission を使う。
- 実装後に評価だけが改善し、成果物品質が落ちている場合は rollback 対象にする。
