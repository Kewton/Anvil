## Context

Follow-up of #447.

Parent follow-up:
- TBD

Issue 447 added SkillTrustTier and focused tests for `agent.skill.permission_denied`, including ExternalDisabled denial, Plan-mode write denial, Verifier bypass, and cap behavior.

## Problem

Permission enforcement is currently well covered by unit/integration tests, but practical production exposure is limited because most internal skills are still built-in direct paths rather than registry-dispatched skills.

The evaluation could not run a meaningful live production scenario where a real skill attempts a denied capability and the actor loop survives.

## Desired Direction

- Add a production-facing or E2E-testable skill path that can intentionally trigger permission denial without unsafe side effects.
- Keep denied execution from reaching the skill body.
- Emit structured `agent.skill.permission_denied` with skill name, tier, requested capability, and reason.
- Ensure denial does not consume a per-turn cap unless explicitly intended.

## Acceptance Criteria

- There is at least one live E2E or integration-style scenario that observes `agent.skill.permission_denied`.
- The actor loop survives and records a recoverable FeedbackFrame or equivalent runtime event.
- Denied skills do not write repo files, run Bash, or mutate state outside the allowed scope.
- The scenario can be reused in future Issue 448-449 evaluation runs.

