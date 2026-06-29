# Ultra Phase Context Continuity Source Parity Work Breakdown

作成日: 2026-06-29

対象計画:

- `workspace/mvp/eval/021-2/ultra_phase_context_continuity_source_parity_plan.md`

## 0. 目的

Phase 021-2 では、`/ultra-plan-run` のうち **phase 間の文脈継続** だけを移植元 anvil に寄せて戻す。

この work breakdown の目的は、以下を実装可能な作業単位へ分解することである。

- ultra-run 経由では phase 間で同じ `SessionSnapshot` を使う。
- step-plan 単体実行では従来どおり独立 session を使う。
- phase 間で artifact / verify failure / repair target / changed paths を bounded summary として引き継ぐ。
- shared session と bounded summary の両方を event / unit test / integration test で観測できるようにする。
- shared session の対象は execution model の step 実行に限定し、planner には bounded summary を prompt として渡す。
- failure path でも partial outcome を失わず、context update event を残す。
- 成功率改善だけでなく、失敗時の診断粒度向上も評価する。

## 1. スコープ境界

### 対象

- `run_step_plan_with_ui_inner` の内部 API 分離。
- ultra-run 内部で使う shared `SessionSnapshot`。
- `StepPlanRunOutcome` など、phase context 更新に必要な structured outcome。
- `StepPlanRunError` など、failure path でも partial outcome を保持する内部型。
- `UltraRunContext` による bounded phase summary。
- `ultra_phase_prompt` への prior context 付与。
- `ultra_context_initialized` / `ultra_phase_context_attached` / `ultra_phase_context_updated` event。
- unit / integration / eval test。
- MVP only targeted eval / manual UAT。

### 非対象

- UltraPlan 生成 prompt / retry / fail-fast の追加変更。
- Next.js profile runtime contract / snapshot の詳細化。
- repair exhaustion から `/ultra-plan-run` recovery prompt を保存する仕組み。
- phase verification の `Invariant / Final` 分離。
- profile final repair の shared session 化。
- Next.js / Space Invaders 専用テンプレート。
- source の `RepairJob` / scaffold pipeline 全体移植。

## 2. 追跡 ID

| ID | 内容 | 主 Phase |
| --- | --- | --- |
| UCC-01 | ultra-run phase ごとに `SessionSnapshot::new()` され、会話履歴が切れる | 1, 2 |
| UCC-02 | `run_step_plan_with_ui_inner` が session を内部生成し、caller から渡せない | 1 |
| UCC-03 | `run_step_plan_with_ui_inner` の戻り値が string だけで、context 更新に必要な情報がない | 1 |
| UCC-04 | phase 2 以降の prompt に prior phase summary がない | 3, 4 |
| UCC-05 | repair failure / no-change / changed paths が後続 phase に渡らない | 3, 4 |
| UCC-06 | context continuity を eval event で観測できない | 5 |
| UCC-07 | shared session と bounded summary の責務が混線するリスク | 3, 6 |
| UCC-08 | step-plan 単体実行の挙動を壊すリスク | 1, 6 |
| UCC-09 | context bloat / 古い失敗の引きずり | 3, 6 |
| UCC-10 | Manual TUI で phase 間 context の有無を確認できない | 5, 7 |
| UCC-11 | step execution failure 時に partial outcome が失われ、context update できない | 1, 3, 6 |
| UCC-12 | planner session まで共有してしまい、StepPlan 生成が不安定化するリスク | 2, 4, 6 |
| UCC-13 | raw stderr / secret-like value が bounded context に混入するリスク | 3, 4, 6 |

## 3. 予定成果物

### Rust 実装候補

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/planner/ultra_context.rs`
- `mvp/anvilminimal/src/planner/mod.rs`
- `mvp/anvilminimal/src/eval_events.rs`

`ultra_context.rs` は新規追加候補である。小さく収まる場合は `runner.rs` 内 private struct から開始してよい。ただし context formatting / bounds / update logic が膨らむ場合は分離する。

### Eval 実装候補

- `mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py`
- `mvp/anvilminimal/scripts/eval_lib/failure_classification.py`
- `mvp/anvilminimal/scripts/eval_lib/run_summary.py`
- `mvp/anvilminimal/scripts/eval_lib/report.py`

### テスト候補

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/tests/tui_integration.rs`
- `mvp/anvilminimal/tests/eval/test_runtime_scoring.py`
- `mvp/anvilminimal/tests/eval/test_failure_snapshot_classification.py`

### 調査 / 結果文書

- `workspace/mvp/eval/021-2/ultra_phase_context_continuity_baseline.md`
- `workspace/mvp/eval/021-2/ultra_phase_context_continuity_implementation_result.md`
- `workspace/mvp/eval/021-2/ultra_phase_context_continuity_eval_result.md`
- `workspace/mvp/eval/021-2/ultra_phase_context_continuity_uat_result.md`

## 4. 実施順

実施順は以下を固定する。

1. Phase 0: baseline / source contract 固定
2. Phase 1: internal API split / structured outcome
3. Phase 2: ultra shared session 導入
4. Phase 3: `UltraRunContext` 設計
5. Phase 4: phase prompt attachment
6. Phase 5: event instrumentation / eval integration
7. Phase 6: unit / integration tests
8. Phase 7: targeted eval / manual UAT
9. Phase 8: 結果整理 / 次フェーズ判定

Phase 2 で shared session を入れるが、成功率だけでは判断しない。Phase 5 以降で context continuity が観測できる状態にしてから、Phase 7 で効果を見る。

## Phase 0: baseline / source contract 固定

### 目的

実装前に、現状の session reset と移植元の shared session 契約を固定する。

### 作業

1. 移植元の `run_ultra_plan` を整理する。
   - `src/agent/minimal_step_runner.rs:580`
   - `src/agent/minimal_step_runner.rs:586`
   - `src/agent/minimal_step_runner.rs:604`
   - `src/agent/minimal_step_runner.rs:610`
2. MVP の session reset 箇所を整理する。
   - `mvp/anvilminimal/src/planner/runner.rs:264`
   - `mvp/anvilminimal/src/planner/runner.rs:276`
   - `mvp/anvilminimal/src/planner/runner.rs:915`
3. 現在の `run_step_plan_with_ui` public behavior を整理する。
4. 既存 `tui_ultra_plan_run_smoke_fake_clients` の検証範囲を確認する。
5. 現在の ultra-plan-run event で context continuity を直接確認できないことを文書化する。
6. `workspace/mvp/eval/021-2/ultra_phase_context_continuity_baseline.md` を作成する。

### 参照対象

- `src/agent/minimal_step_runner.rs`
- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/tests/tui_integration.rs`
- `workspace/mvp/eval/021-2/ultra_phase_context_continuity_source_parity_plan.md`

### 受入条件

- source と MVP の差分が、session ownership / lifetime / caller responsibility の観点で説明できる。
- 021-2 で戻す対象が phase step execution の shared session に限定されている。
- profile final repair / repair handoff / profile runtime contract は対象外として明記されている。

### テスト / 確認

- 文書レビューで UCC-01〜UCC-13 を追跡できる。
- 実装前 baseline として後続 eval 結果と比較できる。

## Phase 1: internal API split / structured outcome

### 目的

step-plan 単体 API を壊さず、ultra-run 内部から session を渡せる API を追加する。

### 作業

1. `StepPlanRunOutcome` を追加する。
   - `summary: String`
   - `completed_steps: usize`
   - `total_steps: usize`
   - `changed_paths: Vec<String>`
   - `verify_failures: Vec<String>`
   - `primary_failure: Option<String>`
   - `repair_targets: Vec<String>`
   - `command_failures: Vec<String>`
   - `repair_attempts: usize`
   - `repair_changed_paths: Vec<String>`
   - `stop_reason: Option<String>`
   - `partial: bool`
2. `run_step_plan_with_ui_inner` を分解する。
   - public wrapper 用の `run_step_plan_with_ui(...) -> anyhow::Result<String>` は維持する。
   - internal 用の `run_step_plan_with_session_with_ui(...) -> anyhow::Result<StepPlanRunOutcome>` を追加する。
   - failure path 用に `StepPlanRunError` または `StepPlanRunResult` を追加し、`partial_outcome` と original error を保持する。
3. `run_step_plan_with_ui` は内部で `SessionSnapshot::new()` し、internal API を呼ぶ。
4. `run_step_plan_with_ui_inner` の既存ロジックを internal API に寄せる。
5. stdout / stderr 文字列 parse で outcome を作らない。
6. `run_step` から changed paths / repair attempts / verify failure を拾えるよう、必要なら `StepRunOutcome` を private struct として追加する。
7. command failure の raw stderr は outcome に入れず、failure kind / command / redacted snippet に制限する。

### 変更対象

- `mvp/anvilminimal/src/planner/runner.rs`

### 受入条件

- `run_step_plan_with_ui` の戻り値と public behavior が変わらない。
- ultra-run 内部から caller-owned `&mut SessionSnapshot` を渡せる。
- `StepPlanRunOutcome` が context 更新に必要な情報を持つ。
- failure path でも `partial_outcome` が取り出せる。
- outcome は structured data から作られ、log text parse に依存しない。
- outcome は raw stderr / secret-like value を保持しない。

### テスト

- `run_plan_passes_step_contract_to_execution_client`
- `run_step_plan_public_api_uses_independent_session`
- `run_step_plan_with_session_uses_caller_session`
- `step_plan_run_outcome_tracks_changed_paths`
- `step_plan_run_outcome_tracks_verify_failure`
- `step_plan_run_error_preserves_partial_outcome`
- `step_plan_run_outcome_redacts_raw_stderr`

## Phase 2: ultra shared session 導入

### 目的

ultra-run 経由の phase step execution で、同じ `SessionSnapshot` を継続して使う。

### 作業

1. `run_ultra_plan_with_ui` の先頭で `let mut ultra_session = SessionSnapshot::new();` を作る。
2. 各 phase の step-plan 実行で `run_step_plan_with_session_with_ui(..., &mut ultra_session, ...)` を呼ぶ。
3. phase 1 の tool call / assistant message が phase 2 execution client の message history に残ることを確認できるようにする。
4. step-plan 単体実行では session が共有されないことを維持する。
5. profile final repair は 021-2 では shared session 化しない。ただし後続 Phase 3 で context に記録できるようにする。
6. planner の StepPlan 生成 session は共有しない。phase prompt に bounded context を入れるだけに留める。

### 変更対象

- `mvp/anvilminimal/src/planner/runner.rs`

### 受入条件

- ultra-run 経由の phase 2 execution client messages に phase 1 の履歴が残る。
- phase 間の shared session は ultra-run 内に閉じている。
- shared session の対象は execution model の step 実行に限定される。
- plan-run 単体を複数回呼んでも session は共有されない。
- `max_iterations`、interrupt、yes mode、workspace confinement の挙動を変えない。

### テスト

- `ultra_run_reuses_session_across_phases`
- `ultra_run_phase_two_messages_include_phase_one_history`
- `standalone_plan_run_does_not_share_session_between_calls`
- `ultra_shared_session_preserves_interrupt_checks`
- `ultra_run_does_not_share_planner_session_between_phases`

## Phase 3: `UltraRunContext` 設計

### 目的

shared session だけでは eval / diagnosis が難しいため、bounded summary として phase context を構造化する。

### 作業

1. `UltraRunContext` を追加する。
2. 最小 field を定義する。
   - `completed_phases: Vec<String>`
   - `created_or_changed_paths: Vec<String>`
   - `last_failed_phase: Option<String>`
   - `last_verify_failures: Vec<String>`
   - `last_repair_changed_paths: Vec<String>`
   - `pending_final_artifacts: Vec<String>`
   - `unresolved_repair_targets: Vec<String>`
3. summary size cap を定義する。
   - changed paths: 最大 20 件
   - verify failures: 最大 5 件
   - repair targets: 最大 5 件
   - prompt 表示: 最大 20 行程度
4. resolved / unresolved の扱いを決める。
   - pass した verify failure は resolved として短く残すか削除する。
   - unresolved failure は次 phase summary に残す。
5. `StepPlanRunOutcome`、profile check result、phase failure result から context を更新する helper を作る。
6. scenario 固有語を保持しない方針を入れる。
7. raw stderr / prompt 全文 / secret-like value を context に入れない redaction helper を用意する。
8. context が切り詰められた場合に `context_truncated` を記録できるようにする。

### 変更対象

- `mvp/anvilminimal/src/planner/runner.rs`
- 必要なら `mvp/anvilminimal/src/planner/ultra_context.rs`
- 必要なら `mvp/anvilminimal/src/planner/mod.rs`

### 受入条件

- `UltraRunContext` が bounded である。
- changed paths / verify failures / repair targets が上限を超えない。
- context は path / command / failure kind / repair target の汎用情報で構成される。
- source の巨大な repair graph を持ち込まない。
- raw stderr / prompt 全文 / secret-like value が context に含まれない。
- context truncation の有無を event に渡せる。

### テスト

- `ultra_context_tracks_completed_phases`
- `ultra_context_tracks_changed_paths`
- `ultra_context_tracks_unresolved_verify_failures`
- `ultra_context_caps_changed_paths`
- `ultra_context_caps_failure_lines`
- `ultra_context_does_not_require_scenario_keywords`
- `ultra_context_redacts_secret_like_values`
- `ultra_context_reports_truncation`

## Phase 4: phase prompt attachment

### 目的

後続 phase の prompt に `Prior ultra context` を追加し、LLM が前 phase の作業履歴を明示的に参照できるようにする。

### 作業

1. `ultra_phase_prompt` の引数に `UltraRunContext` を追加する。
2. context が空の場合は `Prior ultra context:\n- none yet` を出す。
3. context がある場合は以下を含める。
   - completed phases
   - recently changed paths
   - pending final artifacts
   - unresolved verify failures
   - unresolved repair targets
4. prompt 内の context は bounded summary のみとし、長い raw stderr は入れない。
5. phase 1 prompt では context が empty であることを確認する。
6. phase 2 以降では context が含まれることを確認する。
7. planner call には shared execution session の message history を渡さない。

### 変更対象

- `mvp/anvilminimal/src/planner/runner.rs`
- 必要なら `mvp/anvilminimal/src/planner/ultra_context.rs`

### 受入条件

- phase 2 以降の generated step-plan prompt に prior context が入る。
- context が空の phase 1 でも prompt が安定している。
- prompt size が bounded である。
- profile runtime contract の内容はこの Phase で増やさない。
- planner prompt に raw prior conversation history が混入しない。

### テスト

- `ultra_phase_prompt_includes_empty_context_for_first_phase`
- `ultra_phase_prompt_includes_completed_phase_context`
- `ultra_phase_prompt_includes_pending_artifacts`
- `ultra_phase_prompt_includes_unresolved_failure_summary`
- `ultra_phase_prompt_context_is_bounded`
- `ultra_phase_prompt_does_not_include_raw_session_history`

## Phase 5: event instrumentation / eval integration

### 目的

phase context continuity を eval / run logs から観測できるようにする。

### 作業

1. `ultra_context_initialized` event を追加する。
   - `shared_session: true`
   - `context_max_changed_paths`
   - `context_max_failures`
   - `session_message_count`
2. `ultra_phase_context_attached` event を追加する。
   - `phase_id`
   - `completed_phase_count`
   - `changed_path_count`
   - `pending_artifact_count`
   - `last_failure_count`
   - `shared_session: true`
   - `session_message_count`
   - `context_truncated`
3. `ultra_phase_context_updated` event を追加する。
   - `phase_id`
   - `changed_paths`
   - `pending_final_artifacts`
   - `last_failure_kind`
   - `repair_target_count`
   - `session_message_count`
   - `partial_outcome_recorded`
   - `context_truncated`
4. eval 側で context continuity を集計する。
   - `ultra_context_continuity_score`
   - `ultra_shared_session_observed`
   - `ultra_context_attached_after_first_phase`
   - `ultra_context_bounded`
   - `ultra_session_message_growth_observed`
   - `ultra_partial_outcome_recorded`
5. failure classification に必要なら `context_missing` / `context_not_attached` を diagnostic として追加する。
6. report に context event の有無を表示する。

### 変更対象

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py`
- `mvp/anvilminimal/scripts/eval_lib/failure_classification.py`
- `mvp/anvilminimal/scripts/eval_lib/run_summary.py`
- `mvp/anvilminimal/scripts/eval_lib/report.py`

### 受入条件

- ultra-run の events に `ultra_context_initialized` が出る。
- phase 2 以降の events に `ultra_phase_context_attached` が出る。
- success / failure に関わらず、phase 終了時または失敗時に `ultra_phase_context_updated` が出る。
- context item count が上限内であることを eval が判断できる。
- message count の増加から shared session 継続を eval が判断できる。
- failure path で partial outcome が記録されたことを eval が判断できる。
- context continuity の欠落が `unclassified_process_failure` に埋もれない。

### テスト

- `test_runtime_scoring.py::test_ultra_context_continuity_scores_shared_session_events`
- `test_runtime_scoring.py::test_ultra_context_continuity_penalizes_missing_phase_context`
- `test_runtime_scoring.py::test_ultra_context_bounded_score_penalizes_oversized_context`
- `test_runtime_scoring.py::test_ultra_context_continuity_uses_session_message_growth`
- `test_runtime_scoring.py::test_ultra_partial_outcome_recorded_on_failure`
- `test_failure_snapshot_classification.py::test_classifies_missing_ultra_context_diagnostic`
- `python3 -m unittest discover -s mvp/anvilminimal/tests/eval`

## Phase 6: unit / integration tests

### 目的

shared session と bounded context を別々に検証し、plan-run 単体 regressions を防ぐ。

### 作業

1. `runner.rs` 内 unit test を追加・更新する。
2. `tui_integration.rs` の ultra smoke fake clients を拡張する。
3. phase 1 で `Write app.txt`、phase 2 で history / context を確認する fixture を作る。
4. plan-run 単体実行を複数回呼び、session が共有されないことを確認する。
5. verify failure fixture を作る。
   - bounded repair で回復するケース。
   - bounded repair で回復不能なケース。
   - failure path でも partial outcome が context update event に反映されるケース。
6. final contract verification が final phase のみで走ることを確認する。
7. context size cap を超える changed paths / failures を作り、切り詰めを確認する。
8. raw stderr / secret-like value が context summary に入らないことを確認する。
9. phase 2 以降で attach/update event の `session_message_count` が増えることを確認する。

### 変更対象

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/tests/tui_integration.rs`
- `mvp/anvilminimal/tests/eval/test_runtime_scoring.py`
- `mvp/anvilminimal/tests/eval/test_failure_snapshot_classification.py`

### 受入条件

- shared session の message history 継続をテストで確認できる。
- bounded context の prompt attachment をテストで確認できる。
- plan-run 単体の独立 session が保たれる。
- context bloat 防止がテストされる。
- failure run でも context update event が残る。
- failure run の partial outcome が記録される。
- context summary の redaction がテストされる。
- shared session が `shared_session: true` だけでなく message count でも検証される。

### テストコマンド

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml ultra
cargo test --manifest-path mvp/anvilminimal/Cargo.toml tui_ultra
python3 -m unittest discover -s mvp/anvilminimal/tests/eval -p 'test_*.py'
```

## Phase 7: targeted eval / manual UAT

### 目的

021-2 実装後に、context continuity が観測でき、成功率や failure kind が悪化していないかを確認する。

### 作業

1. release build を作成する。
2. MVP only / local LLM unused / ultra-plan-run targeted eval を実行する。
3. 可能なら MVP vs anvildev の ultra-plan-run / plan-run 比較を実行する。
4. eval events から以下を確認する。
   - `ultra_context_initialized`
   - `ultra_phase_context_attached`
   - `ultra_phase_context_updated`
   - `shared_session: true`
   - `session_message_count`
   - `partial_outcome_recorded`
   - `context_truncated`
5. rollback 後 baseline / 021-1 後 baseline と比較する。
6. manual UAT を空 workspace で実施する。
7. `workspace/mvp/eval/021-2/ultra_phase_context_continuity_implementation_result.md` を作成する。
8. `workspace/mvp/eval/021-2/ultra_phase_context_continuity_eval_result.md` を作成する。
9. `workspace/mvp/eval/021-2/ultra_phase_context_continuity_uat_result.md` を作成する。

### 実行コマンド

```bash
cargo fmt --manifest-path mvp/anvilminimal/Cargo.toml -- --check
cargo test --manifest-path mvp/anvilminimal/Cargo.toml
python3 -m unittest discover -s mvp/anvilminimal/tests/eval -p 'test_*.py'
cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml
```

eval コマンドは実装時点の `mvp/anvilminimal/eval/README.md` と `mvp/anvilminimal/scripts/eval-run.py --help` を確認し、現行 CLI と一致させる。

### 受入条件

- Rust tests が通る。
- Python eval tests が通る。
- MVP only ultra-plan-run run で context events が観測できる。
- phase 2 以降に context attached event がある。
- context item count が cap 内である。
- session message count が phase をまたいで増加している。
- failure run で partial outcome が記録される。
- `phase_completion_score` または `ultra_runtime_health_score` が rollback 後 baseline から大きく悪化していない。
- 成功率が下がった場合は、failure kind と context event から理由を説明できる。

### 評価観点

- 成功率だけで判断しない。
- `phase_completion_score`、`runtime_friction_score`、`finalization_score`、`ultra_runtime_health_score` を見る。
- `context_missing` / `context_not_attached` が出ないことを見る。
- `ultra_session_message_growth_observed` と `ultra_context_bounded` を見る。
- failure run では `ultra_partial_outcome_recorded` を見る。
- accepted artifact の品質が改善したかは参考値とし、021-2 単独の必須成功条件にはしない。

## Phase 8: 結果整理 / 次フェーズ判定

### 目的

021-2 の効果と残課題を整理し、次に戻す移植不備を判断する。

### 作業

1. implementation result に以下を記録する。
   - 実装した API
   - public behavior を変えなかった箇所
- shared session を適用した範囲
   - planner session を共有しなかったこと
   - profile final repair を対象外にした理由
2. eval result に以下を記録する。
   - baseline 比較
   - context event 出現率
   - failure kind の変化
   - 指標値の変化
3. UAT result に以下を記録する。
   - TUI で phase context event が残るか
   - 失敗時に phase / pending artifact / unresolved failure が追跡できるか
4. 次フェーズ候補を判定する。
   - profile runtime contract / snapshot
   - repair exhaustion handoff
   - phase verification invariant / final 分離
   - build failure repair target

### 受入条件

- 021-2 の変更が、phase 間 context continuity の改善として説明できる。
- 成功率の増減を、context continuity の有無と切り分けて説明できる。
- 次に戻すべき移植不備が1つに絞れている。
- 021-2 で対象外にした項目が、必要に応じて次計画へ引き継がれている。

## 横展開チェック

021-2 実装完了後、以下を確認する。

- `plan-run` 単体の挙動が変わっていないか。
- `minimal-loop` 単体に shared session の副作用がないか。
- TUI interrupt / ESC / yes mode に影響していないか。
- eval event の追加で既存 report / CSV が壊れていないか。
- context summary に scenario 固有語や prompt 本文の長い転載が入っていないか。
- context summary に raw stderr / secret-like value が入っていないか。
- planner call に execution session history が混入していないか。
- `workspace/` が gitignore されているため、計画文書をコミットする場合は `git add -f` が必要であること。

## 完了定義

021-2 は以下を満たした時点で完了とする。

- ultra-run 経由では phase 間で shared `SessionSnapshot` が継続される。
- step-plan 単体実行では独立 session が維持される。
- `UltraRunContext` が bounded summary を保持する。
- phase 2 以降の prompt に prior context が付与される。
- phase 2 以降の execution client message history に前 phase の履歴が残る。
- eval events で context continuity を観測できる。
- session message count の増加で shared session 継続を確認できる。
- failure path でも partial outcome が context update に反映される。
- context summary は bounded で、scenario 固有 hard coding を含まない。
- context summary は raw stderr / prompt 全文 / secret-like value を含まない。
- planner の StepPlan 生成会話は shared execution session に巻き込まれない。
- Rust tests / Python eval tests が通る。
- targeted eval / manual UAT の結果が `workspace/mvp/eval/021-2` に記録されている。
