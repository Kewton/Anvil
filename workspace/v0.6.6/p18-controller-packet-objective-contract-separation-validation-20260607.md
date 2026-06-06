# P18 Controller Packet / Objective Contract Separation Validation

Date: 2026-06-07

## Purpose

Validate the P0 architecture requirement that controller-owned state must not be
shown to the model as raw user prompt text, while the model still receives the
objective obligations it needs to complete generic/non-coding tasks.

The target architecture is:

- Raw user/session storage may keep `STATE_CONTROL_PACKET` for controller
  reconstruction and audit.
- Model-visible user text, WorkMode classification input, and Working Memory
  active task must hide raw `STATE_CONTROL_PACKET`.
- A sanitized `[Objective Contract]` system message must project the parsed
  `TaskContract` into the model prompt, including required deliverables,
  schema fields, sections, and evidence hints.

## Implementation

Changed files:

- `src/agent/loop_run/task_contract.rs`
  - Added `model_visible_request_text(...)` stripping for embedded or truncated
    `STATE_CONTROL_PACKET`.
  - Added `objective_contract_prompt_message(...)`, derived from
    `TaskContract`, not from raw prompt text.
- `src/agent/loop_run/build_request_messages.rs`
  - Injects the sanitized Objective Contract system message.
  - Sanitizes user history before sending it to the model.
- `src/agent/loop_run/run_turn.rs`
  - Uses model-visible request text for WorkMode classification.
- `src/agent/loop_run/working_memory_messages.rs`
  - Sanitizes Working Memory and repo-context task text sent to the model.

## Key Insight

The first separation attempt hid the raw controller packet but also removed
schema knowledge from the model. In the failed run, the LLM created
`summary.json` but added extra top-level fields because the raw packet was no
longer visible and no structured replacement had been injected.

The fix is not to expose controller state again. The fix is to project the
controller-owned `TaskContract` into a sanitized, generic Objective Contract:

```text
[Objective Contract]
Objective kind: data; deliverable spec: output_file; evidence spec: schema_check.
Required deliverables:
- path=summary.json role=data_output kind=structured_record; write a JSON object
  with exactly these top-level fields and no extra top-level fields: topic|status
```

This keeps controller state out of user prompt text while preserving the
obligations needed for docs/data/research/ops tasks.

## Unit Validation

Commands:

```bash
cargo test --offline --lib objective_contract_prompt_message
cargo test --offline --lib model_visible_request_text
cargo test --offline --lib model_visible_session_messages_
cargo test --offline --lib
```

Result:

- Targeted tests passed.
- Full lib suite passed: 3797 tests.

## Actual LLM Validation

Command:

```bash
cargo run --manifest-path /Users/maenokota/share/work/github_kewton/Anvil-develop/Cargo.toml --offline -- --oneshot --fresh-session --no-footer --trace -y --max-iterations 5 --chat-timeout-secs 300 --state-dir /private/tmp/anvil-p18-control-packet-state-json3 -m qwen3.6:27b-coding-mxfp8 -p 'P18 control packet separation validation. Create summary.json only. STATE_CONTROL_PACKET {"objective":"Create summary.json with fields topic and status","next_required_action":"artifact","required_artifacts":[{"path":"summary.json","role":"data","schema":{"json_fields":["topic","status"]}}]}. Write valid JSON with exactly those required fields. Do not create code scaffolds or any other files.'
```

Result:

- Completed in 1 iteration.
- Tool call used repository-relative `summary.json`.
- Generated JSON contained exactly the required fields:

```json
{
  "topic": "P18 control packet separation validation",
  "status": "completed"
}
```

Trace observations:

- `agent.work_mode.classified` input did not contain `STATE_CONTROL_PACKET`.
- WorkMode confirm prompt did not contain `STATE_CONTROL_PACKET`.
- Main `ollama.generate.request` user message did not contain
  `STATE_CONTROL_PACKET`.
- Main `ollama.generate.request` did contain `[Objective Contract]`.
- `[Working Memory] Active task` was sanitized to:
  `P18 control packet separation validation. Create summary.json only.`
- Raw `session.json` and eval task text still preserved the original user text,
  including the controller packet, for controller reconstruction/audit.

Photon sidecar was unavailable on `127.0.0.1:18765`; this was fail-open and did
not affect Ollama validation.

## Architectural Assessment

This moves Anvil closer to the target architecture:

- Controller-owned state is not a prompt convention the model must interpret.
- The model receives task obligations through one typed projection surface.
- WorkMode remains a compatibility label, not the source of tool policy or task
  semantics.
- Non-coding tasks can carry schema/section/evidence requirements without adding
  provider abstractions or task-specific prompt branches.

Remaining risk:

- The terminal success gate still accepted the successful run based on artifact
  obligations and repo edit. Future work should make terminal success consume
  the same Objective Contract schema check more directly, so success judgement
  and model prompt contract share the same source of truth.
