# v0.6.8 Future Architecture Direction

Date: 2026-06-08

## Executive Summary

v0.6.8 changes the architecture priority.

v0.6.7 showed that a generic/profile-aware controller can improve throughput,
but it also created a critical false-done risk: a coding task could be
misclassified as docs/authoring and terminal `done` after README-only output.

v0.6.8 shows that the false-done class is now controlled:

- zero false-dones in 50 runs
- Node CSV moved from 0/18 in previous rounds to 2/4
- FastAPI passed for the first time
- overall pass rate reached the highest observed value, 17/50 = 34%

The new bottleneck is the opposite failure mode: under-credit. Four runs passed
the independent postcheck but Anvil ended in a non-`done` terminal state. This
means the next architecture milestone is no longer just "make `done` stricter".
It is:

> Make completion exact: keep false-done at zero while recognizing objective-bound
> passing evidence as `done`.

This is a better problem than false-done, but it is still a controller problem.
Adding more repair prompts or benchmark-specific fixes will not be enough if the
controller cannot credit successful evidence consistently.

## What v0.6.8 Proved

### 1. Typed Binding Work Converts Real Failures

The Node CSV result is the strongest evidence so far that typed artifact and
test binding is the right direction. The task no longer closes as README-only,
and it now creates a real Node project with source, package setup, tests, and
README.

Architecture implication:

- The controller should continue moving toward typed deliverable/evidence
  contracts.
- LLM semantic interpretation is useful, but only as an input to contract
  construction.
- Completion must remain bound to observed artifacts and objective-relevant
  evidence.

### 2. False-Done Is Controlled, But Completion Is Too Conservative

v0.6.8 had four independently passing runs that did not terminal `done`:

| Run | Case | Terminal despite passing postcheck |
| --- | --- | --- |
| 188 | Node CSV | `missing_repo_edits` |
| 197 | Rust slug | `missing_verification` |
| 199 | Python markdown | `repair_safe_stop` |
| 226 | Python sales | `repair_safe_stop` |

Architecture implication:

- Strictness without credit produces false-missing.
- A passing owned verifier must become a first-class completion signal.
- `repair_safe_stop` and `missing_*` states need a final evidence reconciliation
  step before terminalization.

The system should still prefer under-claiming over false success, but under-credit
must be measured and reduced. Otherwise Anvil will keep doing unnecessary repair
after it has already succeeded.

### 3. Repair Convergence Improved, But Lifecycle Escalation Is Still Weak

The 50-iteration budget now sometimes binds. This is an improvement over earlier
rounds where the loop often stopped too early, but it also exposes a new issue:
some runs now spend the full budget inside a non-converging repair lifecycle.

Architecture implication:

- Repeated invalid repair proposals should be lifecycle signals, not just retry
  events.
- Repair should have typed reasons such as target mismatch, binding failure,
  destructive edit rejection, setup artifact invalid, verifier failed, and
  evidence unbound.
- After bounded retries, the controller should re-diagnose, switch target,
  reconcile evidence, or safe-stop with an auditable reason.

### 4. PAM Remains Advisory

Across eight rounds, PAM has not shown a reliable effect. In v0.6.8, PAM improved
the 50-run result by one case only.

Architecture implication:

- PAM/context packs may help prompts.
- PAM/evaluate should not influence terminal state.
- Memory should remain advisory until it repeatedly improves controlled
  evaluations without increasing false-done or under-credit.

### 5. Docs Are No Longer The Architecture Test

Docs SRE runbook is 24/24 across the series. That is useful, but it no longer
tests the hard parts of the architecture. The hard parts are:

- coding tasks with setup/test/source ownership
- TDD-style tasks where tests are created before implementation
- data tasks with output schema evidence
- ops/research tasks where command/source observations must be bound to the
  requested objective
- mixed prompts that look like docs but request executable deliverables

Generic architecture validation must keep these hard cases in every cycle.

## Revised Target Architecture

### Layer 1: Semantic Interpretation

Responsibility:

- Use the LLM to interpret the user's objective, ambiguity, task shape, likely
  deliverables, and likely evidence.
- Produce structured candidates, not terminal decisions.

Non-responsibility:

- Do not remove explicit obligations.
- Do not decide `done`.
- Do not directly set tool policy.

Expected outputs:

- objective candidate
- deliverable candidate
- evidence candidate
- uncertainty/conflict notes
- optional diagnostic target candidate during repair

### Layer 2: Contract Adoption Policy

Responsibility:

- Compare semantic candidates against explicit prompt signals, observed artifacts,
  setup/test hints, output file names, and task history.
- Adopt, reject, partially adopt, or strengthen candidate contracts.

Required invariant:

- A candidate can strengthen the contract, but it cannot silently weaken explicit
  source/setup/test/data/command obligations.

v0.6.8 update:

- This layer must also prevent over-conservative adoption. If observed artifacts
  and evidence satisfy the stricter contract, downstream terminal states must be
  allowed to become `done`.

### Layer 3: ObjectiveContract Authority

Responsibility:

- Hold the final controller-owned objective shape.
- Define required deliverables, accepted artifact identities, evidence kind, and
  evidence requirements.
- Be the source of truth for downstream tool policy and completion.

Current gap:

- `ObjectiveContract` is still too much of a projection from legacy task
  contract logic.
- Completion logic still has multiple gates that can disagree.

Target invariant:

- Once `ObjectiveContract` exists, downstream code should not re-infer objective
  shape from raw prompt text, `TaskKind`, `WorkMode`, or local string patterns.

### Layer 4: Artifact and Evidence Ledgers

Responsibility:

- Record observed deliverables and evidence as typed facts.
- Preserve ownership and binding metadata.
- Distinguish "command succeeded" from "objective evidence succeeded".

Required facts:

- artifact role
- artifact identity
- artifact ownership
- evidence runner kind
- evidence exit/result
- binding status
- bound artifact count or bound artifact identities
- stale/superseded status

v0.6.8 update:

- Under-credit shows that evidence facts need to be reconciled before terminal
  states like `missing_verification`, `missing_repo_edits`, and
  `repair_safe_stop` are finalized.

### Layer 5: Tool Policy Projection

Responsibility:

- Choose the next allowed tool class from contract state and ledgers.
- Keep one active write/evidence owner at a time.
- Route to deliverable creation, evidence collection, repair, replan, or safe
  stop.

Required invariant:

- `WorkMode` and plan state may affect UI and permission posture, but not final
  completion authority.
- Bash command classification is execution metadata, not objective proof.
- Tool policy should be projected from missing deliverables/evidence, not from
  coding-first assumptions.

### Layer 6: Evidence Runner and Binding

Responsibility:

- Run objective-appropriate evidence checks.
- Bind evidence to the current contract and owned artifacts.
- Return typed evidence observations.

Examples:

- coding: test/build runner bound to source, setup, and owned tests
- docs: required sections/content checks
- data: output file schema and row/value checks
- research: source fetch or citation evidence
- ops: command observation and safety boundary evidence
- authoring: requested document/content artifact checks

v0.6.8 update:

- Passing owned tests must be creditable even if an earlier repair snapshot was
  incomplete.
- A final evidence reconciliation step should supersede stale safe-stop snapshots
  when current evidence satisfies the contract.

### Layer 7: Completion Authority

Responsibility:

- Decide terminal state.
- Emit a structured reason for `done` and for not-`done`.
- Prefer safe incomplete states when evidence is missing or unbound.

Required invariant:

- `done` requires required deliverables plus required evidence.
- Non-`done` must explain which contract component remains unsatisfied.
- Before terminalizing a missing/safe-stop state, the controller must reconcile
  the latest artifact and evidence ledgers.

v0.6.8 update:

- The central problem is no longer only "make `done` safe".
- The central problem is "make `done` safe and complete enough to recognize
  verified success".

## Updated Roadmap

### P0: Completion Credit Exactness

Goal:

- Keep false-done at zero while reducing under-credit.

Required work:

- Add a final completion reconciliation step before terminalizing
  `missing_repo_edits`, `missing_verification`, `repair_safe_stop`, and
  `repair_exhausted`.
- Promote objective-bound passing evidence to `done` when all required
  deliverables are satisfied.
- Treat stale repair/safe-stop snapshots as historical, not authoritative, when
  newer verifier evidence passes.
- Make under-credit a first-class metric in evaluation summaries:
  independent pass minus Anvil true-done.

Acceptance:

- No false-done regression in Node CSV / README-style coding prompts.
- Previously under-credited patterns such as Node CSV run 188, Rust slug run
  197, markdown run 199, and Python sales run 226 become explainable from logs
  and, where contract/evidence is satisfied, terminal `done`.
- A 50-run evaluation reports both false-done and under-credit counts.

### P1: ObjectiveContract As Completion Source Of Truth

Goal:

- Reduce legacy task-contract disagreement and prevent projection helper sprawl.

Required work:

- Move profile confirmation adoption into one policy module.
- Move final completion decision behind an `ObjectiveContract`-centric API.
- Keep legacy labels as compatibility projections, not primary authority.
- Separate classification, adoption, contract building, evidence reconciliation,
  and terminal decision.

Acceptance:

- Completion decision can be explained without re-reading raw prompt text.
- `TaskKind` and `WorkMode` alone cannot change required deliverables or
  evidence.
- New non-coding objective kinds can be added without modifying coding-specific
  completion gates.

### P2: Rust/Cargo and Setup Artifact Binding

Goal:

- Break the remaining Rust false-missing and Rust 0% cases without adding
  benchmark-specific rules.

Required work:

- Bind Cargo manifest, crate target, integration tests, and source files through
  typed manifest parsing where possible.
- Treat setup artifacts as deliverable components with validation status.
- Distinguish setup syntax failure, missing target, missing test binding, and
  implementation failure.
- Avoid individual benchmark pattern matching; prefer manifest/schema parser
  style validation.

Acceptance:

- Passing `cargo test` with owned tests and valid manifest is creditable.
- Rust slug/NDJSON false-missing patterns are reduced.
- Rust word and TOML walls produce actionable typed failure reasons rather than
  generic repair loops.

### P3: Repair Lifecycle Convergence

Goal:

- Prevent full-budget loops that repeatedly retry invalid repairs.

Required work:

- Track invalid proposal clusters by target, failure reason, and evidence type.
- Escalate after bounded repeated failures:
  - re-diagnose
  - switch target
  - request a narrower repair candidate
  - run evidence reconciliation
  - safe-stop with typed reason
- Keep repair target authority tied to contract components and diagnostic
  evidence.

Acceptance:

- Repair loops expose why they are stuck.
- `max_iterations` becomes rare again for repeated invalid proposals.
- Repair convergence improves without adding task-specific prompt exceptions.

### P4: Generic Non-Coding Expansion With Hard Coding Guardrails

Goal:

- Support docs, data, research, ops, and authoring without weakening coding.

Required work:

- Add small actual-LLM validation sets for:
  - docs runbook/content checks
  - data CSV/JSON transformation with schema evidence
  - research/source fetch tasks
  - ops command-observation tasks
  - authoring/content artifact tasks
- Keep hard coding/TDD cases in every validation cycle.
- Add mixed prompts that intentionally look like docs but request executable,
  data, or command deliverables.

Acceptance:

- Non-coding tasks can complete through objective-specific evidence.
- Coding false-done remains zero.
- Generic additions do not require provider abstraction or benchmark-specific
  controller branches.

### P5: Memory Remains Advisory Until Proven Otherwise

Goal:

- Use memory only where it helps without becoming authority.

Required work:

- Keep PAM and case memory out of terminal decision logic.
- Measure PAM/no-PAM deltas by task kind and by failure class.
- Only promote memory influence if controlled evaluations show repeated gains
  without false-done or under-credit regressions.

Acceptance:

- PAM can improve prompts, but not terminal state.
- Completion remains deterministic over contract, artifacts, evidence, and
  binding.

## Logging Direction

Every terminal decision should be auditable from structured logs.

Required terminal-decision payload:

- objective contract id/version
- task/objective/deliverable/evidence kinds
- required deliverables and satisfaction status
- required evidence and satisfaction status
- evidence runner result
- binding status and bound artifact identities/counts
- active recovery job
- stale/superseded safe-stop or repair snapshots
- final terminal decision
- reason for `done` or reason for not-`done`

The log should make these questions answerable without manual file inspection:

- Was this a generation failure or a completion-credit failure?
- Did the verifier pass?
- Was the verifier evidence bound to owned artifacts?
- Which exact contract component prevented `done`?
- Did a stale repair/safe-stop snapshot override newer evidence?

## Guardrails Against Rule Sprawl

The architecture should not solve v0.6.8 by piling on local string checks.

Keep:

- typed contract adoption
- typed artifact/evidence ledgers
- manifest/schema/parser-style validation
- LLM semantic interpretation for ambiguous intent and diagnosis
- deterministic controller decisions over typed observations

Avoid:

- task-name-specific branches
- benchmark-case-specific repair exceptions
- prompt-only fixes for controller state bugs
- using `TaskKind`, `WorkMode`, or command strings as completion authority
- making PAM or memory a hidden decision source

Rule-based checks are acceptable only when they are structural safety or parsing
boundaries, such as manifest parsing, schema validation, path confinement,
dangerous command screening, or empty-destructive-edit rejection. They should
not become objective semantics.

## Validation Plan

Small implementation steps should continue, but each step needs enough actual
LLM validation to avoid mistaking luck for progress.

Minimum per-step checks:

- unit tests for the typed boundary being changed
- one or more actual local-LLM runs for the affected failure class
- direct postcheck rerun when a task appears complete
- log inspection proving the controller decision is explainable

Periodic capstone checks:

- 20-run smoke across the 10 canonical cases after narrow changes
- 50-run round-robin when completion or recovery authority changes
- report:
  - independent pass
  - Anvil true-done
  - false-done
  - under-credit
  - per-task-kind pass rate
  - terminal distribution

Hard cases that must remain in validation:

- coding CLI
- Rust/Cargo library and CLI
- Node setup/test cases
- Python setup/test cases
- TDD task where tests are authored before implementation
- existing-code feature improvement
- docs
- data transformation
- research/source observation
- ops command observation

## Revised Current Assessment

The target architecture is more credible after v0.6.8 than it was after v0.6.7.
The false-done fix did not merely make the system more conservative; it also
converted Node CSV into real passes. Repair convergence work also broke the
FastAPI zero-pass wall.

However, the architecture is not yet complete. The controller can now be too
strict after success. That means the next improvements should focus less on
"make the model try harder" and more on:

1. reconciling latest evidence before terminal states
2. making `ObjectiveContract` the completion authority
3. binding setup/source/test artifacts through typed ledgers
4. making logs explain false-missing as well as false-done
5. validating every change with actual local LLM runs across hard coding and
   non-coding tasks

The next milestone should be:

> A generic objective/evidence controller that can safely use LLM interpretation,
> reject weak completion, and still credit verified success without relying on
> coding-only assumptions or case-specific rules.

## Continuation Update: 2026-06-08

The first minimal implementation confirmed that P0 must be split into two
separate controller concerns.

P0a is completion credit reconciliation. Conservative terminals such as
`missing_verification`, `missing_repo_edits`, and `repair_safe_stop` may reconcile
to `done` only when the typed contract already evaluates to `Done` from
objective-bound evidence.

P0b is evidence scope accuracy. A verifier run must be broad enough to prove the
project-level objective, not merely broad enough for a newly generated owned test
to pass. Actual local-LLM validation found a Python feature-improvement case
where a generated pytest passed but the existing regression suite failed. The
stdlib Python verifier now runs the full pytest suite while retaining owned test
artifacts as binding metadata.

This changes the next architecture priority:

1. Reconcile completion from typed evidence.
2. Ensure the evidence runner's scope proves the objective.
3. Use diagnostic repair only after both the contract and evidence scope are
   clear.

This keeps the design generic: the controller should not add task-specific
success exceptions. It should collect typed artifacts, run appropriately scoped
evidence, and let the LLM advise repair targets from structured failure packets.

## Continuation Update: Evidence Scope Repair Validation

Further minimal implementation/validation showed that P1 must be made more
precise.

Adding `evidence_scope` to the diagnostic payload was useful but not enough.
Actual local-LLM feature-improvement runs still exhausted repair because the
diagnostic candidate set did not include the existing test file named by the
verifier failure. The controller had treated the changed implementation file as
the primary target, even though full-suite pytest reported
`tests/test_sales.py::test_total`.

The updated direction is:

1. Controller responsibility:
   - record whether evidence came from a project suite or artifact-filtered run
   - extract safe verifier-output artifact paths
   - provide those artifacts as typed candidates/excerpts
   - keep this structural, not task-specific
2. LLM responsibility:
   - judge whether the failing artifact conflicts with higher-authority objective
     evidence
   - choose implementation vs test/setup/docs/data repair target semantically
3. Controller responsibility after LLM:
   - admit only safe, in-scope, contract-preserving targets
   - reject weakening/noop/wrong-target repairs

This means the next milestone is not "add another repair prompt". It is:

> Make failure artifact candidates complete and typed enough for the diagnostic
> LLM to make a real authority decision.

The Rust TDD validation passed after these changes, which suggests the added
scope/candidate plumbing did not regress hard coding tasks. The Python
feature-improvement case remains open and should be used as the next repair
convergence fixture.

## Continuation Update: Candidate Completeness And Data False-Done

Additional minimal validation showed two separate issues.

First, making verifier-output artifacts visible in `FailurePacket` and
`changed_candidates` improves diagnostic input completeness, but it still does
not make the Python feature-improvement fixture converge. The LLM/repair
lifecycle continues implementation-target repair even after full-suite evidence
keeps failing on an existing test artifact. This means the next coding repair
step should be a typed no-progress target reassessment lifecycle, not another
candidate-visibility or prompt-only change.

Second, a non-coding CSV task produced a clear false-done:

- requested `data/output.csv`
- requested columns `id,total`
- requested rows `1,100` and `2,250`
- Anvil returned `done`
- `data/output.csv` contained extra columns and malformed rows
- an extra root `output.csv` was created

This changes the near-term priority:

1. P0: typed data evidence runner for explicit output path, schema, row count,
   and requested rows.
2. P1: no-progress target reassessment for coding repair lifecycle.
3. P2: broader completion-credit improvements after evidence correctness is
   reliable.

The architecture principle is unchanged: do not solve this by adding benchmark
branches. Data correctness should be a generic `EvidenceRunner` capability, and
repair target switching should be a generic lifecycle transition over typed
failure observations.
