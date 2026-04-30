## Context

Follow-up of #447.

Parent follow-up:
- TBD

Issue 447 follow-up testing found a qwen3.6 no-edit convergence failure in a tiny existing-file edit task.

Run:
- `workspace/eval/runs/issue-447/20260429-180454/raw/e2e-verifier-python-qwen36-r2.log`

## Problem

The model read `app.py`, correctly described the required change, but then emitted prose-only responses such as "I need to change..." and never called Edit. Existing retry prompts did not force a tool call, and the turn ended as `missing_repo_edits`.

This is not a VerifierSkill bug. It is an actor-loop enforcement gap when the model describes a concrete edit without executing it.

## Desired Direction

- Detect prose-only responses after Read when the task requires repo edits.
- Add a stronger next-turn protocol that forces one of: Edit, Write, or explicit impossible reason.
- Consider qwen-family-specific small-edit protocol reinforcement when a concrete old/new string is implied.
- Keep the behavior generic and not tied to Python, TypeScript, or one framework.

## Acceptance Criteria

- A repeated small existing-file edit scenario passes 3/3 for qwen3.6.
- When the model emits "I will edit" prose without a tool call, the next prompt explicitly requires an Edit/Write call.
- The retry path does not create unrelated files or deterministic framework fallbacks.
- Logs make this failure mode countable as a distinct no-edit/prose-only retry condition.

