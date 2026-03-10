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
- Ollama defaults to `http://127.0.0.1:11434` and honors `ANVIL_OLLAMA_ENDPOINT`
- LM Studio defaults to `http://127.0.0.1:1234` and honors `ANVIL_LM_STUDIO_ENDPOINT`

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
- pending confirmation state for confirmation-gated actions
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
- `/approve`
- `/deny`
- `/exit`

## Current Limits

- LM Studio live smoke verification is opt-in rather than part of the default suite
- step lifecycle is improved but still heuristic
- approval flow is currently implemented for confirmation-gated tester exec actions
- some design notes still live under `workspace/`

## Manual Smoke Checks

### Ollama

Validated model:

- `qwen3.5:35b`

Example:

```bash
anvil -p "Reply with exactly: OK" --model qwen3.5:35b --network local-only
```

Expected shape:

- response comes from the PM fast path
- output contains `OK`

### LM Studio

Anvil expects LM Studio's OpenAI-compatible endpoint at `http://127.0.0.1:1234/v1/chat/completions`.

Example:

```bash
anvil -p "Reply with exactly: OK" --model lmstudio/<your-model-id> --network local-only
```

Expected shape:

- response comes from the PM fast path
- output contains `OK`
- the `lmstudio/` prefix is only routing metadata and is stripped before the HTTP request

If the request fails:

- confirm LM Studio local server is running
- confirm the loaded model id matches the suffix after `lmstudio/`
- confirm the server exposes the OpenAI-compatible `/v1/chat/completions` route

Optional integration test:

```bash
ANVIL_LM_STUDIO_MODEL=lmstudio/<your-model-id> \
  cargo test --test pm_and_models -- --ignored lm_studio_live_smoke_test
```
