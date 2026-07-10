# Development Guardrails

## Runner Growth Tripwire

`src/planner/runner.rs` and `src/minimal_loop/loop_run.rs` are interim group-3
chokepoints. They may shrink during refactors, but they should not keep
absorbing new subsystems.

The CI/test guard records current baselines and fails if either file grows
above baseline +2%:

- `src/planner/runner.rs`: 28,712 lines
- `src/minimal_loop/loop_run.rs`: 8,347 lines

When adding behavior, put new subsystems in new modules and call them from the
runner. Refactors that shrink these files are allowed; update the baseline only
after the shrink is intentional and reviewed.
