# Source / MVP Trace Manifest

作成日: 2026-06-29

## 1. Baseline

| Item | Value |
| --- | --- |
| baseline commit | `2bc9209131b2ca83564aa7c2fca458e3e83e0124` |
| repo | `/Users/maenokota/share/work/github_kewton/Anvil-develop` |
| MVP binary under evaluation | `mvp/anvilminimal/target/release/anvilminimal` or `target/release/anvilminimal` depending eval command |
| source binary | `anvildev --engine minimal` |
| trace schema | `workspace/mvp/eval/022/parity_gate_trace_schema.md` |
| report schema | `workspace/mvp/eval/022/parity_gate_report_schema.md` |

## 2. Current MVP Runs

| trace_id | subject | run_root | summary | suite | modes | result | known gaps |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `mvp-0229-net-timeout` | MVP anvilminimal | `/private/tmp/anvilminimal-eval-0229-mvp-net-timeout` | `/private/tmp/anvilminimal-eval-0229-mvp-net-timeout/summary.eval.tsv` | `mvp-smoke` | all 4 modes | 36/48 success | release browser evidence failed; G-S12 downgraded by browser/interaction content |
| `mvp-0219-smoke` | MVP anvilminimal | `/private/tmp/anvilminimal-eval-0219-mvp-smoke` | `/private/tmp/anvilminimal-eval-0219-mvp-smoke/summary.eval.tsv` | `mvp-smoke` | all 4 modes | 38/48 success | `failure_kind` blank 10 rows, source same-condition comparison missing |
| `mvp-0219-provider-smoke` | MVP anvilminimal | `/private/tmp/anvilminimal-eval-0219-provider-smoke` | `/private/tmp/anvilminimal-eval-0219-provider-smoke/summary.eval.tsv` | `mvp-provider-smoke` | all 4 modes | 19/24 success | `failure_kind` blank 5 rows, provider drift still planning-visible |

### Current MVP Summary

`mvp-0229-net-timeout`:

| Mode | Success |
| --- | ---: |
| minimal-loop | 9/12 |
| step-plan | 12/12 |
| plan-run | 9/12 |
| ultra-plan-run | 6/12 |

Failure layer:

| Layer | Count |
| --- | ---: |
| runtime | 4 |
| bridge | 5 |
| planning | 3 |

`mvp-0219-smoke`:

| Mode | Success |
| --- | ---: |
| minimal-loop | 10/12 |
| step-plan | 11/12 |
| plan-run | 8/12 |
| ultra-plan-run | 9/12 |

Failure layer:

| Layer | Count |
| --- | ---: |
| runtime | 2 |
| bridge | 4 |
| planning | 4 |

`mvp-0219-provider-smoke`:

| Mode | Success |
| --- | ---: |
| minimal-loop | 6/6 |
| step-plan | 5/6 |
| plan-run | 5/6 |
| ultra-plan-run | 3/6 |

Failure layer:

| Layer | Count |
| --- | ---: |
| planning | 5 |

## 3. Previous MVP Comparison Runs

| trace_id | subject | run_root | summary | result | use |
| --- | --- | --- | --- | --- | --- |
| `mvp-0213-postcommit-smoke` | MVP anvilminimal | `/private/tmp/anvilminimal-0213-postcommit-smoke` | `/private/tmp/anvilminimal-0213-postcommit-smoke/summary.eval.tsv` | 37/48 success | pre-021-5〜021-9 comparison |
| `mvp-rollback-provider-smoke` | MVP anvilminimal | `/private/tmp/anvilminimal-eval-rollback-provider-smoke` | `/private/tmp/anvilminimal-eval-rollback-provider-smoke/summary.eval.tsv` | 18/24 success | provider smoke rollback baseline |

## 4. Source Trace Status

REC-007 で、0229 の MVP run と同条件の `anvildev --engine minimal` trace を登録した。
source trace が観測していない gate は code reference だけでは pass にせず、normalized diff で `fail` とする。

| trace_id | subject | status | note |
| --- | --- | --- | --- |
| `source-0229-net-timeout` | source anvildev | registered | `/private/tmp/anvilminimal-eval-0229-anvildev-net-timeout/runtime-semantics-trace-report.json` |
| `source-code-reference` | source anvildev | partial | source code references は確認済みだが、trace 証跡ではない |

## 4.1 Trace Writer Status

022-2 で `mvp/anvilminimal/scripts/eval_lib/runtime_trace.py` と `mvp/anvilminimal/scripts/eval-trace.py` を追加した。

出力される artifact:

| Artifact | 内容 |
| --- | --- |
| `runtime-semantics-normalized-events.jsonl` | raw event を normalized lifecycle stage / gate ids へ写像した JSONL |
| `runtime-semantics-trace-report.json` | subject、binary kind、stage counts、gate counts、silent exit count、known gaps |
| `runtime-semantics-trace-manifest.md` | 再実行可能な redacted command と run ごとの trace 状態 |
| `runtime-semantics-trace-diff.json` | source/MVP trace report の stage/gate 差分。REC-007 では `workspace/mvp/eval/022/runtime-semantics-trace-diff.json` に保存 |

redaction:

- task prompt は `<redacted-task-prompt>` に置換する。
- API key / bearer token / request id は `eval_lib.redaction` を通す。
- provider raw response body は trace に保存しない。

gate への影響:

- G-S02/G-S03/G-S08/G-S14 は source/MVP trace の両方に観測され `pass`。
- G-S01/G-S04/G-S05/G-S06/G-S07/G-S09/G-S10/G-S11/G-S13/G-S15 は source trace 側の normalized gate 未観測により `fail`。
- G-S12 は trace 上は観測されたが、browser readiness HTTP 500 と interaction canvas unavailable により release gate では `fail`。
- G-S16 は normalized trace 未観測かつ manual TUI evidence が `tui_command_failed` のため `fail`。

## 5. Source Code References Used For Trace Planning

| Stage | Source refs |
| --- | --- |
| UltraPlan generation | `src/agent/minimal_step_runner.rs::generate_ultra_plan` |
| Ultra phase execution | `src/agent/minimal_step_runner.rs::run_ultra_plan` |
| Profiled phase prompt | `src/agent/minimal_step_runner/profile.rs::build_profiled_phase_prompt` |
| Phase profile verify | `src/agent/minimal_step_runner/profile.rs::verify_profile_after_phase` |
| Step repair prompt | `src/agent/minimal_step_runner/repair.rs::build_repair_prompt` |
| Repair exhausted handoff | `src/agent/minimal_step_runner/repair.rs::build_repair_exhausted_report` |
| Task contract | `src/agent/loop_run/task_contract*.rs` |
| Verifier / repair lifecycle | `src/agent/loop_run/verifier*.rs`, `src/agent/loop_run/repair_lifecycle.rs` |
| Scaffold pipeline | `src/agent/loop_run/scaffold_pipeline.rs` |
| Project/dependency probe | `src/agent/loop_run/project_probe.rs`, `src/agent/loop_run/project_verifier.rs`, `src/agent/loop_run/node_*` |
| Provider XML fallback | `src/ollama/xml_fallback.rs` |

## 6. MVP Code References Used For Trace Planning

| Stage | MVP refs |
| --- | --- |
| Plan/phase runner | `mvp/anvilminimal/src/planner/runner.rs` |
| Step plan schema | `mvp/anvilminimal/src/planner/step_plan.rs` |
| Plan verify policy | `mvp/anvilminimal/src/planner/verify.rs` |
| Next.js profile verifier | `mvp/anvilminimal/src/planner/profiles/nextjs.rs` |
| Repair handoff | `mvp/anvilminimal/src/planner/repair.rs` |
| Completion contract | `mvp/anvilminimal/src/minimal_loop/completion.rs` |
| Acceptance evidence | `mvp/anvilminimal/src/minimal_loop/evidence.rs` |
| Repair target | `mvp/anvilminimal/src/minimal_loop/repair_target.rs` |
| Dependency setup/build verify | `mvp/anvilminimal/src/minimal_loop/dependency_setup.rs`, `mvp/anvilminimal/src/minimal_loop/build_verifier.rs` |
| Runtime events | `mvp/anvilminimal/src/eval_events.rs` |
| Eval scoring/classification | `mvp/anvilminimal/scripts/eval_lib/*.py` |

## 7. Trace Gaps Registered As Gate Issues

| Gap | Impact | Gate ids |
| --- | --- | --- |
| source trace lacks normalized lifecycle gates for request/phase/tool/dependency/repair/scaffold/recovery/provider | code reference only cannot pass these gates | G-S01, G-S04, G-S05, G-S06, G-S07, G-S09, G-S10, G-S11, G-S13, G-S15 |
| browser readiness / interaction evidence failed | release gate cannot pass | G-S12 |
| manual TUI UAT trace failed and same-condition normalized trace lacks G-S16 | TUI observability cannot pass | G-S16 |
| latest trace diff attached | comparative gate can resolve pass/fail without `partial` | G-S01〜G-S16 |

## 8. Required Next Trace Commands

MVP:

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --model-profile speed-cloud-5x \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --runs 1 \
  --parallel 5 \
  --provider-limit 5 \
  --binary mvp/anvilminimal/target/release/anvilminimal \
  --binary-kind anvilminimal
```

Source:

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --model-profile speed-cloud-5x \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --runs 1 \
  --parallel 5 \
  --provider-limit 5 \
  --binary anvildev \
  --binary-kind anvildev
```

既存 run root へ後付けで trace artifact を生成する場合:

```bash
python3 mvp/anvilminimal/scripts/eval-trace.py \
  --run-root /path/to/eval-run-root \
  --subject mvp-anvilminimal \
  --binary-kind anvilminimal \
  --binary-path mvp/anvilminimal/target/release/anvilminimal
```

source/MVP の normalized diff を生成する場合:

```bash
python3 mvp/anvilminimal/scripts/eval-trace.py \
  --compare-source-report /path/to/source/runtime-semantics-trace-report.json \
  --compare-mvp-report /path/to/mvp/runtime-semantics-trace-report.json \
  --diff-output /path/to/runtime-semantics-trace-diff.json
```
