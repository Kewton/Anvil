# Legacy And Deterministic Path Removal Plan

## Policy

Legacy and deterministic paths may remain only if they are one of:

- test fixture
- telemetry adapter
- validator assist
- explicit opt-in compatibility fallback

They must not:

- mark a task as `done`;
- override an active `RepairJob`;
- override an active `MissingVerifierJob`;
- provide repair authority by themselves;
- weaken generated tests to make a verifier green.

## Removal Order

1. Inventory all legacy/fallback call sites with owner, condition, and output.
2. Add regression tests around currently useful behavior.
3. Demote legacy path to assist/telemetry if the new typed path is equivalent.
4. Delete the legacy production branch after tests prove no behavior gap.
5. Keep a compatibility wrapper only when the new typed path lacks coverage.

## Candidate Groups

### Group A: Verifier Repair Legacy Bridges

Risk:

- Can select or fill repair targets without a fully accepted semantic plan.

Action:

- Keep only as diagnostic compatibility input.
- Do not allow direct patch application from legacy-only assessment.
- Require accepted plan admission before production patch repair.
- Current v0.4.25 status: `repair_plan_admission.rs` is now the admission
  boundary. Legacy assessment data can be adapted into a brief, but it must
  pass authority validation before the patch provider can run.

### Group B: Deterministic Repair Helpers

Risk:

- Can become use-case-specific and bypass LLM/controller authority.

Action:

- Keep out of production verifier repair.
- Allow only as validator assist or telemetry comparison.

### Group C: Focused Edit / Generic Retry

Risk:

- Can steal control after verifier repair has started.

Action:

- Gate exclusively through `RecoveryDispatchGate` projected from
  `RecoveryOwner`.
- Add tests for every owner state.
- Current v0.4.25 status: owner gating is pinned by
  `recovery_owner_gates_lower_level_fallbacks`; `RepairJob` and
  `MissingVerifierJob` deny generic repo-change recovery, focused edit
  recovery, and deterministic fallback.
- Production recovery branches now consult `RecoveryDispatchGate`, so the
  permission vocabulary is no longer spread as raw `RecoveryOwner::allows_*`
  calls in the actor loop.

Remaining:

- Keep deleting direct recovery branches only after wrapper-level regression
  tests prove equivalent behavior.
- Next deletion candidates are deterministic fallback branches that are now
  reachable only when `RecoveryDispatchGate::allows_deterministic_fallback`
  returns true.

### Group D: Scaffold / Template Fallbacks

Risk:

- Can be mistaken for completed implementation, tests, or docs.

Action:

- Treat as scaffold only.
- Never count as role completion until edited or verified as request-aligned.

## Acceptance

- No active repair job can enter focused edit or deterministic fallback.
- No legacy path can apply a repair patch without accepted plan admission.
- Every retained fallback has a test proving its owner boundary.
