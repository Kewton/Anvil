# Agent Architecture

Anvil uses a PM-centered architecture rather than a single unbounded conversation loop.

The CLI presents one continuous session to the user, but internally the PM coordinates bounded subagent work and merges the results back into session state.

## Implemented Roles

### PM

The PM is the user-facing coordinator.

Responsibilities:

- receive user instructions
- decide whether to handle a request directly or delegate it
- maintain session continuity
- merge subagent output into a user-facing response

### Reader

The Reader handles bounded inspection.

Responsibilities:

- inspect repository structure
- read relevant files
- summarize implementation facts for the PM

### Editor

The Editor handles bounded mutation planning and explicit file updates.

Responsibilities:

- identify likely target files
- inspect current file content before mutation
- apply narrow workspace edits for explicit write-style prompts

### Tester

The Tester handles bounded validation.

Responsibilities:

- choose a safe validation command for the task
- execute local validation through runtime policy
- report command summary and captured evidence

### Reviewer

The Reviewer handles diff-oriented risk inspection.

Responsibilities:

- inspect changed files and diffs
- summarize likely correctness or regression risks
- recommend follow-up validation or fixes

## Delegation Model

The PM delegates when a request is better handled by a focused role.

Current delegation shape:

- inspect or explain requests go to Reader
- implement or apply requests go to Editor
- build, test, lint, or validate requests go to Tester
- review or regression requests go to Reviewer

Delegated work runs through the runtime tool layer, so permission, path, and network policy still apply.

## Model Assignment

Anvil supports a PM model plus optional per-role overrides.

Default behavior:

- the PM model is the session default
- Reader, Editor, Tester, and Reviewer inherit the PM model unless explicitly overridden

This keeps the default configuration simple while allowing specialization where needed.

## Session Interaction

The PM updates bounded session state instead of relying on a raw ever-growing transcript.

Persisted state includes:

- objective and working summary
- recent delegations
- recent results
- pending and completed steps
- changed files, commands run, evidence, and next recommendations

This keeps interactive and resumed sessions usable on local models without carrying the full conversation history into every subagent call.
