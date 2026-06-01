## Implementation Summary

- Added `TaskKind` for `coding`, `docs`, `data`, `research`, and `ops`.
- Added explicit `TaskDeliverable` records with deliverable kind, optional
  artifact role, optional path, and documentation required sections.
- Stored `task_kind` in `CompletionPolicy` so policy records the generic task
  class alongside the existing completion project intent.
- Updated `TaskContract::from_request` to derive generic task kind and
  deliverables while preserving the existing role-based completion gates for
  coding tasks.
- Added focused unit tests for representative generic prompts, coding test
  obligations, and docs-only README section obligations.
- Updated two test-only manual `TaskContract` constructors to populate the new
  explicit fields.
