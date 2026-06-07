# P24 Missing Deliverable Before Evidence Validation

Date: 2026-06-07

## Objective

Reduce a remaining recovery delay in the controller lifecycle.

P23 showed a coding run could create `src/lib.rs`, then attempt the verifier
before `Cargo.toml` existed. The controller recovered, but it wasted a turn and
made success depend on repair after an avoidable verifier failure.

The desired lifecycle is:

```text
MissingDeliverableJob until every required deliverable is present
then MissingEvidenceJob / EvidenceRunner
```

## Root Cause

`task_contract_recovery::satisfied_artifact_job_action` treated a satisfied
artifact completion job as evidence-ready when `contract.verification_required`
was true.

That was too narrow. It checked whether the just-finished target was still
blocking, but it did not re-check other required deliverables in the same
contract.

For a controller contract requiring both:

```text
src/lib.rs
Cargo.toml
```

a satisfied implementation job could advance toward evidence before the manifest
deliverable was complete.

## Change

Added `first_blocking_required_obligation_hint`.

When an artifact completion job is satisfied, the controller now scans all
required artifact roles for a remaining blocking obligation before returning
`RunVerifier` or `Done`.

This keeps the generic recovery lifecycle intact:

```text
satisfied one artifact job
-> re-check all required deliverables
-> continue MissingDeliverableJob if any remain
-> only then evidence
```

The change is generic. It is not Rust-specific and does not add provider or
task-kind-specific repair abstraction.

## Unit Validation

```text
cargo test --offline --lib satisfied_impl_job_still_blocks_on_missing_manifest_deliverable
1 passed

cargo test --offline --lib docs_artifact_satisfied_without_verification_returns_done
1 passed

cargo test --offline --lib satisfied_data_artifact_job_still_blocks_on_schema_diagnostic
1 passed

cargo test --offline --lib
3814 passed; 0 failed
```

The new regression test pins the bug directly:

- `src/lib.rs` artifact job is satisfied
- `Cargo.toml` is still missing
- contract requires evidence command
- expected action is `Continue { missing: [Setup], path: Cargo.toml }`, not
  `RunVerifier`

## Local LLM Validation

Model:

```text
qwen3.6:27b-coding-mxfp8
```

All runs used the current source through:

```text
cargo run --offline -- --oneshot --fresh-session --no-footer --trace -y ...
```

### Coding Run 1

Workdir:

```text
/private/tmp/anvil-p24-gate-coding-work
```

State:

```text
/private/tmp/anvil-p24-gate-coding-state
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

The trace selected `MissingDeliverableJob` for `Cargo.toml` after `src/lib.rs`
was written. There was no premature `Bash cargo test` before the manifest was
created.

### Coding Run 2

Workdir:

```text
/private/tmp/anvil-p24-gate-coding2-work
```

State:

```text
/private/tmp/anvil-p24-gate-coding2-state
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

The same ordering repeated in a separate workspace/state directory.

### Docs Non-Coding Check

Workdir:

```text
/private/tmp/anvil-p24-gate-docs-work
```

State:

```text
/private/tmp/anvil-p24-gate-docs-state
```

Outcome:

```text
final_outcome: done
classified_task_kind: docs
completion_reason: artifact_obligations_satisfied
verify_commands: []
```

The generated `README.md` included:

```text
## Setup
## Usage
```

### Data Non-Coding Check

Workdir:

```text
/private/tmp/anvil-p24-gate-data-work
```

State:

```text
/private/tmp/anvil-p24-gate-data-state
```

Outcome:

```text
final_outcome: done
classified_task_kind: data
completion_reason: artifact_obligations_satisfied
verify_commands: []
```

The generated `summary.json` had exactly:

```json
{
  "topic": "summary",
  "status": "complete"
}
```

## Architecture Insight

This moves the controller closer to the intended architecture:

```text
ObjectiveContract
-> required deliverables complete
-> required evidence complete
-> terminal outcome
```

The key point is that evidence cannot be selected merely because one artifact
job finished. The controller must re-evaluate the full objective contract before
moving stages.

This is important for general-purpose use:

- docs: one section/file being complete must not skip another required section
  or document
- data: one output file being present must not skip schema/content checks
- coding: one source file being present must not skip manifest/test evidence
- future shell/research tasks: one observation must not skip required command or
  source evidence

Remaining follow-up: this reduces premature evidence execution after a satisfied
artifact job. It does not yet fully turn `TaskContract` into a neutral
`ObjectiveContract` type or extract a first-class `EvidenceRunner` enum.
