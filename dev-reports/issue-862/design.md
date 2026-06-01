# Issue 862 Design

## Goal

Introduce a shared `WorkspacePolicy` so Anvil can distinguish user deliverables from controller metadata/runtime files. The policy must keep `prompt.md`, `cmd.txt`, `anvil.out`, `.anvil/**`, and similar runtime files out of artifact evidence and normal model discovery.

## Approach

- Add `WorkspacePolicy` to `src/util/workspace_paths.rs` as the single source for:
  - ignored workspace directories such as `.git`, `.anvil`, `.anvil-state`, build caches, and dependency caches;
  - protected input files such as `prompt.md`, `prompt.txt`, `cmd.txt`, and `command.txt`;
  - generated/runtime metadata such as `anvil.out`, `anvil.err`, `postcheck.*`, session metadata, and top-level `logs/**`;
  - tool read policy: normal tasks deny protected metadata; explicitly allowed log-analysis tasks may read it.
- Keep existing public helper names as compatibility wrappers over the policy to avoid broad call-site churn.
- Update workspace discovery and artifact classification to consume `WorkspacePolicy` instead of local hard-coded lists.
- Gate `Read`, `Glob`, and `Grep` in the tool registry so protected metadata is not visible to the model during ordinary exploration. Directory reads should list only allowed children.
- Add focused regression tests for artifact exclusion, normal discovery exclusion, allowed log-analysis reads, and the v0.4.30-style `cmd.txt` / `prompt.md` first-read failure mode.

## Risks

- `ToolContext` needs one new policy field, so tests that construct it directly must be updated.
- Directory read filtering must preserve normal listing behavior for user files while hiding protected entries.
- The explicit override should be narrow: it only allows reads/discovery of protected metadata, not writes.
