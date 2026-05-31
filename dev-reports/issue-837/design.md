# Issue 837 Design

## Goal

Separate generic Rust/Node project skeleton selection from scaffold materialization, and keep generated skeleton content neutral. The deterministic fallback should create package structure only; task-specific behavior remains for the model or targeted repair.

## Approach

- Add a small `ProjectIntent` classifier for empty-workspace generic code tasks.
- Add `plan_project_skeleton(intent: &ProjectIntent) -> Option<ScaffoldPlan>` as the selection layer.
- Add `materialize_scaffold(...) -> ScaffoldOutcome` as the generation/materialization layer wrapper around the existing deterministic file writer.
- Generate only generic Rust CLI/library and Node CLI/library skeleton files, with neutral pass-through implementation and smoke tests.
- Wire the new plan into the existing mode deterministic fallback path without changing specialized Python or UI fallback gates.

## Test Focus

- Pure unit tests for the three acceptance prompts and generated file paths/content.
- A materialization-path test proving the Rust CLI fallback writes `Cargo.toml` and `src/main.rs` in an empty workspace.
