# Issue 862 Implementation Summary

## Changed

- Added `WorkspacePolicy` in `src/util/workspace_paths.rs`.
  - Classifies workspace paths as `UserDeliverable`, `ProtectedInput`, or `GeneratedMetadata`.
  - Centralizes ignored directories, protected input files, runtime metadata files, top-level log/eval roots, and `postcheck.*`.
  - Supports normal read denial and explicit protected-metadata read allowance for log-analysis tasks.
- Rewired workspace artifact discovery through `WorkspacePolicy`.
  - `workspace_appears_empty` and `meaningful_workspace_files` now use the shared policy.
  - `.anvil/**`, `logs/**`, `postcheck.*`, `prompt.md`, `cmd.txt`, `anvil.out`, and related metadata stay out of user deliverable detection.
- Rewired structured tool discovery through `WorkspacePolicy`.
  - `Read` rejects direct protected metadata reads by default.
  - Directory `Read` hides protected children.
  - `Glob` and `Grep` skip protected metadata/runtime paths by default.
  - `ToolContext.workspace_policy` carries the active policy, with `WorkspacePolicy::for_task_request` enabling protected reads for explicit log-analysis style requests.
- Preserved existing write semantics for generated runtime output.
  - `Write` / `Edit` still reject protected prompt/cmd inputs.
  - Generated output such as `anvil.out` can be written, but remains excluded from evidence/artifact accounting.

## Tests Added

- Tool registry tests for normal metadata discovery filtering.
- Tool registry regression test for v0.4.30-style first reads of `prompt.md` / `cmd.txt`.
- Tool registry test for explicit log-analysis protected metadata read allowance.
- Workspace policy tests for protected matching, `postcheck.*`, `.anvil/**`, `logs/**`, and request-derived allowance.
- Workspace walker test coverage expanded for `.anvil/**` and `postcheck.*`.
