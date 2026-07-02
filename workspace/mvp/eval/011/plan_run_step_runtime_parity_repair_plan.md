# Plan-run step runtime parity repair plan

## Scope

MVP `anvilminimal` の `plan-run` 成功率改善に向けて、直近 eval で顕在化した `plan-run` 経由 minimal loop の step 実行 runtime 問題を修正する。

対象:

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`
- `mvp/anvilminimal/src/minimal_loop/feedback.rs`
- `mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py`
- `mvp/anvilminimal/scripts/eval_lib/run_summary.py`
- `mvp/anvilminimal/scripts/eval_lib/report.py`
- 既存 unit / integration / eval tests

参照した移植元:

- `src/agent/minimal_step_runner.rs`
- `src/agent/minimal_step_runner/verify.rs`
- `src/agent/minimal_loop/loop_run.rs`
- `src/agent/minimal_loop/feedback.rs`
- `src/agent/minimal_repl.rs`

非目的:

- eval scenario 名や固定 artifact 名に依存した分岐を入れない
- `plan-run` の失敗を無条件成功扱いにしない
- verify / postcheck を弱めない
- source の複雑な `loop_run/*` 全体を MVP へ丸ごと戻さない
- `minimal-loop --prompt` 単体の completion contract を破壊しない

## Current Findings

### MVP plan-run failure distribution

直近 MVP 2 回分、plan-run 24 件:

| failure kind | count | 主な意味 |
| --- | ---: | --- |
| `missing_tool_call` | 11 | artifact は作れたが step を完了として閉じられない |
| `required_artifacts_missing` | 9 | current step 中に required artifact が作られない |
| `verify_command_policy_error` | 2 | planner / verify policy で停止 |
| `tool_validation_error` | 1 | tool 引数 / workspace policy で停止 |
| `planner_lint_error` | 1 | planner lint で停止 |

同条件の移植元 `anvildev`:

| binary | plan-run success | 主な失敗 |
| --- | ---: | --- |
| MVP r1 | 0/12 | runtime-side failures |
| MVP r2 | 0/12 | runtime-side failures |
| anvildev fresh | 9/12 | plan lint / verify policy only |

### Metric trend

failure kind 別の MVP 指標平均:

| failure kind | artifact_progress | finalization | runtime_health | prompt_contract |
| --- | ---: | ---: | ---: | ---: |
| `required_artifacts_missing` | 15.6 | 20.0 | 28.0 | 100.0 |
| `missing_tool_call` | 100.0 | 50.0 | 58.7 | 100.0 |

解釈:

- `prompt_contract_score=100.0` なので、Phase 010 で修正した overall goal / verify / expected_result 伝達漏れは主因ではない
- `required_artifacts_missing` は artifact obligation の scope 誤適用または artifact creation recovery の不足
- `missing_tool_call` は artifact 作成後または verify/setup tool 後の finalization 判定が過剰に厳しい
- `plan_quality_score` は高い失敗が多く、静的 YAML 品質だけでは plan-run 成功率を説明できない

## Review Findings Reflected

本計画のレビューで、以下を反映する。

- step scope の required paths 混入を止めるだけでは plan 全体の成果物保証が弱まるため、全 step 完了後の plan-level final contract verification を必須 Phase として追加する
- no-tool finalization の緩和は `tool call seen` だけで一律許可しない。`expected_paths` 未達時は missing artifact feedback / recovery を優先する
- 受け入れ条件は「改善する」だけでなく、最低成功数と failure kind 上限を定量 gate にする
- `plan-run` だけでなく `/run-plan`、TUI slash、`ultra-plan-run` の phase step 実行にも影響するため、非回帰テストと smoke eval を追加する
- `step_obligation_scope` event は `effective_required_paths` の内訳を持たないと原因分析に不足するため、explicit / prompt-extracted / contract / effective の redacted path list または count を出す
- source の `early_success_paths_for_step()` の説明を正確化する。source は current `step.expected_paths` だけを early success paths として渡し、実行前に全 expected paths が既に存在する step では early success paths を渡さない
- completion contract は path merge だけでなく、step turn 内の verify / deferred requirement / iteration extension にも影響し得る。plan-run step ではこれらの contract runtime effect も無効化し、Phase 5 の plan-level final contract verification に一本化する
- eval acceptance は LLM のばらつきを考慮し、可能な限り MVP plan-run を 2 回実行して平均と worst run の両方で見る。1 回のみの場合は provisional result として明記する
- 新しい失敗経路を `unclassified_process_failure` に落とさないため、plan-level final contract failure と obligation scope violation の分類/summary 互換性も完了条件に含める
- `/run-plan`、TUI slash、`ultra-plan-run`、`ultra-step-run` は parse/dispatch だけでなく、同じ step runtime options が実際に使われたことを event または deterministic fixture で確認する

## Root Cause

### RC-01: Context-only final artifacts are treated as current step obligations

MVP:

- `mvp/anvilminimal/src/planner/runner.rs::build_step_prompt()` は `Required final artifacts:` を step prompt に含める
- `mvp/anvilminimal/src/minimal_loop/loop_run.rs::effective_required_paths()` は explicit `step.expected_paths` に加え、prompt から抽出した artifact paths を required paths に合成する
- `extract_requested_artifact_paths()` は `Required final artifacts:` block を読む

その結果、`inspect` / `verify` / `setup` step でも、全体成果物が current step required paths として扱われる。

典型例:

- step: `inspect-workspace`
- `step.expected_paths`: empty
- prompt context: `Required final artifacts: README.md`
- effective required paths: `README.md`
- モデルは Glob を繰り返す
- current step は README を作らないため `required_artifacts_missing`

移植元:

- `src/agent/minimal_step_runner.rs::run_plan()` は `early_success_paths_for_step()` を使う
- step の early success paths は current `step.expected_paths` のみで構成される
- 実行前に current `step.expected_paths` がすべて存在する場合、early success paths は空になる
- overall required final artifacts は prompt context だが、current step completion obligation にはしない

判定:

- これは移植不備
- Phase 010 の「文脈を渡す」修正が、MVP 側の prompt artifact extractor と衝突している

### RC-02: No-tool finalization is stricter than source after any tool was used

MVP:

- `mvp/anvilminimal/src/minimal_loop/loop_run.rs` は `!write_or_edit_seen && looks_like_action_prompt(user_prompt)` の 2 回目 no-tool 応答を `missing tool call for action prompt after feedback` で停止する
- Bash / Read / Glob 等の tool call が既にあっても、Write/Edit が無い step では同じ判定になる

移植元:

- `src/agent/minimal_loop/loop_run.rs` は `!tool_calls_seen` の時だけ missing tool feedback を出す
- verify/setup/inspect step で Bash / Read / Glob を使った後は、no-tool final response で閉じることができる
- step の実成否は runner 側の `verify_step()` で判定される

判定:

- これは移植不備
- `missing_tool_call` 11 件の主要因

### RC-03: Step kind specific completion rules are not explicit enough

MVP の minimal loop は action prompt 全体に同じ no-tool / artifact recovery を適用する。

必要な分岐:

- `implement` / `create` / `edit`: expected paths を作るまで強く誘導
- `verify` / `setup`: Bash 等の tool 実行後は runner の verify result へ委譲
- `inspect`: current step expected paths が空なら、全体 artifact を作らせない
- `report`: explicit blocker のみ成功扱いせず、runner/lint で制御

判定:

- MVP のシンプルさを保ちつつ、`plan-run` 経由時だけ step execution scope を渡す必要がある

## Migration Miss Root Cause Audit

今回の移植不備は、source file の目視確認漏れだけではなく、比較単位の取り方に問題があった。

### MM-RC-01: Prompt contract と effective completion contract を分けて確認していなかった

Phase 010 では source の `build_step_prompt()` と MVP の prompt 内容を中心に比較した。その結果、overall goal / required final artifacts / verify / expected_result を「prompt に含める」ことは復元した。

しかし MVP minimal loop は prompt 内の `Required final artifacts:` block を `effective_required_paths()` で current required paths に変換する。source 側の `run_turn_with_early_success_paths()` は current `step.expected_paths` だけを early success paths として渡す。この差分を function-to-function matrix に入れていなかった。

### MM-RC-02: `minimal-loop` hardening と `plan-run` step scope の相互作用を見落とした

MVP は過去の eval 対応で、`minimal-loop --prompt` 単体の artifact 抽出と completion contract を強化している。これは単体 minimal loop では有効だが、plan-run step prompt に overall required artifacts を含めると current step obligation へ混入する。

つまり今回の不備は「source 機構を移していない」だけでなく、「MVP 側で追加した completion hardening が source step runner の scope と衝突した」ことが根本原因である。

### MM-RC-03: 成功指標が step prompt contract に寄り、effective required path の観測がなかった

`prompt_contract_score` は prompt に必要セクションがあることを測るが、その prompt が loop 内でどの required paths に変換されたかは測っていない。このため `prompt_contract_score=100` でも、plan-run が `required_artifacts_missing` で落ちる状態を事前検知できなかった。

### MM-RC-04: source の no-tool finalization 条件を `write_or_edit_seen` と誤対応した

source の missing tool feedback は `!tool_calls_seen` を前提にしている。MVP は `!write_or_edit_seen` を gate にしており、Bash / Read / Glob を実行した verify/setup/inspect step でも no-tool final が失敗し得る。この差は source の minimal loop feedback を比較対象に含めるまで見えなかった。

### MM-RC-05: plan-run 経路の横断テスト不足

Phase 010 のテストは prompt 内容と planner quality に寄っていた。`run-plan`、TUI slash、ultra phase step execution で同じ step scope が守られるかの横断テストが不足していた。

## Additional Migration Gap Audit

今回追加調査した観点と結果:

| ID | 観点 | source | MVP current | 判定 | 本計画での扱い |
| --- | --- | --- | --- | --- | --- |
| PR-GAP-01 | current step vs context artifact scope | `early_success_paths_for_step()` は `step.expected_paths` のみ | prompt-extracted required final artifacts が混入 | confirmed migration miss | Phase 2 |
| PR-GAP-02 | no-tool finalization gate | `!tool_calls_seen` | `!write_or_edit_seen` | confirmed migration miss | Phase 3 |
| PR-GAP-03 | plan-level final contract | source は post step / postcheck で最終成果物を評価 | step extraction を切ると plan-level guarantee が不足し得る | design gap introduced by repair | Phase 5 |
| PR-GAP-04 | obligation scope observability | source has stdout stop reasons; MVP has eval events but no effective path decomposition | root cause visibility insufficient | instrumentation gap | Phase 6 |
| PR-GAP-05 | `/run-plan` / TUI / ultra phase coverage | all route into source `run_plan()` / `generate_and_run_step_plan()` | tests mainly CLI plan-run | coverage gap | Phase 7 |
| PR-GAP-06 | repair prompt scope | source repair prompt contains required final artifacts as context | MVP repair prompt now also contains context, so same extractor conflict can occur | confirmed risk | Phase 2 / Phase 4 |
| PR-GAP-07 | verify / setup no-write completion | source delegates to `verify_step()` after tool usage | MVP can stop as missing tool call | confirmed migration miss | Phase 3 |
| PR-GAP-08 | planner lint / verify policy parity | both have build order / setup policy checks, details differ | current failures include small number of policy/lint errors | not primary runtime gap | tracked outside core fix unless regression |

No additional high-confidence plan-run runtime migration misses were found in `repair.rs`, `plan_lint.rs`, or `verify.rs` beyond the scope and finalization issues above. Existing differences there are either already addressed by Phase 010, intentional MVP simplification, or planner-quality work rather than the current runtime regression.

## Design Direction

### Principle

`minimal-loop --prompt` と `plan-run step execution` は同じ loop engine を使うが、completion obligation の scope は違う。

| mode | required path source during turn | prompt artifact extraction | contract path merge during turn | contract verification during turn | completion owner |
| --- | --- | --- | --- | --- | --- |
| minimal-loop | prompt / completion contract / explicit required paths | enabled | enabled | enabled | minimal loop |
| plan-run step | current `step.expected_paths` only | disabled by default | disabled by default | disabled by default | runner + minimal loop |
| repair step | current `step.expected_paths` only | disabled by default | disabled by default | disabled by default | runner + repair verifier |
| plan-run final | plan required final artifacts / external completion contract | disabled | enabled at plan level | enabled at plan level | runner final contract check |

### Proposed API

`run_session_with_outcome_with_ui()` に step execution options を追加する。

例:

```rust
pub struct RunSessionOptions {
    pub prompt_artifact_extraction: PromptArtifactExtraction,
    pub completion_contract_path_merge: CompletionContractPathMerge,
    pub completion_contract_verification: CompletionContractVerification,
    pub action_no_tool_policy: ActionNoToolPolicy,
}

pub enum PromptArtifactExtraction {
    Enabled,
    Disabled,
}

pub enum CompletionContractPathMerge {
    Enabled,
    Disabled,
}

pub enum CompletionContractVerification {
    Enabled,
    DisabledDuringStep,
}

pub enum ActionNoToolPolicy {
    RequireWriteForActionPrompt,
    RequireToolOnlyIfNoToolSeen,
}
```

互換性:

- 既存 `run_session_with_outcome_with_ui()` は default options で維持する
- plan-run / repair-run だけ `PromptArtifactExtraction::Disabled` + `CompletionContractPathMerge::Disabled` + `CompletionContractVerification::DisabledDuringStep` + `RequireToolOnlyIfNoToolSeen` を渡す
- plan-level final contract verification は別 helper で行う
- public surface を増やしすぎないため、内部 helper `run_session_with_outcome_with_options()` を追加する形でもよい

## Phase Overview

| Phase | 目的 | 主対象 |
| --- | --- | --- |
| Phase 0 | baseline / source parity evidence 固定 | docs / eval output |
| Phase 1 | step execution options API 追加 | `minimal_loop/loop_run.rs` |
| Phase 2 | plan-run step で prompt artifact extraction を無効化 | `planner/runner.rs`, `minimal_loop/loop_run.rs` |
| Phase 3 | no-tool finalization を source parity へ修正 | `minimal_loop/loop_run.rs` |
| Phase 4 | step kind aware completion policy を明示 | `planner/runner.rs`, tests |
| Phase 5 | plan-level final contract verification を追加 | `planner/runner.rs`, completion |
| Phase 6 | eval 指標に obligation scope を追加 | eval scripts |
| Phase 7 | unit / integration regression tests | Rust / Python tests |
| Phase 8 | MVP vs anvildev eval 比較 | eval runner |
| Phase 9 | risk review / documentation update | workspace docs |

## Phase 0: Baseline And Evidence Lock

### 作業

- 直近 MVP plan-run 2 回分と anvildev fresh の結果を本計画に baseline として固定する
- `required_artifacts_missing` と `missing_tool_call` の代表 run を 1 件ずつ記録する
- source / MVP の該当関数を traceability として記載する

### 代表 run

`required_artifacts_missing`:

- MVP r2
- scenario: `docs-heading-update-small`
- main/planner: `openai -> gemini`
- observation:
  - plan first step: `inspect-workspace`
  - prompt includes `Required final artifacts: README.md`
  - loop effective required paths includes `README.md`
  - model repeats Glob
  - stop: `required_artifacts_missing`

`missing_tool_call`:

- MVP r2
- scenario: `fix-js-date-helper-small`
- main/planner: `gemini -> openai`
- observation:
  - artifact files were written
  - later setup/verify step ran Bash
  - no-tool response after feedback became `missing tool call for action prompt after feedback`

### 受け入れ条件

- baseline counts が本文に記録されている
- source と MVP の該当関数が本文に明記されている
- 以降の eval と比較する指標が固定されている

### テスト

ドキュメントフェーズのみ。

## Phase 1: Add Step Execution Options

### 作業

- `mvp/anvilminimal/src/minimal_loop/loop_run.rs` に options struct / enum を追加する
- 既存 API は default behavior のまま維持する
- internal implementation を options-aware helper へ移す

候補:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PromptArtifactExtraction {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletionContractPathMerge {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompletionContractVerification {
    Enabled,
    DisabledDuringStep,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActionNoToolPolicy {
    RequireWriteForActionPrompt,
    RequireToolOnlyIfNoToolSeen,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RunSessionOptions {
    pub prompt_artifact_extraction: PromptArtifactExtraction,
    pub completion_contract_path_merge: CompletionContractPathMerge,
    pub completion_contract_verification: CompletionContractVerification,
    pub action_no_tool_policy: ActionNoToolPolicy,
}
```

default:

- `PromptArtifactExtraction::Enabled`
- `CompletionContractPathMerge::Enabled`
- `CompletionContractVerification::Enabled`
- `ActionNoToolPolicy::RequireWriteForActionPrompt`

### 受け入れ条件

- 既存 `minimal-loop --prompt` の挙動は変わらない
- options 未指定経路のテストが既存通り通る
- plan-run 用 options を指定できる internal API がある
- completion contract paths を step turn に混ぜる/混ぜないを options で制御できる
- completion contract verify / deferred requirement / iteration extension を step turn で有効/無効にできる
- options は crate 外 public API として不要に広げない

### テスト

- `default_run_session_options_preserve_prompt_artifact_extraction`
- `default_run_session_options_preserve_completion_contract_path_merge`
- `default_run_session_options_preserve_completion_contract_verification`
- `default_run_session_options_preserve_action_no_tool_policy`
- existing minimal loop tests unchanged

## Phase 2: Disable Prompt Artifact Extraction For Plan-run Steps

### 作業

- `run_step()` と repair retry の `run_session_with_outcome_with_ui()` 呼び出しを options-aware helper へ変更する
- plan-run step では:
  - explicit required paths: `step.expected_paths`
  - prompt artifact extraction: disabled
  - completion contract paths: disabled for step scope
  - completion contract verification / deferred requirement checks: disabled for step scope
  - completion contract による iteration extension / artifact recovery enabling: disabled for step scope
- overall final artifacts は prompt context としては残す
- current step obligation は `step.expected_paths` のみに限定する

注意:

- `minimal-loop --prompt` では prompt artifact extraction を維持する
- `completion_contract_json` を CLI で渡す通常実行では既存挙動を維持する
- plan-run では external completion contract がある場合も current step に混ぜない。Phase 5 の plan-level final contract verification で見る

### 受け入れ条件

- `inspect` step の `step.expected_paths` が空なら、`Required final artifacts` block があっても current required paths は空になる
- `implement` step では `step.expected_paths` が required paths として効く
- repair prompt 内の `Required final artifacts` も current required paths に混入しない
- external completion contract paths が current step required paths に混入しない
- external completion contract verify / deferred requirement が current step turn の成否を直接決めない
- completion contract が存在するだけで plan-run step の iteration limit や artifact recovery policy が変化しない
- `required_artifacts_missing` の代表 regression test が通る

### テスト

Rust unit / integration:

- `plan_step_disables_prompt_required_artifact_extraction`
- `minimal_loop_prompt_keeps_required_artifact_extraction`
- `repair_step_disables_prompt_required_artifact_extraction`
- `plan_step_disables_completion_contract_path_merge`
- `plan_step_disables_completion_contract_verification_side_effects`
- `inspect_step_does_not_require_overall_final_artifact`
- `implement_step_still_requires_expected_paths`

## Phase 3: Align No-tool Finalization With Source

### 作業

- MVP minimal loop に `tool_calls_seen` を導入する、または existing `tool_call_count` を使う
- plan-run step options では:
  - no tool call ever seen + action prompt -> feedback / error
  - tool call seen + no Write/Edit + expected paths satisfied or no expected paths -> runner の verify/result 判定へ委譲
  - expected paths missing -> missing artifact feedback / recovery を優先
- `write_or_edit_seen` だけを no-tool finalization の hard gate にしない

source reference:

- `src/agent/minimal_loop/loop_run.rs`
  - `!tool_calls_seen` の場合のみ missing tool call feedback

MVP target:

- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`
  - current `!write_or_edit_seen && looks_like_action_prompt(user_prompt)` gate

### 受け入れ条件

- Bash-only verify/setup step が no-tool final answer で `missing_tool_call` にならない
- No tool at all の implement step は引き続き feedback/error になる
- expected paths が未達の implement step は no-tool final を許さない
- Write/Edit が必要な implement step の安全装置は弱まらない
- finalization_score が `missing_tool_call` 系で改善する

### テスト

- `verify_step_with_bash_then_final_text_is_allowed`
- `setup_step_with_bash_then_final_text_is_allowed`
- `implement_step_no_tool_still_gets_missing_tool_feedback`
- `implement_step_with_read_only_and_missing_expected_paths_still_gets_artifact_feedback`
- `action_prompt_after_read_only_can_delegate_to_runner_when_step_scope_allows`
- existing `repeated_planned_action_without_tool_returns_error` remains valid for default minimal-loop mode

## Phase 4: Step-kind Aware Completion Policy

### 作業

- `runner.rs` 側で step kind に応じた options を決める
- MVP の `StepKind` が `Implement / Inspect / Setup / Verify / Report` に畳まれている前提で、以下を適用する

| StepKind | prompt extraction | no-tool policy | expected path strictness |
| --- | --- | --- | --- |
| `Implement` | disabled | require tool only if no tool seen | strict |
| `Setup` | disabled | require tool only if no tool seen | strict only if expected_paths non-empty |
| `Verify` | disabled | require tool only if no tool seen | strict only if expected_paths non-empty |
| `Inspect` | disabled | require tool only if no tool seen | no implicit final artifact |
| `Report` | disabled | allow only explicit blocker semantics | no implicit final artifact |

- runner の `verify_step()` を最終判定として明確に使う
- `report` は Phase 010 の方針通り、通常成功には使わない

### 受け入れ条件

- `inspect` step が空 workspace で全体 artifact 作成を要求しない
- `verify` step は Bash 実行後、`verify_step()` が pass なら完了する
- `verify` step の verify failure は repair に進む
- `implement` step は expected_paths 未達なら失敗または repair へ進む
- `implement` step は expected_paths 達成前の prose final を完了扱いしない

### テスト

- `inspect_step_with_final_artifact_context_completes_without_creating_artifact`
- `verify_step_bash_success_completes_without_write`
- `verify_step_bash_failure_runs_repair`
- `implement_step_missing_expected_path_runs_recovery`

## Phase 5: Plan-level Final Contract Verification

### 作業

- `run_step_plan_with_ui()` の全 step 完了後に plan-level final verification を追加する
- 検証対象:
  - plan goal から抽出した required final artifacts
  - plan prompt context に明示された required final artifacts
  - `StepPlan` 全体の expected paths union
  - optional external completion contract の required paths / verify / profile requirements
  - suite postcheck は eval harness 側で引き続き最終判定に使う
- step turn では completion contract paths を混ぜないが、plan-level final では混ぜる
- plan-level final verification の結果を eval event と stdout/stderr diagnostic に出す

event 候補:

```json
{
  "event": "plan_final_contract",
  "required_final_artifacts": ["README.md"],
  "missing_final_artifacts": [],
  "external_contract_checked": true,
  "verify_attempts": 1,
  "ok": true
}
```

### 受け入れ条件

- 全 step が通っても required final artifact が欠けていれば plan-run は失敗する
- required final artifact が揃っていれば plan-level final verification は pass する
- external completion contract がある場合、step scope には混ぜず plan-level で検証する
- external completion contract の verify / deferred requirement が fail の場合は plan-level final contract failure として分類できる
- eval success は postcheck と plan-level final contract の両方を満たす

### テスト

- `plan_run_final_contract_fails_when_required_final_artifact_missing`
- `plan_run_final_contract_passes_after_step_artifacts_created`
- `plan_run_external_completion_contract_checked_at_plan_level`
- `plan_run_step_scope_does_not_inherit_external_completion_contract`
- `plan_run_final_contract_fails_when_external_contract_verify_fails`

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

eval runtime scoring に以下を追加する。

最小追加:

- `step_obligation_scope_score`
- `step_obligation_scope_violation_count`

event 候補:

```json
{
  "event": "step_obligation_scope",
  "step_id": "...",
  "step_kind": "...",
  "explicit_required_paths": ["..."],
  "prompt_extracted_paths_enabled": false,
  "prompt_extracted_paths": [],
  "completion_contract_path_merge_enabled": false,
  "completion_contract_paths": [],
  "effective_required_paths": ["..."],
  "initially_missing_paths": ["..."],
  "contract_paths_merged": false
}
```

score 例:

- 100: plan-run step で prompt artifact extraction disabled、explicit paths のみ
- 70: extraction enabled だが prompt extracted paths が 0
- 30: extraction enabled かつ context-only final artifacts が current step に混入
- blank: event missing

### 受け入れ条件

- MVP plan-run rows に `step_obligation_scope_score` が出る
- anvildev / old summaries で event が欠損しても report が壊れない
- MVP post-change run で `step_obligation_scope` event が欠損した場合は diagnostic 上の欠損として扱える
- plan-level final contract failure と obligation scope violation が `unclassified_process_failure` に落ちない
- `prompt_contract_score` と役割が重複しない
- `effective_required_paths` の内訳から scope 混入の有無を診断できる
- path list は workspace-relative のみで、prompt 本文は保存しない
- 指標数が過剰にならないよう、core table には runtime health 系としてまとめる

### テスト

Python:

- `test_step_obligation_scope_score_blank_without_events`
- `test_step_obligation_scope_score_detects_disabled_extraction`
- `test_step_obligation_scope_score_penalizes_context_artifact_merge`
- `test_step_obligation_scope_score_uses_effective_required_path_breakdown`
- `test_summary_schema_accepts_step_obligation_scope_score`
- `test_failure_classification_recognizes_plan_final_contract_failure`
- `test_failure_classification_recognizes_step_obligation_scope_violation`

## Phase 7: Regression Tests

### Rust tests

対象:

- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`
- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/tui/slash.rs`
- `mvp/anvilminimal/src/lib.rs`

追加/更新:

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

### Python eval tests

対象:

- `mvp/anvilminimal/tests/eval/test_runtime_scoring.py`
- `mvp/anvilminimal/tests/eval/test_summary_schema.py`
- `mvp/anvilminimal/tests/eval/test_failure_classification.py`

追加/更新:

- obligation scope metrics
- report compare compatibility
- missing event compatibility
- failure classification for plan final contract / obligation scope failures

### 受け入れ条件

以下が通る。

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml minimal_loop
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::runner
cargo test --manifest-path mvp/anvilminimal/Cargo.toml
python3 -m unittest discover -s mvp/anvilminimal/tests/eval -p 'test_*.py'
```

route coverage tests must assert more than parser dispatch. They must prove that `run_step_plan_with_ui()` receives the same plan-run step options by inspecting deterministic fake-client outcomes or `step_obligation_scope` events.

## Phase 8: Eval Acceptance

### 実行条件

スピード重視、ローカル LLM 未使用。

MVP:

```bash
cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml
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

anvildev:

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

Additional MVP smoke:

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

### Primary acceptance gates

MVP:

- `plan-run` speed-cloud を原則 2 回実行し、平均 success >= 6/12 かつ worst run >= 5/12
- 予算/時間制約で 1 回のみの場合は provisional とし、success >= 6/12 を満たす
- `required_artifacts_missing` <= 1/run
- `missing_tool_call` <= 2/run
- `plan_final_contract_failure` が genuine missing artifact / verify failure として説明可能で、`unclassified_process_failure` に落ちない
- `plan_run_runtime_health_score` avg >= 65
- `step_obligation_scope_score` avg >= 95 for MVP plan-run rows
- `prompt_contract_score` remains 100
- `artifact_progress_score` improves for cases previously `required_artifacts_missing`
- `finalization_score` improves for cases previously `missing_tool_call`

Comparative:

- MVP failure distribution becomes explainable against anvildev
- anvildev comparison remains a reference, not a hard identical-success target
- if anvildev succeeds and MVP fails for same scenario/model pair, failure kind must not be `context final artifact mixed into current step`

### Secondary gates

- `minimal-loop` smoke/regression does not regress
- `step-plan` static metrics do not materially regress
- `/run-plan` and TUI slash plan-run use the same step runtime options
- `ultra-plan-run` does not regress due to step runtime option changes
- `ultra-step-run` does not regress due to step runtime option changes
- verify policy errors are not hidden
- postcheck remains authoritative for success

## Phase 9: Risk Review And Documentation

### Risks

| Risk | 内容 | 対策 |
| --- | --- | --- |
| R1 | prompt artifact extraction を無効化しすぎて expected artifact が作られなくなる | plan-run step では explicit `step.expected_paths` を strict に残す |
| R2 | no-tool finalization を緩めすぎて未作業完了を許す | `RequireToolOnlyIfNoToolSeen` は plan-run step options のみ。default minimal-loop は維持 |
| R3 | verify/setup step が弱くなる | runner `verify_step()` と postcheck を維持 |
| R4 | 指標追加で評価が複雑になる | 追加指標は `step_obligation_scope_score` に限定し、core runtime health へ統合 |
| R5 | eval 過剰適応 | scenario 名分岐禁止。source parity と generic step-kind policy で実装 |
| R6 | anvildev と完全一致を目指して MVP が複雑化する | source の思想を取り込み、MVP API は最小の step execution options に限定 |
| R7 | step scope から external contract を外して plan 全体の保証が弱まる | Phase 5 の plan-level final contract verification を必須化 |
| R8 | completion contract verification だけが step turn に残り、Phase 2 後も step scope が混ざる | `CompletionContractVerification::DisabledDuringStep` を追加し、step-level side effect をテストする |
| R9 | 新規 failure が eval で unclassified になり原因追跡できない | `failure_classification.py` に plan final contract / obligation scope 系分類を追加する |

### Documentation updates

- 本ファイルに実装結果、test result、eval result を追記する
- 必要なら `workspace/mvp/eval/010/step_runner_source_parity_matrix.md` の status を更新する
- 指標追加時は `mvp/anvilminimal/eval/README.md` の metrics section を更新する

## Final Done Definition

Phase 0〜9 が完了し、以下を満たすこと。

- Rust / Python tests が通る
- release build が通る
- MVP plan-run eval が現 baseline から改善する
- `required_artifacts_missing` と `missing_tool_call` が明確に減る
- `step_obligation_scope_score` が plan-run rows に出力される
- plan-level final contract verification が実装され、必要成果物欠落を検出できる
- plan-level final contract failure が分類/summary で追跡でき、unclassified に落ちない
- `minimal-loop --prompt` の既存挙動が regression していない
- `/run-plan`、TUI slash、`ultra-plan-run`、`ultra-step-run` の step execution scope が同じ方針になっている
- anvildev 比較結果と残差理由が本文へ追記されている
- eval scenario 固有分岐が入っていないことをレビュー済み

## Implementation Result 2026-06-26

### Changed files

- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`
  - `RunSessionOptions` と step scope 用 runtime policy を追加
  - plan-run step では prompt artifact extraction / completion contract path merge / completion contract verification を step turn から外す
  - default minimal-loop では従来の prompt artifact extraction / completion contract behavior を維持
  - `step_obligation_scope` eval event を追加し、explicit / prompt-extracted / contract / effective required paths を分離して観測可能にした
  - plan-run step の no-tool finalization を source の考え方に寄せ、Bash/Read/Glob 等の tool 使用後は runner 側 verify へ委譲できるようにした
  - expected path のない inspect/setup/verify step は tool 実行後に `step_tool_observation_completed` で runner へ戻れるようにした
- `mvp/anvilminimal/src/planner/runner.rs`
  - plan-run / repair retry / ultra phase step execution に同じ step runtime options を適用
  - plan-level final contract verification を追加し、step scope から外した final artifacts / external completion contract を plan 完了後に検証
  - ultra phase は final phase のみ plan-level final contract を強制し、中間 phase で最終成果物を過剰要求しないようにした
- `mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py`
  - `step_obligation_scope_score` と violation count を追加
- `mvp/anvilminimal/scripts/eval_lib/run_summary.py`
  - 新 runtime metric を summary schema に追加
- `mvp/anvilminimal/scripts/eval_lib/report.py`
  - core metrics / detailed diagnostics / compare に obligation scope を追加
- `mvp/anvilminimal/scripts/eval_lib/failure_classification.py`
  - `plan_final_contract_failure`
  - `step_obligation_scope_violation`
  - `step_verify_failure`
  - を分類できるようにした
- `mvp/anvilminimal/tests/eval/*`
  - runtime scoring / summary schema / failure classification のテストを追加・更新
- `mvp/anvilminimal/eval/README.md`
  - runtime metrics の読み方に `step_obligation_scope_score` を追記

### Tests run

```bash
python3 -m unittest discover -s mvp/anvilminimal/tests/eval -p 'test_*.py'
cargo test --manifest-path mvp/anvilminimal/Cargo.toml minimal_loop
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::runner
cargo test --manifest-path mvp/anvilminimal/Cargo.toml
cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml
```

結果:

- Python eval tests: 85 tests, 1 skipped, pass
- `cargo test ... minimal_loop`: 64 tests, pass
- `cargo test ... planner::runner`: 32 tests, pass
- full `cargo test`: 270 lib tests + integration tests, pass; live/pty-only tests are ignored by design
- release build: pass

### Eval runs

Network access が必要な provider eval は sandbox 外実行で確認した。

| run root | target | result | failure summary | key metrics |
| --- | --- | ---: | --- | --- |
| `/private/tmp/anvilminimal-eval-011-mvp-plan-run-2-net-real` | MVP plan-run | 10/12 | `postcheck_failure`: 1, cached `unclassified_process_failure`: 1 | runtime_health 73.5, obligation_scope 100.0, artifact_progress 97.5, finalization 45.0, prompt_contract 100.0 |
| `/private/tmp/anvilminimal-eval-011-mvp-plan-run-3-net-real` | MVP plan-run | 9/12 | cached `unclassified_process_failure`: 1, `verify_command_policy_error`: 2 | runtime_health 68.4, obligation_scope 100.0, artifact_progress 86.7, finalization 42.5, prompt_contract 100.0 |
| `/private/tmp/anvilminimal-eval-011-anvildev-plan-run-1-net-real` | anvildev plan-run | 9/12 | `unclassified_process_failure`: 3 | reference comparison |
| `/private/tmp/anvilminimal-eval-011-mvp-cross-mode-1-net-real` | MVP step/plan/ultra smoke | 28 success, 8 failed, 12 skipped | plan-run 9/12, step-plan 12/12, ultra-plan-run 7/12, ultra-step-run skipped | plan-run runtime_health 74.3, obligation_scope 100.0 |

補足:

- 既存 `summary.eval.tsv` の `extras_json` は eval 実行時点の分類を保持するため、後から `eval-report.py` を再実行しても cached `unclassified_process_failure` は書き換わらない。
- 実体を確認した cached `unclassified_process_failure` は `step ... failed verification after bounded repair` であり、新規分類 `step_verify_failure` を追加した。以後の eval run ではこの failure kind として追跡できる。

### Acceptance gate result

| gate | result | note |
| --- | --- | --- |
| MVP plan-run 2回平均 success >= 6/12 | pass | 10/12, 9/12 |
| worst run >= 5/12 | pass | worst 9/12 |
| `required_artifacts_missing` <= 1/run | pass | post-change run では 0 |
| `missing_tool_call` <= 2/run | pass | post-change run では 0 |
| `plan_run_runtime_health_score` avg >= 65 | pass | 73.5 / 68.4 / cross 74.3 |
| `step_obligation_scope_score` avg >= 95 | pass | 100.0 |
| `prompt_contract_score` remains 100 | pass | 100.0 |
| plan-level final contract failure が unclassified に落ちない | pass by unit test | `plan_final_contract_failure` classification を追加済み |
| obligation scope violation が unclassified に落ちない | pass by unit test | `step_obligation_scope_violation` classification を追加済み |
| step verify failure が unclassified に落ちない | pass by unit test | `step_verify_failure` classification を追加済み |

### Remaining failures and next actions

残っている plan-run 失敗は、Phase 011 の主対象だった `required_artifacts_missing` / `missing_tool_call` ではなく、以下に寄った。

- `verify_command_policy_error`
  - planner が shell control syntax や setup/dev-server 相当を含む verify を再生成してしまうケース
  - 対策先は planner repair / verify command sanitizer であり、step runtime scope の問題ではない
- `postcheck_failure`
  - final artifacts は揃っているが scenario postcheck の意味条件を満たさないケース
  - 対策先は plan quality / execution repair feedback
- `step_verify_failure`
  - bounded repair 後も runner verify command が失敗するケース
  - 対策先は verify failure feedback の具体化と planner verify command 品質
- `ultra-plan-run` の `phase_scaffold_error` / `planner_schema_error` / `planner_lint_error`
  - ultra phase scaffold / planner schema 系であり、plan-run step runtime とは別系統

### Anti-overfitting review

- scenario 名、固定 artifact 名、provider 名に依存した成功分岐は追加していない
- verify / postcheck を弱めていない
- plan-run step から final artifact obligation を外した代わりに、plan-level final contract verification を追加して全体成果物保証を維持した
- default `minimal-loop --prompt` の completion contract 強化は維持した
- eval 指標追加は runtime の観測性向上であり、成功判定を変更していない
