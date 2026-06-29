# UltraPlan Generation Source Parity Work Breakdown

作成日: 2026-06-29

対象計画:

- `workspace/mvp/eval/021-1/ultra_plan_generation_source_parity_plan.md`

## 0. 目的

Phase 021-1 では、`/ultra-plan-run` のうち **UltraPlan 生成だけ** を移植元 anvil の契約へ寄せて戻す。

この work breakdown の目的は、以下を実装可能な作業単位へ分解することである。

- system prompt / user prompt の復元
- profile generation rules の注入
- invalid output retry
- planner tool call rejection
- generated metadata normalization
- retry exhaustion 時の fail-fast
- deterministic fallback を通常成功 plan として扱わないこと
- eval / tests で planner failure と runtime failure を分離できること

## 1. スコープ境界

### 対象

- `generate_ultra_plan_with_ui` の入力 prompt / retry / fail-fast。
- UltraPlan 生成時の profile rules。
- UltraPlan 生成時の event / failure classification。
- UltraPlan 生成の unit / integration test。
- MVP only の targeted eval / UAT。

### 非対象

- phase-aware verification。
- phase 間 context continuity。
- runtime recovery / repair handoff。
- `.anvil/runs` 常時 run log。
- final acceptance / capability oracle。
- Next.js / Space Invaders 専用テンプレート。
- UltraPlan YAML schema 変更。
- JSON UltraPlan parser への移行。

## 2. 追跡 ID

| ID | 内容 | 主 Phase |
| --- | --- | --- |
| UPG-01 | UltraPlan 生成が user prompt 1本で profile-unaware | 1, 2 |
| UPG-02 | system prompt / profile rules が未移植 | 1, 2 |
| UPG-03 | invalid output retry が UltraPlan 生成にない | 3 |
| UPG-04 | planner tool call rejection が明示されていない | 3 |
| UPG-05 | retry exhaustion で deterministic fallback が成功 plan になる | 4 |
| UPG-06 | fallback / planner failure の診断分類が粗い | 5, 6 |
| UPG-07 | Next.js topology を hard lint に入れすぎるリスク | 2, 5 |
| UPG-08 | invalid planner output 時の workspace mutation を防ぐテスト不足 | 7 |
| UPG-09 | rollback 後 baseline と改善後の比較条件が曖昧 | 0, 8 |
| UPG-10 | generated metadata を request context で正規化する source 契約が未反映 | 3, 7 |

## 3. 予定成果物

### Rust 実装候補

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/planner/lint.rs`
- `mvp/anvilminimal/src/planner/ultra_plan.rs`
- `mvp/anvilminimal/src/planner/profile.rs`
- `mvp/anvilminimal/src/planner/profiles/nextjs.rs`
- `mvp/anvilminimal/src/eval_events.rs`

### Eval 実装候補

- `mvp/anvilminimal/scripts/eval_lib/failure_classification.py`
- `mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py`
- `mvp/anvilminimal/scripts/eval_lib/run_summary.py`
- `mvp/anvilminimal/scripts/eval_lib/report.py`

### テスト候補

既存の `runner.rs` 内 unit test を基本に、必要なら integration test を追加する。

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/planner/lint.rs`
- `mvp/anvilminimal/tests/planner_ultra_plan_generation.rs`
- `mvp/anvilminimal/tests/eval/test_failure_classification.py`
- `mvp/anvilminimal/tests/eval/test_runtime_scoring.py`

### 調査 / 結果文書

- `workspace/mvp/eval/021-1/ultra_plan_generation_source_parity_baseline.md`
- `workspace/mvp/eval/021-1/ultra_plan_generation_source_parity_implementation_result.md`
- `workspace/mvp/eval/021-1/ultra_plan_generation_source_parity_eval_result.md`

## 4. 実施順

実施順は以下を固定する。

1. Phase 0: baseline / source contract 固定
2. Phase 1: prompt contract 設計
3. Phase 2: profile generation rules 接続
4. Phase 3: retry / tool-call rejection
5. Phase 4: deterministic fallback の fail-fast 化
6. Phase 5: lint / quality guidance / classification 整理
7. Phase 6: eval event / report 指標
8. Phase 7: integration / no-workspace-mutation 検証
9. Phase 8: targeted eval / UAT / 結果整理

Phase 4 で workspace mutation 前 fail-fast を入れるため、Phase 7 までは通常 TUI の成功率だけで判断しない。  
Phase 8 で「成功率の増減」と「false success の減少」を分けて評価する。

## Phase 0: baseline / source contract 固定

### 目的

rollback 後の現状と、移植元 anvil の UltraPlan 生成契約を、実装前に固定する。

### 作業

1. `test0628_001` / `test0628_002` の保存済み UltraPlan が deterministic fallback 形であることを文書化する。
2. MVP current code の `generate_ultra_plan_with_ui` の挙動を整理する。
3. 移植元の `generate_ultra_plan`、`ultra_plan_generation_system_prompt`、`profile_generation_rules`、retry / fail-fast を整理する。
4. 021-1 では JSON parser 移行をしない理由を明記する。
5. 021-1 では UltraPlan schema 変更をしない理由を明記する。
6. `workspace/mvp/eval/021-1/ultra_plan_generation_source_parity_baseline.md` を作成する。

### 参照対象

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/planner/ultra_plan.rs`
- `src/agent/minimal_step_runner.rs`
- `src/agent/minimal_step_runner/profile.rs`
- `src/agent/minimal_step_runner/profiles/nextjs.rs`

### 受入条件

- MVP current behavior と source behavior の差分が、prompt / retry / fallback / tool-call rejection の観点で説明できる。
- `test0628_001` 型の failure が deterministic fallback plan と関連付けて説明できる。
- 021-1 で扱う範囲と扱わない範囲が文書上で明確になっている。

### テスト / 確認

- 文書レビューで UPG-01〜UPG-09 に対応できること。
- 実装前 baseline として後続 eval 結果と比較できること。

## Phase 1: prompt contract 設計

### 目的

UltraPlan 生成に system prompt / user prompt を導入し、source parity の planner contract を MVP YAML shape に落とす。

### 作業

1. `runner.rs` に UltraPlan 生成専用 helper を追加する。
   - `ultra_plan_generation_system_prompt(profile, style, intent)`
   - `ultra_plan_generation_user_prompt(goal, profile, style, intent)`
   - `ultra_plan_generation_messages(goal, config)`
2. system prompt に以下を含める。
   - planner は tools を実行しない。
   - output は MVP UltraPlan YAML shape。
   - phase count は原則 2〜6、最大 8。
   - phase prompt は shell command / REPL command ではない。
   - concrete outcome と verification expectation を含める。
   - required final artifacts を保持する。
3. user prompt に goal / profile / style / intent を含める。
4. 既存 `parse_ultra_plan` / `render_ultra_plan` と互換であることを確認する。

### 変更対象

- `mvp/anvilminimal/src/planner/runner.rs`

### 受入条件

- `generate_ultra_plan_with_ui` が user message 1本ではなく、system + user messages を使う。
- system prompt が JSON ではなく MVP YAML shape を要求する。
- prompt helper が unit test 可能な形で分離されている。

### テスト

- `ultra_plan_prompt_includes_source_parity_rules`
- `ultra_plan_generation_uses_yaml_shape`
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::runner::`

## Phase 2: profile generation rules 接続

### 目的

Next.js など profile 固有の計画生成ルールを UltraPlan 生成 prompt に接続する。

### 作業

1. MVP 側に profile generation rules helper を追加する。
   - `profile_generation_rules(profile, intent)` もしくは既存 profile module への関数追加。
2. Next.js profile rules を compact に定義する。
   - real Next.js app contract。
   - `next/react/react-dom` dependency。
   - `scripts.build = "next build"`。
   - dependency setup と build verification の分離。
   - Tailwind 使用時の dependency/config 整合。
   - port 3011 要求。
3. Rust / Python / Generic などは最小 rules から開始し、scope を広げすぎない。
4. Next.js topology を hard lint にしないことをコメントまたは文書で明確にする。

### 変更対象

- `mvp/anvilminimal/src/planner/profile.rs`
- `mvp/anvilminimal/src/planner/profiles/nextjs.rs`
- `mvp/anvilminimal/src/planner/runner.rs`

### 受入条件

- `nextjs/create` の UltraPlan system prompt に dependency setup / build separation / Tailwind consistency が含まれる。
- Space Invaders 固有語に依存していない。
- profile rules は prompt guidance として入り、hard lint で過剰拒否しない。

### テスト

- `ultra_plan_prompt_includes_nextjs_profile_rules`
- `ultra_plan_prompt_does_not_include_scenario_specific_game_terms`
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml nextjs`

## Phase 3: retry / tool-call rejection

### 目的

invalid UltraPlan output に対して bounded retry を入れ、planner tool call を invalid output として扱う。

### 作業

1. `const ULTRA_PLAN_GENERATION_ATTEMPTS: usize = 3` を追加する。
2. `generate_ultra_plan_with_ui` を retry loop に変更する。
3. 各 attempt で以下を順に判定する。
   - `reply.tool_calls` が空であること。
   - `parse_ultra_plan` が成功すること。
   - parse 成功後、`goal/profile/style/intent` を request context で正規化すること。
   - `lint_ultra_plan_report` が pass すること。
4. tool call がある場合は `ultra_plan_generation_tool_call_rejected` event を出す。
5. parse failure は schema retry prompt を作る。
6. lint failure は lint retry prompt を作る。
7. retry prompt は corrected YAML only を要求する。
8. metadata 正規化は failure ではなく、必要に応じて `ultra_plan_generation_metadata_normalized` event に残す。

### 変更対象

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/eval_events.rs`

### 受入条件

- invalid -> valid の fake planner で valid plan が返る。
- tool call -> valid の fake planner で retry 後 valid plan が返る。
- profile/style/intent の echo 揺れだけでは retry / failure にならず、canonical value に正規化される。
- retry attempt と failure kind が eval event に出る。
- retry loop が最大 3 回を超えない。

### テスト

- `ultra_plan_generation_retries_invalid_output`
- `ultra_plan_generation_rejects_tool_calls`
- `ultra_plan_generation_normalizes_metadata`
- `ultra_plan_generation_records_retry_events`
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml ultra_plan_generation`

## Phase 4: deterministic fallback の fail-fast 化

### 目的

retry exhaustion 時に `UltraPlan::deterministic` を通常成功 plan として返さないようにする。

### 作業

1. `generate_ultra_plan_with_ui` の parse/lint failure branch から `Ok(UltraPlan::deterministic(...))` を除去する。
2. retry exhaustion 時は `anyhow::bail!("invalid generated UltraPlan after corrective retries: ...")` 相当にする。
3. fail-fast 時に phase execution に入らないことを確認する。
4. executable `ultra-plan-*.yaml` を保存する前に失敗することを確認する。
5. degraded diagnostic artifact を保存する場合は、この Phase では optional とし、通常 `ultra-plan-*.yaml` と混同しない。

### 変更対象

- `mvp/anvilminimal/src/planner/runner.rs`

### 受入条件

- retry exhaustion で `UltraPlan::deterministic` が返らない。
- invalid planner output 時に `run_ultra_plan_with_ui` が呼ばれない。
- invalid planner output 時に workspace mutation が起きない。
- `ultra_plan_fallback_success_count` が 0 になる。

### テスト

- `ultra_plan_generation_fails_after_retry_exhaustion`
- `invalid_ultra_plan_does_not_create_successful_run`
- `ultra_plan_run_invalid_planner_output_fails_before_workspace_mutation`
- `ultra_plan_run_invalid_planner_output_does_not_save_executable_plan`

## Phase 5: lint / quality guidance / classification 整理

### 目的

UltraPlan lint を source validate 相当の汎用条件に留め、profile-specific topology は retryable quality guidance として扱う。

### 作業

1. `lint_ultra_plan_report` の現状を確認する。
2. source validate 相当の汎用条件を補う。
   - phase id の空文字 / duplicate。
   - phase count 2〜8。
   - prompt 空文字。
   - prompt が REPL command / shell command だけでない。
3. profile / style / intent は UltraPlan generation path で request context に正規化し、metadata mismatch だけで hard failure にしない。
4. Next.js topology は hard lint ではなく retry prompt へ反映する。
5. failure kind を以下に整理する。
   - `planner_schema_error`
   - `planner_lint_error`
   - `planner_tool_call_error`
   - `ultra_plan_generation_failed`
   - `ultra_plan_degraded_fallback`

### 変更対象

- `mvp/anvilminimal/src/planner/lint.rs`
- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/scripts/eval_lib/failure_classification.py`

### 受入条件

- shell command だけの phase prompt は lint failure になる。
- Next.js topology の不足だけで hard failure にしない。
- metadata mismatch だけで hard failure にしない。
- failure classification が planner / runtime を混同しない。

### テスト

- `ultra_plan_lint_rejects_shell_only_phase_prompt`
- `ultra_plan_lint_rejects_duplicate_phase_id`
- `ultra_plan_nextjs_topology_is_quality_guidance_not_hard_lint`
- `ultra_plan_lint_does_not_fail_on_metadata_echo_mismatch_after_normalization`
- `python3 -m unittest mvp/anvilminimal/tests/eval/test_failure_classification.py`

## Phase 6: eval event / report 指標

### 目的

UltraPlan 生成の成功、retry、fail-fast、degraded fallback を eval で観測できるようにする。

### 作業

1. `eval_events::emit` で以下 event を出す。
   - `ultra_plan_generation_attempt`
   - `ultra_plan_generation_retry`
   - `ultra_plan_generation_succeeded`
   - `ultra_plan_generation_failed`
   - `ultra_plan_generation_tool_call_rejected`
   - `ultra_plan_generation_metadata_normalized`
2. event field を統一する。
   - `attempt`
   - `provider`
   - `model`
   - `profile`
   - `style`
   - `intent`
   - `degraded`
3. failure / normalization event では以下も出す。
   - `failure_kind`
   - `failure_message`
   - `normalized_fields`
   - `raw_profile`
   - `raw_style`
   - `raw_intent`
4. eval summary に以下の集計を追加する。
   - `ultra_plan_generation_success_rate`
   - `ultra_plan_generation_retry_count`
   - `ultra_plan_generation_metadata_normalized_count`
   - `ultra_plan_degraded_fallback_count`
   - `ultra_plan_fallback_success_count`
5. report に planner failure と runtime failure の分離を表示する。

### 変更対象

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py`
- `mvp/anvilminimal/scripts/eval_lib/run_summary.py`
- `mvp/anvilminimal/scripts/eval_lib/report.py`

### 受入条件

- invalid output retry が summary に反映される。
- metadata normalization が summary に反映される。
- degraded fallback が成功扱いで集計されない。
- planner failure が `unclassified_process_failure` に落ちない。

### テスト

- `test_failure_classification.py`
- `test_runtime_scoring.py`
- `test_report.py`
- `python3 -m unittest discover -s mvp/anvilminimal/tests/eval`

## Phase 7: integration / no-workspace-mutation 検証

### 目的

invalid planner output で phase execution に入らず、workspace に中途半端な成果物を作らないことを保証する。

### 作業

1. temp workspace を使う integration test を追加する。
2. fake planner が invalid output を返し続けるケースを作る。
3. fake execution client が呼ばれたら test fail にする。
4. `package.json`, `src/app`, `.next` が作られないことを確認する。
5. retry 後 valid plan の場合は phase execution に入ることを確認する。
6. valid plan の profile/style/intent が config / request context と一致することを確認する。
7. planner が mismatched metadata を返しても、phase prompts が valid なら retry / failure にならないことを確認する。

### 変更対象

- `mvp/anvilminimal/tests/planner_ultra_plan_generation.rs`
- 必要なら `mvp/anvilminimal/src/planner/runner.rs` の test helper visibility。

### 受入条件

- invalid planner output で workspace mutation がない。
- invalid planner output で executable UltraPlan が保存されない。
- valid retry case では正常に executable UltraPlan が保存され、phase execution へ進む。
- mismatched metadata は canonical value に正規化される。

### テスト

- `ultra_plan_run_invalid_planner_output_fails_before_workspace_mutation`
- `ultra_plan_run_invalid_planner_output_does_not_save_executable_plan`
- `ultra_plan_run_retry_then_valid_preserves_profile`
- `ultra_plan_run_normalizes_generated_metadata`
- `ultra_plan_run_valid_plan_executes_phases`

## Phase 8: targeted eval / UAT / 結果整理

### 目的

021-1 実装後に、rollback 後 baseline と比較し、false success が減っているかを確認する。

### 作業

1. release build を作成する。
2. MVP only / local LLM unused / ultra-plan-run targeted eval を実行する。
3. 必要に応じて step-plan を参考値として実行する。
4. rollback 後 baseline と比較する。
5. `test0628_001` 型の「fallback plan で scaffold だけ作成して成功扱い」が再現しないことを確認する。
6. manual UAT を空 workspace で実施する。
7. `workspace/mvp/eval/021-1/ultra_plan_generation_source_parity_implementation_result.md` を作成する。
8. `workspace/mvp/eval/021-1/ultra_plan_generation_source_parity_eval_result.md` を作成する。

### 実行コマンド

```bash
cargo fmt --manifest-path mvp/anvilminimal/Cargo.toml -- --check
cargo test --manifest-path mvp/anvilminimal/Cargo.toml
python3 -m unittest discover -s mvp/anvilminimal/tests/eval
cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml
```

eval コマンドは実装時点の `mvp/anvilminimal/eval/README.md` と `scripts/eval-run.py --help` を確認し、現行 CLI と一致させる。

### 受入条件

- Rust unit / integration tests が通る。
- Python eval tests が通る。
- `ultra_plan_generation_retry_count` が観測できる。
- `ultra_plan_generation_metadata_normalized_count` が観測できる。
- `ultra_plan_fallback_success_count == 0`。
- planner failure が `unclassified_process_failure` に落ちない。
- invalid planner output では workspace が空のまま維持される。
- valid UltraPlan run の成果物品質は、少なくとも deterministic fallback run より悪化していない。

### 評価観点

- process success は一時的に下がってもよい。
- acceptance success だけでなく、false success reduction を見る。
- valid UltraPlan subset の `acceptance_success` / `plan_output_adherence_score` を別集計する。
- planner failure と runtime failure が分離できているかを見る。

## 横展開チェック

021-1 実装完了後、以下を確認する。

- `plan-run` の StepPlan 生成 retry と矛盾していないか。
- Gemini / OpenAI / Ollama の planner provider で prompt shape が破綻していないか。
- JSON tool call 対応をこの phase に混ぜていないか。
- TUI 表示が「何も作らず止まった」理由を最低限説明できているか。
- `UltraPlan::deterministic` が他の用途で必要な場合、通常 generation failure path と混線していないか。
- `workspace/` が gitignore されているため、計画文書をコミットする場合は `git add -f` が必要であること。

## 完了定義

021-1 は以下を満たした時点で完了とする。

- UltraPlan 生成が source-parity prompt contract を使う。
- profile generation rules が UltraPlan 生成 prompt に接続される。
- invalid output / tool call は bounded retry される。
- generated metadata は request context で正規化される。
- retry exhaustion は fail-fast し、deterministic fallback を通常成功 plan として返さない。
- invalid planner output 時に phase execution と workspace mutation が発生しない。
- eval が planner failure と runtime failure を分離できる。
- tests / targeted eval / manual UAT の結果が `workspace/mvp/eval/021-1` に記録されている。
