# P26 Objective Evidence Stage Validation

Date: 2026-06-07

## Objective

Continue the small-step migration toward a generic objective lifecycle by
separating evidence readiness from the remaining `plan_artifact_recovery`
function body.

P25 introduced:

```text
ObjectiveLifecycleStage::MissingDeliverable
ObjectiveLifecycleStage::DeliverablesSatisfied
```

P26 adds the matching evidence-stage vocabulary:

```text
ObjectiveEvidenceStage::MissingEvidence { evidence_kind }
ObjectiveEvidenceStage::SatisfiedOrNotRequired
```

## Root Cause Addressed

Even after the deliverable stage was explicit, the verifier branch still read
`verification_required` directly in the planner body. That kept evidence
selection structurally tied to a coding-era flag instead of the objective
contract's evidence vocabulary.

This is a maintainability problem because future non-coding evidence types
would otherwise need to thread new special cases through the same verifier
branch.

## Change

Added:

```text
ObjectiveEvidenceStage
```

and extracted:

```text
objective_evidence_stage(inputs, verifier_passed, existing_unverified_used, code_or_test_required)
```

The stage carries the generic `EvidenceSpec` from `ObjectiveContract`.

The existing MissingVerifierJob suppression behavior is preserved:

```text
MissingEvidence + suppress retry -> RepairArtifact
MissingEvidence + no suppress -> RunVerifier
```

## Unit Validation

```text
cargo test --offline --lib objective_evidence_stage
2 passed

cargo test --offline --lib controller_state_packet_evidence_command_runs_after_deliverables_exist
1 passed

cargo test --offline --lib objective_lifecycle_stage
3 passed

cargo test --offline --lib satisfied_impl_job_still_blocks_on_missing_manifest_deliverable
1 passed

cargo test --offline --lib
3819 passed; 0 failed
```

The tests pin:

- missing coding evidence is represented as `EvidenceSpec::TestRun`
- MissingVerifierJob suppression still maps to `RepairArtifact`
- docs/data are `SatisfiedOrNotRequired` at the evidence stage
- P24/P25 deliverable-before-evidence ordering still holds

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
/private/tmp/anvil-p26-evidence-coding-work
```

State:

```text
/private/tmp/anvil-p26-evidence-coding-state
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
/private/tmp/anvil-p26-evidence-docs-work
```

State:

```text
/private/tmp/anvil-p26-evidence-docs-state
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
/private/tmp/anvil-p26-evidence-data-work
```

State:

```text
/private/tmp/anvil-p26-evidence-data-state
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
  "topic": "JSON output creation",
  "status": "completed"
}
```

## Architecture Insight

The controller lifecycle is now clearer:

```text
objective_deliverable_stage
-> objective_evidence_stage
-> legacy evaluate/safe-stop bridge
```

This is still incremental, but it reduces the core structural problem:
evidence is no longer just an inline coding verifier branch. It is an
objective-stage decision carrying a generic evidence spec.

Remaining follow-up:

- make `ObjectiveContract` carry enough metadata to avoid direct reads of
  `TaskContract.required_artifacts` and `TaskContract.verification_required`
- rename the remaining TaskContract-facing planner vocabulary only after the
  stage helpers are stable
- add research/ops LLM checks once their evidence runners are first-class
