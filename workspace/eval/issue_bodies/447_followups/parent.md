## Context

Follow-up umbrella for gaps found in the Issue 447 / Epic D evaluation.

Evaluation report:
- `workspace/eval/runs/issue-447/20260429-180454/report.md`

Related issue:
- #447

## Summary

Issue 447 successfully introduced the AgentSkill trait, SkillRegistry, VerifierSkill production dispatch, and SkillTrustTier permission enforcement.

The evaluation confirmed that VerifierSkill is real in production: successful edit turns emit `agent.verifier.completed`, run AutoTest, and compute AnvilScore. In the repeated P4-02 check, `qwen3.5:122b` passed 3/3 and `qwen3.6:27b-coding-nvfp4` passed 2/3.

However, the skill layer is still partial. Reminder/Precaution is still production-dispatched through the legacy direct path, Tester and CaseMemory components remain outside SkillRegistry, and the actor loop still has a no-edit failure mode where the model describes a needed edit but does not call Edit/Write.

## Follow-up Scope

- Route production Reminder/Precaution through SkillRegistry.
- Migrate remaining internal harness components into permissioned skills.
- Strengthen no-edit retry handling for prose-only "I will edit" responses.
- Add a production-facing permission-denied scenario once a suitable skill path exists.
- Track edited-file telemetry cleanup through existing #491 rather than duplicating it here.

## Child Issues

- #510 Route production Reminder and Precaution through SkillRegistry
- #511 Migrate internal harness components into permissioned skills
- #512 Strengthen no-edit retry handling for prose-only edit responses
- #513 Add live permission-denied scenario for SkillTrustTier

## Related Existing Follow-ups

- #491 Filter runtime and verifier artifacts from edited-file telemetry

## Acceptance Criteria

- Child issues are created for concrete implementation gaps.
- Each child issue references this umbrella and #447.
- Future Issue 448-449 evaluations can verify whether the SkillRegistry becomes the unified internal harness layer, not only a Verifier wrapper.
