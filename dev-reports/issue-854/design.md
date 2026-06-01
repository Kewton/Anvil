# Issue 854 Design

## Goal

Make workspace file classification generic for local-first agent deliverables: user-facing artifacts should be distinguished from protected input metadata and generated controller logs before any scaffold, traversal, completion-evidence, or edited-file summary logic consumes paths.

## Approach

- Rename the workspace file class from `ProjectArtifact` to `UserDeliverable` so the SSOT is not coding-project-specific.
- Extend generated metadata classification to top-level Anvil run outputs: `anvil.out`, `anvil.err`, `postcheck.out`, and `postcheck.err`.
- Reuse the workspace classifier in repo-edit observation, so generated metadata/log files cannot enter completion evidence, `turn_edited_relative_paths`, safe-stop edited summaries, or the artifact ledger.
- Keep `ProtectedInput` semantics separate: `prompt.md`, `prompt.txt`, `cmd.txt`, and `command.txt` remain readable metadata but are rejected for `Write`/`Edit` unless a future explicit override is introduced.
- Add focused regression coverage for empty-workspace scaffold decisions with `prompt.md`, `cmd.txt`, and `anvil.out` present, covering Rust, Node, and docs fallback paths.

## Risk

The change is intentionally scoped to the shared classifier and the repo-edit evidence gate. This avoids changing scaffold generation, task contract semantics, or provider abstractions while ensuring every existing traversal consumer sees the same metadata/log exclusion.
