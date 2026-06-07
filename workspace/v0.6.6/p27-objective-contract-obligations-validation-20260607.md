# P27 Objective Contract Obligations Validation

Date: 2026-06-07

## Objective

Move lifecycle decisions closer to the generic objective architecture by making
`ObjectiveContract` carry the minimum obligation metadata needed by the
controller:

```text
required_deliverables
evidence_required
```

P25 and P26 introduced explicit deliverable/evidence stages, but those stages
still read `TaskContract.required_artifacts` and `TaskContract.verification_required`
directly. P27 makes those reads go through the objective projection.

## Root Cause Addressed

The previous stage types were generic in name, but not fully generic in data
ownership:

```text
ObjectiveLifecycleStage / ObjectiveEvidenceStage
  -> still depended on TaskContract.required_artifacts / verification_required
```

That leaves the architecture vulnerable to future task-kind-specific patches.
Adding docs/data/research/ops evidence types would keep pulling lifecycle logic
back toward the coding-era `TaskContract` fields.

## Change

Extended `ObjectiveContract` with:

```text
required_deliverables: Vec<ArtifactRole>
evidence_required: bool
```

and added accessors:

```text
required_deliverables()
has_required_deliverables()
requires_evidence()
```

Updated:

- `objective_deliverable_stage` to iterate `ObjectiveContract.required_deliverables`
- `objective_evidence_stage` to read `ObjectiveContract.requires_evidence`
- `satisfied_artifact_job_action` to use `ObjectiveContract.requires_evidence`
- `first_blocking_required_obligation_hint` to use `ObjectiveContract.required_deliverables`
- `worker_contract` compat projection to preserve the new objective metadata

This keeps the migration incremental. The older `TaskContract` fields still
exist, but the lifecycle stage boundary now consumes the objective-layer view.

## Unit Validation

```text
cargo test --offline --lib objective_contract_carries_lifecycle_obligations
1 passed

cargo test --offline --lib objective_lifecycle_stage
3 passed

cargo test --offline --lib objective_evidence_stage
2 passed

cargo test --offline --lib satisfied_impl_job_still_blocks_on_missing_manifest_deliverable
1 passed

cargo test --offline --lib task_execution_contract_rides_all_six_task_kinds_and_round_trips_objective
1 passed

cargo test --offline --lib
3820 passed; 0 failed
```

The tests pin:

- coding controller packet carries source/setup deliverables and required evidence
- docs controller packet carries docs deliverable without command evidence
- answer-only requests carry no deliverable or evidence obligations
- P24/P25/P26 behavior still holds
- runtime `TaskExecutionContract` projects back to the same `ObjectiveContract`

## Source Check

The stage/recovery boundary no longer contains:

```text
inputs.contract.required_artifacts
inputs.contract.verification_required
contract.required_artifacts.iter()
```

for the newly extracted lifecycle stage paths.

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
/private/tmp/anvil-p27-objective-contract-coding-work
```

State:

```text
/private/tmp/anvil-p27-objective-contract-coding-state
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
/private/tmp/anvil-p27-objective-contract-docs-work
```

State:

```text
/private/tmp/anvil-p27-objective-contract-docs-state
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
/private/tmp/anvil-p27-objective-contract-data-work
```

State:

```text
/private/tmp/anvil-p27-objective-contract-data-state
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
  "topic": "JSON summary creation",
  "status": "completed"
}
```

## Architecture Insight

P27 makes the lifecycle less dependent on coding-era `TaskContract` internals:

```text
ObjectiveContract
  -> required deliverables
  -> evidence requirement
  -> deliverable/evidence stages
```

This is still not complete. The next useful step is to make the evidence runner
itself a first-class objective-layer value instead of mapping
`MissingEvidence` to the legacy verifier path immediately.
