# Performance Notes

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

- provider prefill share: `prompt_eval_count` versus `eval_count` when the
  provider reports both fields;
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
- use late-turn `prompt_eval_count` deltas as the cache-effect signal, since
  the history-heavy turns show the 106C prefix-stability effect more clearly
  than the first turn.

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
run, especially whether `prompt_eval_count` drops after the first turn in the
same session.

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
- measured cache benefit zero: prompt_eval stayed near the estimated prompt
  size instead of dropping after turn 1;
- default `prompt_layout` now resolves to `legacy`, with `stable` retained
  behind the flag for A/B runs and replay.

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
