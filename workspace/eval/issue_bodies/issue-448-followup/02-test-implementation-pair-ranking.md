## Summary

RepoContext should rank implementation and test files as a pair when the task is test-driven.

Issue 448 showed that RepoGraph can select the implementation file, but may miss the adjacent test file in the initial context.

## Problem

In the Rust E2E run, RepoContext ranked `src/pricing.rs`, but not `tests/pricing_test.rs`, even though the user asked to make the pricing tests pass. The model later found the test via `Glob`, but this should be available in the initial context.

## Acceptance Criteria

- When a test file matches a task term, rank the corresponding implementation file and the test file together.
- When an implementation file matches a task term and a nearby test file exists, include the test file as a graph neighbor.
- Emit enough telemetry to identify whether a file was selected by lexical match, script discovery, import edge, or test/implementation pairing.
- Add E2E coverage where the model fixes a failing test without needing a broad `Glob` after the first prompt.

## Evidence

- Issue 448 Rust qwen3.6 R2 passed, but initial ranking selected implementation only.
- Report: `workspace/eval/runs/issue-448/20260430-070803/report.md`
