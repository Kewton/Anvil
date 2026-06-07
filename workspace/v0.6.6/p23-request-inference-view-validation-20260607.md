# P23 RequestInferenceView Validation

Date: 2026-06-07

## Objective

Make the P22 controller-state / natural-language split harder to regress.

P22 fixed the behavioral bug, but the code still depended on local convention:

- parse `ControllerStatePacket` from the raw request
- separately call `model_visible_request_text`
- remember to pass the visible text, not the raw request, into inference and
  WorkMode classification

That is easy to break when adding non-coding task kinds or new evidence runners.

## Change

Introduced `RequestInferenceView` in `task_contract.rs`.

It gives one typed request view:

- `controller_state`: structural controller state, used by `TaskContract`
- `visible_text`: sanitized natural-language text, used by inference and model
  prompt surfaces
- `controller_packet_at_start`: whether the turn is controller-owned and should
  skip WorkMode natural-language classification

`run_turn.rs` now uses `RequestInferenceView` for WorkMode classification. A
leading `STATE_CONTROL_PACKET` sets WorkMode to `Auto`; embedded packets still
allow classification, but only with the packet-stripped visible text.

The implementation parses a controller packet once per view and derives both
controller state and visible text from that parse. This keeps the code aligned
with the intended architecture: controller state is structural authority, while
natural-language heuristics see only user-visible language.

## Unit Validation

```text
cargo test --offline --lib request_inference_view
2 passed

cargo test --offline --lib controller_state_packet_
8 passed

cargo test --offline --lib model_visible_request_text
1 passed

cargo test --offline --lib
3813 passed
```

The tests pin:

- embedded packets keep controller state but do not contaminate visible text
- leading packets are marked as controller-owned turns
- existing controller packet contracts still produce required artifacts,
  schema obligations, and evidence commands

## Local LLM Validation

Model:

```text
qwen3.6:27b-coding-mxfp8
```

All runs used the final P23 source through:

```text
cargo run --offline -- --oneshot --fresh-session --no-footer --trace -y ...
```

### Coding: leading packet, manifest/source/evidence command

Workdir:

```text
/private/tmp/anvil-p23-view2-coding-work
```

State:

```text
/private/tmp/anvil-p23-view2-coding-state
```

Outcome:

```text
final_outcome: done
classified_task_kind: coding
completion_reason: verifier_evidence_satisfied
verify_commands: cargo test --manifest-path Cargo.toml
```

Created files:

```text
Cargo.toml
Cargo.lock
src/lib.rs
```

Verifier evidence:

```text
5 passed
```

Observation: the model attempted the verifier before `Cargo.toml` existed once.
The controller recovered by returning to `MissingDeliverableJob` for
`Cargo.toml`, then ran the required evidence command successfully. This is not a
regression from the request-view refactor, but it is a useful follow-up signal:
rejected or premature verifier attempts should not delay missing-deliverable
selection.

### Docs: leading packet, required headings

Workdir:

```text
/private/tmp/anvil-p23-view2-docs-work
```

State:

```text
/private/tmp/anvil-p23-view2-docs-state
```

Outcome:

```text
final_outcome: done
classified_task_kind: docs
completion_reason: artifact_obligations_satisfied
```

Created file:

```text
README.md
```

The generated README included required headings:

```text
## Setup
## Usage
```

### Data: embedded packet, schema obligation

Workdir:

```text
/private/tmp/anvil-p23-view2-data-work
```

State:

```text
/private/tmp/anvil-p23-view2-data-state
```

Outcome:

```text
final_outcome: done
classified_task_kind: data
completion_reason: artifact_obligations_satisfied
```

Created file:

```text
summary.json
```

The WorkMode classification input was:

```text
Create summary.json only. Write valid JSON with exactly those two top-level fields and no code files.
```

It did not include `STATE_CONTROL_PACKET` or `required_artifacts`.

The Objective Contract still carried the schema:

```text
Objective kind: data
path=summary.json role=data_output kind=structured_record
top-level fields: topic|status
```

## Architecture Insight

This step does not add a provider abstraction or a new task-kind-specific repair
loop. It narrows the request boundary:

```text
raw request -> RequestInferenceView -> ObjectiveContract + visible language
```

That supports coding and non-coding tasks with the same lifecycle:

```text
typed contract -> missing deliverable -> evidence runner/artifact check -> terminal outcome
```

The next likely follow-up is to remove the remaining timing gap where a model can
try an evidence command before all deliverables are present and only then gets
routed back to `MissingDeliverableJob`.
