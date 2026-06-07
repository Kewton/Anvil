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

## 2026-06-07 Adoption And Evidence Follow-Up

### Code Changes

- Added `ProjectProfileAdoptionDecision` so profile adoption is a typed decision with loggable fallback reasons.
- Narrowed contradiction rejection to the harmful case observed in validation: a high-confidence profile that tries to turn a non-source objective into `SourceFiles`.
- Allowed data/docs/research profiles to correct a bad first-pass setup/ops objective instead of being rejected because the first pass was already contaminated.
- Hardened ProjectProfile parsing for unquoted enum values such as `deliverable_kind: data` and `evidence_kind: content_check`.
- Changed DataOutput completion so raw repo edits do not count as completion evidence without structured-data evidence or an artifact excerpt.
- Changed missing-role target selection to prefer explicit contract identities, avoiding fallback drift from `summary.json` to conventional `output.csv`.
- Filtered ProjectProfile-derived DataOutput artifact paths through the existing output-context scan, so input files such as `inventory.csv` are not promoted to writable deliverables when the sidecar misplaces them in `primary_artifacts`.

### Static Validation

- `cargo test project_profile --lib`: pass, 25 tests
- `cargo test project_profile_projection --lib`: pass, 8 tests
- `cargo test data_capability_e2e_tests --lib`: pass, 15 tests
- `cargo test data_output_repo_edit_without_excerpt_is_not_completion_authority --lib`: pass
- `cargo test data_schema_mismatch_is_not_ready_just_because_path_exists --lib`: pass
- `cargo fmt --all -- --check`: pass
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo test --lib`: pass, 3870 tests

### Local LLM Revalidation

| Case | Workspace | Result | Notes |
| --- | --- | --- | --- |
| data CSV -> JSON | `/private/tmp/anvil-profile-data-adopt5.Beu1T2` | pass once | Wrote `summary.json` with `total_count=5`, `total_value=950`; no setup/source/test files. |
| data CSV -> JSON repeat | `/private/tmp/anvil-profile-data-adopt7.ZUNozv` | partial | Input file was no longer edited, but `summary.json` had `total_value=850`; controller still accepted parse-ready but semantically wrong data. |
| research source -> report | `/private/tmp/anvil-profile-research-adopt.k6e9WH` | pass | Wrote `report.md`; no Cargo/package drift. Sidecar classified as document, which is acceptable for a written report artifact. |
| ops observation | `/private/tmp/anvil-profile-ops-adopt.hFCiaH` | partial pass | Wrote `ops-observation.md`, but did not actually run `pwd`; ops still needs command-observation evidence enforcement. |
| Rust TDD initials | `/private/tmp/anvil-profile-coding-adopt.zmsRr5` | pass | Created `Cargo.toml`, `src/lib.rs`, `tests/lib.rs`; Anvil ran `cargo test` and completed. |

### Updated Interpretation

- The biggest improvement is not prompt wording alone. It is separating profile adoption from `TaskContract` construction and using typed deliverable/evidence semantics to decide whether the profile can override a bad first pass.
- Data success required two layers: LLM-side semantic classification (`deliverable_kind=data`) and controller-side evidence discipline (DataOutput cannot complete on file existence alone).
- Data is still not robust. The controller now protects input files from being promoted to writable deliverables, but it does not verify transformation semantics such as `total_value = sum(count * price)`.
- Research no longer falls into code scaffolding, but report-style research currently projects through the document path. That is acceptable for the artifact lifecycle, but future reporting should preserve `research_report` when the sidecar provides it.
- Ops remains the weakest non-coding lane: it can create the requested observation file, but command-observation evidence is not yet enforced.

## 2026-06-07 CommandObservation Evidence Follow-Up

### Code Changes

- Preserved ProjectProfile `evidence_kind=command_observation` as `ObjectiveEvidenceKind::SafetyBoundaryEvidence` even when the deliverable is a document.
- Added `CompletionEvidence::CommandObservation` and routed `EvidenceRunnerKind::OpsCommandObservation` through the same completion evidence set used by objective lifecycle decisions.
- Added an ObjectiveContract-based Bash-only `EvidenceAction` tool policy:
  - waits until required deliverables are present,
  - allows only `Bash` while safety-boundary evidence is missing,
  - releases after successful command observation evidence is recorded.
- Added pre-reply and reply-side command-observation evidence handling so `RunVerifier` / stale `MissingVerifierJob` do not fall back to coding verifier authoring.
- Kept simple file deliverables with `SafetyBoundaryEvidence` out of docs section behavior checks when the obligation has no explicit schema/required sections.
- Kept existing docs required-section failures covered so this does not weaken normal docs validation.

### Static Validation

- `cargo test command_observation --lib`: pass, 9 tests
- `cargo test evidence_runner --lib`: pass, 11 tests
- `cargo test docs_required_section_failure_records_generic_evidence_failed_attempt --lib`: pass
- `cargo check --lib`: pass
- `cargo build`: pass

### Local LLM Revalidation

Models:

- main: `qwen3.5:9b`
- sidecar: `qwen3.5:2b`

Prompt:

```text
Run the local command `pwd` and write ops-observation.md containing the exact observed current directory from that command. Do not create source code, tests, scripts, Cargo.toml, package.json, or setup files.
```

| Workspace | Result | Notes |
| --- | --- | --- |
| `/private/tmp/anvil-command-observation-work4.jdbBL6` | fail | False success was blocked, but model kept rewriting the file instead of running Bash. |
| `/private/tmp/anvil-command-observation-work5.S1XOQF` | partial | Bash-only policy rejected extra Write attempts and eventually induced `Bash pwd`, but evidence did not reach task-contract completion because an artifact target was still active. |
| `/private/tmp/anvil-command-observation-work6.2P4n6m` | partial | Command evidence was recorded, but docs behavior coverage reselected the same UsageDocs artifact after Bash. |
| `/private/tmp/anvil-command-observation-work7.D4XODZ` | fail | ProjectProfile confirmed `SafetyBoundaryEvidence`, but pre-reply control dispatched MissingVerifierJob before the command-evidence handler ran. |
| `/private/tmp/anvil-command-observation-work8.zImTZm` | fail | Pre-reply handler emitted the command-evidence note but returned `Continue`, so the model never received the Bash-only turn. |
| `/private/tmp/anvil-command-observation-work9.P0YtBg` | pass | Completed in 3 iterations: extra `Read` failed, wrote `ops-observation.md`, controller required actual command evidence, model ran `Bash pwd`, and Anvil completed. |

Final generated file:

```text
# Observation

Current working directory: /private/tmp/anvil-command-observation-work9.P0YtBg
```

Final evidence log:

- `agent.project_profile.classified`: `deliverable_kind=Document`, `evidence_kind=CommandObservation`
- `agent.completion_evidence.observed`: `repo_edit` for `ops-observation.md`
- `agent.completion_evidence.observed`: `ops_command_observation`, `exit_status=0`, `safety_boundary_passed=true`

### Updated Interpretation

- The fix that mattered was not another prompt phrase. It was preserving objective evidence separately from deliverable kind, then letting controller policy require the missing evidence action.
- `CommandObservation` is now a small proof that the intended architecture works: the LLM can classify the semantic need, while the controller turns that into a typed contract, a Bash-only tool policy, and deterministic completion evidence.
- The remaining risk is content binding. The controller confirms that the command was run and that the document exists, but it does not yet compare command stdout against document content for arbitrary commands. That should be a future EvidenceRunner-level check, not a docs-specific string rule.
