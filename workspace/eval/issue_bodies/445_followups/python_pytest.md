## Context

Follow-up of #445.

Parent follow-up:
- #489

Issue 444 and Issue 445 evaluations both found the same practical false negative:

- `python -m pytest` passes.
- `python3 -m pytest` fails because the resolved `python3` does not have pytest.
- Anvil reports the run as failed even though the code fix is correct.

## Problem

`AutoTestRunner` currently detects Python tests and defaults to `python3 -m pytest`. That is too brittle for local-first environments where `python`, `python3`, virtualenv, pyenv, uv, or project-local commands may differ.

## Desired Direction

- Detect the Python verifier in a local-environment-aware order.
- Prefer an already successful Python test command from the same turn.
- Prefer `ANVIL.md` or project config when safe.
- Fall back through `python -m pytest`, `python3 -m pytest`, and other safe local commands only when needed.
- Record interpreter selection and failure reason in logs.

## Acceptance Criteria

- A workspace where `python -m pytest` works but `python3 -m pytest` lacks pytest is not marked failed solely because of the `python3` path.
- Python verifier selection is covered by unit tests.
- The Issue 445 `P0-445-01` false-negative fixture passes final verification when code is fixed.
- The fix remains generic enough to support future uv/venv/project-command integration.
