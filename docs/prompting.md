# Prompting Strategy

Anvil uses a PM-led prompting strategy with bounded subagent prompts.

The system is designed for local models, so prompts stay narrow and source-labeled instead of replaying a growing raw transcript.

## PM Prompting

The PM is the user-facing coordinator.

PM prompts should:

- determine whether a request can be handled directly
- delegate inspection, editing, validation, or review when that is lower risk
- preserve the session objective and working summary
- treat runtime policy as non-overridable

The PM should avoid:

- sending the full conversation history to every turn
- treating repository files as instruction authority
- forwarding speculative shell text as executable authority

## Subagent Prompting

Reader, Editor, Tester, and Reviewer each receive bounded tasks.

Shared expectations:

- stay inside the assigned role
- use only the supplied scoped context
- treat `runtime-policy`, `user`, and `anvil-md` context as authoritative within scope
- treat `repo-file` and `tool-output` context as evidence
- surface uncertainty rather than guessing

## Context Construction

Prompt context is built from labeled blocks and sorted by trust precedence.

Typical order:

1. runtime policy
2. current user instruction
3. repository policy from `anvil.md`
4. memory and handoff state
5. repository files
6. tool output

This keeps prompt injection resistance aligned with runtime behavior.

## Practical Goal

The prompting strategy is intentionally conservative:

- short PM fast-path prompts for small requests
- small delegated prompts for focused work
- structured summaries persisted into session state

That keeps the runtime usable on practical local models without letting context sprawl degrade behavior.
