## Summary

Follow-up for Issue 448 / Epic E evaluation.

Issue 448 introduced RepoGraph, graph-aware RepoContext, and path-aware ANVIL.md. The foundation works, especially when explicit paths are present, but additional E2E showed that generic discovery, test pairing, Python quality gates, mode classification, and telemetry hygiene still need follow-up work.

## Child Issues

- [ ] #527
  Generic project-level candidate discovery is too weak.
- [ ] #531
  Test/implementation pair ranking should be stronger.
- [ ] #530
  Python test quality gate should accept existing tests.
- [ ] #528
  Mode/protocol classification should not override docs/path-scoped tasks.
- [ ] #529
  Changed-file telemetry and runtime artifact hygiene need cleanup.

## Evaluation Evidence

- Run directory: `workspace/eval/runs/issue-448/20260430-070803`
- Report: `workspace/eval/runs/issue-448/20260430-070803/report.md`
- Tracking: `workspace/eval/tracking/summary.md`

## Desired Outcome

After these child issues are completed, rerun the Issue 448 evaluation scenarios and confirm:

- explicit-path tasks still pass;
- generic Node/Rust/Python test requests produce useful candidates;
- path-scoped ANVIL.md works for both source and docs edits;
- Python existing-test tasks do not fail with false `missing_repo_edits`;
- changed-file telemetry separates user edits from runtime artifacts.
