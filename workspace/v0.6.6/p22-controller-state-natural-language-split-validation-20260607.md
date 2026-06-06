# P22 Controller State / Natural Language Split Validation

Date: 2026-06-07

## Objective

Move Anvil closer to a general-purpose, maintainable controller architecture by
separating controller-owned state from user-visible natural-language inference.

This validation focused on a concrete failure:

- A `STATE_CONTROL_PACKET` required `Cargo.toml`, `src/lib.rs`, and
  `cargo test --manifest-path Cargo.toml`.
- The local LLM created only `src/lib.rs`.
- The controller then selected `SetupBootstrap`, constrained the model to Bash,
  and the run ended with `tool_call_format_error`.

## Root Cause

`ArtifactRole::Setup` was overloaded.

It represented both:

- manifest/config deliverables, such as `Cargo.toml` or `package.json`
- environment/dependency setup work, where `SetupBootstrap` and Bash policy are
  appropriate

The controller also fed raw `STATE_CONTROL_PACKET` text into natural-language
inference. That let structural words such as `role=manifest`, `role=setup`, and
`evidence_command` contaminate behavior projection and WorkMode-style policy
signals.

Result: the controller treated a missing manifest deliverable as install/setup
bootstrap work instead of a normal `MissingDeliverableJob` target.

## Changes

Implemented a narrow refactor:

- Added `has_required_setup_install_intent(contract)`.
  - `SetupBootstrap` now requires install intent plus required setup role.
  - Manifest/config deliverables remain normal artifact obligations.
- Kept `has_required_setup_artifact(contract)` for compatibility.
- Split raw controller packet parsing from natural-language inference.
  - `ControllerStatePacket::parse` still reads the raw request.
  - task-kind, behavior, output-context, docs/data/research/ops heuristics now
    use `model_visible_request_text(request)` when a controller packet exists.
- Made controller `evidence_command` require command evidence for coding tasks.
- Tightened `UsageDocs` completion.
  - Required headings, required-section schemas, research notes, and ops
    runbooks are content-gated.
  - Repo-edit evidence alone can no longer satisfy those obligations.

## Unit Validation

Targeted tests passed:

```text
cargo test --offline --lib controller_state_packet_
8 passed

cargo test --offline --lib should_install_setup_bootstrap_false
5 passed

cargo test --offline --lib readme_only_does_not_complete_implementation_contract
1 passed

cargo test --offline --lib
3811 passed
```

Key assertions:

- controller `evidence_command` makes verification mandatory
- controller-owned `role=manifest/setup` vocabulary does not project a setup
  bootstrap label
- after `Cargo.toml` and `src/lib.rs` exist, missing command evidence selects
  `RunVerifier`
- docs section mentions without headings do not complete a required-section
  docs obligation
- manifest deliverables keep `ArtifactRole::Setup` compatibility but do not
  trigger `SetupBootstrap`
- a README request that includes usage docs remains incomplete when only generic
  docs repo-edit evidence exists, because the explicit `UsageDocs` deliverable
  is still missing

## Local LLM Validation

Model:

```text
qwen3.6:27b-coding-mxfp8
```

### Failure 1: before install-intent separation

Workdir:

```text
/private/tmp/anvil-p22-evidence-command-work
```

State:

```text
/private/tmp/anvil-p22-evidence-command-state
```

Outcome:

```text
tool_call_format_error
```

The model wrote only `src/lib.rs`. The controller moved into Bash-only
`SetupBootstrap`, and the run failed instead of writing the missing manifest.

### Failure 2: after install-intent separation only

Workdir:

```text
/private/tmp/anvil-p22-evidence-command-work2
```

State:

```text
/private/tmp/anvil-p22-evidence-command-state2
```

Outcome:

```text
tool_call_format_error
```

Trace still selected:

```text
reason_label: setup_bootstrap
```

This showed that the fix was incomplete. Raw controller packet vocabulary was
still contaminating natural-language behavior projection.

### Success 1: after natural-language split

Workdir:

```text
/private/tmp/anvil-p22-evidence-command-work3
```

State:

```text
/private/tmp/anvil-p22-evidence-command-state3
```

Outcome:

```text
done
completion_reason: verifier_evidence_satisfied
classified_task_kind: coding
```

The run repaired itself in the expected order:

```text
iteration 2: wrote src/lib.rs
iteration 3: wrote Cargo.toml
iteration 4: ran cargo test --manifest-path Cargo.toml
```

Final message:

```text
Completed requested repository changes and verified them with cargo test --manifest-path Cargo.toml.
```

Verifier evidence included:

```text
9 tests passed
```

### Success 2: non-coding docs control

Workdir:

```text
/private/tmp/anvil-p22-docs-control-work
```

State:

```text
/private/tmp/anvil-p22-docs-control-state
```

Prompt required `README.md` with `Setup` and `Usage` sections through a
controller packet, while the visible instruction asked the model to mention
those words only in prose.

Outcome:

```text
done
completion_reason: artifact_obligations_satisfied
classified_task_kind: docs
```

The model wrote actual headings:

```text
### Setup
### Usage
```

Trace confirmed that WorkMode classification input stripped the raw controller
packet, while the Objective Contract still carried the required sections.

## Architecture Insight

The controller packet must be structural authority, not natural-language prompt
content.

For maintainability and generality:

- structural obligations should be parsed from controller state
- task-kind and behavior heuristics should see only model-visible user text
- tool policy should depend on typed intent, obligations, and evidence state,
  not raw prompt tokens
- overloaded roles must be narrowed by typed predicates before they influence
  tool policy
- content-gated deliverables need evidence-specific completion rules

This keeps coding and non-coding tasks on the same lifecycle:

```text
ObjectiveContract -> MissingDeliverableJob -> EvidenceRunner -> terminal outcome
```

without adding provider abstraction or task-kind-specific repair loops.
