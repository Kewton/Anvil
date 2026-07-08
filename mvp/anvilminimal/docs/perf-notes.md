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
