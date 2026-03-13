# Phase 9 Validation Plan

## Purpose

Compare Anvil against `vibe-local` on the axes defined in `workspace/1_ProductDefinition/anvil-comparison-axes.md`.

## First Pass Scope

1. Startup and first-use flow
2. First prompt latency
3. Follow-up turn latency
4. Interrupt and recovery behavior
5. Long-session resume quality
6. UX clarity under tool-heavy turns

## Measurement Rules

- Run both tools on the same machine.
- Use the same local model whenever possible.
- Record cold-start and warm-start values separately.
- Record exact prompt text.
- Record any manual setup or retry steps.
- Label every row as either `Measured` or `OperationalScore`.
- Prefer benchmark harness runs for command-level latency where automation is possible.
- Keep scored rows explicitly separate from measured rows in findings and scorecards.

## Automation Baseline

- Use the Rust benchmark harness in `src/metrics/mod.rs` for repeated command execution timing.
- Store raw command benchmark artifacts before reducing them into scorecard rows.
- Record raw per-run timings in the run log, then write averaged values into the scorecard.
- Treat qualitative UX or maturity judgments as `OperationalScore` until a repeatable harness exists.

## Required Outputs

- completed scorecard
- short findings summary
- raw run log
- list of blockers or fairness caveats
