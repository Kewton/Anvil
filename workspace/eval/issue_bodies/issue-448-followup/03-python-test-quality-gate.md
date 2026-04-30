## Summary

Python quality gating is too blunt when an existing test file already exists.

Issue 448 additional Python E2E made the correct source edit after reading the existing test, but failed because the runtime required a new test artifact.

## Problem

The policy currently treats "test requested" as "add a new Python test artifact". That is wrong when the user explicitly points to an existing test file or the repo already has relevant tests.

The correct quality condition should be:

- read or identify an existing relevant test, then run it; or
- add a test only when no relevant test exists; or
- run a small self-test only when the project has no test framework available.

## Acceptance Criteria

- If a relevant existing `test_*.py` or `*_test.py` file is read and the source is edited, the quality gate should require test execution, not a new test file.
- If `python3 -m pytest` is unavailable, try the project-configured Python command or a self-test fallback before failing.
- Track whether verification used existing tests, newly added tests, or self-test.
- Avoid false `missing_repo_edits` when the implementation edit is already present and the existing test file was used.

## Evidence

- Additional Issue 448 Python run: `raw/e2e-python-graph-qwen36.log`
- Source was correctly changed from `return a + b` to `return a * b`.
- Local `python3 -m pytest` failed because pytest was not installed in that interpreter.
