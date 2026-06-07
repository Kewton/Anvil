# Project Profile Refactor Validation - 2026-06-07

## Scope

- Extracted project language / shape inference out of `task_contract.rs` into `project_profile.rs`.
- Added a bounded LLM project-profile confirmation parser/prompt boundary so future controller wiring can consume semantic profile JSON without growing `TaskContract` string rules.
- Wired the ProjectProfile second pass into the per-turn `TaskContract` authority so the LLM-confirmed objective profile can adjust deliverable / evidence details before the `OnceCell` is sealed.
- Hardened ProjectProfile parsing for common local-LLM JSON-ish drift (`confidence: "high"`, Python-style `None` / `False`, null-like array entries) while keeping the downstream contract typed.
- Split README `setup` section wording from environment setup/bootstrap intent.
- Guarded `SetupBootstrap` projection fallback so Docs / Authoring / Data / Research artifacts are not forced into Bash-only setup mode.
- Extended implementation non-goal detection to cover `source code` phrasing.

## Static Validation

- `cargo fmt --all`: pass
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo test project_profile --lib`: pass, 14 tests
- `cargo test --lib`: pass, 3855 tests

## Local LLM Validation

Model:

- `qwen3.6:27b-coding-mxfp8`

### Docs-Only Case

Prompt:

```text
Write README.md with setup, usage, and troubleshooting sections for a small local backup CLI. Do not create source code.
```

Observed before the fix:

- `setup` section text was interpreted as `setup_bootstrap`.
- The model was restricted to Bash-only setup policy and failed.
- After the first fix, the model wrote `README.md` but TaskContract incorrectly requested `implementation`, causing unwanted `backup_cli.py` and tests.

Observed after the fix:

- Completed in 1 iteration.
- Wrote only `README.md`.
- No source files or tests were created.

Workspace:

- `/private/tmp/anvil-profile-docs-B694gE`

### Runtime ProjectProfile Confirmation Case

Prompt:

```text
Write README.md with setup, usage, and troubleshooting sections for a small local backup CLI. Do not create source code.
```

Model:

- main: `qwen3.5:9b`
- sidecar: `qwen3.5:2b`

Observed before parser hardening:

- The new ProjectProfile dispatch fired, but sidecar JSON drift caused fallback:
  - `confidence: "high"` failed strict `f32` parsing.
  - `forbidden_artifacts: [None]` failed strict JSON parsing.

Observed after parser hardening:

- `agent.project_profile.classified` emitted `status="confirmed"`.
- Confirmed profile: `deliverable_kind=Document`, `primary_artifacts=["README.md"]`, `needs_environment_setup=false`, `confidence=1.0`.
- Completed in 1 iteration.
- Wrote only `README.md` under the test workspace. No source files or tests were created.

Workspace:

- `/private/tmp/anvil-profile-docs-runtime.VzZrwi`

### Rust/TDD Case

Prompt:

```text
Create a Rust library in src/lib.rs implementing pub fn slugify(input: &str) -> String. Use TDD by adding tests, and verify with cargo test.
```

Observed:

- The model created `src/lib.rs`, `tests/lib.rs`, and `Cargo.toml`.
- Anvil stopped with `safe_stop_verifier_missing`.
- Manual `cargo test` failed because the generated implementation called `.dedup()` on an iterator without a supporting trait/import.

Interpretation:

- The project-profile refactor did not regress coding artifact creation.
- The remaining failure is in code generation / repair-verifier convergence, not in docs/setup misclassification.

## 2026-06-07 Follow-Up Refactor

### Code Changes

- Extracted `ProjectProfileConfirmation -> TaskContract inputs` into `project_profile_projection.rs`.
- Centralized profile adoption confidence in `project_profile::confirmation_is_authoritative`.
- Changed ProjectProfile confirm gating from `TaskKind != Coding` to ObjectiveContract uncertainty / conflict:
  - no required deliverable: skip
  - uncertain classification: confirm
  - source deliverable mixed with docs/data roles: confirm
  - non-source deliverables: confirm
- Hardened profile parsing for singleton enum arrays and object-shaped artifact entries.
- Updated profile prompt so the user request is authoritative and first-pass Rust/setup/coding signals are low-priority hints.
- Updated profile prompt so `primary_artifacts` means output deliverables only, not input files.
- Split docs section inference from evidence wording:
  - `Verify by reading README.md` no longer creates a required `testing` section.
  - `Do not create source code or tests` no longer creates a required `testing` section.
  - explicit `Overview` is preserved as a required section.
- Changed artifact-directed policy so `Write` / `Edit` remain target-only while `Read` may inspect workspace files needed to produce the target.

### Static Validation

- `cargo fmt --all -- --check`: pass
- `cargo test project_profile --lib`: pass, 20 tests
- `cargo test docs_verify_instruction_does_not_become_testing_section --lib`: pass
- `cargo test artifact_directed_policy_allows_workspace_read_before_target_write --lib`: pass
- `cargo test artifact_directed_policy_rejects_wrong_target_path --lib`: pass
- `cargo test artifact_directed_recovery_message_body_masks_and_is_byte_stable --lib`: pass
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo test --lib`: pass, 3863 tests

### Local LLM Validation Matrix

Models:

- non-coding main: `qwen3.5:9b`
- sidecar: `qwen3.5:2b`
- coding main: `qwen3.6:27b-coding-mxfp8`

| Case | Workspace | Result | Notes |
| --- | --- | --- | --- |
| docs README only | `/private/tmp/anvil-profile-docs3.MfwgRo` | pass | `agent.project_profile.classified status=confirmed`; completed in 1 iteration; wrote only `README.md`. |
| data CSV -> JSON | `/private/tmp/anvil-profile-data5.PVFHIH` | fail | Improved from input-edit failure: the model read `inventory.csv` and wrote `summary.json`, but later drifted to `Cargo.toml`; terminal `evidence_repair_exhausted`. |
| research source -> report | `/private/tmp/anvil-profile-research.o9b2fb` | fail | Read `source.md`, but sidecar confirmed `deliverable_kind=Code`; run drifted into `src/main.rs` / `Cargo.toml`; terminal `max_iterations`. |
| ops observation file | `/private/tmp/anvil-profile-ops.5Iem5G` | pass | Wrote `ops-observation.md`; completed in 2 iterations. |
| Rust TDD slugify | `/private/tmp/anvil-profile-coding.Htzl8C` | pass | Created `Cargo.toml`, `src/lib.rs`, `tests/lib.rs`; `cargo test` passed. |

### Current Interpretation

- The direction is better: docs, ops, and hard coding/TDD now work under actual local LLM validation.
- The main unresolved gap is still not "model weakness" alone. It is controller authority contamination:
  - wrong first-pass Rust/setup signals still leak into sidecar classification,
  - sidecar can return a high-confidence but semantically contradictory profile,
  - `TaskContract` still accepts high confidence as enough authority.
- The next architectural step should make profile adoption validate semantic consistency against the request/ObjectiveContract, not just `confidence >= threshold`.
- Data and research need an explicit input/output contract surface:
  - inputs are readable dependencies,
  - deliverables are writable targets,
  - evidence checks run after deliverables,
  - artifact repair must never turn readable inputs or forbidden setup manifests into required deliverables.
