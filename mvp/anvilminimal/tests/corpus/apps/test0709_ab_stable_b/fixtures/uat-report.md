# UAT Report: prompt_layout stable vs legacy A/B

Date: 2026-07-09

## Scope

Task type: UAT execution only. No code changes and no commits were made to `mvp/anvilminimal` sources.

Fixed variables:
- Scenario: GAME prompt from `mvp/anvilminimal/docs/uat/scenarios.md`
- Prompt: `/ultra-plan-run --profile nextjs あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。`
- Model/provider:
  - `--model qwen3.6:27b-coding-nvfp4 --provider ollama`
  - `--planner-model qwen3.6:27b-coding-nvfp4 --planner-provider ollama`
- Context: `--yes --context-budget 65536`
- One variable only: `--prompt-layout stable|legacy`

Precondition evidence:
- `git pull`: already up to date.
- `git status --short`: clean before runs.
- `cargo build --release`: passed.
- `anvilminimal --version`: `anvilminimal 0.1.0 4f37fe90 2026-07-08T15:55:03Z`
- `anvilminimal --help | grep prompt-layout`: `--prompt-layout <stable|legacy>`
- `ollama list`: `qwen3.6:27b-coding-nvfp4` present.
- Playwright probe present: `~/.anvil/tools/interaction-probe/node_modules/playwright/package.json`
- Port 3011 was checked/cleaned between runs.

Workspaces preserved:
- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0709_ab_stable_a`
- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0709_ab_stable_b`
- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0709_ab_legacy_a`
- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0709_ab_legacy_b`

## Matrix

| Run | Layout | Terminal / stop reason | No-tool stagnation count | Rescue fired | Phases | Time profile | Executor prompt_eval first 4 |
| --- | --- | --- | ---: | --- | --- | --- | --- |
| stableA | stable | `Status: completed`; `Stop reason: completed` | 2 | `artifact_stagnation_feedback`: yes; `setup_scaffold_completed`: no | 4/4 | `provider 99% [planner 25% / executor 40% / repair 33%] · installs 0% · builds 0% · probe 1% · total 40m12s · tokens prompt_eval=893185 eval=68855` | `1845,3049,3761,4289` |
| stableB | stable | `Status: failed`; `Stop reason: ultra final acceptance failed after bounded repair: capability_evidence_unresolved:restart_or_recoverable_state_evidence; browser_interaction_failed:input_state_change_missing_after_start; incomplete; Recovery artifact check: prompt_parse_ok=true, yaml_parse_ok=true, command_targets_valid=true` | 3 | `artifact_stagnation_feedback`: yes; `setup_scaffold_completed`: no | 4/5; failed `build-verification` | `provider 100% [planner 25% / executor 37% / repair 38%] · installs 0% · builds 0% · probe 0% · total 60m59s · tokens prompt_eval=1023823 eval=103560` | `1954,2476,3067,3857` |
| legacyA | legacy | `Status: failed`; `Stop reason: ultra final acceptance failed after bounded repair: capability_evidence_unresolved:restart_or_recoverable_state_evidence; browser_interaction_failed:input_state_change_missing_after_start; incomplete; Recovery artifact check: prompt_parse_ok=true, yaml_parse_ok=true, command_targets_valid=true` | 0 | no | 3/4; failed `styling-and-build-verification` | `provider 99% [planner 16% / executor 47% / repair 36%] · installs 0% · builds 0% · probe 0% · total 48m21s · tokens prompt_eval=923318 eval=81439` | `1769,2469,2702,2752` |
| legacyB | legacy | `Status: failed`; `Stop reason: ultra final acceptance failed after bounded repair: capability_evidence_unresolved:restart_or_recoverable_state_evidence; browser_interaction_failed:input_state_change_missing_after_start; incomplete; Recovery artifact check: prompt_parse_ok=true, yaml_parse_ok=true, command_targets_valid=true` | 0 | no | 3/4; failed `build-verification-and-final-check` | `provider 99% [planner 35% / executor 49% / repair 16%] · installs 0% · builds 0% · probe 0% · total 31m56s · tokens prompt_eval=610223 eval=53633` | `1774,2589,2674,2723` |

Notes:
- `prompt_layout` telemetry matched the flag in every run.
- Failure runs all preserved both recovery artifacts:
  - stableB: `repair-*.md` 1, `recovery-ultra-plan-*.yaml` 1
  - legacyA: `repair-*.md` 1, `recovery-ultra-plan-*.yaml` 1
  - legacyB: `repair-*.md` 1, `recovery-ultra-plan-*.yaml` 1
- stableA completed without recovery handoff artifacts.
- `prompt_eval_count` did not show the expected stable signature of turn 1 large and turns 2+ sharply smaller. In this run set, both layouts show comparable first-turn values and increasing early prompt_eval counts. The setup stagnation signal is therefore the stronger discriminant.

## Decision Rule Applied

From `mvp/anvilminimal/docs/perf-notes.md`:

> Pre-committed discrimination rule: if setup-phase `no_tool_missing_artifacts` stagnation occurs in one layout and not the other, prompt layout is the cause. If both layouts stagnate, treat it as model behavior and rely on deterministic scaffold rescue. If neither stagnates, keep the speed decision based on the prefill and wall-clock telemetry.

Observed:
- stableA: setup-phase `no_tool_missing_artifacts` stagnation occurred 2 times.
- stableB: setup-phase `no_tool_missing_artifacts` stagnation occurred 3 times.
- legacyA: 0 times.
- legacyB: 0 times.

Conclusion:

`prompt_layout=stable` caused a setup-phase no-tool stagnation regression in this A/B set. Layout regression is **CONFIRMED** by the pre-committed rule.

Recommendation:
- Keep the tail-restatement fix and rerun the same A/B matrix, or
- Revert the default to `legacy` if the fix is not yet in the tested build or does not eliminate the stable-only stagnation.

Accuracy should take priority over speed. stableA reached full success only after two setup stagnation rescues; stableB had three setup stagnation rescues and later failed final acceptance. The legacy runs still failed final acceptance, but they did not reproduce the setup-phase no-tool missing-artifacts failure mode.

## Appendix A: Summary Grep Output

```text
=== test0709_ab_stable_a ===
layout_events=  58 stable;
no_tool_count=2
setup_scaffold_completed=0
rescue_or_stagnation=8
prompt_eval_first4=1845,3049,3761,4289
recovery_md=0
recovery_yaml=0
Status: completed
Command status: completed
Stop reason: completed
Final acceptance: full_success
Release gate: pass
Time profile: provider 99% [planner 25% / executor 40% / repair 33%] · installs 0% · builds 0% · probe 1% · total 40m12s · tokens prompt_eval=893185 eval=68855
completed: - initialize-project-and-dependencies (completed)
completed: - implement-core-game-engine (completed)
completed: - integrate-ui-and-observability-hooks (completed)
completed: - verify-build-contract (completed)
failed: - none

=== test0709_ab_stable_b ===
layout_events=  63 stable;
no_tool_count=3
setup_scaffold_completed=0
rescue_or_stagnation=6
prompt_eval_first4=1954,2476,3067,3857
recovery_md=1
recovery_yaml=1
Status: failed
Command status: failed
Stop reason: ultra final acceptance failed after bounded repair: capability_evidence_unresolved:restart_or_recoverable_state_evidence; browser_interaction_failed:input_state_change_missing_after_start; incomplete; Recovery artifact check: prompt_parse_ok=true, yaml_parse_ok=true, command_targets_valid=true
Final acceptance: incomplete
Release gate: failed
Time profile: provider 100% [planner 25% / executor 37% / repair 38%] · installs 0% · builds 0% · probe 0% · total 60m59s · tokens prompt_eval=1023823 eval=103560
completed: - project-setup-and-config (completed)
completed: - core-game-loop-and-rendering (completed)
completed: - game-mechanics-and-collision (completed)
completed: - ui-overlays-and-observability-hooks (completed)
failed: - build-verification (failed)

=== test0709_ab_legacy_a ===
layout_events=  54 legacy;
no_tool_count=0
setup_scaffold_completed=0
rescue_or_stagnation=0
prompt_eval_first4=1769,2469,2702,2752
recovery_md=1
recovery_yaml=1
Status: failed
Command status: failed
Stop reason: ultra final acceptance failed after bounded repair: capability_evidence_unresolved:restart_or_recoverable_state_evidence; browser_interaction_failed:input_state_change_missing_after_start; incomplete; Recovery artifact check: prompt_parse_ok=true, yaml_parse_ok=true, command_targets_valid=true
Final acceptance: incomplete
Release gate: failed
Time profile: provider 99% [planner 16% / executor 47% / repair 36%] · installs 0% · builds 0% · probe 0% · total 48m21s · tokens prompt_eval=923318 eval=81439
completed: - project-setup-and-config (completed)
completed: - core-game-engine (completed)
completed: - ui-controls-and-observability (completed)
failed: - styling-and-build-verification (failed)

=== test0709_ab_legacy_b ===
layout_events=  51 legacy;
no_tool_count=0
setup_scaffold_completed=0
rescue_or_stagnation=0
prompt_eval_first4=1774,2589,2674,2723
recovery_md=1
recovery_yaml=1
Status: failed
Command status: failed
Stop reason: ultra final acceptance failed after bounded repair: capability_evidence_unresolved:restart_or_recoverable_state_evidence; browser_interaction_failed:input_state_change_missing_after_start; incomplete; Recovery artifact check: prompt_parse_ok=true, yaml_parse_ok=true, command_targets_valid=true
Final acceptance: incomplete
Release gate: failed
Time profile: provider 99% [planner 35% / executor 49% / repair 16%] · installs 0% · builds 0% · probe 0% · total 31m56s · tokens prompt_eval=610223 eval=53633
completed: - project-setup-and-dependencies (completed)
completed: - core-game-engine-and-rendering (completed)
completed: - ui-integration-and-observability (completed)
failed: - build-verification-and-final-check (failed)
```

## Appendix B: Raw No-Tool Stagnation Events

Command shape:

```sh
jq -r 'select(.event=="artifact_stagnation_feedback" and .last_model_action=="no_tool_missing_artifacts") | @json' <events.jsonl>
```

stableA:

```json
{"attempt":1,"attempt_limit":3,"event":"artifact_stagnation_feedback","last_model_action":"no_tool_missing_artifacts","missing_paths":["src/app/page.tsx"],"non_edit_streak":3,"schema_version":"1","target_attempt":1,"target_path":"src/app/page.tsx"}
{"attempt":2,"attempt_limit":3,"event":"artifact_stagnation_feedback","last_model_action":"no_tool_missing_artifacts","missing_paths":["src/app/page.tsx"],"non_edit_streak":3,"schema_version":"1","target_attempt":2,"target_path":"src/app/page.tsx"}
```

stableB:

```json
{"attempt":1,"attempt_limit":3,"event":"artifact_stagnation_feedback","last_model_action":"no_tool_missing_artifacts","missing_paths":["src/app/page.tsx"],"non_edit_streak":3,"schema_version":"1","target_attempt":1,"target_path":"src/app/page.tsx"}
{"attempt":2,"attempt_limit":3,"event":"artifact_stagnation_feedback","last_model_action":"no_tool_missing_artifacts","missing_paths":["src/app/page.tsx"],"non_edit_streak":3,"schema_version":"1","target_attempt":2,"target_path":"src/app/page.tsx"}
{"attempt":3,"attempt_limit":3,"event":"artifact_stagnation_feedback","last_model_action":"no_tool_missing_artifacts","missing_paths":["src/app/page.tsx"],"non_edit_streak":3,"schema_version":"1","target_attempt":3,"target_path":"src/app/page.tsx"}
```

legacyA:

```text
(no output)
```

legacyB:

```text
(no output)
```

## Appendix C: Setup Scaffold / 105B Rescue Grep

Command shape:

```sh
grep -h 'setup_scaffold_completed\|105B\|missing_entrypoint\|artifact_stagnation_feedback' <events.jsonl>
```

Result:
- `setup_scaffold_completed`: no output in all four runs.
- literal `105B`: no output in all four runs.
- `artifact_stagnation_feedback`: stableA and stableB only, shown in Appendix B.
- `missing_entrypoint`: stableA and stableB only, associated with the setup-phase `src/app/page.tsx` missing entrypoint repair target.

Representative stableA lines around the rescue path:

```json
{"event":"verify_repair_turn","failure_signature":"missing=smoke-check.js|src/app/global.d.ts|src/app/globals.css|src/app/layout.tsx|src/app/page.tsx;dependency=;commands=;verifier_commands=;profile=missing_required_evidence:nextjs_route_evidence;compile=","has_edit":false,"inspect_only":true,"no_edit_turns":1,"repair_target":"missing_entrypoint","schema_version":"1"}
{"changed_paths_after":["package.json","tsconfig.json","postcss.config.js","tailwind.config.ts"],"changed_paths_before":["package.json","tsconfig.json","postcss.config.js","tailwind.config.ts"],"contract_enforcement":"observe","event":"loop_stop","failure_signature":"missing=smoke-check.js|src/app/global.d.ts|src/app/globals.css|src/app/layout.tsx|src/app/page.tsx;dependency=;commands=;verifier_commands=;profile=missing_required_evidence:nextjs_route_evidence;compile=","missing_capabilities":[],"missing_evidence":["nextjs_route_evidence"],"missing_obligations":[],"no_edit_turns":1,"phase_scope":"initialize-project-and-dependencies","reason":"verify_repair_no_change_observed","recovery_prompt_path":"","recovery_ultra_plan_path":"","recovery_yaml_missing":true,"repair_follow_through":"no_change","repair_target":"missing_entrypoint","repair_target_followed":false,"repair_turn_changed_paths":[],"schema_version":"1","session_scope":"plan-run-step","step_kind":"implement","target_relation":"no_change"}
{"attempt":1,"attempt_limit":3,"event":"artifact_stagnation_feedback","last_model_action":"no_tool_missing_artifacts","missing_paths":["src/app/page.tsx"],"non_edit_streak":3,"schema_version":"1","target_attempt":1,"target_path":"src/app/page.tsx"}
{"attempt":2,"attempt_limit":3,"event":"artifact_stagnation_feedback","last_model_action":"no_tool_missing_artifacts","missing_paths":["src/app/page.tsx"],"non_edit_streak":3,"schema_version":"1","target_attempt":2,"target_path":"src/app/page.tsx"}
```

Representative stableB lines around the rescue path:

```json
{"event":"verify_repair_turn","failure_signature":"missing=src/app/page.tsx;dependency=;commands=;verifier_commands=;profile=missing_required_evidence:nextjs_route_evidence;compile=","has_edit":false,"inspect_only":true,"no_edit_turns":1,"repair_target":"missing_entrypoint","schema_version":"1"}
{"changed_paths_after":["src/app/globals.css","src/app/global.d.ts","src/app/layout.tsx"],"changed_paths_before":["src/app/globals.css","src/app/global.d.ts","src/app/layout.tsx"],"contract_enforcement":"observe","event":"loop_stop","failure_signature":"missing=src/app/page.tsx;dependency=;commands=;verifier_commands=;profile=missing_required_evidence:nextjs_route_evidence;compile=","missing_capabilities":[],"missing_evidence":["nextjs_route_evidence"],"missing_obligations":[],"no_edit_turns":1,"phase_scope":"project-setup-and-config","reason":"verify_repair_no_change_observed","recovery_prompt_path":"","recovery_ultra_plan_path":"","recovery_yaml_missing":true,"repair_follow_through":"no_change","repair_target":"missing_entrypoint","repair_target_followed":false,"repair_turn_changed_paths":[],"schema_version":"1","session_scope":"plan-run-step","step_kind":"implement","target_relation":"no_change"}
{"attempt":1,"attempt_limit":3,"event":"artifact_stagnation_feedback","last_model_action":"no_tool_missing_artifacts","missing_paths":["src/app/page.tsx"],"non_edit_streak":3,"schema_version":"1","target_attempt":1,"target_path":"src/app/page.tsx"}
{"attempt":2,"attempt_limit":3,"event":"artifact_stagnation_feedback","last_model_action":"no_tool_missing_artifacts","missing_paths":["src/app/page.tsx"],"non_edit_streak":3,"schema_version":"1","target_attempt":2,"target_path":"src/app/page.tsx"}
{"attempt":3,"attempt_limit":3,"event":"artifact_stagnation_feedback","last_model_action":"no_tool_missing_artifacts","missing_paths":["src/app/page.tsx"],"non_edit_streak":3,"schema_version":"1","target_attempt":3,"target_path":"src/app/page.tsx"}
```

## Appendix D: Executor Provider Turn Prompt Eval

Command shape:

```sh
jq -r 'select(.event=="provider_turn_duration" and .caller_scope=="executor") | .prompt_eval_count' <events.jsonl> | head -4
```

stableA:

```text
1845
3049
3761
4289
```

stableB:

```text
1954
2476
3067
3857
```

legacyA:

```text
1769
2469
2702
2752
```

legacyB:

```text
1774
2589
2674
2723
```

## Appendix E: Prompt Layout Telemetry

Command shape:

```sh
jq -r 'select(.prompt_layout!=null) | .prompt_layout' <events.jsonl> | sort | uniq -c
```

```text
stableA:   58 stable
stableB:   63 stable
legacyA:   54 legacy
legacyB:   51 legacy
```

