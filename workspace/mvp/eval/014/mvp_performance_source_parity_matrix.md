# MVP Performance Source Parity Matrix

作成日: 2026-06-27

対象計画:

- `workspace/mvp/eval/014/mvp_performance_generalized_repair_plan.md`
- `workspace/mvp/eval/014/mvp_performance_generalized_repair_work_breakdown.md`

## 目的

Phase 1 以降の実装修正に入る前に、移植元 Anvil の minimal step runner と MVP `anvilminimal` の差分を棚卸しし、今回採用する差分と採用しない差分を明確にする。

この文書は Phase 7 の gate として扱う。

## 調査コマンド

```bash
rg -n "build_step_prompt|build_repair_prompt|ultra_phase_prompt|verify_plan_final_contract|run_step_plan_with_ui|run_ultra_plan_with_ui" \
  src mvp/anvilminimal/src workspace/mvp/eval/010 workspace/mvp/eval/011
```

```bash
rg -n "handle_command|/plan-run|/ultra-plan-run|run_plan_file_with_ui|generate_and_run_ultra_plan_with_ui|/plan-steps" \
  mvp/anvilminimal/src/tui mvp/anvilminimal/src/planner
```

```bash
rg -n "minimal.*step|step.*runner|PlanRun|plan-run|ultra" \
  src/agent src/modes src -g '*.rs'
```

## 判定基準

| status | 意味 |
|---|---|
| `adopt` | MVP に必要で、小さい API として採用する |
| `keep-different` | MVP の設計上、意図的に移植元と違うままにする |
| `defer` | 必要性はあるが今回の scope では大きすぎる |
| `reject` | MVP 設計に反するため採用しない |

`adopt` 以外は Phase 1-5 の実装対象にしない。

## Parity Matrix

| 観点 | 移植元 | MVP 現状 | status | 今回の扱い | evidence |
|---|---|---|---|---|---|
| step prompt contract | `src/agent/minimal_step_runner.rs::build_step_prompt` | `mvp/anvilminimal/src/planner/runner.rs::build_step_prompt` | `adopt` | 既に復元済み。overall goal / required final artifacts / expected paths / verify / expected_result / bounded repair policy を regression test で維持する。 | `runner.rs:1482`, `workspace/mvp/eval/010/step_runner_source_parity_matrix.md` |
| step prompt route | `run_plan` から各 step prompt を実行 client へ渡す | `run_step_plan_with_ui_inner` -> `run_step` -> `build_step_prompt` | `adopt` | 既に復元済み。TUI/CLI の `/plan-run` が同じ runner に到達することを smoke で確認する。 | `runner.rs:252`, `runner.rs:316`, `tui/slash.rs:49` |
| repair prompt context | `src/agent/minimal_step_runner/repair.rs::build_repair_prompt` | `planner/repair.rs::build_repair_prompt_with_context` | `adopt` | 既に plan/step/verify/error context を持つ。Phase 4 で command failure reason と progress warning の欠落を regression guard にする。 | `repair.rs:27`, `runner.rs:360` |
| step verification | source step verify | `planner/verify.rs::verify_step` | `adopt` | verify policy は弱めない。Phase 2 で診断理由を構造化し、retry/policy classification が同じ情報を使えるようにする。 | `verify.rs:95`, `verify.rs:137` |
| plan lint | source plan validation / lint | `planner/lint.rs::lint_step_plan_report` | `adopt` | 既存 lint を維持。Phase 1 で schema/lint retry prompt に structured issue を戻し、invalid plan を通さず修復の説明力を上げる。 | `runner.rs` の `lint_step_plan_report` 呼び出し |
| ultra phase prompt | `src/agent/minimal_step_runner/profile.rs::build_profiled_phase_prompt` | `planner/runner.rs::ultra_phase_prompt` | `adopt` | workspace snapshot / profile runtime contract / required final artifacts は復元済み。Phase 3 で phase-local validation event と phase completion diagnostics を追加する。 | `runner.rs:1583`, `workspace/mvp/eval/010/step_runner_source_parity_matrix.md` |
| ultra phase execution | source `run_ultra_plan` が phase ごとに `/plan-run` 相当を生成実行 | MVP `run_ultra_plan_with_ui` が phase ごとに `generate_step_plan_with_ui` と `run_step_plan_with_ui_inner` を使う | `adopt` | shared StepPlan pipeline を使う設計は維持。Phase 3 は別 runtime を作らず event/contract validation を補強する。 | `runner.rs:668`, `runner.rs:696`, `runner.rs:735` |
| plan-level final contract | source final artifact/profile handling | `verify_plan_final_contract`, `minimal_loop/completion.rs` | `adopt` | Phase 5 で postcheck-derived repair を入れる場合も、通常 runtime には eval-only oracle を入れない。completion contract/profile contract/verification result 由来に限定する。 | `runner.rs:296`, `runner.rs:442` |
| provider fallback/tool handling | source provider fallback | MVP provider clients + minimal loop tool call handling | `keep-different` | provider abstraction を増やさない。Phase 6 は provider failure の診断分離を主目的にし、retry policy 変更は live API 仮説検証が通る場合だけに限定する。 | `workspace/mvp/eval/014/mvp_performance_generalized_repair_plan.md` |
| TUI slash handoff | source minimal REPL slash commands | `tui/slash.rs::handle_command` | `adopt` | CLI と同じ runner を使う。コマンド名は MVP の実装通り `/plan-steps`, `/plan-run`, `/ultra-plan-run` とする。 | `tui/slash.rs:43`, `tui/slash.rs:49`, `tui/slash.rs:65` |
| path confinement | source workspace confinement | MVP `path_guard`, `resolve_plan_file_path`, verify manifest path guard | `adopt` | 維持。Phase 2 の verify diagnosis でも workspace escape を許可しない。 | `runner.rs:1661`, `verify.rs:166` |
| source large architecture | source `minimal_step_runner` 周辺の大きな module 分割 | MVP `planner/*` + `minimal_loop/*` | `reject` | MVP の小さい API を維持し、source 全体の再移植や provider abstraction 増加は行わない。 | `AGENTS.md`, 計画の非目的 |

## Phase 1-5 へ渡す Adopt 項目

| phase | adopt item | 実装方針 |
|---|---|---|
| Phase 1 | plan lint / schema retry | structured issue を retry prompt と eval event に載せる。invalid plan は引き続き拒否する。 |
| Phase 2 | step verification | verify command diagnosis を構造化する。verify policy は弱めない。 |
| Phase 3 | ultra phase prompt / phase execution | shared StepPlan pipeline を維持し、phase-local validation と diagnostics を追加する。 |
| Phase 4 | repair prompt context | plan/step/verify/progress context を repair prompt と event で追えるようにする。 |
| Phase 5 | final contract / postcheck boundary | eval-only oracle を通常 runtime に入れず、contract-derived reason だけ bounded repair に使う。 |

## Keep-Different / Defer / Reject

| item | status | 理由 |
|---|---|---|
| provider abstraction の拡張 | `keep-different` | MVP は ollama/openai/gemini を小さい provider surface で扱う方針。今回の目的は性能改善であり provider layer 再設計ではない。 |
| source module layout の全面復元 | `reject` | MVP の切り出し方針に反する。必要な contract だけ小さく戻す。 |
| eval suite の `expected_artifacts` を runtime に直接埋め込む | `reject` | 評価過適応になる。runtime へ渡せるのは eval harness が生成した completion contract か、StepPlan/profile/verification 由来の情報に限定する。 |
| provider HTTP failure の自動 retry 拡張 | `defer` | API 挙動依存が強い。Phase 6 で診断分離し、retry 変更は live smoke と別計画で扱う。 |

## Regression Check

| 既存 parity repair | 状態 | 確認 |
|---|---|---|
| step prompt に overall goal / expected_result / verify / expected_paths がある | OK | `build_step_prompt` に全項目あり |
| repair prompt に plan/step/verify/error context がある | OK | `RepairContext` と `VerificationReport` を使用 |
| ultra phase prompt に workspace snapshot / profile runtime contract がある | OK | `ultra_phase_prompt` に snapshot と preferred verify を含む |
| plan-level final contract と step-level obligation が分離している | OK | `verify_plan_final_contract` と `verify_step` が別関数 |
| TUI slash が CLI と同じ runner を通る | OK | `/plan-run` と `/ultra-plan-run` は planner runner へ直接委譲 |

## Gate 判定

Phase 1-5 の実装に進んでよい。

ただし、以下を守る。

- source と異なる provider retry は今回の性能改善に含めない。
- verify policy は緩和しない。
- postcheck oracle を normal runtime に入れない。
- `/step-plan` という存在しないコマンド名は使わず、MVP の実コマンド `/plan-steps` を使う。
