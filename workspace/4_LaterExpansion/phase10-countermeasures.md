# Phase 10 Countermeasures

This document turns the latest Phase 10 review findings into concrete follow-up
actions.

## 1. Plan Mutation Should Become Typed Events

Current state:

- plan editing works through snapshot replacement
- this is practical, but less explicit than the architecture's event-driven model

Countermeasure:

- add typed internal events for plan mutation
  - `PlanItemAdded`
  - `PlanFocusChanged`
  - `PlanCleared`
- route plan changes through a small planner subsystem instead of direct snapshot replacement
- keep the TUI dependent on `AppStateSnapshot`, but derive that snapshot from planner events

Implementation order:

1. add planner event types
2. add a planner reducer that updates plan state
3. replace direct snapshot mutation in `App` with planner event application
4. add tests that assert event -> snapshot projection

Expected effect:

- better alignment with the architecture document
- easier future support for checkpoints and batched execution review
- less implicit mutation in `App`

## 2. OpenAI-Compatible Backend Needs Better Parity

Current state:

- OpenAI-compatible backend exists
- it shares the provider contract
- it is still closer to a simple one-shot path than the Ollama path

Countermeasure:

- add backend parity targets instead of treating all providers as equal by default
- define the minimum parity set:
  - token streaming
  - error normalization
  - interrupt/cancel mapping
  - structured tool-response handling
- keep Ollama as the reference backend and measure parity against it

Implementation order:

1. add streaming response parsing for OpenAI-compatible SSE responses
2. normalize backend-specific error bodies into typed provider errors
3. verify structured tool-response flow on OpenAI-compatible output
4. add parity checks in provider integration tests

Expected effect:

- cleaner provider-layer expansion
- lower risk when adding LM Studio or similar backends
- more truthful capability claims in validation

## 3. Large-Repo Retrieval Needs a Dedicated Subsystem

Current state:

- long-context handoff exists
- retrieval and compaction are still missing

Countermeasure:

- create a dedicated retrieval subsystem instead of embedding repository search in the agent loop
- separate three concerns:
  - indexing
  - query-time retrieval
  - session compaction / summary snapshots

Recommended architecture:

- `retrieval/index`
  - file metadata
  - lightweight symbol or path index
  - content chunk records
- `retrieval/query`
  - path-aware search
  - recency-aware ranking
  - result shaping for provider handoff
- `session/compaction`
  - long-session summaries
  - pinned facts
  - prior-tool-result summaries

Implementation order:

1. add repository file inventory and indexed file list
2. add path/name/content hybrid retrieval for large repos
3. add retrieval result rendering in the operator console
4. add session compaction snapshots tied to context budget
5. add benchmark scenarios against the comparison axes

Expected effect:

- direct progress against the largest remaining competitive gap versus `vibe-local`
- better large-repo responsiveness without overloading the provider
- stronger Phase 9 validation data

## 4. Advanced UX Should Protect Core Clarity

Current state:

- operator console clarity is strong
- richer UX is still mostly open-ended

Countermeasure:

- treat advanced UX as controlled additions, not a style expansion pass
- require every new UI feature to preserve:
  - actor separation
  - state legibility
  - plan visibility
  - low-noise status area

Recommended additions:

- session timeline view
- focused tool-progress panel
- diff-oriented result summaries
- compact retrieval-result previews

Guardrails:

- no new UI region without a clear ownership rule
- no duplication between footer, answer body, and tool logs
- no feature that hides current state or pending approval status

Implementation order:

1. add session timeline slash command
2. add focused tool-progress rendering for multi-tool turns
3. add diff-oriented completion summaries
4. run UX regression checks against the comparison axes

Expected effect:

- richer operator experience without drifting into clutter
- easier comparison against Claude Code-level UX

## Recommended Next Priority

The best next major slice is:

1. large-repo retrieval

Reason:

- it is the biggest remaining competitive weakness
- it improves both usability and validation scores
- it provides the missing substrate for long-session quality

After that:

2. planner events and execution checkpoints
3. backend parity improvements
4. advanced UX layers
