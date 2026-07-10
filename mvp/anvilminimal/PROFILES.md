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

Profile UltraPlan presets remain planner-tier gated rather than a global
default. UAT test0711_bs_001 showed `--plan-preset profile` produced 4/4 full
qwen27-planner runs and removed roughly half of planner time, while the
gemma31-planner sample regressed and already planned in about 15-38 seconds.
Named presets with qwen27 planner models therefore default to the profile
UltraPlan preset when no `plan_preset` is configured, and gemma-family planner
presets keep the `none` default; explicit CLI `--plan-preset` always overrides
that config-derived default.

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
7. Add or update one row in the conformance matrix under
   `tests/conformance/`. The row must exercise the profile through the normal
   plan or ultra-plan runner path with fake clients and probe overrides, then
   pass the reusable interface-contract checkers.
8. Add focused profile proof tests for domain-specific outputs only after the
   shared conformance row passes.

Do not add provider abstractions, profile-specific runner branches, or
profile-specific repair loops.

## Development Guardrails

New entry points and execution boundaries must be registered in
`tests/protection_coverage_audit.rs` before they are used. Add or update the
declarative protection row for the affected category, including the site
predicate, required wrapper/type, and allowlist, then add a conformance pathway
row when the boundary can affect runner outcomes.

This rule exists because the instruction 92 parity-hole incidents showed the
same failure class recurring at opt-in call sites: compile parsers received
display excerpts instead of full command output, verify commands bypassed the
shared normalizer, and terminal records were missing on some exit paths. New
boundaries must make those bypasses type-impossible or CI-detected.

## Definition Of Done

A new profile is not complete until `cargo test --test conformance` passes with
its matrix row. The conformance suite owns the shared runner/profile interface
contracts: earned assurance, monotonic rebind, authority symmetry,
detect/repair pairing, honest terminal records, oracle tri-state handling, and
degradation labeling. New profiles add rows, not bespoke copies of those
assertions.
