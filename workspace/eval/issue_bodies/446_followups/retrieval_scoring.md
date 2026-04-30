## Context

Follow-up of #446.

Parent follow-up:
- #498

Issue 446 evaluation confirmed that CaseRetrieval works for near-identical tasks, but a related task was skipped:

- Prior case: add README Usage for Widget CLI.
- Later task: add README Troubleshooting related to Widget CLI Usage.
- Result: candidate_count=1 but skipped as `below_threshold`.

## Problem

The retrieval threshold/scoring is conservative enough that useful related cases can be missed. This limits practical benefit in real workflows, where follow-up tasks are usually adjacent rather than identical.

## Desired Direction

- Revisit scoring weights for task text, touched files, repo fingerprint, and work mode.
- Give stronger weight to same target file plus same domain words.
- Keep safeguards against noisy or stale cases.
- Log enough score detail to explain why a useful case was skipped.

## Acceptance Criteria

- A related README task for the same target file and same domain can retrieve at least one useful prior case.
- Near-identical tasks still retrieve as before.
- Irrelevant cases remain below threshold.
- Tests cover at least: exact match, adjacent task, same file/different domain, different file/same words.
