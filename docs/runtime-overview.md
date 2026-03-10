# Runtime Overview

## Scope

This document describes the runtime surface that is implemented today.

## Main Flow

Anvil runs through a PM-centered loop.

- small requests can go through the PM fast path
- code-oriented requests are delegated to Reader, Editor, Tester, or Reviewer
- delegated work executes through permission-checked runtime tools
- session state is persisted after prompt turns

## Implemented Model Routing

- Ollama handles default model names
- LM Studio handles models prefixed with `lmstudio/`
- PM and subagents can inherit the PM model or use explicit per-role overrides

## Implemented Tooling

- file read
- file write
- repository search
- command execution
- diff inspection
- environment inspection

All tool calls pass through the runtime permission layer.

## Permission Model

Permission modes:

- `read-only`
- `workspace-write`
- `full-access`

Network modes:

- `disabled`
- `local-only`
- `enabled-with-approval`

Current MVP behavior:

- reads, search, env inspection, and diff are allowed
- writes are blocked in `read-only`
- local validation commands require at least `workspace-write`
- networked and destructive commands remain confirmation-gated

## Trust Model

- user input is highest-priority instruction input
- `anvil.md` is loaded as repository policy context
- repository files and tool output are treated as lower-trust evidence
- prompt context is rendered in trust order before model execution

## Session Model

Persisted session state includes:

- objective and working summary
- recent delegations
- recent results
- pending and completed steps
- changed files, commands run, and evidence for recent delegated turns

Supported state actions:

- create session with `anvil`
- execute single prompt with `anvil -p`
- resume session with `anvil resume <session-id>`
- export/import handoff files

## Interactive CLI

Interactive mode supports multi-turn stdin-driven sessions for both fresh and resumed sessions.

Slash commands:

- `/help`
- `/status`
- `/snapshot`
- `/models`
- `/history`
- `/exit`

## Current Limits

- LM Studio live smoke verification is not yet automated
- step lifecycle is improved but still heuristic
- end-to-end coverage is still small and fixture-based
- documentation is still being promoted out of `workspace/`
