# UAT Report: Q1 Final Round with qwen planner / ornith executor

## Scope

- Target workspace: `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-999-Q1-62_002`
- Binary used by runs: `anvilminimal 0.1.0 build=9e49e4df`
- `run_start.build_commit`: `9e49e4df`
- Note: a standalone `anvilminimal --version` earlier printed `0604a76b 2026-07-05T09:07:30Z`, while runtime banners and events for this round report `build=9e49e4df`. Treat `9e49e4df` as the evidence-backed build for this UAT.
- Model mapping used:
  - replacement for `gemini-3.5-flash`: planner `qwen3.6:27b-coding-nvfp4` / `ollama`
  - replacement for `gemini-3.1-flash-lite`: implementation `ornith:35b` / `ollama`
- Matrix: CLI, TOOL, CONTENT, GAME x 2 runs.
- Rule: no manual interrupt; each run must terminate by runtime classification.

## Preflight

- `cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml`: passed.
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`: passed.
  - lib: 741 passed, 0 failed, 13 ignored.
  - CLI parse / conformance / corpus / eval script / generality / live provider / safety / TUI suites: passed.
- `ollama list`: both required local models were present.
  - `ornith:35b`
  - `qwen3.6:27b-coding-nvfp4`
- Interaction probe dependency exists and `/setup-interaction-probe` reported Playwright `1.61.1` ready for web scenarios.

## Executive Summary

必須条件の「人間の interrupt なしで終端するか」は満たしました。8/8 が自然終端しました。

ただし品質分布は前回から大幅に悪化しました。今回は 0 full / 0 partial / 8 failed です。全ランが final acceptance 前に停止し、browser readiness / interaction probe / persistence / state dimension 評価まで到達していません。

主因は、実行モデル `ornith:35b` が `Write.path` に workspace 内ファイルの絶対パスを渡し、tool の path confinement が `absolute path is not allowed` で拒否したことです。7/8 がこの理由で phase 0 failed になりました。残り1本の `GAME b` は phase scaffold 時の Ollama API request error です。

今回の結果は「成果物品質の天井」ではなく、`qwen planner + ornith executor` 構成における tool args compatibility / recovery の問題として扱うべきです。

## Result Table

| Scenario | Run | Status | Failed phase | Main reason | Planner retry/errors | Max provider turn | Timeouts | Recovery |
|---|---:|---|---|---|---|---:|---|---|
| CLI | a | failed | `project-setup-and-cli-entry` | `absolute path is not allowed` | 0 | 21.7s | 0 | prompt + YAML |
| CLI | b | failed | `project-setup` | `absolute path is not allowed` | 0 | 6.0s | 0 | prompt + YAML |
| TOOL | a | failed | `project-setup-and-config` | `absolute path is not allowed` | `verify_command_policy_error`, `planner_lint_error` | 13.2s | 0 | prompt + YAML |
| TOOL | b | failed | `project-setup-and-dependencies` | `absolute path is not allowed` | quality issue: missing build verify | 8.5s | 0 | prompt + YAML |
| CONTENT | a | failed | `project-setup` | `absolute path is not allowed` | `verify_command_policy_error`, `planner_lint_error` | 6.9s | 0 | prompt + YAML |
| CONTENT | b | failed | `project-setup` | `absolute path is not allowed` | 0 | 11.6s | 0 | prompt + YAML |
| GAME | a | failed | `project-scaffold-and-config` | `absolute path is not allowed` | 0 | 97.0s | 0 | prompt + YAML |
| GAME | b | failed | `project-setup` | Ollama request error at scaffold | `phase_scaffold_error` | n/a | 0 | prompt + YAML |

Timeouts column covers `provider_turn_timeout` and `verify_command_timeout`; both were 0 across the sampled events. No browser/interaction evidence was collected because every run stopped before final acceptance.

## Evidence

### CLI

- `cli_a`
  - Summary: `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-999-Q1-62_002/cli_a/.anvil/runs/019f31f7-c8ce-7d10-9545-b13319eec7f4/summary.md`
  - Events: `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-999-Q1-62_002/cli_a/.anvil/runs/019f31f7-c8ce-7d10-9545-b13319eec7f4/events.jsonl`
  - Recovery YAML: `.anvil/plans/recovery-ultra-plan-phase-project-setup-and-cli-entry-019f31fa-dad9-7553-ac06-cb6efcd245c9.yaml`
  - Created task artifacts: none.

- `cli_b`
  - Summary: `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-999-Q1-62_002/cli_b/.anvil/runs/019f31fc-649a-77f0-ad05-6eaac061ad74/summary.md`
  - Events: `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-999-Q1-62_002/cli_b/.anvil/runs/019f31fc-649a-77f0-ad05-6eaac061ad74/events.jsonl`
  - Recovery YAML: `.anvil/plans/recovery-ultra-plan-phase-project-setup-019f31ff-e6c9-7dd2-bfb2-564965b9fc0e.yaml`
  - Created task artifacts: none.

### TOOL

- `tool_a`
  - Summary: `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-999-Q1-62_002/tool_a/.anvil/runs/019f3200-b45d-7933-bd88-5f7e1dd2ee4f/summary.md`
  - Events: `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-999-Q1-62_002/tool_a/.anvil/runs/019f3200-b45d-7933-bd88-5f7e1dd2ee4f/events.jsonl`
  - Recovery YAML: `.anvil/plans/recovery-ultra-plan-phase-project-setup-and-config-019f3208-3740-76e2-b87a-51a1c3cedb9a.yaml`
  - Created task artifacts: none.

- `tool_b`
  - Summary: `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-999-Q1-62_002/tool_b/.anvil/runs/019f320a-7b47-7822-b600-bbf5bb5add0d/summary.md`
  - Events: `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-999-Q1-62_002/tool_b/.anvil/runs/019f320a-7b47-7822-b600-bbf5bb5add0d/events.jsonl`
  - Recovery YAML: `.anvil/plans/recovery-ultra-plan-phase-project-setup-and-dependencies-019f320e-a987-77b1-8ec3-c06cd7e0c974.yaml`
  - Created task artifacts: `package.json`, `package-lock.json`, `node_modules/`.
  - No `src/app/page.tsx`; no browser evidence.

### CONTENT

- `content_a`
  - Summary: `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-999-Q1-62_002/content_a/.anvil/runs/019f320f-1838-7e10-9a11-11568bd619e2/summary.md`
  - Events: `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-999-Q1-62_002/content_a/.anvil/runs/019f320f-1838-7e10-9a11-11568bd619e2/events.jsonl`
  - Recovery YAML: `.anvil/plans/recovery-ultra-plan-phase-project-setup-019f321c-9315-73d1-81ee-3ae06d2ca450.yaml`
  - Created task artifacts: `package.json`.

- `content_b`
  - Summary: `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-999-Q1-62_002/content_b/.anvil/runs/019f321c-f053-7910-9868-d37793661f76/summary.md`
  - Events: `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-999-Q1-62_002/content_b/.anvil/runs/019f321c-f053-7910-9868-d37793661f76/events.jsonl`
  - Recovery YAML: `.anvil/plans/recovery-ultra-plan-phase-project-setup-019f3221-2068-7d83-908b-051ae7bab18b.yaml`
  - Created task artifacts: `package.json`, `package-lock.json`, `next.config.js`, `postcss.config.js`, `tailwind.config.ts`, `tsconfig.json`, `node_modules/`.
  - No `src/app/page.tsx`; no browser evidence.

### GAME

- `game_a`
  - Summary: `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-999-Q1-62_002/game_a/.anvil/runs/019f3221-7ae8-7ff3-b4b5-7d4ce66829ea/summary.md`
  - Events: `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-999-Q1-62_002/game_a/.anvil/runs/019f3221-7ae8-7ff3-b4b5-7d4ce66829ea/events.jsonl`
  - Recovery YAML: `.anvil/plans/recovery-ultra-plan-phase-project-scaffold-and-config-019f3226-3a96-7332-a9fe-5cb21c490ed8.yaml`
  - Created task artifacts: `package.json`, `package-lock.json`, `postcss.config.js`, `tailwind.config.ts`, `tsconfig.json`, `src/app/global.d.ts`, `src/app/globals.css`, `src/app/layout.tsx`, `node_modules/`.
  - No `src/app/page.tsx`; no browser evidence.

- `game_b`
  - Summary: `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-999-Q1-62_002/game_b/.anvil/runs/019f3226-a1e7-7b81-a8dd-a12f8e43ad4d/summary.md`
  - Events: `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-999-Q1-62_002/game_b/.anvil/runs/019f3226-a1e7-7b81-a8dd-a12f8e43ad4d/events.jsonl`
  - Recovery YAML: `.anvil/plans/recovery-ultra-plan-phase-project-setup-019f322e-1a14-7033-af57-93230edd50d8.yaml`
  - Created task artifacts: none.

## Detailed Findings

### 1. Absolute path tool args dominate the round

Representative event from `cli_a`:

```json
{"event":"tool_call_raw","name":"Write","arguments":{"argument_summaries":{"path":{"preview":"/Users/<user>/share/work/localwork/commandagent_mvp/01/test0704-999-Q1-62_002/cli_a/pyproject.toml"}}}}
{"event":"tool_execute","name":"Write","status":"error","error_kind":"path_confinement_error"}
{"event":"ultra_phase_failed","phase_id":"project-setup-and-cli-entry","reason":"absolute path is not allowed"}
```

Representative event from `tool_b`:

```json
{"event":"tool_call_raw","name":"Write","arguments":{"argument_summaries":{"path":{"preview":"/Users/<user>/share/work/localwork/commandagent_mvp/01/test0704-999-Q1-62_002/tool_b/tsconfig.json"}}}}
{"event":"tool_execute","name":"Write","status":"error","error_kind":"path_confinement_error"}
```

This is not an unsafe outside-workspace write. The absolute path points back into the current workspace, but the runtime correctly rejects absolute paths under the current tool policy. `ornith:35b` appears more likely than the previous Gemini executor to emit absolute file paths in tool calls.

### 2. Recovery handoff worked

All failed task runs saved both:

- `.anvil/repairs/repair-*.md`
- `.anvil/plans/recovery-ultra-plan-*.yaml`

Summaries report `prompt_parse_ok=true`, `yaml_parse_ok=true`, and `command_targets_valid=true`. This part of the recovery mechanism behaved correctly.

### 3. Planner retry / policy pressure is visible with qwen

`qwen3.6:27b-coding-nvfp4` generated valid UltraPlans, but StepPlan generation hit policy/lint pressure in several runs:

- `TOOL a`: `verify_command_policy_error: setup step may not run build/test verification`, then `planner_lint_error: instruction is too long`
- `CONTENT a`: same two-step pattern
- `TOOL b`: quality issue for missing deterministic build verify

This did not cause final failure directly in most runs, but it increased wall-clock time and indicates qwen planner needs either tighter prompt constraints or stronger deterministic normalization.

### 4. Browser / persistence / interaction quality could not be measured

All web scenarios stopped before final acceptance. Therefore:

- `browser_readiness`: not reached
- `browser-interaction.json`: not produced for scenario acceptance
- `persistence_after_reload`: not evaluated
- `state_dimensions_changed`: none / unavailable
- plan adherence residuals: not meaningful, because no run reached final acceptance

### 5. Comparison with previous Q1 final round

Previous report: `workspace/management/runs/uat-test0704-999-Q1-62-001/uat-report.md`

| Round | Model / planner | Full | Partial | Failed | Main observation |
|---|---|---:|---:|---:|---|
| Q1-62-001 | `gemini-3.5-flash` / `gemini-3.5-flash` | 6 | 1 | 1 | Most runs reached browser/final acceptance; residual issues were interaction/persistence quality. |
| Q1-62-002 | `ornith:35b` / `qwen3.6:27b-coding-nvfp4` | 0 | 0 | 8 | Runs fail before acceptance, mostly due recoverable workspace-absolute tool paths rejected by tool policy. |

This is a model/provider compatibility regression for the local-model configuration, not a useful measurement of final artifact quality.

## Created Artifacts Summary

| Scenario | Run | Created task artifacts outside `.anvil` |
|---|---:|---|
| CLI | a | none |
| CLI | b | none |
| TOOL | a | none |
| TOOL | b | `package.json`, `package-lock.json`, `node_modules/` |
| CONTENT | a | `package.json` |
| CONTENT | b | `package.json`, `package-lock.json`, `next.config.js`, `postcss.config.js`, `tailwind.config.ts`, `tsconfig.json`, `node_modules/` |
| GAME | a | `package.json`, `package-lock.json`, `postcss.config.js`, `tailwind.config.ts`, `tsconfig.json`, `src/app/global.d.ts`, `src/app/globals.css`, `src/app/layout.tsx`, `node_modules/` |
| GAME | b | none |

No run produced a complete CLI implementation or a complete Next.js route implementation.

## Judgement

### Mandatory termination gate

Pass narrowly. 8/8 naturally terminated without manual interrupt. However, most failures happened before meaningful implementation acceptance.

### Quality distribution

Fail. 0/8 reached full or partial success. The round cannot support a claim about artifact quality because the primary failure is tool-call path compatibility.

### Most important problem

The local executor model emits absolute paths for `Write.path`. MVP currently rejects them without attempting safe in-workspace normalization. For local-model compatibility, this creates a phase-0 failure mode even when the intended target is clearly inside the workspace.

## Follow-up Prompt

```text
Q1-62-002 with qwen3.6 planner and ornith:35b executor produced 0/8 accepted runs. The dominant failure is tool_call_raw Write.path using a workspace-absolute path, followed by tool_execute path_confinement_error: absolute path is not allowed. Please implement and test a safe recoverable tool-args normalization layer:

1. If a tool path is absolute and canonicalizes under the current workspace root, normalize it to a workspace-relative path before execution and emit a diagnostic event such as tool_args_path_normalized.
2. If a tool path is absolute but outside the workspace, continue rejecting it as path_confinement_error.
3. Keep shell/path safety strict; do not weaken workspace confinement.
4. Add provider/tool-args fixtures for ornith-like absolute path Write calls and for unsafe outside-workspace absolute paths.
5. Re-run Q1 smoke for CLI + one web scenario with qwen planner / ornith executor before attempting the full 8-run round again.

Also inspect qwen planner retry pressure:
- setup step may not run build/test verification
- instruction is too long
- missing deterministic build verify

Do not relax release gates or count recovery handoff as success.
```
