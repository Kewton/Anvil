# anvil-memory.md Auto-Recording Policy

## Purpose

`anvil-memory.md` is an optional persistent memory artifact maintained by Anvil when memory is enabled.

Its purpose is to help Anvil improve collaboration over time by remembering stable, user-specific guidance such as:

- repeated corrections
- preferred response style
- workflow preferences
- recurring complaints
- explicit "do this / don't do this" instructions from the user

It is not a raw chat log and should not become a transcript dump.

---

## Design Goals

- Preserve only high-signal, reusable guidance
- Avoid storing sensitive or unnecessary content
- Improve future task execution and answer quality
- Keep the file small, reviewable, and easy to prune
- Separate user memory from repository instructions in `anvil.md`
- avoid turning repository-local state into a hidden persistence channel

---

## Principles

### 1. Record stable preferences, not transient conversation

Only store information likely to remain useful across future interactions.

Good examples:

- user prefers concise answers
- user dislikes nested bullets
- user wants tests run before completion when possible
- user prefers non-interactive Git commands

Bad examples:

- today's one-off request text
- temporary debugging notes
- every individual file touched in a past session

### 2. Summarize, do not transcribe

Entries should be short summaries of learned guidance.
Do not paste long dialogue excerpts into memory.

### 3. Bias toward explicit user feedback

Prefer recording items that the user directly stated or repeatedly corrected.
Do not over-infer personality traits from a single message.

### 4. Avoid sensitive retention

Do not store secrets, tokens, personal data, or confidential content unless the product explicitly supports secure storage and the user has clearly opted in.

### 5. Treat memory as lower-priority state, not authority

Memory is useful for continuity, but it must not override:

- runtime safety policy
- explicit user instructions in the current session
- repository policy from `anvil.md`

### 6. Prefer out-of-repository storage by default

Memory should live outside the repository unless the user explicitly requests repository-local storage.
This reduces accidental commits, leakage through Git history, and confusion between repository policy and user-specific state.

### 7. Keep memory editable and replaceable

Anvil should be able to revise or remove stale guidance when newer user feedback supersedes it.

---

## What to Record

Record information that is:

- user-specific
- reusable
- operationally useful
- likely to remain stable

### Recommended categories

#### Response Preferences

- prefers concise vs detailed responses
- prefers prose vs bullets
- wants findings first in reviews
- wants direct, non-fluffy tone

#### Execution Preferences

- prefers tests to be run whenever feasible
- prefers minimal diffs
- prefers not to modify unrelated files
- prefers local-first approaches over cloud dependence

#### Workflow Preferences

- prefers implementation over brainstorming by default
- prefers using certain tools or commands first
- prefers specific output formats for plans, reviews, or summaries

#### Repeated Corrections

- "do not start answers with acknowledgements"
- "include full file paths when asked"
- "avoid overly verbose explanations"

#### Stable Environment Preferences

- prefers a specific shell or package manager when alternatives exist
- prefers specific model providers such as Ollama or LM Studio
- prefers local execution or offline-capable flows
- prefers a specific permission mode or disallows network by default

---

## What Not to Record

Do not record the following by default.

### Sensitive Data

- API keys
- access tokens
- passwords
- personal addresses
- private credentials
- confidential repository content copied verbatim
- raw command output containing secrets or credentials

### High-Churn or Low-Signal Details

- one-off tasks
- temporary TODOs
- ephemeral bug reports
- specific commands from every session
- long excerpts of conversation

### Weak Inferences

- guessed personality traits
- guessed business strategy
- assumptions based on a single interaction
- speculative preferences not stated by the user

### Repository-Specific Rules

Do not store repository instructions in `anvil-memory.md` if they belong in `anvil.md`.

Examples:

- coding style for one repository
- repository test commands
- file layout rules for one project

Those should go into `anvil.md`.

### Runtime Policy

Do not store runtime-granted authority as if it were a user preference.

Examples:

- "network is always allowed"
- "destructive commands can run without confirmation"
- "full-access should be assumed for this machine"

---

## Update Rules

Anvil should update memory only when at least one of these conditions is met:

1. The user explicitly states a persistent preference
2. The user repeats the same correction more than once
3. The user rejects a pattern of behavior and indicates a stable alternative
4. A new preference clearly supersedes an older one

Anvil should avoid writing memory for every conversation.
In the MVP, auto-write should be conservative or opt-in.

---

## Revision Rules

When new information conflicts with old memory:

- prefer newer explicit user guidance
- update the existing entry rather than duplicating it
- remove stale or contradictory items

The file should behave like a curated preference summary, not an append-only log.

---

## Suggested File Structure

```md
# anvil-memory.md

## Response Preferences

- Prefer concise answers by default
- Avoid conversational acknowledgements at the start

## Execution Preferences

- Prefer minimal diffs
- Run relevant tests when feasible
- Prefer `read-only` for inspection tasks
- Keep network disabled unless explicitly needed

## Workflow Preferences

- Prefer implementation over extended planning when the request is actionable

## Explicit Avoidances

- Do not use destructive Git commands without explicit approval
- Do not overwrite unrelated user changes

## Last Reviewed

- 2026-03-10
```

---

## Suggested Auto-Write Policy

Anvil should not immediately write every new preference as soon as it appears.

Safer policy:

- collect candidate memory items during a session
- validate them against the rules above
- write only high-confidence items at the end of the task or session
- refuse writes that contain likely secrets, repository-specific policy, or transient command output

This reduces noise and overfitting.

---

## Suggested Confidence Levels

Anvil may internally classify candidate memory like this:

- `high`
  - explicit user preference
  - repeated correction
  - stable instruction format
- `medium`
  - strong implied preference seen multiple times
- `low`
  - weak inference from a single interaction

Only `high` confidence items should be auto-written by default.

---

## Examples

### Good memory entries

- User prefers concise answers unless they ask for depth
- User wants file paths as full absolute paths when requested
- User dislikes promotional or overly enthusiastic phrasing
- User prefers implementation over planning when the request is concrete
- User prefers network-disabled sessions unless they explicitly opt in

### Bad memory entries

- User asked about README copy on March 10
- User edited file X in one session
- User might be interested in Rust
- Entire conversation transcript pasted into memory
- Run `npm install` automatically whenever tests fail

---

## Operational Recommendation

`anvil-memory.md` should be:

- stored outside the repository by default
- automatically created by Anvil only when memory is enabled
- automatically updated only for high-confidence stable guidance
- easy for the user to inspect
- easy for the user to reset or edit manually

The goal is better long-term collaboration, not silent accumulation of noisy data.
