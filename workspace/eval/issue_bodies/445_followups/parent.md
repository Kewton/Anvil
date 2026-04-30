## Context

Follow-up umbrella for gaps found in the Issue 445 / Epic B evaluation.

Evaluation report:
- `workspace/eval/runs/issue-445/20260429-012433/report.md`

Related issue:
- #445

## Summary

Issue 445 was valuable as a verification and temporary-testing foundation, but the evaluation found several practical gaps that should be tracked explicitly so they are not lost in Issues 446-449.

## Follow-up Scope

- Connect project-local verification guidance and successful in-turn verification to final AutoTestRunner success criteria.
- Remove Python verifier false negatives caused by hard-coded `python3 -m pytest`.
- Clean changed-file telemetry so runtime/cache artifacts do not count as user repo edits.
- Ensure qwen3.5 focused edit recovery and no-test Tester Skill paths are covered by later work.

## Child Issues

- #492 Align final verifier with ANVIL.md and successful in-turn verification
- #490 Fix Python AutoTestRunner false negatives from hard-coded python3 pytest
- #491 Filter runtime and verifier artifacts from edited-file telemetry

## Acceptance Criteria

- Child issues are created for each concrete implementation gap.
- Each child issue references this umbrella and #445.
- Future Issue 446-449 evaluations can reference this umbrella to check whether the gaps were closed.
