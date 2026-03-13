# Competitive Validation

This directory tracks Anvil vs `vibe-local` comparison work.

Contents:

- `validation-plan.md`
- `scorecard-template.md`
- `run-log.md`

Use this directory to record actual benchmark runs, not design assumptions.

Rules:

- mark each result as `Measured` or `OperationalScore`
- keep raw measured runs in the run log
- use the scorecard to summarize winners without hiding evidence type
- prefer benchmark artifacts generated from `src/metrics/mod.rs` for command-level latency checks
