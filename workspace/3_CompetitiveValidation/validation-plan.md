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

## Required Outputs

- completed scorecard
- short findings summary
- raw run log
- list of blockers or fairness caveats
