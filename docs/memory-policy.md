# Memory Policy

## Scope

This document defines the intended policy for optional persistent user memory stored as `anvil-memory.md`.

## Purpose

`anvil-memory.md` is for stable, user-specific guidance that improves future collaboration.

Good examples:

- response style preferences
- execution preferences
- repeated corrections
- durable workflow preferences

It is not a transcript log.

## Core Rules

- record stable preferences, not one-off requests
- summarize, do not transcribe
- prefer explicit user feedback over inference
- do not store secrets or sensitive content
- treat memory as lower-priority than current user instructions, runtime policy, and `anvil.md`
- prefer out-of-repository storage by default
- allow stale items to be revised or removed

## What Belongs In Memory

- concise vs detailed answer preference
- findings-first review preference
- preference for minimal diffs
- preference to run tests when feasible
- preference for local-first or specific model providers
- repeated “do this / don’t do this” corrections

## What Does Not Belong In Memory

- API keys or credentials
- raw confidential content
- one-off tasks
- temporary debugging notes
- repository-specific coding rules
- runtime authority such as “network is always allowed”

## Update Policy

Memory should only be updated when:

1. the user states a persistent preference
2. the user repeats the same correction
3. the user rejects a recurring behavior and provides a stable alternative
4. newer explicit guidance supersedes older memory

## Suggested Shape

```md
# anvil-memory.md

## Response Preferences

- Prefer concise answers
- Avoid conversational acknowledgements

## Execution Preferences

- Prefer minimal diffs
- Run relevant tests when feasible

## Workflow Preferences

- Prefer implementation over extended planning when the request is actionable
```
