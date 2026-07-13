# Development Guardrails

## Runner Growth Tripwire

`src/planner/runner.rs` and `src/minimal_loop/loop_run.rs` are interim group-3
chokepoints. The extracted planner and minimal-loop leaf modules also have
file-level growth budgets so they do not become replacement chokepoints.

The CI/test guard records current baselines and fails if any file grows
above baseline +2%:

- `src/planner/runner.rs`: 18,242 lines
- `src/minimal_loop/loop_run.rs`: 7,444 lines
- `src/minimal_loop/repair_pressure.rs`: 746 lines
- `src/planner/repair_targeting.rs`: 597 lines
- `src/planner/final_acceptance.rs`: 2,942 lines
- `src/planner/ultra_plan_flow.rs`: 1,570 lines
- `src/planner/assurance.rs`: 1,311 lines
- `src/planner/profiles/nextjs.rs`: 3,684 lines
- `src/minimal_loop/evidence.rs`: 6,702 lines

The same guard also measures production code and `#[cfg(test)]` code
separately. The total baseline above remains enforced; these split baselines
prevent test growth from hiding production growth, or production shrink from
masking test bloat:

| file | production baseline | test baseline |
| --- | ---: | ---: |
| `src/planner/runner.rs` | 9,904 | 8,339 |
| `src/minimal_loop/loop_run.rs` | 4,960 | 2,485 |
| `src/minimal_loop/repair_pressure.rs` | 278 | 468 |
| `src/planner/repair_targeting.rs` | 459 | 138 |
| `src/planner/final_acceptance.rs` | 2,937 | 5 |
| `src/planner/ultra_plan_flow.rs` | 1,570 | 0 |
| `src/planner/assurance.rs` | 1,305 | 6 |
| `src/planner/profiles/nextjs.rs` | 2,361 | 1,323 |
| `src/minimal_loop/evidence.rs` | 4,088 | 2,694 |

When adding behavior, put new subsystems in new modules and call them from the
runner. Refactors that shrink these files are allowed; lower the baseline only
after the shrink is intentional and reviewed. Do not raise a baseline to admit
growth.

Declarative Next.js and evidence knowledge must be changed in
`src/planner/profiles/nextjs/knowledge.toml` and
`src/minimal_loop/evidence_knowledge.toml`, respectively. Before adopting a
knowledge change, run and record the Space, Breakout, and Quiz scenario matrix
so vocabulary or contract drift is measured independently from Rust logic.
