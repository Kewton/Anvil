# Development Guardrails

## Runner Growth Tripwire

`src/planner/runner.rs` and `src/minimal_loop/loop_run.rs` are interim group-3
chokepoints. The extracted planner leaf modules also have file-level growth
budgets so they do not become replacement chokepoints.

The CI/test guard records current baselines and fails if any file grows
above baseline +2%:

- `src/planner/runner.rs`: 18,242 lines
- `src/minimal_loop/loop_run.rs`: 8,347 lines
- `src/planner/repair_targeting.rs`: 597 lines
- `src/planner/final_acceptance.rs`: 2,942 lines
- `src/planner/ultra_plan_flow.rs`: 1,570 lines
- `src/planner/assurance.rs`: 1,311 lines

When adding behavior, put new subsystems in new modules and call them from the
runner. Refactors that shrink these files are allowed; lower the baseline only
after the shrink is intentional and reviewed. Do not raise a baseline to admit
growth.
