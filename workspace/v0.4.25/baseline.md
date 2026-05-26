# v0.4.25 Baseline

## Scope

This baseline captures the state before the v0.4.25 cleanup work continues.
Unrelated working-tree noise is intentionally excluded from commits and
evaluation.

## Known Unrelated Working Tree Noise

- `.claude/scheduled_tasks.lock` deleted
- `.agents/skills/source-command-*` untracked directories
- `scripts/photon_weekly_observe.sh` untracked

These are not part of the repair-control cleanup.

## Known Good Direction

- Wrong-stack verifier false-positive `done` is blocked.
- Verifier repair can now terminate through `RepairJob` instead of falling
  through to generic retry.
- Ambiguous generated assertion expectations are rejected instead of being used
  to mutate implementation indefinitely.
- Missing verifier recovery can request project-local setup metadata.

## Known Remaining Failure Modes

- Safe stop is structurally controlled but must be more actionable.
- Objective verifier failures still need better repair convergence.
- Legacy and deterministic branches remain numerous and need classification
  before deletion.
- `turn.rs` still contains too much state control, validation, prompt
  assembly, reporting, and regression test surface.
- Manual smoke evaluation is useful but not yet a stable regression gate.

## Verification Commands

Required after each cleanup slice:

```bash
cargo fmt --check
cargo test --lib -q
cargo build --release
```

Expanded checks before final commit:

```bash
cargo clippy --all-targets -- -D warnings
```

## Baseline Acceptance

The baseline is valid only if:

- unrelated dirty files are not staged;
- repair-control changes have targeted tests;
- any safe stop is treated as success only when actionable.
