# Performance Notes

## Generation Volume Breakdown

The first duration-calibrated GAME run was generation-dominated: `45m28s`
total, with `4m38s` spent in prefill and `40m04s` in generation. The wall clock
signal is now the useful lever; the older count-only cache verdicts stay
invalidated.

For the current corpus fixture `tests/corpus/apps/test0708_012/fixtures/events-final-acceptance-pending.jsonl`,
the implementation rewrite happened at the `implement-visual-polish-tsx` turn
(`src/app/page.tsx` write). That turn replaced the page implementation, but the
final artifact still retained the `data-anvil-*` hooks, so the failure read as a
behavioral miss rather than a literal hook-drop regression. The cheapest
surviving measure is guidance: keep extending the instrumented skeleton and
preserve `data-anvil-*` attributes instead of treating the scaffold as disposable
output.

The summary renderer now surfaces a `Generation profile` block alongside the
existing time profile. It groups provider turns by caller scope
(`planner`/`executor`/`repair`/`acceptance-repair`) and by turn type
(`full-file Write`, `Edit`, `prose-only`, `tool-call`) so the volume shape stays
visible even when the run itself is duration-only.

## 15-Minute Verdict

The measured successful-run median is `66,590` eval tokens. At roughly `28
tok/s`, the implied generation floor is about `40m`, so a `15m` wall-clock
target is physically unreachable on qwen27b at the current artifact scale.

Decision options:

- run the same campaign on a faster model family, such as the a3b class;
- move the workload onto hardware or a quantization setup that raises
  throughput materially;
- split the target into two tiers: qwen27b accepts the `40m` floor, and the
  fast tier keeps the `15m` goal.

The current report does not expose a bottom-up token verification `V1` result.
If that value is missing from the campaign report, reconstruct it in the next
analysis pass and add it to the campaign checklist instead of backfilling it
silently.

## Speed Track 1 Prefix Audit

Audit source: reconstructed prompts from the corpus event fixtures
`tests/corpus/apps/test0708_012/fixtures/events-final-acceptance-pending.jsonl`,
`tests/corpus/apps/test0708_009/fixtures/events-zero-edit-regeneration.jsonl`, and
`tests/corpus/apps/test0708_005/fixtures/events-early-death-profile.jsonl`, using the
current prompt builders. The fixtures record raw planner output previews and step
contract/progress events, but not full prompt bodies (`prompt_body_saved=false`), so
the audit reconstructs the prompt inputs from fixture phase/step/report fields and
the checked-in builders.

| Prompt family | Consecutive prompts audited | Common prefix before fix | First divergence | Cause |
| --- | --- | ---: | --- | --- |
| UltraPlan generation | initial generation -> schema/lint retry | about one stable system message; user prompt diverges immediately | first user-message line (`Create an UltraPlan...` vs retry feedback) | retry attempt counter and failure feedback are front-loaded in the user prompt |
| StepPlan generation | phase `setup-and-styling` -> phase `game-engine-core` | 34 bytes (`Create a step plan for this task:\n`) | the phase goal text | dynamic phase task appears before stable profile expectations and hard constraints |
| Step execution | adjacent StepPlan steps in the same phase | stable executor header plus the shared overall goal; divergence at `Current step id` | step id/instruction block | per-step fields precede stable final artifact/capability/evidence sections and execution rules |
| Repair turns | anchored repair -> compact repair on the same compile failure | 0 to one line depending on rung | first line (`Repair step...` vs `Repair session mode: compact`) | rung/mode and failure feedback are front-loaded before shared repair rules and compile context |
| Final-acceptance repair | repair attempt 1 -> attempt 2 for the same final gate failure | stable final-repair title plus goal/profile context; divergence at `attempt: N/M` | repair budget line | per-attempt counter is in the early failure block before stable obligations, remedies, and bounded rules |

Fix direction: keep every section and its wording, but move stable policy,
profile/goal/plan context, and invariant guidance before variable counters,
fresh feedback, carry-forward state, and retry/rung labels. This is an ordering
change only; verification semantics and prompt meaning remain unchanged.

## Baseline Measurement Protocol

Speed-track comparisons use a before/after pair, not an assertion. Run one GAME
scenario on the qwen27b single-model local configuration before the prefix
stability change, or use the nearest corpus timing run as the baseline when a
fresh pre-change run is unavailable. Then run the same GAME scenario after the
change with the same host, model, profile, context budget, and Ollama residency
settings.

The committed comparison surface is the `Time profile:` line in `summary.md`.
Record:

- provider duration split: `prompt_eval_duration`, `eval_duration`, and
  `load_duration` when the provider reports them;
- prompt-eval counts remain for observability, but they do not prove cache
  reuse on Ollama 0.31.1 because `prompt_eval_count` reports the total prompt
  equivalent even on cache hits;
- total wall clock from the same profile line;
- whether the run used single-model planner/executor and Ollama keep-alive.

Treat movement as observational unless the run identity, hardware, model
residency, and scenario prompt are held constant.

Measurement-discipline correction:

- compare completed runs only;
- keep the compared scope equal across layouts, profiles, and scenario
  prompts;
- use the A/B stableA completed run as the baseline for this track
  (`4/4` phases, `40m12s`, `prompt_eval=893185`);
- treat the post-fix completed run as the first valid comparison point;
- use late-turn `prefill_seconds` and prompt growth together as the cache
  effect signal, since `prompt_eval_count` is count-equivalent noise on Ollama
  0.31.1 rather than a reuse indicator.

## Prompt Layout A/B Protocol

The `prompt_layout` setting is an instrumentation toggle for discriminating
layout-sensitive model behavior. It never changes verification semantics.

- `stable`: the 104B prefix-stable order, with stable policy/profile sections
  first and per-turn state at the tail. Tail blocks restate the current
  objective so the dynamic end of the prompt is self-sufficient.
- `legacy`: the pre-104B section order, preserved for A/B measurement.

For the qwen27b GAME regression test0708_018, run two fresh local GAME attempts
per layout:

1. `prompt_layout = "stable"`
2. `prompt_layout = "legacy"`

Use the same model, host, profile, context budget, keep-alive setting, and
scenario prompt. Record the `Time profile:` line and provider telemetry for each
run, especially whether `prefill_seconds` stays small after the first turn even
as prompt growth continues in the same session.

Pre-committed discrimination rule: if setup-phase `no_tool_missing_artifacts`
stagnation occurs in one layout and not the other, prompt layout is the cause.
If both layouts stagnate, treat it as model behavior and rely on deterministic
scaffold rescue. If neither stagnates, keep the speed decision based on the
prefill and wall-clock telemetry.

Decision outcomes:

- keep stable when it preserves behavior and improves prefill/wall clock;
- keep stable plus the tail-objective fix when only the tail was insufficient;
- revert the default to legacy if stable alone reproducibly causes setup-phase
  no-tool stagnation that legacy avoids.

Verdict for test0708_018:

- stable-layout behavioral regression confirmed;
- measured cache benefit verdict invalidated by calibration: the count-based
  proxy was not a valid cache signal on Ollama 0.31.1;
- default `prompt_layout` now resolves to `legacy`, with `stable` retained
  behind the flag for A/B runs and replay.

Calibration note for the current Ollama 0.31.1 chat path:

- KV cache is effective in generate/chat/tools-chat, but `prompt_eval_count`
  still reports the total prompt equivalent on cache hits. Cache judgments that
  relied on count deltas are therefore `INVALIDATED-METRIC`.
- The current cache signal is duration-based: small `prefill_seconds` while
  prompt size grows.
- The stable-layout behavioral regression verdict remains unchanged because it
  is based on behavior evidence, not token counts.
- Current speed verdict: cache closed; generation-dominated; the 106C/D
  wall-clock effect is approximately zero when compared at equal scope.

## History-Size Audit

The corpus audit shows prompt growth is dominated by tool-result echoes rather
than model prose. The bound-history change now clips tool payloads at the
session boundary and on outbound request assembly, so the prompt no longer
replays full file/command bodies.

| history component | observed growth driver | mitigation |
| --- | --- | --- |
| tool-result echoes | file contents, build output, browser transcripts | bound at source and on request assembly |
| model text | plan/repair narration and step explanations | stable prefix hoisting; tail-only variability |
| fresh feedback | missing-path lists, repair hints, and progress nudges | keep the tail concise and objective-first |
