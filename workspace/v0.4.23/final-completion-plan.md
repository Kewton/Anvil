# Final Completion Plan

## Goal

Complete means Anvil's verifier-control flow is structurally reliable, not that
one FastAPI CRUD run happens to pass.

The accepted terminal states are:

- `done` after owned artifacts are complete and the verifier passes.
- actionable safe stop when the verifier failure is ambiguous, unsafe to repair,
  or cannot be repaired within bounded budgets.

The rejected terminal states are:

- verifier-owned `missing_repo_edits`.
- green by deleting or weakening tests without accepted semantic authority.
- deterministic template / fallback acting as verifier repair authority.
- repeated re-entry into generic retry while `RepairJob` or
  `MissingVerifierJob` owns the loop.

## Current Remaining Gaps

1. Synthetic end-to-end transition tests are now present for the controller
   core. They cover verifier diagnostic, accepted patch, rerun, pass,
   malformed diagnostics, invalid patch, ambiguous authority, and missing
   verifier bootstrap.

2. Legacy verifier compatibility helpers still exist.
   This is acceptable temporarily, but complete achievement requires either
   deleting them or proving they cannot become production dispatch again.
   A source-structure guard now proves the production actor loop and arbiter
   candidate builder do not call the legacy verifier repair decision bridge.

3. Local LLM smoke evaluation has not been run after the latest controller
   change.
   The controller is verified, but model-facing behavior still needs empirical
   confirmation.

4. Missing-verifier bootstrap is still a sibling job, not a unified
   `RepairJob` state.
   This can remain if `LoopControlAction` is the single dispatch source and
   tests prove it cannot be hijacked. Folding it into `RepairJob` is optional,
   not a prerequisite, unless later tests expose divergence.

## Execution Update 2026-05-25

Implemented in this pass:

- Added a transition-table test covering all controller-owned states for
  `determine_loop_control_action` and `RecoveryOwner`.
- Added negative assertions that `RepairJob` and `MissingVerifierJob` owners
  cannot enter generic repo-change recovery, focused-edit recovery, or
  deterministic fallback.
- Added synthetic verifier state-machine tests for success, malformed
  diagnostics, invalid patch, ambiguous authority, missing-verifier setup, and
  invalid missing-verifier setup.
- Added a production source-structure guard proving verifier repair dispatch in
  `run_actor_loop` and `build_arbiter_candidates` does not call the legacy
  verifier repair decision bridge.

Verified so far:

```bash
cargo test loop_control_action_tests --lib
cargo test synthetic_verifier_e2e --lib
cargo test production_repair_dispatch_does_not_call_legacy_decision_bridge --lib
cargo test repair_job --lib
```

Remaining before claiming full completion:

- Full verification gate has passed.
- Local LLM 5+5 smoke evaluation has been run and classified.
- A small non-FastAPI generality smoke has been run.
- Full completion is not yet claimed because the smoke results still expose
  verifier environment isolation and patch-convergence issues.

Latest empirical result:

- FastAPI CRUD no-PAM: 4/5 `done`, 1/5 `verifier_failed`.
- FastAPI CRUD PAM live: 4/5 `done`, 1/5 `verifier_failed`.
- Generality smoke:
  - Rust CLI: `transport_error` from 300s auto-test timeout.
  - Python data script: verifier repair safe stop `patch_rejected_repeatedly`.
  - Node utility: verifier repair safe stop `patch_rejected_repeatedly`.

Conclusion:

- The dispatch-source unification goal is materially improved: verifier-owned
  states did not fall into generic `missing_repo_edits` in this smoke set.
- The original "complete and stable local LLM coding agent" goal is still not
  fully achieved. Remaining failures are now concentrated in verifier sandbox /
  environment isolation and repair-patch convergence.

## Phase 1: Lock The Controller Contract

Purpose: make dispatch ownership impossible to regress accidentally.

Tasks:

1. Add a compact transition-table test module for `determine_loop_control_action`
   and `RecoveryOwner`.
2. Cover these states:
   - plan mode with repair state present -> model turn only.
   - stale repair-pending flag without job -> model turn only.
   - active `RepairJob` diagnostic -> repair owner.
   - active `RepairJob` patch -> repair owner.
   - active `RepairJob` rerun -> repair owner.
   - active `RepairJob` safe stop -> repair owner.
   - active `MissingVerifierJob` setup edit -> missing-verifier owner.
   - active `MissingVerifierJob` rerun -> missing-verifier owner.
   - active `MissingVerifierJob` safe stop -> missing-verifier owner.
   - artifacts complete with no active job -> verifier run.
   - missing artifacts with no active job -> model turn / artifact completion.
3. Add negative assertions that verifier owners cannot allow:
   - generic repo-change recovery.
   - focused-edit fallback.
   - deterministic fallback.

Acceptance:

- If a second verifier dispatch source is reintroduced, controller tests fail.
- Artifact completion remains separate and still allowed before verifier repair.

## Phase 2: Add Synthetic Verifier E2E

Purpose: validate the state machine without local LLM nondeterminism.

Tasks:

1. Introduce synthetic harness helpers that drive:
   - verifier result.
   - diagnostic outcome.
   - patch proposal outcome.
   - verifier rerun delta.
2. Add six scenarios:
   - verifier fails -> diagnostic accepted -> implementation patch accepted ->
     verifier passes -> `done`.
   - verifier fails -> malformed diagnostics until budget -> safe stop.
   - verifier fails -> invalid patch proposals -> re-diagnostic or safe stop,
     never generic `missing_repo_edits`.
   - assertion mismatch with ambiguous authority -> safe stop, no test
     weakening.
   - missing verifier -> setup edit observed -> verifier rerun.
   - missing verifier -> repeated no-tool / invalid setup -> missing-verifier
     safe stop.
3. Record the owner and terminal state for each synthetic scenario.

Acceptance:

- Every scenario ends in `done` or actionable safe stop.
- No verifier-owned synthetic scenario exits with `missing_repo_edits`.
- No synthetic scenario uses deterministic fallback as semantic authority.

## Phase 3: Legacy Cleanup With Parity Guard

Purpose: remove old logic only after equivalent tests exist.

Tasks:

1. Search production code for `VerifierRepairDecision`.
2. Confirm all remaining references are either:
   - `#[cfg(test)]`, or
   - comments / migration documentation.
3. Delete legacy tests that duplicate transition-table or synthetic E2E
   coverage.
4. Keep one small compatibility test only if it protects an active migration
   seam.
5. Add a source-structure guard:
   - production `turn.rs` must not call `verifier_repair_decision`.
   - production verifier repair dispatch must flow through
     `dispatch_repair_job_step`.

Acceptance:

- No production dispatch, policy, or recovery note depends on
  `VerifierRepairDecision`.
- The old deterministic repair path is validator assist / telemetry only, not
  a verifier repair decision-maker.

## Phase 4: Local LLM Smoke Evaluation

Purpose: verify model-facing behavior after controller correctness is pinned.

Evaluation command:

```bash
anvildev -m qwen3.6:27b-coding-nvfp4 --sidecar-model qwen3.5:9b -y --fresh-session --no-footer --deterministic-fallback full --max-iterations 50
```

Input task:

```text
FastAPIでcrudのAPIを開発してください。使用方法をREADME.mdに記述してください。テストコードも実装してください。
```

Order:

1. no-PAM 5 runs.
2. PAM 5 runs.
3. Classify terminal states.
4. If no verifier-control regression appears, run no-PAM 20 and PAM 20.

Record for each run:

- terminal state.
- verifier owner at failure, if any.
- whether verifier ran.
- whether repair entered `RepairJob`.
- whether terminal state was `done`, safe stop, artifact stall, diagnostic
  malformed, patch invalid, environment failure, or model protocol failure.
- whether any test edit weakened assertions.

Acceptance:

- No verifier-owned run ends with generic `missing_repo_edits`.
- Successful runs must be verifier-backed.
- Unsafe or ambiguous failures must stop with actionable safe stop.
- Failures are classified by phase, not just counted as pass/fail.

## Phase 5: Generality Check

Purpose: prevent overfitting to FastAPI CRUD.

Tasks:

1. Run at least three non-FastAPI smoke tasks after the 10-run smoke:
   - Rust CLI with tests and README.
   - Python data processing script with tests and README.
   - small Node/JS utility with tests and README.
2. Verify that controller behavior is phase-based:
   - artifact completion.
   - verifier discovery.
   - verifier repair.
   - safe stop.
3. Do not add framework-specific repair rules unless they are generalized into
   project verifier or artifact classification.

Acceptance:

- No new FastAPI / CRUD / ToDo-specific production branch is introduced.
- Any new heuristic is expressed as artifact role, verifier evidence, target
  ownership, or repair authority, not as a specific app pattern.

## Phase 6: Final Verification Gate

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

If sandboxing blocks mock-server tests, rerun the same command outside the
sandbox and record that reason.

## Final Completion Criteria

The work is fully complete only when all conditions are true:

1. Controller ownership is covered by transition-table tests.
2. Synthetic verifier E2E covers success, invalid repair, ambiguous authority,
   missing verifier, and stale pending states.
3. Production verifier repair dispatch is only `RepairJob::next_action()` via
   `LoopControlAction`.
4. Production missing-verifier bootstrap dispatch is only
   `MissingVerifierJob::next_action()` via `LoopControlAction`.
5. Verifier-owned states cannot enter generic repo-change retry, focused-edit
   fallback, local small-edit fallback, or deterministic fallback.
6. Legacy `VerifierRepairDecision` is test-only or removed.
7. Local LLM smoke shows no verifier-control regression.
8. Generality smoke does not reveal FastAPI/CRUD-specific coupling.
9. Full verification commands pass.
10. Final commit contains only related source and planning-document changes.

## Recommended Implementation Order

1. Phase 1 transition-table tests.
2. Phase 2 synthetic verifier E2E.
3. Phase 3 legacy cleanup.
4. Phase 6 verification gate.
5. Phase 4 no-PAM/PAM 5-run smoke.
6. Phase 5 generality smoke.
7. If stable, Phase 4 expanded 20/20 evaluation.
8. Update `workspace/v0.4.23` results.
9. Final verification gate again.
10. Commit.

This order keeps the controller deterministic before spending time on local LLM
evaluation, and prevents model variance from hiding state-control bugs.
