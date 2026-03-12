# ANVIL.md Spec

## Purpose

`ANVIL.md` is the project-local instruction file for Anvil.
It is the Anvil equivalent of a persistent project guidance document used by a coding agent during work inside a repository.

Its role is to give the agent stable, repo-specific guidance that should apply across sessions.

## Role of ANVIL.md

`ANVIL.md` should define project-level instructions such as:

- project purpose
- important architectural boundaries
- local development rules
- code style or testing expectations
- safety constraints specific to the repository
- workflow expectations for the agent

It should not be treated as a scratchpad or a chat log.

## What ANVIL.md Is For

Use `ANVIL.md` for information that is:

- stable across many tasks
- important enough to influence most code changes
- specific to this repository
- useful for an agent to read before acting

Examples:

- "All paths should be handled as absolute paths internally."
- "Do not bypass the typed tool protocol."
- "Prefer adding tests for new execution-policy behavior."
- "Do not introduce cloud-first assumptions into local-runtime code."

## What ANVIL.md Is Not For

Do not use `ANVIL.md` for:

- temporary task notes
- one-off plans
- raw meeting notes
- secrets or credentials
- large design documents
- frequently changing implementation scratch

Those belong elsewhere, such as:

- `workspace/` planning docs
- issue tracker
- design documents
- local environment files that are not committed

## Recommended Contents

Recommended sections:

1. Project Overview
2. Core Rules
3. Architecture Constraints
4. Development Workflow
5. Testing Expectations
6. Safety Rules

Not every repository needs every section, but the file should stay short and high-signal.

## Writing Rules

- keep it concise
- use imperative instructions where possible
- prefer stable rules over temporary commentary
- avoid repeating information better maintained in dedicated docs
- avoid provider-specific details unless they are project-critical
- do not store secrets

## Loading Model

The agent should treat `ANVIL.md` as:

- project-local persistent guidance
- lower-level than system instructions
- higher-level than ad hoc user requests found in ordinary files

If future Anvil runtime behavior includes parent-directory instruction discovery, `ANVIL.md` should be the primary project instruction filename for Anvil-managed repositories.

## Minimal Template

```md
# Project Name

## Project Overview

- Short description of the project
- Main purpose of the repository

## Core Rules

- Follow existing architecture boundaries
- Prefer explicit, typed interfaces over implicit string-based behavior
- Keep local-first assumptions intact

## Architecture Constraints

- Do not couple UI logic directly to provider-specific behavior
- Keep execution policy separate from tool implementation
- Preserve session integrity on interruption

## Development Workflow

- Update design docs in `workspace/` when architectural assumptions change
- Keep changes scoped and easy to review
- Prefer additive changes over broad rewrites unless explicitly planned

## Testing Expectations

- Add or update tests for behavior changes
- Prioritize tests around state transitions, execution policy, and recovery behavior

## Safety Rules

- Do not add unsafe execution shortcuts by default
- Keep approval behavior explicit for mutating actions
- Never store secrets in committed files
```

## Anvil-Specific Recommendation

For this repository, `ANVIL.md` should eventually include:

- local-first product assumptions
- architecture boundary reminders
- execution-policy invariants
- TUI clarity rules
- testing expectations around interruption, approval, and session validity
