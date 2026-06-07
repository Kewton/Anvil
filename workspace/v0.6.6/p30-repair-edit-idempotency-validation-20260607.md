# P30/P31: Repair Reassessment, Substantive Deliverables, and Idempotent Edit Validation

Date: 2026-06-07
Branch: develop

## Goal

Move Anvil closer to a general-purpose, maintainable controller architecture by reducing coding-specific repair loops and by making controller/tool behavior more deterministic under weak local LLM output.

This pass focused on three small changes:

1. Treat verifier cheap-check unavailability as a bounded `RejectedNoCandidate` repair outcome and force target reassessment.
2. Stop counting explicit existing source/test/setup files as completed deliverables when they contain only empty/comment placeholder content.
3. Make `Edit(old_string, new_string, replace_all=false)` idempotent when the same already-applied edit is emitted again.

## Implementation Summary

- `repair_job.rs`
  - Added a no-candidate repair outcome constructor scoped to the active repair target.
- `repair_job_dispatch.rs`
  - Routes `VerifierRepairPassOutcome::Unavailable` through repair lifecycle handling instead of a blind continue.
- `verifier_orchestration.rs`
  - Records `TargetReassessmentRequired` after a no-candidate rejected repair.
- `artifact_state_projection.rs`
  - Requires substantive content before an evidence-gated explicit existing candidate can satisfy a deliverable.
  - Code-like roles (`Implementation`, `Test`, `Setup`) ignore comment-only lines.
  - Non-code roles such as docs remain content-based and do not treat Markdown headings as comments.
- `tools/edit.rs`
  - Added an idempotency guard for repeated non-`replace_all` edits.
  - If `new_string` is already present and all `old_string` matches are contained inside that already-applied replacement, the tool returns `edit already applied` without rewriting the file.

## Unit Validation

Passed:

- `cargo test --offline --lib`
  - Result: `3829 passed; 0 failed`
- New targeted coverage:
  - `target_specific_unavailable_repair_records_no_candidate_outcome`
  - `no_candidate_invalid_repair_forces_target_reassessment`
  - `explicit_evidence_gated_empty_existing_source_does_not_count_as_deliverable`
  - `repeated_prefix_edit_is_idempotent`
  - `repeated_edit_still_allows_separate_old_match`

Sandbox note:

- Some artifact projection tests require `mockito`; sandboxed execution fails with `Operation not permitted`.
- The full lib suite passed under the approved elevated test execution.

## Actual Local LLM Validation

Model:

- `qwen3.6:27b-coding-mxfp8`

### 1. TDD: Comment-Only Source No Longer Counts As Completed

Workspace:

- `/private/tmp/anvil-p30-substantive-tdd-work`

Prompt shape:

- Start from `Cargo.toml` plus comment-only `src/lib.rs`.
- Ask the model to write failing tests first, then implement `password_score`, then pass `cargo test`.

Result:

- Terminal: `repair_exhausted`
- Edited files: `Cargo.toml`, `src/lib.rs`, `tests/password_strength.rs`
- Important improvement:
  - The controller no longer treated comment-only `src/lib.rs` as a completed implementation.
  - The model wrote both `tests/password_strength.rs` and `src/lib.rs` before verifier execution.

Failure observed:

- `Cargo.toml` had three duplicated `[[test]]` entries.
- Manual verifier:
  - `cargo test --offline --manifest-path /private/tmp/anvil-p30-substantive-tdd-work/Cargo.toml`
  - Failed with duplicate test target names.

Interpretation:

- The substantive-content gate fixed the premature verifier transition.
- The next bottleneck was non-idempotent repeated edits to setup/config artifacts.

### 2. TDD After Idempotent Edit Guard

Workspace:

- `/private/tmp/anvil-p31-idempotent-tdd-work`

Result:

- Terminal: `tool_call_format_error`
- Edited files: `Cargo.toml`
- `Cargo.toml` contained only one `[[test]]` section after termination.

Interpretation:

- The duplicate-manifest failure did not recur in this run.
- The run stopped earlier because the model repeated malformed tool-call markup after format-recovery prompts.
- Remaining blocker is model-output protocol recovery, not the edit idempotency guard.

### 3. Feature Improvement

Workspace:

- `/private/tmp/anvil-p31-feature-work`

Task:

- Improve existing `normalize_slug` to trim, collapse whitespace, treat punctuation as separators, avoid edge hyphens, and pass existing tests.

Result:

- Terminal: `done`
- Edited files: `Cargo.lock`, `src/lib.rs`, `tests/slug.rs`
- Manual verifier:
  - `cargo test --offline --manifest-path /private/tmp/anvil-p31-feature-work/Cargo.toml`
  - Result: 3 tests passed.

Observed friction:

- After an implementation edit and a model-run `cargo test`, the controller still requested another repository edit on the current artifact target.
- The run eventually recovered, but wasted iterations and rewrote the test file.

Interpretation:

- Feature improvement can succeed with the current architecture.
- The controller still has a state synchronization issue between model-driven verification, artifact completion, and objective evidence.

### 4. Simple Coding From Empty Workspace

Workspace:

- `/private/tmp/anvil-p31-coding-work`

Task:

- Create `Cargo.toml`, `src/lib.rs`, `tests/add.rs`, implement `add`, and pass `cargo test`.

Result:

- Terminal: `safe_stop_verifier_missing`
- Edited files: `Cargo.toml`, `src/lib.rs`, `tests/add.rs`
- Manual verifier:
  - `cargo test --offline --manifest-path /private/tmp/anvil-p31-coding-work/Cargo.toml`
  - Result: 3 tests passed.

Key log evidence:

- `completion_probe.decision` selected `run_verifier`.
- `project_unit.verifier_selection` found Rust/cargo verifier with high confidence.
- `generated_test_preflight.rejected` reported `missing_contract_coverage`.
- `agent.verifier.missing` reported `owned_test_artifacts_count=0`.

Interpretation:

- This is a false negative terminal decision.
- The actual repository satisfies the objective and evidence command, but the preflight/owned-test projection prevents verifier execution.
- This is now a higher-priority bottleneck than raw model capability for simple coding tasks.

## Current Root Cause Assessment

The remaining failures are not simply "model is weak".

The local LLM makes mistakes, but the controller still has three structural failure modes:

1. **Evidence authority is fragmented**
   - `completion_probe` can decide a verifier exists while `generated_test_preflight` blocks the same verifier.
   - A valid project can reach `safe_stop_verifier_missing` even when `cargo test` passes.

2. **Artifact progress and objective evidence are not fully synchronized**
   - Feature improvement succeeded, but only after extra retry loops.
   - The controller did not immediately accept that source/test/setup artifacts plus test evidence should advance to completion.

3. **Tool protocol recovery remains too brittle**
   - TDD after the edit fix stopped at `tool_call_format_error`.
   - Repeating the same "emit valid XML tool call" instruction was insufficient.
   - The controller needs a more deterministic recovery path for malformed but semantically recoverable tool calls.

## Architectural Implications

The changes in this pass are aligned with general-purpose Anvil:

- `RejectedNoCandidate` target reassessment is not coding-specific.
- Substantive deliverable checks apply to source, tests, setup, docs, data, and shell artifacts via role policy.
- Idempotent editing is a tool-level invariant useful for code, docs, config, CSV, JSON, and runbooks.

However, the architecture is not yet at the target state.

Next highest-priority work:

1. Unify verifier availability and owned evidence authority.
   - If `completion_probe` can select a high-confidence evidence runner, `generated_test_preflight` must not independently force `safe_stop_verifier_missing` without exposing a repairable obligation.
   - False negative `safe_stop_verifier_missing` should become either `run_evidence` or a bounded `MissingEvidenceJob`.

2. Make test/evidence preflight contract-aware rather than path-name brittle.
   - The generated test in `/private/tmp/anvil-p31-coding-work/tests/add.rs` passed real cargo execution.
   - Preflight rejected it as `missing_contract_coverage`.
   - Contract coverage should be checked against objective/evidence semantics, not only static heuristics that can reject valid tests.

3. Replace repeated format prompt retries with structured tool-call repair.
   - When the model emits a recoverable XML/JSON tool-call shape, the controller should repair or narrow the next action, not just accumulate generic format reminders.

4. Treat model-run verification and controller-run evidence as one lifecycle.
   - If the model invokes `cargo test` successfully, that evidence should be admitted or deterministically re-run by the controller.
   - Otherwise the controller wastes iterations by asking for additional edits after evidence already exists.

## Bottom Line

This pass made measurable progress on two concrete causes of repair non-convergence:

- premature completion of empty/comment-only deliverables
- cumulative corruption from repeated identical edits

The hard-task validation also exposed the next deeper bottleneck:

- verifier/evidence authority fragmentation can still turn a passing repository into `safe_stop_verifier_missing`.

That false negative is now the most important blocker to improving success rate for both coding and non-coding objective/evidence workflows.
