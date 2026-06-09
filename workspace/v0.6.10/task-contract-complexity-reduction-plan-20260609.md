# task_contract.rs Complexity Reduction Plan

Date: 2026-06-09

## Purpose

`agent/loop_run/task_contract.rs` is still the largest complexity hotspot in
Anvil. After the latest extraction it is 11,186 lines, while the surrounding
`task_contract_*` modules are small and easier to reason about.

The goal is not to make the file smaller for its own sake. The goal is to make
Anvil's objective/evidence controller maintainable, general-purpose, and safe
to extend beyond coding tasks without adding benchmark-specific branches or
hidden rule-based authority.

## Current Shape

Current line counts:

| File | Lines |
| --- | ---: |
| `task_contract.rs` | 11,186 |
| `task_contract_recovery.rs` | 776 |
| `task_contract_artifact_predicates.rs` | 412 |
| `task_contract_recovery_planning.rs` | 264 |
| `task_contract_controller_packet.rs` | 266 |
| `task_contract_data_output_context.rs` | 236 |
| other `task_contract_*` modules | mostly under 250 |

The remaining responsibilities in `task_contract.rs` are:

- taxonomy and core data structures
  - `ArtifactRole`, `TaskKind`, `ObjectiveKind`
  - deliverable/evidence enums and schemas
  - `TaskContract`, `CompletionDecision`, `RecoveryTargetHint`
- objective contract projection
- completion evaluation
- artifact recovery planning entry point
- request/task/project inference
- setup and verifier-prerequisite inference
- docs/data/ops obligation construction
- explicit path normalization and path-token masking
- role/path matching and observed-artifact utilities
- many unit tests for all of the above

This creates three risks:

1. **semantic drift**: new behavior can be added in raw request helpers rather
   than through ObjectiveContract candidate/admission.
2. **recovery drift**: repair and completion can keep re-reading raw request
   state instead of explaining a blocked contract component.
3. **test locality drift**: tests for extracted boundaries remain in the
   original file, making future refactors look riskier than they are.

## Target End State

`task_contract.rs` should become a thin compatibility facade:

- re-export stable internal types
- keep `TaskContract::from_request` and `TaskContract::evaluate` as small
  delegating entry points
- hold only cross-boundary orchestration that has not yet been migrated

Target size:

- Phase 1 target: below 8,000 lines
- Phase 2 target: below 5,000 lines
- Phase 3 target: below 3,000 lines
- Final target: below 1,500 lines, with no individual production module above
  roughly 1,000 lines unless it is a pure type/schema module

## Design Principles

Keep:

- deterministic completion authority over ObjectiveContract, artifact ledger,
  evidence ledger, and repair ledger
- local-first Ollama design
- no provider abstraction
- typed safety checks for paths, tools, manifests, schema, evidence scope, and
  terminal state
- `pub(super) use` facade re-exports during migration to avoid broad call-site
  churn

Avoid:

- TOML/FastAPI/Node/Rust benchmark-specific branches
- adding more raw request keyword helpers as the primary semantic path
- WorkMode, PAM, prompt confidence, or assistant prose becoming terminal
  authority
- moving complexity into one new large module
- broad rewrite of the actor loop while refactoring task contract boundaries

Preferred pattern:

> Let the LLM produce semantic candidates and small contract-bound generations.
> Let the controller admit candidates, bind artifacts/evidence, enforce safety,
> and decide terminal state deterministically.

## Proposed Module Boundaries

### 1. `task_contract_taxonomy.rs`

Move:

- `ArtifactRole`
- `TaskKind`
- `ObjectiveKind`
- `ObjectiveDeliverableKind`
- `ObjectiveEvidenceKind`
- `DeliverableKind`
- `DeliverableFormat`
- `DeliverableSchema`
- `TaskIntent`
- `ProjectLanguage`
- `ProjectShape`
- `VerificationRequirement`

Keep exhaustive decision points near the enum when they are true 1:1 decisions:

- label/from_label
- deliverable kind mapping
- evidence kind mapping
- role-specific next-action text

Rationale:

- low behavioral risk
- large readability gain
- makes future non-coding roles explicit without adding controller rules in the
  main file

Validation:

- enum round-trip tests
- role mapping tests
- `cargo test --lib task_contract`

### 2. `task_contract_core.rs`

Move:

- `ObjectiveContract`
- `TaskContract`
- `TaskClassification`
- `TaskDeliverable`
- `DeliverableObligation`
- `StructuredRecordSchema`
- `ProjectIntent`
- `CompletionDecision`
- `SafeStopReason`
- `RecoveryTargetHint`
- `RecoveryTarget`
- `ArtifactState`

Keep methods that are pure data constructors or accessors. Move heavy
evaluation and inference out separately.

Rationale:

- separates "what is the contract?" from "how did we infer/evaluate it?"
- gives other modules typed inputs without importing the old giant module

Validation:

- constructor sanitization tests
- serialization/display tests where applicable
- no behavior change in task_contract test suite

### 3. `task_contract_path_context.rs`

Move:

- path token splitting and masking
- `OutputContextScan`
- shared cue vocabulary
- bounded before/after context helpers
- `normalize_explicit_artifact_path`
- `normalize_explicit_user_artifact_path`
- obligation-path validation and sanitization

Existing `task_contract_data_output_context.rs` should depend on this module,
not on `task_contract.rs`.

Rationale:

- current extraction still made several helpers `pub(super)` from
  `task_contract.rs`; this keeps the next boundary clean
- path normalization is safety infrastructure, not task-contract orchestration

Validation:

- path masking tests
- protected path tests
- data/docs source/output discriminator tests
- explicit path obligation tests

### 4. `task_contract_request_inference.rs`

Move first, then reduce:

- `infer_task_kind`
- `infer_intent`
- `infer_project_language`
- `infer_project_shape`
- `infer_verification_requirement`
- `request_asks_for_*`
- setup negation helpers
- docs/research/ops/authoring classification predicates

Important: extraction is only the first step. The longer-term direction is to
replace much of this raw request helper stack with:

1. LLM-produced semantic candidates
2. deterministic contract admission
3. fallback helper use only when no semantic candidate is available or when
   safety admission requires deterministic parsing

Rationale:

- this is the main source of rule-sprawl risk
- extracting it makes the debt visible before changing behavior

Validation:

- existing classification parity tests
- non-coding lifecycle category tests
- docs/data/research/ops prompt fixtures
- local LLM smoke for docs, data, ops, and coding/TDD after any behavior change

### 5. `task_contract_obligation_builder.rs`

Move:

- docs obligation inference
- data obligation inference
- ops command-observation/runbook obligations
- explicit artifact obligation extraction
- default path selection
- obligation merge/dedup rules
- expected column/row extraction

Input should become a small typed struct, for example:

```rust
struct ObligationBuildInput<'a> {
    request: &'a str,
    scan: &'a OutputContextScan,
    task_kind: TaskKind,
    intent: TaskIntent,
    project_intent: &'a ProjectIntent,
    semantic_candidate: Option<&'a SemanticCandidate>,
}
```

Do not add new per-benchmark branches. If an obligation requires semantic
interpretation, prefer adding it to the semantic candidate/admission path.

Rationale:

- obligation construction is where coding/non-coding generality should live
- separating it makes contract-bound generation easier later

Validation:

- docs required-section tests
- data schema/expected-row tests
- ops command observation tests
- explicit path and protected path tests
- local LLM data JSON and docs section smoke

### 6. `task_contract_evaluator.rs`

Move:

- `TaskContract::evaluate_inner`
- owned-test safe-stop gate
- required role satisfaction
- artifact-ready checks
- `observed_artifacts`
- verifier binding predicates

Keep `TaskContract::evaluate` as a facade method delegating into the evaluator.

Rationale:

- terminal authority must be explainable and isolated
- future false-missing fixes should land in evidence binding, not in request
  inference or recovery prose

Validation:

- completion authority tests
- false-done guard tests
- bound verifier tests
- data/docs non-executable verifier tests
- `cargo test --lib task_contract`

### 7. `task_contract_recovery_entry.rs`

Move the remaining recovery entry points still in `task_contract.rs`:

- `ArtifactRecoveryInputs`
- `ArtifactRecoveryAction`
- `plan_artifact_recovery`
- `ObjectiveLifecycleStage`
- objective deliverable stage helpers
- unexpected data output recovery action

Keep existing `task_contract_recovery.rs` and
`task_contract_recovery_planning.rs`; this step should move only the remaining
facade-level lifecycle entry logic.

Rationale:

- repair lifecycle already has several modules, but the entry point still pulls
  evaluation, obligations, and recovery together in the giant file
- this prepares a future `RepairTargetDecision` boundary

Validation:

- recovery ordering tests
- missing deliverable vs missing evidence tests
- docs/data/ops recovery tests
- local LLM TDD repair smoke

### 8. Test Relocation

Move tests out of `task_contract.rs` as each boundary moves.

Target layout:

- taxonomy tests near `task_contract_taxonomy.rs`
- request inference tests near `task_contract_request_inference.rs`
- obligation tests near `task_contract_obligation_builder.rs`
- evaluator tests near `task_contract_evaluator.rs`
- recovery tests near recovery modules

Rationale:

- line-count reduction is not enough if all behavior tests remain centralized
- localized tests make future changes safer and easier to review

## Execution Order

### Phase A: Facade-Safe Type Extraction

Scope:

1. create `task_contract_taxonomy.rs`
2. create `task_contract_core.rs`
3. re-export from `task_contract.rs`
4. relocate only tests that directly exercise moved enum/data behavior

Expected result:

- 1,500-2,000 line reduction
- no behavior change
- low merge risk

Validation:

- `cargo fmt`
- `cargo test --lib task_contract`
- `cargo test --lib task_contract_input_projection`
- no LLM validation required unless behavior changes unexpectedly

### Phase B: Path/Context Boundary Cleanup

Scope:

1. create `task_contract_path_context.rs`
2. move `OutputContextScan`, path masking, explicit path normalization, and
   context helpers
3. update `task_contract_data_output_context.rs` and docs/source discriminators
   to use the new module

Expected result:

- removes current helper re-export pressure from `task_contract.rs`
- reduces safety/path logic coupling

Validation:

- `cargo test --lib data_capability`
- `cargo test --lib data_path_direction_uses_nearest_governing_cue`
- explicit path/protected path tests
- local LLM data JSON smoke

### Phase C: Obligation Construction Split

Scope:

1. create `task_contract_obligation_builder.rs`
2. move docs/data/ops/explicit obligation construction
3. introduce a small typed build input
4. keep raw request helper behavior unchanged initially

Expected result:

- makes contract-bound generation feasible
- reduces the largest remaining logic cluster

Validation:

- docs/data/ops obligation tests
- `cargo test --lib task_contract`
- local LLM:
  - docs required sections
  - explicit JSON data output
  - ops command observation

### Phase D: Evaluator Split

Scope:

1. create `task_contract_evaluator.rs`
2. move completion decision and safe-stop gating
3. keep `TaskContract::evaluate*` as facade methods

Expected result:

- terminal authority becomes isolated
- future false-missing work lands in one place

Validation:

- completion authority tests
- bound verifier tests
- task_contract suite
- local LLM coding/TDD smoke

### Phase E: Request Inference Extraction And Candidate Admission Prep

Scope:

1. create `task_contract_request_inference.rs`
2. move request helper stack without behavior changes
3. introduce a future-facing `SemanticCandidate` / `AdmissionInput` shape behind
   a feature-neutral path
4. log candidate/admission rejection reasons when available

Expected result:

- rule-heavy code is isolated
- the next architecture step can replace helper decisions with LLM semantic
  candidates without touching evaluator/recovery

Validation:

- classification parity tests
- non-coding category tests
- local LLM:
  - docs
  - data
  - ops
  - coding/TDD
  - one hard coding probe

### Phase F: Recovery Entry Split

Scope:

1. move remaining artifact recovery entry point and lifecycle stage helpers
2. route future repair changes through a typed `RepairTargetDecision`
3. keep existing repair modules intact

Expected result:

- repair lifecycle no longer depends on the main task contract file as a
  behavioral hub
- repair_exhausted diagnostics can report blocked contract components more
  directly

Validation:

- recovery planning tests
- repair lifecycle tests
- local LLM TDD repair smoke
- one hard probe: Rust/Cargo or Node formatter

## Complexity Guardrails

Per-step constraints:

- one responsibility boundary per commit
- no new benchmark-specific semantic branch
- no new Agent-level lifecycle flag
- no terminal-state behavior change without a dedicated evaluator test
- no path/tool safety helper moved without equivalent tests
- do not introduce a module that immediately exceeds roughly 1,000 production
  lines unless it is pure data/taxonomy

Review checklist:

- Does this new helper belong in semantic candidate/admission instead?
- Does this new lifecycle state belong in TurnState, LoopState, or a ledger?
- Does this new recovery behavior indicate the generation phase was too broad?
- Can the failure be explained by ObjectiveContract + artifact/evidence facts
  without reading assistant prose?

## Validation Matrix

Minimum deterministic validation after every phase:

- `cargo fmt`
- focused unit tests for the moved boundary
- `cargo test --lib task_contract`

Additional validation when behavior changes:

- local LLM docs/content task
- local LLM data/schema task
- local LLM ops command-observation task
- local LLM coding/TDD task
- manual postcheck of generated artifacts/evidence

Hard-case validation every two behavior-affecting phases:

- TOML parser task
- Rust/Cargo binding task
- Node JSON formatter task
- FastAPI CRUD task

Do not claim architecture-level success from one or two good runs:

- use 20-run smoke for local improvement claims
- use 50-run round-robin for architecture improvement claims
- report pass rate, false-done, false-missing, terminal distribution, and
  per-task-kind results

## Success Criteria

The refactor is successful when:

- `task_contract.rs` is below 1,500 lines
- evaluator, obligation builder, request inference, recovery entry, taxonomy,
  and path/context responsibilities are separate
- adding a new non-coding task shape does not require editing the evaluator
- adding a new evidence kind does not require raw request helper changes
- repair_exhausted reports a blocked contract component and admitted target
  history
- false-done remains zero in validation
- local LLM hard coding probes improve through contract-bound generation and
  typed repair decisions, not benchmark-specific branches

## Immediate Next Commit Candidates

Recommended next sequence:

1. Phase A1: move taxonomy enums and label/from_label mappings.
2. Phase A2: move core data structs and constructor-only methods.
3. Phase B1: move path/context scan helpers into `task_contract_path_context.rs`.
4. Phase C1: move explicit artifact obligation extraction and default path
   builders.
5. Phase D1: move `TaskContract::evaluate_inner` into evaluator module.

This order keeps risk low: type and pure utility moves first, behavior-heavy
request inference later, repair entry last.
