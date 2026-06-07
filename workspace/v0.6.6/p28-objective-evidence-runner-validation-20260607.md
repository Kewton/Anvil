# P28 Objective Evidence Runner Validation

Date: 2026-06-07

## Objective

Move `MissingEvidence` closer to a generic EvidenceRunner model.

P26 introduced `ObjectiveEvidenceStage`, and P27 made it consume
`ObjectiveContract` obligations. P28 adds a small first-class runner enum:

```text
ObjectiveEvidenceRunner::Command(EvidenceSpec)
ObjectiveEvidenceRunner::ArtifactAcceptance(EvidenceSpec)
ObjectiveEvidenceRunner::NotRequired
```

This keeps the current behavior unchanged while making the evidence path
explicitly extensible beyond coding verifiers.

## Root Cause Addressed

Before P28, `ObjectiveEvidenceStage::MissingEvidence` carried only
`EvidenceSpec`. That still made the runtime action implicit:

```text
MissingEvidence -> RunVerifier
```

That is too coding-shaped for general-purpose Anvil. Docs/data often satisfy
evidence through artifact acceptance, and answer-only tasks need no evidence
runner at all.

## Change

Added:

```text
ObjectiveEvidenceRunner
```

and changed:

```text
ObjectiveEvidenceStage::MissingEvidence { runner }
ObjectiveEvidenceStage::SatisfiedOrNotRequired { runner }
```

Current mapping:

- coding with required command evidence -> `Command(TestRun)`
- docs/data artifact-only completion -> `ArtifactAcceptance(ContentCheck/SchemaCheck)`
- answer-only -> `NotRequired`
- legacy existing-unverified code/test fallback -> `Command(evidence_kind)`

The outward recovery action remains unchanged:

```text
MissingEvidence + no suppression -> RunVerifier
MissingEvidence + suppression -> RepairArtifact
SatisfiedOrNotRequired -> no recovery action
```

## Unit Validation

```text
cargo test --offline --lib objective_evidence
3 passed

cargo test --offline --lib objective_lifecycle_stage
3 passed

cargo test --offline --lib controller_state_packet_evidence_command_runs_after_deliverables_exist
1 passed

cargo test --offline --lib task_execution_contract_rides_all_six_task_kinds_and_round_trips_objective
1 passed

cargo test --offline --lib
3821 passed; 0 failed
```

The tests pin:

- missing coding evidence uses `Command(TestRun)`
- docs/data use artifact acceptance runners
- answer-only uses `NotRequired`
- deliverable-before-evidence ordering remains intact
- runtime objective projection still round-trips

## Local LLM Validation

Model:

```text
qwen3.6:27b-coding-mxfp8
```

All runs used:

```text
cargo run --offline -- --oneshot --fresh-session --no-footer --trace -y ...
```

### Coding

Workdir:

```text
/private/tmp/anvil-p28-evidence-runner-coding-work
```

State:

```text
/private/tmp/anvil-p28-evidence-runner-coding-state
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
/private/tmp/anvil-p28-evidence-runner-docs-work
```

State:

```text
/private/tmp/anvil-p28-evidence-runner-docs-state
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
/private/tmp/anvil-p28-evidence-runner-data-work
```

State:

```text
/private/tmp/anvil-p28-evidence-runner-data-state
```

Outcome:

```text
final_outcome: done
classified_task_kind: data
completion_reason: artifact_obligations_satisfied
verify_commands: []
```

The generated `summary.json` included the required top-level fields:

```json
{
  "topic": "JSON output file",
  "status": "completed"
}
```

## Architecture Insight

P28 is still a small step, but it makes the next extraction clearer:

```text
ObjectiveEvidenceRunner
  -> Command
  -> ArtifactAcceptance
  -> NotRequired
```

This gives future research/ops/visual/content evidence a place to attach
without adding another coding-specific verifier gate.

Remaining follow-up:

- make the actual runner execution path consume `ObjectiveEvidenceRunner`
  rather than mapping command evidence immediately to the legacy verifier action
- add research/ops runner variants once their concrete evidence observations are
  represented in the same lifecycle
