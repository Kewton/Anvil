# Issue #855 Design Note

Extend `TaskContract` so required artifact obligations are not limited to user-explicit filenames. The contract should derive manifest obligations for Rust (`Cargo.toml`) and Node (`package.json`) CLI/library/package requests, retain requested path obligations, and use those obligations when deciding whether a task can complete.

Smallest coherent change:

- Keep `required_artifacts` as the role-level compatibility surface.
- Populate `required_artifact_identities` with inferred manifest obligations plus explicit request paths.
- Make `evaluate()` conservative when obligations exist by requiring matching repo-edit evidence for each obligation path.
- Keep `plan_artifact_recovery()` authoritative in production because it has artifact-state paths and can satisfy obligations from existing/changed artifacts.
- Feed obligations into deterministic scaffold planning so scaffold filenames track requested implementation paths where practical.
- Preserve docs-only behavior: docs contracts should complete from required docs artifacts without requiring executable verifiers.
