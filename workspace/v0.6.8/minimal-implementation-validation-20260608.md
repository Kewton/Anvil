# v0.6.8 Minimal Implementation / Validation Log

Date: 2026-06-08

## Scope

This continuation focused on the v0.6.8 architecture goal:

- keep false-done at zero
- reduce under-credit when objective-bound evidence already satisfies the contract
- keep the mechanism generic across coding and non-coding tasks
- avoid adding benchmark-specific pattern matching

## Minimal Implementation

### 1. Terminal completion credit reconciliation

Added a final reconciliation step before terminalization.

The controller can now promote selected conservative terminals to `done` only
when the controller-owned task contract already evaluates to `Done` from typed
evidence and bound owned test artifacts.

Allowed source terminals:

- `missing_repo_edits`
- `missing_verification`
- `safe_stop_verifier_weak`
- `safe_stop_verifier_missing`
- `repair_safe_stop`

Rejected source terminals:

- `verifier_failed`
- `transport_error`
- `repair_exhausted`

Reason:

The successful evidence ledger is not yet ordered strongly enough to override a
raw verifier failure or a tool/transport failure. The first increment therefore
handles under-credit only where the terminal state is conservative/missing-like.

### 2. Python verifier scope correction

Actual LLM validation exposed a false-done risk in feature-improvement work:
Anvil created a new generated pytest file, ran only that owned test, returned
`done`, but the existing regression test suite still failed.

The fix changed the stdlib Python pytest verifier to:

- require at least one owned test artifact as binding metadata
- run the full pytest suite with `python3 -m pytest -q -p no:cacheprovider`
- keep owned test artifacts out of the positional pytest arguments

This is intentionally not a task-specific rule. It aligns Python with Cargo/npm:
owned tests prove that the current task contributed test evidence, while the full
suite remains the completion evidence for the project.

## Actual Local LLM Validation

Model configuration used:

- main: `qwen3.6:27b-coding-nvfp4`
- sidecar: `qwen3.5:9b`
- command family: `anvildev -m ... --sidecar-model ... -y --fresh-session --no-footer --deterministic-fallback full-template`

### Coding: Node CSV CLI

Prompt shape:

- create Node CLI
- read `input.csv`
- write `output.json`
- include `package.json`
- add npm test and run tests

Result:

- terminal: `verifier_failed`
- artifacts: `package.json`, `src/index.js`, `tests/index.test.js`
- independent `npm test`: failed

Interpretation:

The controller did not false-complete. This validates the conservative side of
the reconciliation rule.

### TDD: Rust slug library

Prompt shape:

- create Rust library crate
- implement slugify
- add integration tests first
- run `cargo test`

Result:

- terminal: `safe_stop_verifier_missing`
- artifacts: `Cargo.toml`, `src/lib.rs`, `tests/slugify.rs`
- independent `cargo test`: failed because the integration test imported the
  wrong crate name

Interpretation:

The run remained safe, but repair convergence is still weak for setup/test
binding mistakes. This should be addressed as diagnostic repair targeting, not
as a completion-credit relaxation.

### Data: CSV output

Prompt shape:

- create `data/output.csv`
- include requested `id,total` rows
- no source code

Result:

- terminal: `done`
- independent file/schema check: passed

Interpretation:

The non-coding path remains compatible with the architecture. No executable
verifier was forced onto a data-only task.

### Docs: Runbook

Prompt shape:

- create `docs/runbook.md`
- include required runbook sections

Result:

- terminal: `done`
- independent heading check: passed

Interpretation:

Docs remain stable. They are useful as a regression guard, but no longer stress
the hard parts of the architecture.

### Feature improvement: Python existing test suite

Prompt shape:

- existing `sales.py`
- existing `tests/test_sales.py`
- improve `summarize()` to add `average`
- keep total behavior
- update tests and run pytest

Before the verifier-scope fix:

- terminal: `done`
- generated test passed
- existing regression test failed

After the verifier-scope fix:

- terminal: `max_iterations`
- full pytest suite caught the existing failing test
- no false-done occurred

Interpretation:

This is the most important new insight. Completion exactness requires both:

1. objective-bound evidence credit
2. evidence scope correctness

Crediting a narrow generated test is not enough for feature-improvement tasks.
The controller must treat existing project regressions as part of completion
evidence whenever a project-level verifier is available.

## Architecture Revision From This Validation

The v0.6.8 direction should split P0 into two subproblems.

### P0a: Completion credit reconciliation

If the typed ObjectiveContract is already satisfied by observed deliverables and
bound evidence, conservative terminal labels should reconcile to `done`.

This is now minimally implemented for selected missing/safe-stop states.

### P0b: Evidence scope accuracy

Evidence must be wide enough to prove the objective, not merely prove that a new
owned artifact can pass in isolation.

Current rule:

- project-level test runners should run the project-level suite
- owned artifacts remain binding metadata

This should generalize to other evidence runners:

- data: validate the requested output and schema, not just file existence
- docs: validate requested sections/content, not just markdown existence
- research: validate source observations/citations, not just a prose report
- ops: validate requested command observations, not just command success

### P1: Repair targeting from verifier diagnostics

The Python feature-improvement run no longer false-completes, but it also does
not converge. The repair worker needs a typed diagnostic target that can tell the
LLM which project artifacts are still contradicted by the full evidence run.

This should not become a pile of language-specific regex fixes. The safer shape
is:

- verifier result packet
- failing file/test/function excerpts when available
- objective contract obligations
- observed changed artifact roles
- LLM semantic advisor proposes repair target
- controller adopts only safe, contract-preserving repair targets

## Maintainability Check

The implementation remains small:

- one final reconciliation hook at the actor-loop exit point
- one pure reconciliation predicate with unit tests
- one verifier command constructor scope change for stdlib pytest

The implementation does not add:

- provider abstraction
- benchmark-specific task cases
- extra work modes
- raw prompt string gates for completion

The main complexity risk remains concentrated in `task_contract.rs`. Future work
should move new completion/evidence helpers into smaller modules instead of
adding projection helpers to the contract file.

## Next Minimal Steps

1. Add a typed verifier result packet that records full-suite failure scope and
   owned-artifact binding separately.
2. Feed that packet into diagnostic repair prompts as controller-owned context.
3. Validate on the same Python feature-improvement fixture until the controller
   either converges or safe-stops with a precise target.
4. Repeat with Rust TDD crate-name binding failures.
5. Add data/research/ops LLM validations where evidence scope is checked by
   typed schema/source/command observations rather than coding-first tests.
