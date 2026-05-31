# Issue 834 Design Note

## Goal

Trace why an empty-workspace Node CLI task can complete the scaffold recovery path without launching a deterministic Node scaffold and without generating `package.json`.

## Approach

- Treat this as an investigation deliverable, not a scaffold implementation.
- Anchor the trace at the current deterministic scaffold chokepoints in `src/agent/loop_run/scaffold_pipeline.rs`, then follow caller gates through post-reply recovery, work-mode classification, and empty-workspace request prompting.
- Reproduce the failure with local pure checks where possible so the report names exact file:line gates instead of only narrative symptoms.
- State whether skeleton separation alone would fix the observed path.

## Expected Change Scope

- Add issue report files under `dev-reports/issue-834/`.
- Add a focused regression-style unit test only if an existing pure classifier/gate exposes the failure without requiring Ollama, Node, or network access.
