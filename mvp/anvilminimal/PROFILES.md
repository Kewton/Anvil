# Domain Profiles

`DomainProfile` is the boundary between the runner's architecture and
domain-specific knowledge.

The runner owns:

- phase execution and repair loops
- evidence arbitration
- final acceptance as the sole success gate
- oracle tri-state handling through `VerificationReport`
- recovery handoff generation

A profile owns:

- scaffold paths expected for the domain
- dependency readiness and the sanctioned setup requirement
- build oracle command recognition and compile-error parsing
- serve or behavior probe invocation
- invariants and deterministic repairs
- goal capability inference and evidence vocabulary hooks

Profiles must not special-case runner control flow. If a new domain needs a
new lifecycle shape, extend `DomainProfile` first and keep the runner calling
the trait.

## Implementing A Profile

1. Add a profile module under `src/planner/profiles/`.
2. Implement `DomainProfile` with defaults for anything not applicable.
3. Register it in `src/planner/profile.rs`.
4. Keep dependency setup authority-gated and offline-aware.
5. Return compile errors as `CompileError` so existing repair targeting,
   rollback, and recovery handoff paths work unchanged.
6. Emit behavior evidence through the profile behavior probe, not by bypassing
   final acceptance.
7. Add proof tests that run through the same plan or ultra-plan runner paths.

Do not add provider abstractions, profile-specific runner branches, or
profile-specific repair loops.
