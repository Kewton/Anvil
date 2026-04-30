## Context

Follow-up of #446.

Parent follow-up:
- #498

Issue 446 evaluation found that logs and CaseRecord tests mask secret-like values, but raw `session.json` still retains the original user message.

This may be acceptable for local replay, but it is risky for future evaluation logs, A/B reports, or training dataset export.

## Problem

There is no clearly separated safe export path for session data that may contain raw user input. CaseRecord is compact and masked, but future observability/export features may accidentally consume raw session storage.

## Desired Direction

- Keep raw local session storage if needed for replay.
- Add an explicit export-safe view that masks or excludes raw user messages.
- Ensure evaluation/training export never reads raw secret-bearing fields without redaction.
- Reuse existing secret masking utilities.

## Acceptance Criteria

- Secret-like values in raw user messages are redacted in any eval/export artifact.
- CaseRecord and AntiPattern export paths remain compact and masked.
- Tests cover user message, tool args, command text, stderr/stdout, and nested JSON payloads.
- Documentation states the distinction between raw local session storage and export-safe data.
