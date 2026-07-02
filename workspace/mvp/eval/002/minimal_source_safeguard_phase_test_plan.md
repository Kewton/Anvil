# SG-01〜SG-38 対応 Phase 計画とテスト計画

作成日: 2026-06-25

対象棚卸し:

- `workspace/mvp/eval/002/minimal_source_safeguard_gap_inventory.md`

## 目的

MVP `anvilminimal` に対して、直接移植元 minimal 系の安全装置を source parity 基準で段階移植する。

## 実施ステータス

2026-06-25 時点で、残SG根本対策計画の Phase R0〜R8 を実施済み。

- `mvp/anvilminimal/tests/safety_parity_traceability.rs` の対象SGは全て実テスト名へ対応済み。
- `defer:` が残った場合に失敗する `safety_traceability_has_no_defer_after_remaining_sg_completion` を追加済み。
- 主要 verification は `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` で green。最終確認として clippy `-D warnings` を実施する。

ここでの source parity は、旧 heavy loop の artifact ledger / no-progress recovery を含まない。対象は `src/agent/minimal_*` と、その minimal 実行経路から直接使われる `src/tools/*` / `src/util/workspace_paths.rs` の安全装置に限定する。

検証面では、MVP の入口である `mvp/anvilminimal/src/providers/*`、`src/tui/*`、`src/eval_events.rs`、`tests/*` も対象に含める。ただし、これらは安全装置そのものの追加SGではなく、provider差・TUI操作性・eval診断を確認するための受け皿として扱う。

## 実装原則

- 先に unit / fake-client regression を追加し、source safety の意味を固定する。
- `required_paths` のような同名・類似機能は、source と同じ停止位置・feedback位置まで確認する。
- provider live eval は最後に回し、まず deterministic fake-client / local process test で収束条件を確認する。
- 旧 heavy loop の no-progress recovery は混ぜない。必要なら別計画で扱う。
- TUI / CLI 操作性は維持する。`anvilminimal --prompt`、TUI slash command、`--plan-run`、`--ultra-plan-run` の既存入口を壊さない。

## レビュー反映

- R-001: `SG-12` が Phase一覧と Phase 2 の対象SGから漏れていたため、prompt安全規則として Phase 2 に追加する。
- R-002: MVP crate は `mvp/anvilminimal/Cargo.toml` 配下の独立 workspace なので、smoke の Rust test command は `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` に固定する。
- R-003: Phase 0 の red regression 確認は「Phase 0時点の現行MVPで失敗すること」と明記し、実装後の green 条件と混同しないようにする。
- R-004: `workspace/mvp/eval/002/minimal_source_safeguard_gap_inventory.md` との観点別レビュー結果を反映し、SG追加なしで対象範囲・Phase依存・TUI/CLI smoke・eval観測性を補強する。
- R-005: SG-24 は source parity の既定を「phaseごとにprofile verificationで止める」と明記し、final-only repair は明示的 opt-in として扱う。
- R-006: 作業具体化レビュー結果を反映し、red baseline は default test suite に失敗を残さない方針へ明確化する。
- R-007: eval script path と live provider smoke command を実際の `mvp/anvilminimal` 構成に合わせる。
- R-008: `--test live_provider -- --ignored` は Ollama ignored tests も巻き込むため、OpenAI/Gemini live smoke は `live_openai` / `live_gemini` filter付きに分離する。
- R-009: 完了条件レビューに基づき、artifact path negative test、provider tool-call既知障害、TUI実操作、eval数値ゲート、Bash終了保証を追加する。

## レビュー観点

| 観点 | 判定 | 計画への反映 |
|---|---|---|
| SG網羅性 | 棚卸しと本計画はいずれも SG-01〜SG-38 の38件で一致 | Phase 0 / Definition of Done で機械的差分ゼロ確認を維持 |
| source parity境界 | direct minimal に存在しない汎用 no-progress detector は本計画へ混ぜない | no-progress detector は別計画候補に留める |
| Phase依存 | 成果物契約、feedback、plan契約、repair、tool安全性、profile契約の順序は概ね妥当。ただし SG-24 の既定挙動が曖昧 | Phase 6 を source parity default stop に修正 |
| テスト可能性 | unit/fake-client中心は妥当。TUI/CLI 操作性の smoke が不足 | Phase 7 smoke に CLI/TUI entrypoint tests を追加 |
| eval観測性 | stop reason だけでは provider schema failure と loop convergence failure の切り分けが不足 | Phase 7 raw event に tool call名、arguments shape、provider error kind を追加 |
| 運用リスク | `.env` live provider とローカルLLMなし speed-cloud は分ける必要がある | Phase 7 で speed-cloudを先に実行し、local LLM は別枠の非ブロッキングにする |
| landing時テスト方針 | Phase 0 の red baseline は実装前確認であり、完了時に失敗テストを残す条件ではない | red確認後、landing時はgreen化または理由付き `#[ignore]` にする |
| 実行コマンド | root から実行する前提では `scripts/eval-run.py` が誤り | `mvp/anvilminimal/scripts/eval-run.py` に統一 |
| live smoke分離 | `live_provider.rs` には OpenAI/Gemini/Ollama の ignored tests が同居する | speed-cloud のOpenAI/Gemini smokeは filter付きで実行し、OllamaはローカルLLM smokeとして別扱い |
| 完了条件の判定力 | smoke有無だけでは、path安全性、provider shape、TUI実操作、eval品質、Bash終了保証を十分に判定できない | 各Phaseの negative test と Phase 7 数値ゲートに追加 |

## Phase一覧

| Phase | 主目的 | 対象SG | 優先度 |
|---|---|---|---|
| Phase 0 | traceability と失敗再現テストの土台 | 全SG | P0 |
| Phase 1 | loop停止と成果物契約を先に直す | SG-01, SG-02, SG-03, SG-06, SG-33 | P0 |
| Phase 2 | no-tool / feedback / prompt / 履歴衛生を source parity に寄せる | SG-04, SG-05, SG-07, SG-08, SG-09, SG-10, SG-12, SG-27, SG-28, SG-29, SG-30 | P0/P1 |
| Phase 3 | deterministic verify と plan契約を強化する | SG-13, SG-14, SG-15, SG-16, SG-17, SG-18, SG-21, SG-22, SG-37 | P1 |
| Phase 4 | repair収束と失敗診断を強化する | SG-19, SG-20, SG-26 | P0/P1 |
| Phase 5 | tool安全性とcontext保護を強化する | SG-11, SG-31, SG-32, SG-35, SG-36, SG-38 | P1/P2 |
| Phase 6 | ultra/profile契約をsource parityに近づける | SG-23, SG-24, SG-25, SG-34 | P1 |
| Phase 7 | eval分類・回帰suite・live provider検証 | 全SG、特に SG-01, SG-03, SG-19, SG-24, SG-26, SG-33 | P0 |

## Phase 0: Traceability と失敗再現テストの土台

### 実装内容

- SG-01〜SG-38 をコード上のテスト名へ対応付ける `tests/safety_parity_*` の構成を決める。
- fake `ChatClient` で次の失敗を deterministic に再現できるようにする。
  - toolを出し続けるが expected path は作成済み
  - no-tool final だが file change がない
  - requested artifact が未作成
  - invalid step plan が single huge step に落ちる
  - repair が missing path を減らさない
- eval fixture 側では `expected_artifacts` を prompt 内契約として注入できるテスト用 scenario を追加する。

### テスト計画

- Rust unit:
  - `safety_traceability_all_sg_have_test_or_defer_note`
  - `fake_client_can_reproduce_tool_only_max_iteration`
  - `fake_client_can_reproduce_no_tool_completion_without_write`
- Python eval unit:
  - `test_expected_artifacts_can_be_rendered_as_required_final_artifacts`
  - `test_failure_kind_max_iterations_snapshot`

### 受け入れ条件

- SG-01〜SG-38 の各項目に、実装対象テスト名または明示的な defer 理由がある。
- Phase 0時点の現行MVPで、少なくとも SG-01 / SG-03 / SG-19 の regression test が失敗することを確認できる。
- その red regression は実装前確認として記録し、Phase完了時の default test suite には失敗テストを残さない。

## Phase 1: Loop停止と成果物契約

### 対象SG

- SG-01: post-tool `early_success_paths`
- SG-02: plan-run step expected_paths の early success 化
- SG-03: `minimal-loop --prompt` への成果物契約注入
- SG-06: requested artifacts missing feedback
- SG-33: Required final artifacts の phase / step / repair 継承

### 実装内容

- `run_session_with_required_paths_with_ui` で、tool実行後にも `required_paths` を確認する。
- source と同じ意味の `early_success_paths` を config または call引数として明示する。
- eval `expected_artifacts` を `minimal-loop --prompt` へ渡すため、promptに `Required final artifacts` block を追加する。
- `--prompt` 通常実行でも prompt内の明示 path を抽出し、不足時feedbackに使う。
- prompt path extraction は workspace-relative artifact のみ許可し、`../`, absolute path, symlink escape, `.anvil`, `target`, `node_modules` を拒否する。
- `plan-run` / `ultra-plan-run` の generated prompt に `Required final artifacts` を継承する。

### テスト計画

- Rust unit:
  - `early_success_paths_stop_after_tool_execution`
  - `plan_step_expected_paths_stop_after_tool_execution`
  - `prompt_requested_artifact_feedback_then_write`
  - `requested_artifact_path_extraction_rejects_escape_and_metadata_paths`
  - `requested_artifact_path_extraction_rejects_symlink_escape`
  - `required_final_artifacts_are_preserved_in_step_prompt`
  - `required_final_artifacts_are_preserved_in_ultra_phase_prompt`
- Python eval unit:
  - `test_minimal_loop_command_includes_expected_artifacts_contract`
  - `test_postcheck_expected_artifacts_match_required_final_artifacts`
- Integration:
  - fake client: `Write(package.json)` を返し続ける client が max_iterations 前に成功停止する。

### 受け入れ条件

- expected path 作成済みの場合、final assistant応答がなくても loop が成功停止する。
- `minimal-loop --prompt` でも eval scenario の `expected_artifacts` が loop内feedbackに使われる。
- requested artifact path extraction が workspace外、controller metadata、generated artifacts を成果物契約へ入れない。
- `plan-run` の各stepで expected path が揃ったら verify に進む。
- `minimal loop reached max_iterations (12)` の exact件数が、該当fake regressionで 0 になる。

## Phase 2: Feedback / Prompt / 履歴衛生

### 対象SG

- SG-04: completion without write
- SG-05: planned action without tool
- SG-07: missing relative imports
- SG-08: Edit anchor mismatch feedback
- SG-09: recoverable tool error as feedback
- SG-10: ephemeral feedback / failed assistant discard
- SG-12: system prompt safety rules
- SG-27: empty response feedback
- SG-28: missing tool call for action prompt
- SG-29: XML fallback prompt mode
- SG-30: tool-call assistant preamble removal

### 実装内容

- source の `FeedbackState` 相当を MVP に追加する。
- no-tool失敗応答は session history から破棄し、feedbackは次requestだけに注入する。
- `completion_without_write_feedback` と `requested_artifact_feedback` は env flag で無効化可能にする。
- frontend source の relative import 解決チェックを final前に実施する。
- Edit anchor mismatch は hard error ではなく、Readし直しを促す feedback に変換する。
- source prompt の主要安全規則を MVP system prompt に入れる。
  - repository/file事実が必要な場面では tool を使う
  - final response を「これから実施する予定」だけで終えない
  - 観測していない file/test/build 結果を成功扱いしない
  - path は workspace-relative に扱い、workspace外へ出ない
- XML fallback 時は system prompt を XML用に切り替え、XML tool call例を入れる。
- prompt履歴へ渡す assistant tool-call message は content を空にする。

### テスト計画

- Rust unit:
  - `empty_response_gets_one_retry_feedback`
  - `completion_without_write_feedback_then_write_then_complete`
  - `planned_action_without_tool_discards_failed_assistant`
  - `missing_tool_call_only_for_action_prompts`
  - `missing_relative_import_gets_repair_prompt`
  - `edit_anchor_feedback_is_ephemeral`
  - `system_prompt_contains_source_safety_rules`
  - `xml_fallback_prompt_keeps_source_safety_rules`
  - `tool_call_assistant_preamble_is_not_reprompted`
  - `xml_fallback_prompt_contains_tool_call_example`
- Integration:
  - TUI fake clientで feedback 後も raw assistant history が汚れないことを確認する。

### 受け入れ条件

- no-tool失敗応答は session save 後にも残らない。
- feedback message は最終session履歴に永続化されない。
- action prompt に no-tool final が返った場合、少なくとも1回は tool使用を促す。
- Native tool call / XML fallback の両方で、system prompt に source prompt 相当の安全規則が含まれる。
- XML fallback後、次requestのsystem promptに `<anvil_tool_call>` 例がある。

## Phase 3: Verify と Plan契約

### 対象SG

- SG-13: verify command allowlist
- SG-14: typed `StepKind` / `expected_result`
- SG-15: step kind contract
- SG-16: semantic plan lint
- SG-17: invalid planner output corrective retry
- SG-18: step / repair max iteration cap
- SG-21: verification failure aggregation
- SG-22: verifier precondition failure
- SG-37: plan / ultra validation 強化

### 実装内容

- MVP `PlanStep` に typed kind と `expected_result` を追加する。
- YAML parse/render を backward compatible にし、既存planは `kind=work`, `expected_result=pass` として読む。
- `validate_verify_command` を source同等の allowlist に近づける。
- `verify_step` は先頭1件で止めず、missing paths と command failures を複数保持する。
- `npm run build` + Next dependency + missing `node_modules/.bin/next` を dependency_missing として分類する。
- invalid generated plan は `StepPlan::single(goal)` に落とさず、最大3回 corrective prompt を返す。
- plan/ultra validation で duplicate id、id文字種、長さ、shell command風 instruction、REPL command風 phase を拒否する。
- step実行とrepair実行の max_iterations cap を source相当へ近づける。

### テスト計画

- Rust unit:
  - `verify_command_rejects_shell_control_syntax`
  - `verify_command_rejects_install_or_network_setup`
  - `step_kind_contract_rejects_setup_with_build_verify`
  - `verify_expected_result_fail_requires_command`
  - `verify_step_aggregates_missing_paths_and_command_failures`
  - `nextjs_build_missing_next_binary_is_dependency_missing`
  - `invalid_planner_output_gets_corrective_retry`
  - `duplicate_step_ids_are_rejected`
  - `shell_command_instruction_is_rejected`
  - `ultra_phase_repl_command_is_rejected`
- Python eval unit:
  - plan scoring fixtureで expected_paths / verify / responsibility boundary が維持されること。

### 受け入れ条件

- invalid planner output が single huge step に fallback しない。
- verify command に `&&`, `|`, `npm install`, long-running dev server が混入した plan を拒否する。
- TDD red step の `expected_result: fail` が parse/render/verify で保持される。
- plan-run の step単位 failure が複数 failureを含む診断になる。

## Phase 4: Repair収束と失敗診断

### 対象SG

- SG-19: progress-aware bounded repair
- SG-20: repair exhausted report
- SG-26: stop reason / eval classification

### 実装内容

- repair loop を最大 turn数と file-changing repair数で bounded にする。
- initial turn / repair turn の `Write` / `Edit` path を収集する。
- missing expected paths が減っていない場合、repair prompt に progress warning を入れる。
- repair exhausted report に以下を入れる。
  - missing expected paths
  - changed files
  - repeated changed files
  - verification failures
  - initial / repair stop reason
  - suggested replan command
- plan-run / ultra-plan-run の出力に step stop reason を含める。
- eval failure classification に `max_iterations`, `verification_failed_after_max_iterations`, `repair_exhausted` を追加する。

### テスト計画

- Rust unit:
  - `repair_prompt_includes_no_expected_path_progress_warning`
  - `repair_turns_are_bounded`
  - `file_changing_repair_attempts_are_bounded`
  - `repair_exhausted_report_includes_missing_paths_and_changed_files`
  - `repaired_after_max_iterations_stop_reason_is_reported`
- Python eval unit:
  - `test_classify_max_iterations`
  - `test_classify_repair_exhausted`
  - `test_report_groups_max_iterations_separately`
- Integration:
  - fake clientで初回 max_iterations 後、repairで expected path作成し成功する。

### 受け入れ条件

- repairが同じfileだけを繰り返す場合、専用warningがpromptに入る。
- repair exhausted時に人間が次に見るべき path / failure / command が report から分かる。
- eval report で `minimal loop reached max_iterations` が `unclassified_process_failure` にならない。

## Phase 5: Tool安全性とContext保護

### 対象SG

- SG-11: workspace policy
- SG-31: compaction evidence protection
- SG-32: Bash timeout / cancel / classifier
- SG-35: Read/Glob/Grep output cap / filtering
- SG-36: Edit fallback
- SG-38: Bash output shaping

### 実装内容

- source の workspace policy を MVP へ移植または同等簡易実装する。
- Read/Glob/Grep で `.git`, `.anvil`, `node_modules`, `target`, generated metadata を通常探索から除外する。
- large Read と large Bash/cat output は head/tail summary と focused read guidance にする。
- Grep は最大hit数と最大出力文字数で制限する。
- compact は最新 user、直近 Read/Edit tool result、既存 summary 置換を保護する。
- Bash に timeout/cancel flag/process termination を導入する。
- Edit に already-applied/no-op/normalized-line/token-anchor fallback を追加する。

### テスト計画

- Rust unit:
  - `workspace_policy_hides_controller_metadata_from_glob`
  - `read_large_file_is_summarized`
  - `grep_output_is_bounded`
  - `compaction_keeps_recent_read_and_edit_outputs`
  - `edit_already_applied_is_success`
  - `edit_normalized_line_fallback_succeeds`
  - `bash_long_running_command_times_out`
  - `bash_cancel_flag_terminates_child`
  - `bash_timeout_terminates_child_process`
  - `bash_timeout_reports_structured_timeout_outcome`
  - `bash_cancel_reports_structured_cancelled_outcome`
  - `bash_large_cat_output_is_summarized`
- Integration:
  - workspaceに `node_modules`, `.anvil`, `target` を置き、Glob/Grep の通常結果に出ないことを確認する。

### 受け入れ条件

- large file / large command output で context budget が急増しない。
- compaction後も最新依頼と修復に必要なRead/Edit証跡が残る。
- ESC interrupt / cancel flag で長時間Bashが止まる。
- Bash timeout / cancel 後に子プロセスが残らず、tool result に timeout/cancelled の structured outcome が残る。
- Editの空白差や既適用patchで無駄にrepairへ落ちない。

## Phase 6: Ultra/Profile契約

### 対象SG

- SG-23: profiled phase prompt
- SG-24: per-phase profile verification
- SG-25: Next.js profile contract
- SG-34: data profile protected input verification

### 実装内容

- ultra phase prompt に original goal、required final artifacts、profile/style/intent、workspace snapshot、runtime contract を入れる。
- profile verification を phase 後に実施し、source parity の既定では non-final phase の `ProfileContractFailed` でも即停止する。
- final-only profile repair は明示的 opt-in 設定に限定し、既定では使わない。
- Next.js contract に source の不足分を追加する。
  - Tailwind directives と依存/config整合
  - `tsconfig.rootDir` が `app/` を排除しない
  - `@/*` alias と `compilerOptions.paths`
  - `scripts.build` の弱体化検出
- MVP 既存の entry/layout/deps/build/dev checks は弱体化させず、source不足分だけを追加する。
- data profile で raw/input data の protected snapshot を取り、削除/サイズ変更を検出する。

### テスト計画

- Rust unit:
  - `profiled_phase_prompt_contains_original_goal_and_contract`
  - `nextjs_contract_rejects_tailwind_without_toolchain`
  - `nextjs_contract_rejects_rootdir_src_excluding_app`
  - `nextjs_contract_rejects_build_script_weakening`
  - `nextjs_contract_keeps_existing_entry_and_layout_checks`
  - `non_final_profile_contract_failure_stops_by_default`
  - `final_only_profile_repair_requires_explicit_opt_in`
  - `data_profile_snapshot_protects_raw_inputs`
  - `data_profile_detects_raw_input_modification`
- Integration:
  - fake ultra planで中間phaseがprofile contractを壊した時の挙動を確認する。

### 受け入れ条件

- Next.js profileで `scripts.build = echo ok` のような偽成功が通らない。
- non-final phase の profile contract violation は既定で phase failure になる。
- final phase まで profile repair を遅延する挙動は明示的 opt-in なしでは有効にならない。
- data profileで raw input data の削除/改変が pass しない。
- ultra phase内の step planner が original goal / port / expected artifacts を失わない。

## Phase 7: Eval分類・回帰suite・Live Provider検証

### 対象SG

- 全SG
- 特に SG-01, SG-03, SG-19, SG-24, SG-26, SG-33

### 実装内容

- SG別 regression scenario を eval suite に追加する。
- `minimal-loop`, `plan-run`, `ultra-plan-run` の各modeで source safety が効くケースを最低1つずつ置く。
- provider live は speed-cloud profile で OpenAI / Gemini のみ先に実施し、ローカルLLMは別枠にする。
- raw event に stop reason / missing paths / repair exhausted summary / raw tool call名 / arguments shape / provider error kind を保存する。
- CLI/TUI entrypoint smoke を追加し、既存操作性を regression として固定する。
- OpenAI `function_call.arguments` string decode と Gemini function calling schema を provider regression として固定する。
- TUI banner、ESC interrupt、代表 `/ultra-plan-run --profile nextjs ...` 入力を PTY smoke で固定する。

### テスト計画

- Python eval:
  - `test_eval_minimal_loop_early_success`
  - `test_eval_plan_run_repair_progress`
  - `test_eval_ultra_plan_required_artifacts_carryover`
  - `test_eval_failure_kind_max_iterations_not_unclassified`
  - `test_eval_report_contains_stop_reason_breakdown`
  - `test_eval_events_include_tool_call_shape_and_provider_error_kind`
  - `test_eval_quality_gates_require_no_unclassified_failures`
  - `test_eval_quality_gates_require_required_artifacts_postcheck_pass`
- Provider unit/integration:
  - `openai_function_call_arguments_string_is_decoded`
  - `gemini_function_call_request_uses_current_tool_schema`
  - `provider_tool_call_shape_errors_are_classified`
- Smoke:
  - `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`
  - `cargo test --manifest-path mvp/anvilminimal/Cargo.toml --test cli_parse`
  - `cargo test --manifest-path mvp/anvilminimal/Cargo.toml --test tui_repl`
  - `cargo test --manifest-path mvp/anvilminimal/Cargo.toml --test tui_pty`
  - `cargo test --manifest-path mvp/anvilminimal/Cargo.toml --test tui_integration`
  - `ANVIL_LIVE_PROVIDER_TESTS=1 cargo test --manifest-path mvp/anvilminimal/Cargo.toml --test live_provider live_openai -- --ignored` with `.env` `OPENAI_API_KEY` when live smoke is requested
  - `ANVIL_LIVE_PROVIDER_TESTS=1 cargo test --manifest-path mvp/anvilminimal/Cargo.toml --test live_provider live_gemini -- --ignored` with `.env` `GEMINI_API_KEY` when live smoke is requested
  - `pytest mvp/anvilminimal/tests/eval`
  - `mvp/anvilminimal/scripts/eval-run.py --model-profile speed-cloud --modes minimal-loop,plan-run,ultra-plan-run --runs 1`

### 受け入れ条件

- speed-cloud / local LLMなし eval で `max_iterations` が専用分類される。
- exact `minimal loop reached max_iterations (12)` は Phase 1/4 対象の fake regression では 0。
- deterministic fake eval suite で `unclassified_process_failure=0`、Phase 1/4 対象scenarioの `max_iterations=0`、required artifacts postcheck pass rate 100% を満たす。
- live provider eval で provider schema error と loop収束 failure が別分類される。
- OpenAI `function_call.arguments` string、Gemini function calling schema の既知障害が unit/integration test で固定される。
- raw event だけで provider schema failure、tool-call argument shape failure、loop convergence failure を切り分けられる。
- TUI `/plan-run` / `/ultra-plan-run` の slash command smoke が fake provider で通る。
- TUI起動時に banner が表示され、ESC が running turn を interrupt し、代表 `/ultra-plan-run --profile nextjs ...` 入力が受理される。
- reportに mode/provider/scenario 別の stop reason breakdown が出る。

## Phase間依存

| 依存 | 理由 |
|---|---|
| Phase 1 before Phase 4 | repair強化前に early success / expected artifacts の意味を固定する必要がある |
| Phase 2 before Phase 5 | history hygiene がないと compaction/tool output改善の効果を測りにくい |
| Phase 3 before Phase 6 | profile/ultra contract は typed step / verify contract に依存する |
| Phase 4 before Phase 7 | eval分類は repair / stop reason の出力に依存する |
| Phase 5 before Phase 7 full live eval | workspace policy / output shaping がないと live eval の失敗原因がcontext膨張や内部metadata参照に埋もれる |

## 優先実装順

短期で `minimal loop reached max_iterations` に効かせる順序:

1. Phase 1
2. Phase 2 の SG-27 / SG-28 / SG-04 / SG-05
3. Phase 4
4. Phase 7 の max_iterations分類と raw event 保存

品質とsource parityを戻す順序:

1. Phase 3
2. Phase 5
3. Phase 6
4. Phase 7 full eval

## Definition of Done

- SG-01〜SG-38 がすべて、実装済み・意図的defer・別計画へ移管のいずれかで分類されている。
- 実装済みSGには unit/integration/eval の少なくとも1つの regression test がある。
- default test suite に意図的な失敗テストが残っていない。red baseline は実装前確認として記録し、landing時はgreen化または理由付き `#[ignore]` にする。
- `minimal-loop --prompt` / TUI `/plan-run` / TUI `/ultra-plan-run` の既存操作性が維持されている。
- 代表操作 `anvilminimal --yes --context-budget 65536 --model qwen3.6:27b-coding-nvfp4 --planner-model gemini-3.5-flash --planner-provider gemini --provider ollama` と TUI `/ultra-plan-run --profile nextjs ...` が regression として固定されている。
- requested artifact path extraction の negative test が通り、workspace外・metadata・generated artifact・symlink escape が成果物契約に混入しない。
- OpenAI / Gemini の live provider smoke は `.env` と `ANVIL_LIVE_PROVIDER_TESTS=1` がある時に provider filter 付きで実行でき、Ollama live smoke を巻き込まない。
- OpenAI / Gemini live smoke で provider schema failure と loop convergence failure が分離して分類される。
- OpenAI arguments string decode と Gemini function calling schema の provider regression test が通る。
- `.env` の `OPENAI_API_KEY` / `GEMINI_API_KEY` を使う eval は、ローカルLLMなしの speed-cloud profile で実行できる。
- deterministic fake eval suite で `unclassified_process_failure=0`、対象scenarioの `max_iterations=0`、required artifact postcheck pass rate 100% を満たす。
- Bash timeout/cancel 後に子プロセスが残らず、structured timeout/cancelled outcome が記録される。
- `workspace/mvp/eval/002/minimal_source_safeguard_gap_inventory.md` のSG一覧と、このPhase計画の対象SGに差分がない。
- SG差分確認は `rg -o "SG-[0-9]{2}" <file> | sort -u` 相当で機械的に実施し、両文書とも38件であることを確認する。
- 旧 heavy loop 由来の汎用 no-progress detector は、本計画の完了条件に混ぜない。必要なら別計画で追加する。

## 残SG根本対策によるDoD追補

`workspace/mvp/eval/002/minimal_source_remaining_sg_completion_plan.md` の作成後は、次のSGについて「意図的defer・別計画へ移管」を完了扱いしない。

- `SG-07`, `SG-08`, `SG-11`
- `SG-14`, `SG-15`, `SG-16`, `SG-18`, `SG-19`, `SG-20`, `SG-21`
- `SG-25`, `SG-31`, `SG-32`, `SG-34`, `SG-35`, `SG-36`, `SG-38`

上記SGの最終完了条件は、残SG計画の R8 に従う。

- `mvp/anvilminimal/tests/safety_parity_traceability.rs` から対象SGの `defer:` が消えている。
- 対象SGすべてに実テスト名が対応している。
- SG別 fixture matrix で `unknown=0`、pass rate 100% を満たす。
- deterministic fake eval で `unclassified_process_failure=0`、`max_iterations=0`、required artifact postcheck pass 100% を満たす。
- `workspace/mvp/eval/002/minimal_source_safeguard_phase0_7_implementation_report.md` が更新され、前回 `defer` としたSGが完了済みに移っている。
