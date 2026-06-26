# Plan-run step runtime parity repair work breakdown

対象計画:

- `workspace/mvp/eval/011/plan_run_step_runtime_parity_repair_plan.md`

目的:

- MVP `anvilminimal` の `plan-run` 経由 step 実行 runtime を移植元 `anvildev` の考え方へ寄せる
- `minimal-loop --prompt` 単体の completion contract 強化は維持する
- `plan-run` step 中の current obligation と plan-level final contract を分離する
- `required_artifacts_missing` と `missing_tool_call` の主要因を runtime 側で解消する

非目的:

- scenario 名、固定 artifact 名、profile 名に依存した分岐を入れない
- plan-run 失敗を成功扱いにする緩和を入れない
- verify / postcheck / planner lint を弱めない
- 移植元 `loop_run/*` 全体を MVP へ丸ごと戻さない
- 指標だけを改善し、実行完走率に効かない変更を入れない

## Implementation Order

1. Phase 0 で baseline と代表失敗を固定する
2. Phase 1 で `minimal_loop` に options API を追加し、既定動作の非回帰を先にテストする
3. Phase 2 で `plan-run` / repair step だけ prompt artifact extraction と contract path merge を無効化する
4. Phase 3 で no-tool finalization を `tool call seen` 基準へ寄せる
5. Phase 4 で step kind ごとの completion policy を runner 側に閉じ込める
6. Phase 5 で plan-level final contract verification を追加し、全体成果物保証を戻す
7. Phase 6 で eval 指標に obligation scope を追加する
8. Phase 7 で unit / integration / route regression を固める
9. Phase 8 で MVP と anvildev の eval 比較を行う
10. Phase 9 でリスクレビューとドキュメント反映を行う

## Review Adjustments Reflected

今回のレビューで、目的達成・完了条件・テスト計画に対して以下を追加した。

- completion contract は path merge だけでなく verify / deferred requirement / iteration extension にも影響するため、plan-run step では contract runtime effect 全体を無効化し、plan-level final contract verification へ集約する
- eval は 1 回だけの成功数ではばらつきを吸収しきれないため、原則 2 回実行し、平均と worst run の両方を gate にする
- plan-level final contract failure と obligation scope violation が `unclassified_process_failure` へ落ちないよう、failure classification と summary 互換性を作業範囲に含める
- `/run-plan`、TUI slash、`ultra-plan-run`、`ultra-step-run` は parse/dispatch だけでなく、同じ step runtime options が使われたことを deterministic fixture または event で検証する
- Phase 2 で step scope から final artifact / external contract を外すため、Phase 5 の plan-level final contract verification は任意ではなく必須とする

## Phase 0: Baseline And Evidence Lock

### 作業

- `workspace/mvp/eval/011/plan_run_step_runtime_parity_repair_plan.md` の baseline 表を更新する
- 直近 MVP plan-run 2 回分と anvildev 1 回分について、以下を本文末尾に追記する
  - run root
  - command
  - success count
  - failure kind distribution
  - representative scenario
  - representative stderr / event excerpt の要約
- 代表失敗を 2 件固定する
  - `required_artifacts_missing`: `inspect` step が `Required final artifacts` を current required path として扱うケース
  - `missing_tool_call`: Bash / Read / Glob 実行後に prose final が `missing tool call` になるケース
- source / MVP の traceability を固定する
  - source: `src/agent/minimal_step_runner/verify.rs::early_success_paths_for_step`
  - source: `src/agent/minimal_loop/loop_run.rs` の `!tool_calls_seen` gate
  - MVP: `mvp/anvilminimal/src/planner/runner.rs::run_step`
  - MVP: `mvp/anvilminimal/src/minimal_loop/loop_run.rs::run_session_with_outcome_with_ui`
  - MVP: `mvp/anvilminimal/src/minimal_loop/loop_run.rs::effective_required_paths`
  - MVP: `mvp/anvilminimal/src/minimal_loop/loop_run.rs::extract_requested_artifact_paths`

### 完了条件

- baseline が plan 本文に残っている
- 代表失敗 2 件について、現象、直接原因、対象関数が追える
- 以降の eval で比較する primary gate が明記されている

### テスト

- ドキュメントフェーズのため自動テストなし
- `rg -n "required_artifacts_missing|missing_tool_call|early_success_paths_for_step|effective_required_paths" workspace/mvp/eval/011/plan_run_step_runtime_parity_repair_plan.md`

## Phase 1: Add Step Execution Options API

### 対象ファイル

- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`

### 作業

- `RunSessionOptions` と関連 enum を `pub(crate)` で追加する
  - `PromptArtifactExtraction`
  - `CompletionContractPathMerge`
  - `CompletionContractVerification`
  - `ActionNoToolPolicy`
- `Default` を実装し、既存の `minimal-loop --prompt` と同じ動作にする
  - prompt artifact extraction: enabled
  - completion contract path merge: enabled
  - completion contract verification: enabled
  - no-tool action policy: current default behavior
- 既存 `run_session_with_outcome_with_ui(...)` は署名を維持し、内部で default options を渡す
- options-aware helper を追加する
  - 候補名: `run_session_with_outcome_with_options(...)`
  - crate 外へ不要に公開しない
- `effective_required_paths(...)` を options-aware にするため、以下のどちらかに整理する
  - `effective_required_paths_with_options(...)`
  - `RequiredPathSources` のような内部 struct を返す helper
- path source の内訳を保持できるよう、最低限以下を算出する
  - explicit required paths
  - prompt extracted paths
  - completion contract paths
  - effective required paths
- completion contract の step-level side effects を options で分ける
  - required path merge
  - verify / deferred requirement check
  - iteration extension
  - artifact recovery enabling

### 実装上の注意

- `RunSessionOutcome` の既存 field は破壊しない
- options は `Config` に混ぜない。plan-run step 実行の一時的 runtime policy として扱う
- `CompletionContract::load_for_config(config)` は従来通り読んでよいが、step scope へ path を混ぜるかは options で分ける
- `CompletionContract::load_for_config(config)` が成功しても、plan-run step options では verify / deferred requirement / iteration extension が動かないようにする
- default path は既存テストを通すため、`effective_required_paths` の既定結果を変えない

### 完了条件

- 既存 `run_session_with_outcome_with_ui(...)` の呼び出し元は変更不要のまま compile する
- options 未指定経路では prompt artifact extraction と completion contract path merge が有効
- options 指定経路では prompt artifact extraction と completion contract path merge を個別に無効化できる
- options 指定経路では completion contract verification / deferred requirement / iteration extension を step turn で無効化できる
- options は crate 外 public API になっていない

### テスト

- Rust unit:
  - `default_run_session_options_preserve_prompt_artifact_extraction`
  - `default_run_session_options_preserve_completion_contract_path_merge`
  - `default_run_session_options_preserve_completion_contract_verification`
  - `default_run_session_options_preserve_action_no_tool_policy`
  - `run_session_options_can_disable_prompt_artifact_extraction`
  - `run_session_options_can_disable_completion_contract_path_merge`
  - `run_session_options_can_disable_completion_contract_verification_side_effects`
- 実行:
  - `cargo test --manifest-path mvp/anvilminimal/Cargo.toml minimal_loop`

## Phase 2: Disable Prompt Artifact Extraction For Plan-run Steps

### 対象ファイル

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/planner/repair.rs`
- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`

### 作業

- `planner/runner.rs::run_step(...)` の初回実行で plan-run step 用 options を渡す
- repair retry の `run_session_with_outcome_with_ui(...)` 呼び出しも同じ options-aware helper へ変更する
- plan-run step 用 options を runner 側 helper に閉じ込める
  - 候補名: `step_run_session_options(step: &PlanStep) -> RunSessionOptions`
- plan-run step では以下にする
  - explicit required paths: `step.expected_paths`
  - prompt artifact extraction: disabled
  - completion contract path merge: disabled
  - completion contract verification / deferred requirement checks: disabled during step
  - completion contract による iteration extension / artifact recovery enabling: disabled during step
  - no-tool action policy: `RequireToolOnlyIfNoToolSeen`
- `build_step_prompt(...)` は `Required final artifacts` を context として残す
- `Required final artifacts` は current step obligation ではなく、Phase 5 の plan-level final contract で確認する
- repair prompt でも `Required final artifacts` は context として残しつつ、current required paths へは混ぜない

### 実装上の注意

- `minimal-loop --prompt` 単体では prompt artifact extraction を維持する
- CLI で completion contract を渡す通常 minimal loop の behavior は維持する
- `step.expected_paths` が空の `inspect` / `verify` / `setup` step で、overall final artifact を required path にしない
- `implement` step の `step.expected_paths` は strict に残す
- external completion contract の verify / deferred requirement は Phase 5 の plan-level final contract でだけ評価する

### 完了条件

- `inspect` step の `step.expected_paths` が空なら、prompt に `Required final artifacts` があっても current required paths は空
- `implement` step では `step.expected_paths` が required paths として効く
- repair prompt 内の `Required final artifacts` が current required paths に混入しない
- external completion contract paths が plan-run step current required paths に混入しない
- external completion contract verify / deferred requirement が current step turn の成否を直接決めない
- completion contract が存在するだけで plan-run step の iteration limit や artifact recovery policy が変化しない
- representative `required_artifacts_missing` の regression test が通る

### テスト

- Rust unit / integration:
  - `plan_step_disables_prompt_required_artifact_extraction`
  - `minimal_loop_prompt_keeps_required_artifact_extraction`
  - `repair_step_disables_prompt_required_artifact_extraction`
  - `plan_step_disables_completion_contract_path_merge`
  - `plan_step_disables_completion_contract_verification_side_effects`
  - `inspect_step_does_not_require_overall_final_artifact`
  - `implement_step_still_requires_expected_paths`
- 実行:
  - `cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::runner`
  - `cargo test --manifest-path mvp/anvilminimal/Cargo.toml minimal_loop`

## Phase 3: Align No-tool Finalization With Source

### 対象ファイル

- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`
- `mvp/anvilminimal/src/minimal_loop/feedback.rs`

### 作業

- 現在の `!write_or_edit_seen && looks_like_action_prompt(user_prompt)` gate を options-aware にする
- plan-run step 用 options では source と同様に `tool call seen` を基準にする
  - tool call が一度もない action prompt: feedback / error
  - Bash / Read / Glob 等の tool call はあるが Write/Edit がない: expected paths が満たされるか空なら runner 判定へ委譲
  - expected paths が未達: missing artifact feedback / recovery を優先
- `tool_call_count > 0` を `tool_calls_seen` として扱うか、明示変数を追加する
- default minimal-loop mode は既存の write/edit safety を維持する
- no-tool prose が「これから tool を使う」と述べるだけの progress text の扱いは既存の安全装置を維持する

### 実装上の注意

- no-tool finalization の緩和は plan-run step options に限定する
- expected paths がある implement step では、path 未達の prose final を成功扱いしない
- verify/setup/inspect step で Bash / Read / Glob の後に final text が返った場合、runner の `verify_step(...)` へ委譲できる状態にする
- `missing_tool_call` を単純に潰すのではなく、no tool ever seen の action prompt は従来通り feedback する

### 完了条件

- Bash-only verify/setup step が no-tool final answer で `missing_tool_call` にならない
- Read/Glob-only inspect step が expected_paths empty なら runner へ戻れる
- No tool at all の implement step は引き続き feedback/error になる
- expected paths 未達の implement step は no-tool final を許さない
- representative `missing_tool_call` の regression test が通る

### テスト

- Rust unit:
  - `verify_step_with_bash_then_final_text_is_allowed`
  - `setup_step_with_bash_then_final_text_is_allowed`
  - `inspect_step_with_glob_then_final_text_is_allowed_when_no_expected_paths`
  - `implement_step_no_tool_still_gets_missing_tool_feedback`
  - `implement_step_with_read_only_and_missing_expected_paths_still_gets_artifact_feedback`
  - `default_minimal_loop_repeated_action_without_tool_still_errors`
- 実行:
  - `cargo test --manifest-path mvp/anvilminimal/Cargo.toml minimal_loop`

## Phase 4: Step-kind Aware Completion Policy

### 対象ファイル

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/planner/step_plan.rs`

### 作業

- `StepKind` ごとの runtime options と expected path strictness を runner 側に明示する
- `StepKind::Implement`
  - prompt extraction disabled
  - contract path merge disabled
  - expected_paths strict
  - no tool ever seen は feedback/error
- `StepKind::Setup`
  - prompt extraction disabled
  - expected_paths がある場合のみ strict
  - Bash 等の tool 後は no-write final を許し runner verify へ委譲
- `StepKind::Verify`
  - prompt extraction disabled
  - verify command / `verify_step(...)` を最終判定にする
  - Bash 等の tool 後は no-write final を許す
- `StepKind::Inspect`
  - prompt extraction disabled
  - no implicit final artifact
  - expected_paths empty なら artifact 作成を要求しない
- `StepKind::Report`
  - Phase 010 の方針を維持し、通常成功用の作業 step にしない
  - explicit blocker / final summary 用の扱いを runner 側で閉じる

### 実装上の注意

- StepKind ごとの policy は scenario ではなく step role に基づける
- report step の扱いを緩めて plan-run 成功率だけを上げない
- `verify_step(...)` の failure は repair に進める

### 完了条件

- `inspect` step が空 workspace で全体 artifact 作成を要求しない
- `verify` step は Bash 成功後、`verify_step(...)` が pass なら完了
- `verify` step の verify failure は repair に進む
- `implement` step は expected_paths 未達なら失敗または repair に進む
- `implement` step は expected_paths 達成前の prose final を完了扱いしない

### テスト

- Rust integration:
  - `inspect_step_with_final_artifact_context_completes_without_creating_artifact`
  - `verify_step_bash_success_completes_without_write`
  - `verify_step_bash_failure_runs_repair`
  - `implement_step_missing_expected_path_runs_recovery`
  - `report_step_does_not_mask_missing_implementation_artifact`
- 実行:
  - `cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::runner`

## Phase 5: Plan-level Final Contract Verification

### 対象ファイル

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/minimal_loop/completion.rs`
- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`

### 作業

- `run_step_plan_with_ui(...)` の全 step 完了後に plan-level final contract verification を追加する
- 検証対象を整理する
  - plan goal から抽出した required final artifacts
  - plan prompt context に明示された required final artifacts
  - `StepPlan` 全体の expected paths
  - optional external completion contract の required paths / verify
  - profile-specific requirement は既存 profile/postcheck の責務を超えない範囲で扱う
- helper を作る
  - 候補名: `verify_plan_final_contract(...)`
  - 戻り値は pass/fail と missing paths / verify diagnostics を持つ report struct
- 全 step 完了後、final contract が fail なら plan-run を fail にする
- eval event を出す
  - event: `plan_final_contract`
  - required_final_artifacts count/list
  - missing_final_artifacts count/list
  - external_contract_checked
  - ok
- stdout/stderr diagnostic は簡潔にし、prompt 本文や絶対パスを出さない

### 実装上の注意

- Phase 2 で step scope から final artifact を外すため、この Phase は必須
- plan-level final contract は step execution 中の required paths へ混ぜない
- external completion contract の verify / deferred requirement は step turn ではなくこの helper で評価する
- suite postcheck は eval harness 側の最終判定として維持する
- verify policy error を握りつぶさない

### 完了条件

- 全 step が通っても required final artifact が欠けていれば plan-run は失敗する
- required final artifact が揃っていれば plan-level final verification は pass する
- external completion contract がある場合、step scope には混ぜず plan-level で検証する
- external completion contract の verify / deferred requirement が fail の場合は plan-level final contract failure として分類できる
- eval success は postcheck と plan-level final contract の両方を満たす
- `plan_final_contract` event が出る

### テスト

- Rust integration:
  - `plan_run_final_contract_fails_when_required_final_artifact_missing`
  - `plan_run_final_contract_passes_after_step_artifacts_created`
  - `plan_run_external_completion_contract_checked_at_plan_level`
  - `plan_run_step_scope_does_not_inherit_external_completion_contract`
  - `plan_run_final_contract_fails_when_external_contract_verify_fails`
  - `plan_final_contract_event_is_emitted`
- 実行:
  - `cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::runner`

## Phase 6: Add Obligation Scope Metrics

### 対象ファイル

- `mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py`
- `mvp/anvilminimal/scripts/eval_lib/run_summary.py`
- `mvp/anvilminimal/scripts/eval_lib/report.py`
- `mvp/anvilminimal/scripts/eval_lib/failure_classification.py`
- `mvp/anvilminimal/tests/eval/test_runtime_scoring.py`
- `mvp/anvilminimal/tests/eval/test_summary_schema.py`
- `mvp/anvilminimal/tests/eval/test_failure_classification.py`
- `mvp/anvilminimal/eval/README.md`

### 作業

- MVP runtime event `step_obligation_scope` を出す
  - 実装場所候補: `minimal_loop/loop_run.rs` の required paths 算出直後
  - plan-run step 以外の default minimal loop でもイベントを出してよいが、score 対象は mode/context で分ける
- event field:
  - `event`
  - `step_id` or `session_scope`
  - `step_kind`
  - `explicit_required_paths`
  - `prompt_extracted_paths_enabled`
  - `prompt_extracted_paths`
  - `completion_contract_path_merge_enabled`
  - `completion_contract_paths`
  - `effective_required_paths`
  - `initially_missing_paths`
  - `contract_paths_merged`
- path list は workspace-relative path のみ
- eval scoring に以下を追加する
  - `step_obligation_scope_score`
  - `step_obligation_scope_violation_count`
- summary / report に新指標を追加する
  - missing event の old run / anvildev run で壊れない
  - core table では runtime health group として扱う
- failure classification に新規 runtime failure を追加する
  - `plan_final_contract_failure`
  - `step_obligation_scope_violation`
- `mvp/anvilminimal/eval/README.md` に指標の意味と読み方を追記する

### score 案

- 100:
  - plan-run step で prompt artifact extraction disabled
  - completion contract path merge disabled
  - effective paths が explicit paths と一致
- 70:
  - extraction enabled だが prompt extracted paths が 0
  - または old run 互換で違反が観測されない
- 30:
  - context-only final artifacts が current step effective paths に混入
  - contract paths が step current paths に混入
- blank / null:
  - event missing で算出不能

### 完了条件

- MVP plan-run rows に `step_obligation_scope_score` が出る
- anvildev / old summaries で event 欠損でも report が壊れない
- MVP post-change run で `step_obligation_scope` event が欠損した場合は diagnostic 上の欠損として扱える
- plan-level final contract failure と obligation scope violation が `unclassified_process_failure` に落ちない
- `prompt_contract_score` と役割が重複しない
- path source 内訳から scope 混入の有無を診断できる
- path list は workspace-relative のみで、prompt 本文や絶対パスを保存しない

### テスト

- Python:
  - `test_step_obligation_scope_score_blank_without_events`
  - `test_step_obligation_scope_score_detects_disabled_extraction`
  - `test_step_obligation_scope_score_penalizes_context_artifact_merge`
  - `test_step_obligation_scope_score_uses_effective_required_path_breakdown`
  - `test_summary_schema_accepts_step_obligation_scope_score`
  - `test_report_compare_handles_missing_obligation_scope_metric`
  - `test_failure_classification_recognizes_plan_final_contract_failure`
  - `test_failure_classification_recognizes_step_obligation_scope_violation`
- 実行:
  - `python3 -m unittest discover -s mvp/anvilminimal/tests/eval -p 'test_*.py'`

## Phase 7: Regression Tests And Route Coverage

### 対象ファイル

- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`
- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/tui/slash.rs`
- `mvp/anvilminimal/src/lib.rs`
- `mvp/anvilminimal/tests/*`

### 作業

- Phase 1〜6 の unit tests を通す
- route coverage を追加する
  - CLI `plan-run`
  - file-based `/run-plan` 相当
  - TUI slash `/plan-run`
  - `ultra-plan-run` phase step execution
  - `ultra-step-run` phase/step execution if supported by the eval matrix
- fake client / deterministic fixture で以下を再現する
  - inspect step が `Required final artifacts` を context に持つが artifact 作成を要求されない
  - verify step が Bash 後に final text を返しても missing_tool_call にならない
  - implement step が expected path を作るまで完了しない
  - final contract が欠けた場合に plan-run が失敗する
- existing tests で minimal-loop default behavior が変わっていないことを確認する

### 完了条件

- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml minimal_loop` が通る
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::runner` が通る
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` が通る
- `python3 -m unittest discover -s mvp/anvilminimal/tests/eval -p 'test_*.py'` が通る
- `/run-plan`、TUI slash、`ultra-plan-run` の step execution scope が同じ options を使う
- route coverage は parser dispatch だけでなく、deterministic fake-client outcome または `step_obligation_scope` event で同じ options が使われたことを確認する

### テスト一覧

- Rust:
  - `plan_step_disables_prompt_required_artifact_extraction`
  - `minimal_loop_prompt_keeps_required_artifact_extraction`
  - `inspect_step_does_not_require_overall_final_artifact`
  - `verify_step_with_bash_then_final_text_is_allowed`
  - `implement_step_no_tool_still_gets_feedback`
  - `plan_run_fix_js_date_helper_like_flow_completes_step_after_artifacts`
  - `plan_run_docs_inspect_like_flow_does_not_require_readme_in_inspect_step`
  - `run_plan_file_uses_same_step_runtime_options`
  - `tui_plan_run_uses_same_step_runtime_options`
  - `ultra_plan_run_phase_steps_use_same_step_runtime_options`
  - `ultra_step_run_uses_same_step_runtime_options`
- Python:
  - Phase 6 の scoring tests 一式

## Phase 8: Eval Acceptance And Comparison

### 事前準備

- release build:

```bash
cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml
```

### MVP plan-run eval

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --model-profiles mvp/anvilminimal/eval/model_profiles.yaml \
  --model-profile speed-cloud \
  --modes plan-run \
  --runs 1 \
  --parallel 6 \
  --context-budget 65536 \
  --binary mvp/anvilminimal/target/release/anvilminimal \
  --binary-kind anvilminimal \
  --run-root /private/tmp/anvilminimal-eval-011-mvp-plan-run-1
```

Primary gate 判定では、同じ条件で run root だけを変えて 2 回目も実行する。

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --model-profiles mvp/anvilminimal/eval/model_profiles.yaml \
  --model-profile speed-cloud \
  --modes plan-run \
  --runs 1 \
  --parallel 6 \
  --context-budget 65536 \
  --binary mvp/anvilminimal/target/release/anvilminimal \
  --binary-kind anvilminimal \
  --run-root /private/tmp/anvilminimal-eval-011-mvp-plan-run-2
```

### anvildev plan-run eval

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --model-profiles mvp/anvilminimal/eval/model_profiles.yaml \
  --model-profile speed-cloud \
  --modes plan-run \
  --runs 1 \
  --parallel 6 \
  --context-budget 65536 \
  --binary anvildev \
  --binary-kind anvildev \
  --run-root /private/tmp/anvilminimal-eval-011-anvildev-plan-run-1
```

### MVP cross-mode smoke

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --model-profiles mvp/anvilminimal/eval/model_profiles.yaml \
  --model-profile speed-cloud \
  --modes step-plan,plan-run,ultra-plan-run,ultra-step-run \
  --runs 1 \
  --parallel 6 \
  --context-budget 65536 \
  --binary mvp/anvilminimal/target/release/anvilminimal \
  --binary-kind anvilminimal \
  --run-root /private/tmp/anvilminimal-eval-011-mvp-cross-mode-1
```

### 追加確認

- 必要に応じて `minimal-loop,step-plan` の smoke を 1 回実行する
- run summary から以下を抽出し、plan 本文に追記する
  - success count
  - failure kind distribution
  - `plan_run_runtime_health_score`
  - `step_obligation_scope_score`
  - `artifact_progress_score`
  - `finalization_score`
  - `prompt_contract_score`
  - representative failed YAML / events の定性評価

### Primary gate

- MVP `plan-run` speed-cloud を原則 2 回実行し、平均 success >= 6/12 かつ worst run >= 5/12
- 予算/時間制約で 1 回のみの場合は provisional とし、success >= 6/12 を満たす
- `required_artifacts_missing` <= 1/run
- `missing_tool_call` <= 2/run
- `plan_final_contract_failure` が genuine missing artifact / verify failure として説明可能で、`unclassified_process_failure` に落ちない
- `plan_run_runtime_health_score` avg >= 65
- `step_obligation_scope_score` avg >= 95 for MVP plan-run rows
- `prompt_contract_score` remains 100
- `artifact_progress_score` improves for previous required missing cases
- `finalization_score` improves for previous missing tool cases

### Comparative gate

- anvildev comparison remains reference, not hard identical target
- anvildev succeeds and MVP fails for same scenario/model pair の場合、failure kind が `context final artifact mixed into current step` 系ではない
- MVP の残失敗が planner lint / verify policy / genuine postcheck failure へ寄る
- `ultra-plan-run` / `ultra-step-run` の cross-mode smoke で step runtime options 変更による新規 runtime failure が増えていない

## Phase 9: Risk Review And Documentation

### 対象ファイル

- `workspace/mvp/eval/011/plan_run_step_runtime_parity_repair_plan.md`
- `workspace/mvp/eval/010/step_runner_source_parity_matrix.md` if present
- `mvp/anvilminimal/eval/README.md`

### 作業

- 実装後の結果を plan 本文に追記する
  - changed files
  - tests run
  - eval run roots
  - before/after metrics
  - remaining failures
  - whether each acceptance gate passed
- source parity matrix がある場合、以下の status を更新する
  - current step vs context artifact scope
  - no-tool finalization gate
  - plan-level final contract
  - route coverage
- eval README に新指標を追記する
- scenario 固有分岐がないことを review checklist に記録する

### 完了条件

- Phase 0〜8 の結果が追跡可能
- 受け入れ gate の pass/fail が明記されている
- 残失敗がある場合、failure kind と次アクションが書かれている
- eval 指標の読み方が README から分かる
- 「評価に過剰適応していないか」の確認観点が本文に残っている

## Cross-phase Review Checklist

- `minimal-loop --prompt` default behavior は変わっていないか
- plan-run step scope では `step.expected_paths` 以外の path が current obligation に混ざっていないか
- plan-run step scope では external completion contract verify / deferred requirement / iteration extension が current turn に混ざっていないか
- plan-level final contract で全体成果物保証が残っているか
- no-tool finalization は plan-run step options のみ source parity へ寄せているか
- implement step の未作業完了を許していないか
- verify/setup/inspect の no-write completion を runner が検証しているか
- repair retry も initial step と同じ options を使っているか
- `/run-plan`、TUI slash、`ultra-plan-run` に同じ policy が適用されているか
- `ultra-step-run` に同じ policy が適用されているか
- event / metrics は prompt 本文や絶対パスを保存していないか
- plan final contract / obligation scope 系 failure が unclassified になっていないか
- scenario 固有分岐、固定 artifact 名分岐、provider 固有の成功扱いを入れていないか

## Full Test Command Set

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml minimal_loop
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::runner
cargo test --manifest-path mvp/anvilminimal/Cargo.toml
python3 -m unittest discover -s mvp/anvilminimal/tests/eval -p 'test_*.py'
cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml
```

## Rollback Plan

- Phase 1 の options API は default behavior を維持するため、plan-run 呼び出し側の options 適用を外せば既存挙動へ戻せる
- Phase 2 / Phase 3 の変更は `planner/runner.rs` の options helper と `minimal_loop/loop_run.rs` の options branch に閉じ込める
- Phase 5 の plan-level final contract は helper 呼び出しを外せば切り戻せるが、Phase 2 とセットで扱う。Phase 2 だけ残して Phase 5 を戻すと全体成果物保証が弱くなる
- Phase 6 の metrics は missing event compatible にし、old summary / anvildev summary を壊さない

## Final Done Definition

- Phase 0〜9 の作業が完了している
- Rust / Python tests が通っている
- release build が通っている
- MVP plan-run eval が baseline から改善している
- `required_artifacts_missing` と `missing_tool_call` が primary gate 内に収まっている
- `step_obligation_scope_score` が plan-run rows に出力されている
- plan-level final contract verification が必要成果物欠落を検出できる
- plan-level final contract failure が分類/summary で追跡でき、unclassified に落ちない
- `minimal-loop --prompt` の既存挙動が regression していない
- `/run-plan`、TUI slash、`ultra-plan-run`、`ultra-step-run` の step execution scope が同じ方針になっている
- anvildev 比較結果と残差理由が plan 本文へ追記されている
- eval scenario 固有分岐が入っていないことをレビュー済み

## Execution Status 2026-06-26

Phase 0〜9 は実施済み。

| Phase | status | evidence |
| --- | --- | --- |
| Phase 0 | done | baseline / source parity evidence を plan 本文に固定 |
| Phase 1 | done | `RunSessionOptions` と default behavior 非回帰テストを追加 |
| Phase 2 | done | plan-run step で prompt artifact extraction / completion contract step effects を無効化 |
| Phase 3 | done | plan-run step の no-tool finalization を source parity へ修正 |
| Phase 4 | done | `RunSessionStepKind` と step-kind options を runner 経由で適用 |
| Phase 5 | done | plan-level final contract verification を追加 |
| Phase 6 | done | `step_obligation_scope_score` / classification / report schema を追加 |
| Phase 7 | done | Rust / Python regression tests を追加・更新 |
| Phase 8 | done | MVP plan-run 2回、anvildev comparison、cross-mode smoke を実行 |
| Phase 9 | done | plan 本文と本 work breakdown に結果・残リスクを反映 |

### Commands verified

```bash
python3 -m unittest discover -s mvp/anvilminimal/tests/eval -p 'test_*.py'
cargo test --manifest-path mvp/anvilminimal/Cargo.toml minimal_loop
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::runner
cargo test --manifest-path mvp/anvilminimal/Cargo.toml
cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml
```

### Eval evidence

- MVP plan-run:
  - `/private/tmp/anvilminimal-eval-011-mvp-plan-run-2-net-real`: 10/12
  - `/private/tmp/anvilminimal-eval-011-mvp-plan-run-3-net-real`: 9/12
- anvildev plan-run:
  - `/private/tmp/anvilminimal-eval-011-anvildev-plan-run-1-net-real`: 9/12
- MVP cross-mode:
  - `/private/tmp/anvilminimal-eval-011-mvp-cross-mode-1-net-real`
  - plan-run 9/12
  - step-plan 12/12
  - ultra-plan-run 7/12
  - ultra-step-run 12 skipped

### Gate result

- `required_artifacts_missing`: 0 in post-change MVP plan-run runs
- `missing_tool_call`: 0 in post-change MVP plan-run runs
- `step_obligation_scope_score`: 100.0 in MVP plan-run rows
- `prompt_contract_score`: 100.0
- `plan_run_runtime_health_score`: 68.4〜74.3
- remaining failures are planner verify policy, postcheck, step verify, or ultra scaffold/schema/lint; not the Phase 011 step obligation scope issue

### Follow-up candidates

- `verify_command_policy_error`: planner repair / verify command sanitizer の改善
- `postcheck_failure`: execution repair feedback と plan quality の改善
- `step_verify_failure`: bounded repair feedback と verify command generation の改善
- `ultra-plan-run`: phase scaffold / schema / lint の別計画化
