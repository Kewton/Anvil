# Complete Achievement Execution Plan

## Objective

The objective is not "make the FastAPI CRUD evaluation pass once." The
objective is to make Anvil's verifier-related state control structurally
predictable:

```text
observed facts
  -> LoopControlAction
  -> exactly one dispatcher
  -> bounded observation
  -> updated facts
  -> done or actionable safe stop
```

Completion is reached only when active verifier-owned jobs cannot be hijacked by
legacy deterministic fallback, focused-edit recovery, generic repo-change retry,
or stale pending flags.

## Current State After Latest Implementation

Done:

- `RepairJob` dispatch is routed through `LoopControlAction`.
- `MissingVerifierJob` has `VerifierBootstrapNextAction`.
- missing-verifier `RerunVerifier` and `SafeStop` are routed through
  `dispatch_missing_verifier_job_step()`.
- missing-verifier no-tool / invalid-tool setup attempts are counted on the
  missing-verifier job budget.
- pre-model deterministic fallback is blocked when `RepairJob` or
  `MissingVerifierJob` owns the loop.
- `RecoveryOwner` now gates generic repo-change retry, local small-edit
  fallback, focused fallback, and deterministic fallback.
- `workspace/v0.4.23/dispatch-source-inventory.md` contains the recovery
  dispatch audit table.
- verification after the owner-gate implementation passed:
  `cargo fmt --check`, `cargo test loop_control_action_tests --lib`,
  `cargo test missing_verifier_job --lib`, `cargo test repair_job --lib`,
  `cargo test task_contract --lib` outside the sandbox for mockito,
  `cargo test --lib` outside the sandbox for mockito,
  `cargo clippy --all-targets -- -D warnings`, `cargo build --release`, and
  `git diff --check`.

Not done:

- synthetic verifier E2E has not yet been added.
- local LLM smoke evaluation has not yet been run after the latest change.
- legacy compatibility helpers still exist in tests.

## Phase 1: Dispatch-Source Audit

Goal: prove where verifier-owned execution can still branch.

Tasks:

1. Inventory every branch in `turn.rs` that can produce:
   - `ExitReason::MissingRepoEdits`
   - local LLM small-edit fallback
   - deterministic scaffold / deterministic quality fallback
   - focused edit recovery note or focused edit fallback
   - generic no-tool / empty-reply repo-change retry
2. Mark each branch as one of:
   - pre-artifact completion
   - artifact completion
   - verifier repair
   - missing verifier bootstrap
   - ordinary protocol recovery
3. For verifier repair and missing verifier bootstrap, confirm the branch is
   owned by `LoopControlAction` or remove it from that state.

Acceptance criteria:

- A short audit table exists in `workspace/v0.4.23`.
- Every production branch that can emit `missing_repo_edits` has an explicit
  phase owner.
- No branch is classified as "unknown".

## Phase 2: Recovery Ownership Gate

Goal: make the ownership rule executable, not just documented.

Add one small helper boundary:

```rust
enum RecoveryOwner {
    None,
    ArtifactCompletion,
    RepairJob,
    MissingVerifierJob,
}
```

or an equivalent minimal pure projection if a new enum is unnecessary.

The helper should answer:

```text
Can generic repo-change recovery run now?
Can focused edit recovery run now?
Can deterministic fallback run now?
Can protocol retry consume this failure?
```

Tasks:

1. Replace ad hoc `job_owned_loop` checks with a central owner projection.
2. Gate generic repo-change recovery with `owner == None`.
3. Gate focused edit fallback with `owner == None` or explicit artifact owner.
4. Gate deterministic fallback with `owner == None`.
5. Preserve artifact completion behavior. Artifact completion can still use its
   own bounded recovery, because it is pre-verifier and not a verifier repair
   owner.

Acceptance criteria:

- active `RepairJob` cannot enter generic repo-change retry.
- active `RepairJob` cannot apply local LLM small-edit fallback.
- active `RepairJob` cannot apply deterministic scaffold / quality fallback.
- active `MissingVerifierJob` cannot enter generic repo-change retry.
- tests cover each forbidden transition.

## Phase 3: Budget Separation

Goal: protocol errors and semantic repair failures must not consume the same
budget or terminate through the wrong exit reason.

Tasks:

1. Define the budget owner for each failure kind:
   - malformed tool XML / parser failure outside jobs -> protocol budget
   - no tool during artifact completion -> artifact completion budget
   - invalid verifier patch -> `RepairJob` invalid proposal budget
   - missing-verifier setup no tool / invalid tool -> `MissingVerifierJob`
     setup budget
2. Add tests proving budget isolation:
   - invalid missing-verifier setup does not increment generic repo-change retry
   - invalid verifier repair proposal does not emit `missing_repo_edits`
   - ordinary no-tool outside job still uses protocol / repo-change recovery
3. Ensure safe stop text is actionable and does not claim completion.

Acceptance criteria:

- verifier-owned failures end as verifier failed / missing verification safe
  stop, not generic missing repo edits.
- ordinary protocol recovery still works outside verifier-owned states.

## Phase 4: Transition-Table Tests

Goal: pin the controller behavior compactly.

Add table tests for:

| Input State | Expected Owner | Expected Dispatch |
| --- | --- | --- |
| missing artifact | artifact / model | artifact-directed recovery |
| artifacts complete | none | verifier run |
| verifier failed + `RepairJob` diagnostic needed | repair | diagnostic dispatcher |
| verifier failed + `RepairJob` patch needed | repair | patch dispatcher |
| repair patch applied | repair | verifier rerun |
| repair exhausted | repair | actionable safe stop |
| verifier missing + no setup edit | missing verifier | setup edit request |
| verifier missing + setup edit observed | missing verifier | verifier rerun |
| verifier missing exhausted | missing verifier | actionable safe stop |
| stale pending flag without job | none | no repair dispatch |

Negative tests:

- repair owner rejects deterministic fallback.
- repair owner rejects focused-edit fallback.
- repair owner rejects generic no-tool repo-change retry.
- missing-verifier owner rejects deterministic fallback.
- missing-verifier owner rejects generic no-tool repo-change retry.
- artifact completion remains allowed to use artifact-specific recovery.

Acceptance criteria:

- tests fail if a future change reintroduces a second verifier dispatch source.

## Phase 5: Synthetic E2E

Goal: validate state transitions without local LLM nondeterminism.

Synthetic scenarios:

1. verifier failure with accepted implementation repair -> verifier pass -> done
2. verifier failure with invalid patch proposals -> replan or safe stop
3. assertion mismatch with ambiguous authority -> safe stop, no test weakening
4. missing verifier -> setup edit -> rerun verifier
5. missing verifier -> repeated no-tool setup -> missing-verifier safe stop
6. stale pending flag -> no verifier repair dispatch

Acceptance criteria:

- every scenario ends in done or actionable safe stop.
- zero scenarios end in verifier-phase `missing_repo_edits`.
- zero scenarios apply deterministic fallback as semantic authority.

## Phase 6: Legacy Compatibility Cleanup

Goal: remove old paths after tests prove parity.

Tasks:

1. Search production code for `VerifierRepairDecision`.
2. Move remaining test-only helpers into a clearly named legacy test module or
   delete tests replaced by transition-table coverage.
3. Audit deterministic fallback call sites and label them as:
   - normal non-verifier recovery
   - artifact bootstrap
   - validator assist
   - telemetry only
4. Remove or gate any deterministic call site that can still run under
   verifier-owned states.

Acceptance criteria:

- no production dispatch, policy, or recovery note depends on
  `VerifierRepairDecision`.
- deterministic fallback is not a verifier semantic authority.

## Phase 7: Local LLM Smoke Evaluation

Run only after Phases 1-6 pass.

Evaluation order:

1. no-PAM 5 runs
2. PAM 5 runs
3. classify failures by terminal state and owner
4. if stable, no-PAM 20 runs and PAM 20 runs

Pass criteria:

- verifier pass -> `done`
- ambiguous unsafe repair -> actionable safe stop
- no verifier-owned run exits with generic `missing_repo_edits`
- no run reaches green by unaccepted test weakening

Failure categories to record:

- artifact phase stall
- verifier diagnostic malformed
- repair patch invalid
- repair no progress
- missing verifier setup failed
- local LLM protocol failure
- unrelated environment failure

## Phase 8: Verification And Commit Gate

Required commands:

```bash
cargo fmt --check
cargo test loop_control_action_tests --lib
cargo test missing_verifier_job --lib
cargo test repair_job --lib
cargo test task_contract --lib
cargo test --lib
cargo clippy --all-targets -- -D warnings
cargo build --release
git diff --check
```

If sandboxing blocks mock-server tests, rerun the same test command outside the
sandbox with approval and record it in the result.

## Final Completion Criteria

The work is complete when all of these are true:

- controller tests prove one owner per state
- synthetic E2E proves done or actionable safe stop
- production code has no verifier repair dispatch outside `LoopControlAction`
- verifier-owned states cannot produce generic `missing_repo_edits`
- deterministic fallback is not semantic authority for verifier repair
- local LLM smoke shows no verifier-control regression
- verification commands pass

## Implementation Order

1. Phase 1 audit table
2. Phase 2 owner gate and negative tests
3. Phase 3 budget separation tests and fixes
4. Phase 4 transition-table expansion
5. Phase 5 synthetic E2E
6. Phase 6 legacy cleanup
7. Phase 7 local LLM smoke
8. Phase 8 verification / commit

This order intentionally delays local LLM evaluation until state-machine tests
are strong enough. Otherwise failures look like model quality problems even
when the real bug is controller ownership.
