# v0.6.7 Architecture Continuation Iteration 4 - 2026-06-07

## Scope

This iteration followed the successful hard Rust TDD run recorded in
`architecture-continuation-iteration3-20260607.md`.

The goal was to verify two things:

- the reporting surface can distinguish an earlier same-turn repair-exhausted
  snapshot from a later final verifier success
- the previous Rust TDD success was not enough evidence by itself, so the same
  task shape was rerun with an actual local LLM

## Implemented Small Steps

### 1. Final verifier report can supersede stale safe-stop snapshots

Run 13 exposed a reporting problem: the session state had final
`last_verifier_invocation.exit_code = 0`, but `job-reports.jsonl` only contained
an earlier `repair_exhausted` snapshot. Runtime behavior was correct, but the
analysis surface was misleading.

The fix keeps the safe-stop event as historical data, but allows a final
`agent.verification.report` to be emitted after verifier success in the same
turn.

Important constraints:

- no runtime control-flow change
- no deletion of the earlier safe-stop event
- no extra provider abstraction
- only the verification report dedup key is reopened
- stale safe-stop linkage is cleared before the final success snapshot

This keeps the report stream append-only while making the latest verifier state
observable.

### 2. Controller repair must not empty a target file

Run 14 produced a new counterexample. The controller-applied repair edited
`Cargo.toml`, but the resulting file was empty. The next verifier still failed
on manifest parsing, and the loop later spent many iterations rejecting repair
proposals.

The new guard rejects repair candidates whose resulting target contents are
empty or whitespace-only.

This is a generic destructive-edit guard, not a Rust- or Cargo-specific rule.
In verifier repair, emptying a target file is almost never a valid repair. If a
future non-coding objective genuinely needs an empty output file, that should
be represented through the objective/evidence contract, not through verifier
repair.

## Actual LLM Validation

### Run 13 recap: success was real but not sufficient

Workspace: `/private/tmp/anvil-v067-tdd-13`

Result:

- final `cargo test` passed
- direct rerun confirmed 4 integration tests passed
- final session state recorded verifier exit code `0`
- stale `job-reports.jsonl` still showed earlier repair-exhausted linkage

This justified the final verification report fix.

### Run 14: repeated task shape failed again

Workspace: `/private/tmp/anvil-v067-tdd-14`

Result:

- terminal state: `max_iterations`
- edited files: `Cargo.toml`, `src/lib.rs`, `tests/palindrome.rs`
- initial verifier reached diagnostic/repair
- setup repair was applied to `Cargo.toml`
- after repair, `Cargo.toml` was empty
- direct `cargo test` failed with missing `[package]` or `[workspace]`
- session unresolved errors included:
  - `patch provider admission rejected: target_mismatch`
  - `verifier_repair_pass_invalid: validation_failed`
- the loop then spent many iterations rejecting invalid controller repair
  proposals instead of converging

Direct confirmation:

```text
cargo test
error: failed to parse manifest at `/private/tmp/anvil-v067-tdd-14/Cargo.toml`

Caused by:
  manifest is missing either a `[package]` or a `[workspace]`
```

This run is important because it rejects the interpretation that run 13 proved
the architecture is already robust. The current direction is still right, but
the repair lifecycle needs better destructive-edit safety and retry/replan
control.

## Verification

Unit and build checks:

- `cargo test final_verifier_success_report_supersedes_same_turn_repair_exhausted_snapshot --lib`
- `cargo test job_report_e2e_tests --lib`
- `cargo test safe_stop_e2e_tests::repair_exhausted --lib`
  - required non-sandbox execution because mockito's local server could not
    bind inside the sandbox
- `cargo test candidate_content_validation_rejects_empty_target_file_candidate --lib`
- `cargo test repair_patch_validation --lib`
- `cargo test verifier_orchestration --lib`
- `cargo check --lib`
- `cargo build`

All passed.

Actual LLM checks:

- run 13: hard Rust TDD passed
- run 14: same task shape failed with empty setup artifact and invalid proposal
  loop

## Architecture Assessment

The architecture still moved in the right direction, but run 14 narrowed the
remaining bottleneck.

The core remaining problem is not simply "the model is weak". The model can
identify setup/test targets and sometimes propose usable edits. The controller
still lacks enough typed lifecycle pressure when repair proposals are repeatedly
invalid.

Specifically:

- target mismatch and validation failures are being treated as retryable for too
  long
- destructive but syntactically representable edits can still slip through if
  they are outside a language cheap-check
- setup repair needs evidence-bound validation, not only file-path admission
- repeated invalid repair proposals should trigger diagnostic replanning,
  target switch, or safe incomplete state before max iterations

## Direction Update

P0 remains a typed controller architecture:

1. LLM interprets semantic failure and proposes intent.
2. Controller validates target, role, ownership, and destructive-edit safety.
3. Evidence runner remains the completion authority.
4. Repeated invalid proposals are lifecycle signals, not just local retry
   events.

Next small implementation hypotheses:

- add a bounded invalid-proposal counter per active target/failure cluster that
  forces diagnostic replanning before max iterations
- separate `target_mismatch` from generic `validation_failed` in the repair
  lifecycle ledger
- add setup-artifact candidate checks that are generic over manifest/config
  shape, without hard-coding benchmark tasks
- continue validating with actual LLM across Rust TDD, existing-code feature
  improvement, Node CLI, Python CLI, docs, data, and ops tasks
