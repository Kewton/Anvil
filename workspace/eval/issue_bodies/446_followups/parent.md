## Context

Follow-up umbrella for gaps found in the Issue 446 / Epic C evaluation.

Evaluation report:
- `workspace/eval/runs/issue-446/20260429-115907/report.md`

Related issues:
- #446
- #489 / #491 for earlier telemetry follow-up context

## Summary

Issue 446 successfully introduced Case Memory and CBR primitives: successful turns create CaseRecords, similar future turns can retrieve `Relevant Local Cases`, and repeated failures can become `Avoid Patterns`.

The evaluation also showed that memory is not yet a quality-improvement engine by itself. It improves context and sometimes speed, but still needs better retrieval scoring, quality filtering, documentation-aware scoring, secret-safe export handling, and stronger anti-pattern enforcement.

## Follow-up Scope

- Tune CaseRetrieval so related tasks are recalled without over-injecting noise.
- Make CaseRecord reuse quality-aware, not only success-aware.
- Treat documentation edits as user-visible artifacts in documentation mode.
- Ensure raw session data is safe for future evaluation/training export.
- Connect repeated AntiPatterns to stronger action gating, not only prompt text.
- Track existing edited-file telemetry cleanup through #491 rather than duplicating it here.

## Child Issues

- #500 Tune CaseRetrieval scoring for related but non-identical tasks
- #503 Make CaseMemory quality-aware, not only completion-aware
- #501 Treat documentation edits as user-visible artifacts in AnvilScore
- #499 Add export-safe redaction for raw session data
- #502 Connect AntiPattern memory to runtime action gating

## Related Existing Follow-ups

- #491 Filter runtime and verifier artifacts from edited-file telemetry

## Acceptance Criteria

- Child issues are created for concrete implementation gaps.
- Each child issue references this umbrella and #446.
- Future Issue 447-449 evaluations can verify whether CaseMemory improves quality, not only recall.
