# MVP Performance Generalized Repair Validation Results

作成日: 2026-06-27

対象:

- `workspace/mvp/eval/014/mvp_performance_generalized_repair_plan.md`
- `workspace/mvp/eval/014/mvp_performance_generalized_repair_work_breakdown.md`

## Summary

Phase 0〜9 の実装・検証を実施した。

ローカル test / release build / representative known eval / representative blind eval / anvildev comparison / TUI route smoke は完了した。

計画にあった full 3-run known/blind eval は、Next.js large の plan-run / ultra-plan-run が長時間化し、このターンでは完走できなかった。途中結果は provider/API ではなく長時間実行の問題であり、release readiness の残リスクとして記録する。

## Tests

| command | result |
|---|---|
| `python3 -m pytest mvp/anvilminimal/tests/eval -q` | `96 passed, 1 skipped, 49 subtests passed` |
| `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` | `283 passed` in lib tests, integration tests passed, ignored live/pty tests unchanged |
| `cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml` | pass |
| `cargo test --manifest-path mvp/anvilminimal/Cargo.toml tui_ultra_plan_run_smoke_fake_clients` | pass |
| `cargo test --manifest-path mvp/anvilminimal/Cargo.toml parses_user_ultra_plan_run_nextjs_command` | pass |

## Symlink / Binary

```text
which anvilminimal
/Users/maenokota/.local/bin/anvilminimal
```

```text
/Users/maenokota/.local/bin/anvilminimal -> /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/target/release/anvilminimal
```

判定:

- `cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml` で通常実行の `anvilminimal` に反映される。

## Eval Results

### MVP known representative

Command:

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --model-profiles mvp/anvilminimal/eval/model_profiles.yaml \
  --model-profile speed-cloud \
  --modes step-plan,plan-run,ultra-plan-run \
  --scenario fix-js-date-helper-small \
  --runs 1 \
  --parallel 2 \
  --timeout-sec 180 \
  --binary mvp/anvilminimal/target/release/anvilminimal \
  --binary-kind anvilminimal \
  --run-root /private/tmp/anvilminimal-eval-014-mvp-known-scenario-net
```

Result:

- success: `6/6`
- report: `/private/tmp/anvilminimal-eval-014-mvp-known-scenario-net/report.md`

Mode summary:

| mode | success | p50_exec_sec | avg_score |
|---|---:|---:|---:|
| step-plan | 2/2 | 2.4 | 89.5 |
| plan-run | 2/2 | 21.0 | 95.2 |
| ultra-plan-run | 2/2 | 83.2 | 93.8 |

Key metrics:

| metric | value |
|---|---:|
| plan-run postcheck_stability_score | 100.0 |
| plan-run finalization_score | 87.5 |
| plan-run execution_contract_adherence_score | 87.5 |
| ultra phase_completion_score | 100.0 |
| ultra phase_plan_validity_score | 100.0 |
| ultra ultra_runtime_health_score | 100.0 |

### MVP blind representative

Command:

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-blind.yaml \
  --model-profiles mvp/anvilminimal/eval/model_profiles.yaml \
  --model-profile speed-cloud \
  --modes step-plan,plan-run,ultra-plan-run \
  --scenario duration-parser-small \
  --runs 1 \
  --parallel 2 \
  --timeout-sec 180 \
  --binary mvp/anvilminimal/target/release/anvilminimal \
  --binary-kind anvilminimal \
  --run-root /private/tmp/anvilminimal-eval-014-mvp-blind-scenario-net
```

Result:

- success: `6/6`
- report: `/private/tmp/anvilminimal-eval-014-mvp-blind-scenario-net/report.md`

Mode summary:

| mode | success | p50_exec_sec | avg_score |
|---|---:|---:|---:|
| step-plan | 2/2 | 4.7 | 92.0 |
| plan-run | 2/2 | 11.6 | 93.3 |
| ultra-plan-run | 2/2 | 41.2 | 92.0 |

Key metrics:

| metric | value |
|---|---:|
| plan-run postcheck_stability_score | 100.0 |
| plan-run finalization_score | 87.5 |
| plan-run execution_contract_adherence_score | 100.0 |
| ultra phase_completion_score | 100.0 |
| ultra phase_plan_validity_score | 100.0 |
| ultra ultra_runtime_health_score | 100.0 |

### anvildev known representative

Command:

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --model-profiles mvp/anvilminimal/eval/model_profiles.yaml \
  --model-profile speed-cloud \
  --modes step-plan,plan-run,ultra-plan-run \
  --scenario fix-js-date-helper-small \
  --runs 1 \
  --parallel 2 \
  --timeout-sec 180 \
  --binary anvildev \
  --binary-kind anvildev \
  --run-root /private/tmp/anvilminimal-eval-014-anvildev-known-scenario-net
```

Result:

- success: `4/6`
- report: `/private/tmp/anvilminimal-eval-014-anvildev-known-scenario-net/report.md`

Mode summary:

| mode | success | p50_exec_sec | avg_score |
|---|---:|---:|---:|
| step-plan | 2/2 | 7.9 | 63.1 |
| plan-run | 1/2 | 7.7 | 43.4 |
| ultra-plan-run | 1/2 | 8.7 | 59.4 |

Comparison:

- compare: `/private/tmp/anvilminimal-eval-014-known-scenario-compare.md`

| metric | anvildev | MVP | delta |
|---|---:|---:|---:|
| success_rate | 66.7 | 100.0 | +33.3 |
| valid_plan_generated_rate | 83.3 | 100.0 | +16.7 |
| plan_quality_score_avg | 72.5 | 89.8 | +17.3 |
| executable_plan_score_avg | 26.2 | 84.3 | +58.1 |
| verify_strength_score_avg | 66.2 | 82.0 | +15.8 |
| artifact_ownership_score_avg | 19.5 | 88.0 | +68.5 |
| overall_score_avg | 55.3 | 92.9 | +37.6 |

## Full Eval Attempt

Full 3-run known eval:

- run root: `/private/tmp/anvilminimal-eval-014-mvp-smoke-net`
- requested: `108 runs`
- status: interrupted before summary write
- observed progress: successful rows were produced, but long-running ultra-plan-run rows made total completion impractical in this turn

Full 1-run known eval:

- run root: `/private/tmp/anvilminimal-eval-014-mvp-smoke-1run-net`
- requested: `36 runs`
- status: interrupted before summary write
- completed before interruption: 35/36 visible result lines
- remaining blocker: `nextjs-space-invaders-large` ultra-plan-run with Gemini main / OpenAI planner was still running

このため、本結果では representative known/blind eval を完走証跡として採用し、full 3-run は未完了リスクとして扱う。

## Qualitative Review

### known: `fix-js-date-helper-small`

Representative plan-run YAML:

- `/private/tmp/anvilminimal-eval-014-mvp-known-scenario-net/runs/mvp-smoke__fix-js-date-helper-small__plan-run__openai-gpt-5.4-mini__gemini-gemini-3.5-flash__r1/workdir/.anvil/plans/plan-019f08c7-26ca-7f80-96aa-ce3cc3a54336.yaml`

確認結果:

- required final artifact `date-helper.js` を implement step が所有している。
- deterministic smoke check を `smoke-check.js` として別 artifact 化し、verify step は `node smoke-check.js` の single command になっている。
- verify command policy に違反していない。
- `smoke-check.js` は追加 artifact だが、verify 用補助 artifact として自然。

### blind: `duration-parser-small`

Representative plan-run YAML:

- `/private/tmp/anvilminimal-eval-014-mvp-blind-scenario-net/runs/mvp-blind__duration-parser-small__plan-run__openai-gpt-5.4-mini__gemini-gemini-3.5-flash__r1/workdir/.anvil/plans/plan-019f08c9-6ab9-7f83-af0b-991fb82fc70a.yaml`

成果物:

- `/private/tmp/anvilminimal-eval-014-mvp-blind-scenario-net/runs/mvp-blind__duration-parser-small__plan-run__openai-gpt-5.4-mini__gemini-gemini-3.5-flash__r1/workdir/duration_parser.js`

確認結果:

- `duration_parser.js` を expected artifact として exactly once に近い形で所有している。
- `node duration_parser.js` の self-test で postcheck と一致する。
- parser は `2h 15m`, `45s`, `1h30m`, `1.5h` を検証しており、prompt 制約を満たしている。
- 固定 scenario 名に依存した実装ではない。

## Residual Risks

1. Full 3-run eval は未完走。
   - 特に Next.js large の ultra-plan-run は時間が長い。
   - 別途 timeout/process cleanup を改善した上で再実行が必要。

2. representative eval は small scenario 中心。
   - known/blind とも 6/6 だが、大規模 Next.js の実行成功率まではこの文書では保証しない。

3. `runtime_friction_score` が ultra 成功時でも低く出るケースがある。
   - 成功していても long-running multi-phase 実行を強く penalize している可能性がある。
   - 指標の意味は「失敗予兆」だけでなく「実行摩擦」なので、成功ケースで 0 になる妥当性は別途見直し候補。

4. anvildev comparison は representative scenario のみ。
   - full comparison ではないため、性能差の一般化はこの結果だけでは断定しない。

## Phase 9 判定

完了:

- release build: pass
- symlink: release binary を参照
- CLI help: `--plan-steps`, `--plan-run`, `--ultra-plan-run`, `--run-plan`, `--run-ultra-plan` が表示
- TUI route fake-client smoke: pass
- slash parser: `/ultra-plan-run --profile nextjs ...` pass

未完了:

- 実 TTY での手動 ESC 動作確認
- ユーザー指定の Next.js large ultra-plan-run の end-to-end 完走確認

