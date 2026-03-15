# Phase 11 Plan

## Goal

Strengthen Anvil from a solid local-first prototype into a more mature coding
agent for repeated real-world use.

## Priority 1

- Planning rigor
  - execution checkpoints
  - tool-batch review before mutating runs
  - stronger typed planner events instead of ad-hoc snapshot updates

- Retrieval quality
  - semantic or symbol-aware ranking
  - retrieval-aware context handoff into provider requests
  - better large-repo navigation and narrowing flows

## Priority 2

- Provider operations
  - multi-provider configuration UX
  - clearer diagnostics for remote-compatible backends
  - stronger parity for structured tool-response handling across backends

- Operator UX polish
  - live tool-progress updates for longer tool runs
  - diff-oriented completion summaries
  - stronger timeline/session navigation

## Success Criteria

- Planning changes are explicit, reviewable, and resumable.
- Retrieval helps long-session coding tasks in medium and large repositories.
- Backend switching and diagnosis are operator-readable.
- UX remains legible even as more capability is added.

## Out of Scope for Phase 11

- Full semantic embedding infrastructure with external services
- Heavy GUI features outside the terminal-first product shape
- Cloud-first optimization work
