## Context

Follow-up of #446.

Parent follow-up:
- #498

Issue 446 evaluation showed successful README edits, but `AnvilScore.user_visible_artifact` remained `false`.

## Problem

Documentation work is a valid user-visible artifact, especially in documentation mode. If Markdown/documentation edits are not reflected in AnvilScore, later CaseMemory and evaluation logic get an incomplete view of success.

## Desired Direction

- Make file classification mode-aware.
- In documentation mode, count relevant `.md` / docs edits as user-visible artifacts.
- Avoid treating internal runtime artifacts as user-visible work.
- Keep code-mode scoring unchanged unless docs are explicitly requested.

## Acceptance Criteria

- A successful README-only task in documentation mode records `user_visible_artifact=true`.
- Documentation edits can contribute to CaseRecord success quality.
- Runtime files such as `.anvil-state` remain excluded from artifact scoring.
- Tests cover documentation mode and non-documentation mode.
