## Context

Follow-up of #446.

Parent follow-up:
- #498

Issue 446 evaluation showed that qwen3.5 received `Relevant Local Cases` and completed faster, but the edit quality regressed:

- It duplicated the README `Usage` section.
- The run was faster, but not high quality.

## Problem

CaseMemory currently treats a completed turn as useful memory even when the resulting edit may be structurally messy. This can optimize speed while preserving or amplifying bad edit patterns.

## Desired Direction

- Store or derive a lightweight quality signal for CaseRecords.
- Prefer cases that are both successful and clean.
- Penalize cases associated with duplicate sections, messy docs, failed final verification, or later correction.
- Keep the record compact and local-LLM friendly.

## Acceptance Criteria

- CaseRecord selection can prefer high-quality cases over merely completed cases.
- README duplicate-section outcomes are not promoted as strong positive examples.
- Retrieval prompt includes only compact, actionable details.
- Evaluation can distinguish speed improvement from quality improvement.
