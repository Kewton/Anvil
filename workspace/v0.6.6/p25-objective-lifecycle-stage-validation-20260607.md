# P25 Objective Lifecycle Stage Validation

Date: 2026-06-07

## Objective

Move the recovery planner one step closer to the intended generic
architecture:

```text
ObjectiveContract
-> MissingDeliverableJob until required deliverables are complete
-> MissingEvidenceJob / EvidenceRunner only after deliverables are complete
-> terminal outcome
```

The concrete change is intentionally small. It does not rename `TaskContract`
or introduce a provider abstraction. It separates the deliverable-stage
decision inside `plan_artifact_recovery` into an explicit
`ObjectiveLifecycleStage`.

## Root Cause Addressed

The planner had already become behaviorally correct for the P24 case, but the
structure still mixed:

- required deliverable scanning
- evidence/verifier readiness
- repair action selection

inside one function body.

That makes future general-purpose expansion fragile because docs/data/research
changes can accidentally alter evidence timing while touching deliverable
checks.

## Change

Added:

```text
ObjectiveLifecycleStage
- MissingDeliverable { missing, target_hint }
- DeliverablesSatisfied
```

and extracted:

```text
objective_deliverable_stage(inputs, observed)
```

`plan_artifact_recovery` now first asks the objective lifecycle stage whether a
deliverable is still missing. Evidence/verifier logic is reached only after the
stage reports `DeliverablesSatisfied`.

This is generic. It uses existing `ArtifactRole` obligations, so coding,
documentation, and data tasks share the same stage gate.

## Unit Validation

```text
cargo test --offline --lib objective_lifecycle_stage
3 passed

cargo test --offline --lib controller_state_packet_missing_deliverable_precedes_missing_evidence
1 passed

cargo test --offline --lib controller_state_packet_evidence_command_runs_after_deliverables_exist
1 passed

cargo test --offline --lib non_coding_docs_deliverable_gap_projects_to_generic_missing_deliverable_job
1 passed

cargo test --offline --lib
3817 passed; 0 failed
```

The first full `cargo test --offline --lib` attempt inside the sandbox failed
because mockito could not start a local server (`Operation not permitted`). The
same command passed when rerun with escalation.

The tests pin:

- controller packet deliverables block evidence until all required paths exist
- evidence is selected only after controller deliverables are satisfied
- docs/data use the same `ObjectiveLifecycleStage::MissingDeliverable` shape

## Local LLM Validation

Model:

```text
qwen3.6:27b-coding-mxfp8
```

All runs used:

```text
cargo run --offline -- --oneshot --fresh-session --no-footer --trace -y ...
```

The first sandboxed attempt could not reach `127.0.0.1:11434` and was rerun
with escalation for the Ollama localhost API.

### Coding

Workdir:

```text
/private/tmp/anvil-p25-stage-coding-work
```

State:

```text
/private/tmp/anvil-p25-stage-coding-state
```

Outcome:

```text
final_outcome: done
classified_task_kind: coding
completion_reason: verifier_evidence_satisfied
verify_commands: cargo test --manifest-path Cargo.toml
```

Observed order:

```text
iteration 2: Write src/lib.rs
iteration 3: Write Cargo.toml
iteration 4: controller runs cargo test --manifest-path Cargo.toml
```

Trace:

```text
iteration_seq 0: MissingDeliverableJob
iteration_seq 2: MissingDeliverableJob
iteration_seq 3: no active artifact job; verifier path selected
```

There was no premature verifier run before both required deliverables existed.

### Docs

Workdir:

```text
/private/tmp/anvil-p25-stage-docs-work
```

State:

```text
/private/tmp/anvil-p25-stage-docs-state
```

Outcome:

```text
final_outcome: done
classified_task_kind: docs
completion_reason: artifact_obligations_satisfied
verify_commands: []
```

The generated `README.md` included `## Setup` and `## Usage`.

### Data

Workdir:

```text
/private/tmp/anvil-p25-stage-data-work
```

State:

```text
/private/tmp/anvil-p25-stage-data-state
```

Outcome:

```text
final_outcome: done
classified_task_kind: data
completion_reason: artifact_obligations_satisfied
verify_commands: []
```

The generated `summary.json` included exactly the required top-level fields:

```json
{
  "topic": "JSON output",
  "status": "complete"
}
```

## Architecture Insight

This is not the final architecture yet, but it removes one source of
controller fragility:

```text
deliverable completeness is now an explicit lifecycle stage
```

That matters for general-purpose Anvil because the same stage can later be
fed by non-coding contracts:

- document sections
- structured output files
- command observations
- research/source evidence
- visual/content observations

without adding task-kind-specific loops.

Remaining follow-up:

- promote the remaining evidence/verifier readiness branch into an explicit
  evidence-stage type
- make `ObjectiveContract` carry enough stage metadata so lifecycle code no
  longer reads `TaskContract` fields directly
- keep `WorkMode` as advisory only; objective lifecycle should decide tool
  policy from contract/stage state
