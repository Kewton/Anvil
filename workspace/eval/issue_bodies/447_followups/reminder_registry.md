## Context

Follow-up of #447.

Parent follow-up:
- TBD

Issue 447 evaluation showed that `ReminderSkill` exists as an adapter and has smoke-test coverage, but production reminder dispatch still uses the legacy `Agent::maybe_invoke_reminder` direct-call path.

Evidence:
- `workspace/eval/runs/issue-447/20260429-180454/report.md`
- `workspace/eval/runs/issue-447/20260429-180454/analyzed/p4_event_summary.md`

## Problem

Strict P4-01 expected Reminder/Precaution to be invoked through the SkillRegistry. In the practical runs, Reminder emitted `agent.reminder.failed` / `agent.reminder.skipped`, but no `agent.skill.*` registry event appeared.

This keeps Reminder outside the same observability and permission boundary as VerifierSkill.

## Desired Direction

- Register ReminderSkill in the production SkillRegistry path.
- Replace or shrink the direct `Agent::maybe_invoke_reminder` dispatch path.
- Preserve existing ReminderGate behavior, per-turn cap behavior, and current `agent.reminder.*` event compatibility where needed.
- Ensure Reminder failure remains recoverable and does not crash the actor loop.

## Acceptance Criteria

- A failure-driven reminder run emits a registry-backed skill invocation event or an equivalent SkillRegistry event path.
- Existing Reminder unit and integration tests still pass.
- P4-01 can be verified by a live E2E run, not only smoke tests.
- No duplicate reminder invocation occurs in one turn.

