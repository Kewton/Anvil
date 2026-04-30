## Context

Follow-up of #445.

Parent follow-up:
- #489

Issue 445 evaluation showed that the model could run the correct project-local verifier, but the final runtime verifier still chose a different command and marked the task failed.

Example from evaluation:

- LLM ran `python -m pytest -q` from `ANVIL.md` and it passed.
- Final `AutoTestRunner` reran `python3 -m pytest`.
- `/Library/Developer/CommandLineTools/usr/bin/python3` did not have pytest, so the run was reported as failed.

## Problem

Final success judgement is not aligned with project-local verification instructions or successful in-turn Bash verification.

This creates false negatives: the implementation is correct, but the agent reports failure because the final verifier selected a different command.

## Desired Direction

- Prefer safe project-local verification commands from `ANVIL.md`.
- Prefer a safe verifier command that already succeeded during the same turn.
- Avoid hard-coded Python interpreter assumptions when an equivalent successful command is available.
- Keep command selection generic and safe, not Python-only.

## Acceptance Criteria

- If `ANVIL.md` specifies a safe verifier such as `python -m pytest -q`, final verification uses it or records why it was rejected.
- If a safe Bash verification command succeeded after repo edits, final success judgement can reuse or honor that result.
- The Issue 445 scenario `P0-445-02` no longer fails due to `python3 -m pytest` when `python -m pytest -q` passed.
- Structured logs record the chosen final verifier source: `anvil_md`, `successful_bash`, `autodetect`, or `fallback`.
