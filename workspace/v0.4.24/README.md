# v0.4.24 Work Plan: Responsibility Cleanup And Complexity Reduction

## Purpose

This plan targets the remaining structural complexity in Anvil's local-LLM
agent loop. The goal is not to add another recovery branch. The goal is to make
completion trustworthy by simplifying responsibility boundaries, eliminating
false-positive `done`, and making verifier repair dispatch predictable.

The immediate quality problem is now clear from v0.4.23 evaluation:

- Uncontrolled repair-provider timeout is mitigated.
- Generic completion is still weak.
- The most serious correctness bug is false-positive `done`, exemplified by a
  Rust library request passing with only `README.md` plus a Python dummy test.
- Most non-trivial failures now reach controlled safe stops, but repair quality
  and artifact evidence are still insufficient.

## Desired End State

Anvil should finish each task in exactly one of these states:

1. `done`: verified, request-aligned artifacts exist, and the verifier matches
   the requested project type.
2. actionable safe stop: Anvil can explain why it cannot safely complete, with
   concrete next actions.

The following states must not be accepted:

- green verifier for the wrong stack or wrong artifact type
- placeholder or dummy tests counted as required tests
- setup scaffold counted as implementation
- generic retry taking over verifier repair
- LLM prose treated as evidence of completion
- unsafe test weakening used to make verifier green

## Responsibility Model

### `turn.rs`

Target responsibility:

- advance the actor loop
- call the current active job's dispatcher
- execute selected tool calls
- emit user-visible progress and final result

Must not own:

- verifier repair state decisions
- artifact role completion rules
- verifier command selection policy
- semantic authority decisions
- string-based repair classification

### `ArtifactCompletionJob`

Target responsibility:

- track required artifact roles for the active request
- own role-specific retry budget
- select the next required artifact target
- decide whether a role has enough evidence to move forward

Must not own:

- verifier command selection
- repair target selection after verifier failure
- generic "repo changed" success

### `ArtifactLedger` / `EvidenceLedger`

Target responsibility:

- store typed evidence for artifacts
- bind each evidence item to request intent, artifact role, project unit, and
  verifier scope
- exclude `.anvil-state`, logs, external harness files, generated evaluation
  output, and paths outside the task workspace

Must not own:

- retry policy
- LLM prompting
- repair patch validation

### `ProjectProbe` / `ProjectVerifier`

Target responsibility:

- infer project units from files, manifests, edited paths, and request intent
- choose verifiers compatible with the project unit
- refuse mismatched verifier paths

Must not own:

- semantic repair planning
- artifact role completion
- final `done` decision without artifact evidence

### `RepairJob`

Target responsibility:

- be the sole verifier-repair state machine
- drive `FailurePacket -> RepairPlan -> PatchProposal -> PatchValidation ->
  Apply -> VerifierDelta -> next_action`
- own repair budgets, repeated rejection handling, target changes,
  re-diagnostic, and safe stops

Must not own:

- initial implementation generation
- artifact completion before verifier
- project-unit discovery beyond consuming typed verifier/project evidence

### `SafetyValidator`

Target responsibility:

- validate patch intent before apply
- detect test weakening and implementation weakening
- reject path escape, duplicate/noop edits, unsafe target mismatch, and invalid
  patch shapes

Must not own:

- deciding whether to retry or safe stop
- selecting the verifier

### LLM Roles

Main LLM:

- initial implementation
- narrow edits selected by Anvil

Diagnostic LLM:

- convert verifier failure evidence into structured repair plans

Patch-provider LLM:

- propose a minimal patch for an already accepted plan and selected target

PAM:

- advisory retrieval only
- never authority
- never sufficient evidence for completion

## Work Packages

### WP0: Freeze Baseline And Inventory Dispatch Paths

Goal: establish an exact map of current control flow before changing it.

Tasks:

- Inventory every production branch in `turn.rs` that can produce `done`,
  `missing_repo_edits`, `verifier_failed`, `repair_safe_stop`, or
  `repair_exhausted`.
- Inventory every branch that can install or override:
  - focused edit recovery
  - generic retry
  - artifact completion
  - missing verifier recovery
  - verifier repair
  - deterministic fallback
- Classify each branch as:
  - production main path
  - fallback path
  - validator assist
  - telemetry only
  - test only
  - legacy candidate for deletion
- Produce a dispatch-source table with caller, condition, state read, state
  mutation, and intended owner.

Deliverables:

- `workspace/v0.4.24/dispatch-inventory.md`
- no production code change

Acceptance:

- Every production call site that changes active recovery/repair state is
  listed.
- Any unclassified branch blocks implementation work.

### WP1: Define A Single Active-Job Arbiter

Goal: make the loop ask one arbiter which job owns the next step.

Tasks:

- Define active job priority:
  1. user interrupt / terminal result
  2. verifier repair job
  3. missing verifier job
  4. artifact completion job
  5. setup/bootstrap job
  6. normal model turn
- Ensure the arbiter returns a typed `LoopControlAction`.
- Remove direct production dispatch that bypasses the arbiter.
- Keep legacy branches behind explicit compatibility wrappers during migration.

Deliverables:

- `ActiveJobArbiter` or equivalent small module
- unit tests for priority and mutual exclusion

Acceptance:

- No verifier repair action is dispatched outside `RepairJob::next_action()` or
  a committed `RepairJobDriver` step.
- Artifact completion cannot run while verifier repair is active.
- Focused edit/generic retry cannot override an active repair job.

### WP2: Make Artifact Evidence Request-Aligned

Goal: prevent file existence or wrong-language tests from satisfying required
artifacts.

Tasks:

- Extend artifact evidence with:
  - artifact role
  - project unit id
  - language/runtime family
  - source of evidence
  - edited-this-session flag
  - verifier binding if available
- Distinguish:
  - scaffold/setup evidence
  - request-specific implementation evidence
  - test artifact evidence
  - docs artifact evidence
- Reject test evidence that does not match the request's project unit.
- Reject implementation evidence that is only setup scaffold.
- Reject `done` when required roles are satisfied by unrelated files.

Deliverables:

- artifact evidence schema update
- projection helpers
- tests for cross-language false positives

Acceptance:

- Rust library request cannot be completed with only Python tests.
- Node CLI request cannot be completed with Python/Rust tests.
- Python CLI request cannot be completed with only `requirements.txt` or README.
- Docs-only request can complete with only docs, and does not require tests.

### WP3: Bind Verifier Selection To Project Unit

Goal: the verifier must prove the requested project, not any convenient file.

Tasks:

- Introduce or harden `ProjectUnit` detection:
  - Rust: `Cargo.toml`, `src/lib.rs`, `src/main.rs`, `tests/*.rs`
  - Python: `.py`, `pyproject.toml`, `requirements.txt`, `tests/test_*.py`
  - Node: `package.json`, JS/TS entrypoints, `node --test` tests
  - docs-only: Markdown artifacts, no code verifier required
- Derive verifier candidates from project unit plus artifact evidence.
- Refuse fallback to unrelated verifier family.
- For multiple project units, prefer the unit touched by implementation and
  test evidence for the current request.
- If no safe verifier exists, produce actionable safe stop rather than false
  green.

Deliverables:

- project-unit verifier binding helper
- verifier selection tests

Acceptance:

- Rust request uses cargo verifier or safe-stops.
- Node request uses npm/node verifier or safe-stops.
- Python request uses pytest/python verifier or safe-stops.
- No request reaches `done` through a verifier for a different runtime family.

### WP4: Replace String-Based Repair Control With Typed Events

Goal: remove fragile control decisions based on error-message substring
matching.

Tasks:

- Define typed repair event inputs:
  - `ProviderTimeout`
  - `MalformedPatch`
  - `WrongTarget`
  - `AmbiguousAuthority`
  - `NoSafeTarget`
  - `UnsafePatch`
  - `NoopPatch`
  - `DuplicatePatch`
  - `VerifierUnavailable`
- Convert known production sources to emit typed events directly.
- Keep string parsing only as a legacy telemetry adapter, not as primary
  control.
- Add tests proving typed events drive `RepairJob` transitions.

Deliverables:

- typed event constructors
- string parsing downgraded or isolated

Acceptance:

- New production repair paths do not depend on searching `"timeout"`,
  `"malformed"`, `"authority"`, or similar strings.
- Logs may contain text, but state transitions consume typed data.

### WP5: Make `RepairJob` The Only Verifier-Repair Dispatcher

Goal: finish the repair-dispatch migration.

Tasks:

- Route every verifier failure through:
  - `FailurePacket`
  - `RepairJob::apply_event`
  - `RepairJob::next_action` or `RepairJobDriver::begin_next_step`
- Remove direct calls that independently decide:
  - re-diagnostic
  - patch target
  - verifier rerun
  - safe stop
- Ensure `AcceptedRepairPlan` is mandatory before patch generation.
- Ensure invalid/malformed patch proposal budgets are job-level, not ad hoc
  loop counters.

Deliverables:

- reduced repair branches in `turn.rs`
- state-machine transition tests

Acceptance:

- A verifier failure cannot enter generic recovery.
- A rejected patch either retries within budget, replans, changes target, or
  safe-stops by `RepairJob` decision.
- There is no production patch repair without an accepted plan.

### WP6: Isolate Or Delete Legacy Recovery

Goal: remove complexity that can interfere with the main pipeline.

Tasks:

- Move legacy deterministic repair helpers behind explicit module boundaries.
- Classify each deterministic helper:
  - delete now
  - test-only
  - telemetry-only
  - explicit opt-in fallback
  - validator assist
- Remove production calls that silently materialize domain-specific templates.
- Add compile-time or runtime assertions that legacy fallback cannot override
  an active job.

Deliverables:

- `workspace/v0.4.24/legacy-recovery-inventory.md`
- code deletion or isolation PR

Acceptance:

- No FastAPI/ToDo/CRUD/Python CSV template path is in the normal production
  flow unless explicitly opted in.
- Legacy helpers cannot produce `done` by themselves.

### WP7: Normalize Workspace Paths Once

Goal: prevent absolute-path-like repair targets and scope leakage.

Tasks:

- Define a single path normalization helper for:
  - artifact evidence
  - repair target hints
  - verifier scopes
  - edited-file tracking
  - user-visible summaries
- Reject:
  - absolute paths
  - `..`
  - symlink escape
  - control characters
  - external temp/log paths
  - `.anvil-state` and harness output
- Convert existing call sites to the single helper.

Deliverables:

- path normalization SSOT
- tests for unsafe and external paths

Acceptance:

- Repair target summaries never show `private/tmp/...` as a repository file.
- Evidence cannot include `.anvil-state`, logs, or external harness files.

### WP8: Stabilize Docs-Only Artifact Execution

Goal: docs-only tasks should reliably write the target document or safe-stop.

Tasks:

- Ensure docs-only work selects documentation artifact target immediately.
- Prevent setup/bootstrap from taking over docs-only work.
- Give docs artifact completion the same role-specific retry accounting as
  implementation/test.
- If the model repeatedly refuses to write docs, safe-stop with actionable
  reason instead of generic `missing_repo_edits`.

Deliverables:

- docs artifact tests
- docs-only smoke case

Acceptance:

- SRE runbook request creates `README.md` or a named Markdown document.
- No placeholder tests are created for docs-only tasks.
- No setup command is required for docs-only tasks.

### WP9: Strengthen Done Gate

Goal: make `done` harder than safe stop.

Tasks:

- Define `DoneEvidence`:
  - required artifact roles satisfied
  - project-unit-compatible verifier passed
  - no unsafe weakening accepted
  - no unresolved repair job
  - no scaffold-only implementation
  - no unrelated verifier family
- Move final `done` decision into a small pure function.
- Add negative tests for false-positive completion.

Deliverables:

- `done_gate.rs` or equivalent helper
- regression tests

Acceptance:

- Rust slug false-positive scenario fails the done gate.
- A passing unrelated pytest run cannot complete a Rust/Node request.
- Safe stop is preferred over uncertain done.

### WP10: Improve Repair Patch Convergence Without Domain Templates

Goal: increase completion rate without adding use-case-specific templates.

Tasks:

- Feed patch provider:
  - selected target
  - exact failing assertions or compiler errors
  - relevant file excerpts
  - accepted plan
  - allowed change kind
  - prior rejected patch reason
- Avoid huge context and repeated stale instructions.
- Add target switch or replan when the same target repeatedly fails.
- Prefer implementation repair over test repair unless the accepted plan
  explicitly marks test bug/import/setup mismatch.
- Keep test weakening rejection strict.

Deliverables:

- patch-provider prompt shape review
- synthetic verifier failures for Python, Rust, and Node

Acceptance:

- Near-miss cases should either converge faster or safe-stop with a more
  specific reason.
- No weaker tests are accepted to force green.

### WP11: Test Strategy

Unit tests:

- active-job priority
- artifact evidence role binding
- project unit detection
- verifier family selection
- done gate false-positive rejection
- repair typed event transitions
- path normalization rejection
- docs-only artifact target

Synthetic E2E tests:

- Rust request plus Python dummy test must not complete.
- Node request plus pytest-only verifier must not complete.
- Python setup-only files must not satisfy implementation.
- docs-only request must not require test artifact.
- verifier failure cannot enter generic retry while repair job is active.
- repeated malformed patches replan or safe-stop through `RepairJob`.

Local LLM smoke:

Run only after unit and synthetic E2E gates pass.

Small gate:

- no-PAM 5 cases:
  - Rust library
  - Python CLI
  - Node CLI
  - docs-only
  - FastAPI/API
- PAM 3 cases:
  - Rust library
  - Python CLI
  - Node CLI

Expanded gate:

- no-PAM 20 mixed cases
- PAM 10 mixed cases

Quality-adjusted scoring:

- valid done
- false-positive done
- actionable safe stop
- uncontrolled failure
- uncontrolled timeout

Acceptance before large evaluation:

- false-positive done: 0
- uncontrolled timeout: 0
- uncontrolled generic recovery during repair: 0
- docs-only pre-edit failure: 0 in small gate

### WP12: Observability And Reports

Tasks:

- Emit structured job lifecycle events with closed enum labels.
- Record project unit, verifier family, done-gate decision, and rejected
  evidence reason.
- Add per-run quality-adjusted summary.
- Keep raw LLM text out of control-state logs where possible.

Deliverables:

- eval summary additions
- developer-facing trace examples

Acceptance:

- A false-positive attempt explains which evidence was rejected.
- A safe stop explains whether the blocker was verifier selection, artifact
  evidence, patch safety, or repair budget.

## Recommended Execution Order

1. WP0 inventory.
2. WP9 done gate skeleton with current evidence, focused on false-positive
   rejection.
3. WP2 artifact evidence role/project-unit binding.
4. WP3 verifier selection binding.
5. WP1 active-job arbiter hardening.
6. WP5 repair dispatcher completion.
7. WP4 typed event migration.
8. WP7 path normalization.
9. WP8 docs-only stability.
10. WP6 legacy recovery isolation/deletion.
11. WP10 patch convergence improvements.
12. WP11 and WP12 full validation and reporting.

This order is deliberate. The false-positive `done` bug is more dangerous than
repair exhaustion because it reports success incorrectly. Therefore the done
gate and verifier/evidence binding must come before further repair-quality work.

## Parallelization Plan

Can run in parallel:

- WP0 inventory and WP7 path normalization design.
- WP2 artifact evidence and WP3 verifier binding, after agreeing on
  `ProjectUnit` shape.
- WP4 typed event migration and WP6 legacy inventory.
- WP8 docs-only stability and WP9 done-gate negative tests.

Should not run in parallel:

- WP1 active-job arbiter and WP5 repair dispatcher migration unless ownership
  boundaries are frozen first.
- WP2/WP3 schema changes and WP9 final done gate if they modify the same
  completion APIs.

## Risks

- Over-correcting false positives can increase safe stops. This is acceptable
  initially; safe stop is preferable to incorrect `done`.
- Project unit detection can become another rule-heavy subsystem. Keep it based
  on manifests, edited paths, and verifier compatibility, not domain templates.
- Removing legacy recovery too early can regress current safe-stop behavior.
  Isolate first, delete after tests prove equivalent control.
- LLM prompt tuning can hide controller bugs. Controller tests must pass before
  local LLM evaluation.

## Definition Of Completion For v0.4.24

The work is complete when:

- false-positive `done` is blocked by tests
- verifier selection is bound to project unit
- artifact evidence is role and project-unit aware
- verifier repair dispatch is owned by `RepairJob`
- typed events replace primary string parsing for new repair-control paths
- legacy recovery is isolated from production main path
- docs-only work is stable in small smoke
- local LLM small gate has:
  - 0 false-positive done
  - 0 uncontrolled timeout
  - 0 repair-to-generic-recovery escape
  - all failures either actionable safe stops or known verifier repair
    convergence failures

## Immediate Next Tasks

1. Create `dispatch-inventory.md`.
2. Add regression test for Rust library false-positive done.
3. Add done-gate helper that rejects verifier/artifact family mismatch.
4. Bind verifier selection to project unit for Rust/Python/Node/docs.
5. Re-run the Rust slug false-positive case before any larger evaluation.

## 2026-05-26 Implementation Notes

### Applied

- Added `workspace/v0.4.24/dispatch-inventory.md`.
- Added request-aware `ProjectUnit` probing for verifier selection.
- Routed both task-contract verifier and post-loop success verifier through the request-aware probe when active request text exists.
- Rejected the false-positive shape where a Rust/cargo request has only Python test/docs artifacts.
- Filtered verifier candidates so an explicit Rust request cannot run a Python pytest candidate; TypeScript remains compatible with package.json test scripts.
- Centralized request-aligned artifact target normalization at `set_artifact_recovery_target_from_hint`.
- Added Rust/cargo test target synthesis so artifact completion uses `tests/main.rs` instead of `tests/test_main.py` for Rust requests.

### Verification

- `cargo fmt --check`: pass
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo test project_probe --lib -q`: pass
- `cargo test --lib -q`: pass with elevated permissions because mockito needs local server binding
- `cargo build --release`: pass

### Smoke Result

Case:

```text
文字列スラッグ生成用のRustライブラリを開発してください。空白や記号をハイフンに正規化し、小文字化します。README.mdに使用方法を書き、cargo testで動くテストコードも実装してください。
```

Observed after the first fix:

- false-positive `done` no longer occurs.
- Anvil safe-stopped instead of accepting Python-only test/docs as completion.

Observed after target normalization:

- artifact completion selected `tests/main.rs`.
- the model generated a Rust integration test and README.
- verifier ran `cargo test --test main`.
- remaining failure moved to verifier repair convergence: repair repeatedly edited `src/lib.rs` but did not fix the unresolved crate import in `tests/main.rs`.

Current assessment:

- The false-positive completion bug is addressed.
- The next structural blocker is still RepairJob convergence and target selection after compiler diagnostics. The unresolved import failure should have led to either a test import fix or package-name alignment, but the repair loop kept targeting implementation.

### Continued Fixes

Additional generic issues found while extending the Rust slug smoke:

- WorkMode/TaskContract did not reliably treat `Rust library` / `crate` /
  `package` / Japanese `ライブラリ` requests as implementation work when the
  request also mentioned README/tests.
- The first actor iteration could still be unrestricted even when the
  TaskContract already knew the first missing artifact target. This allowed
  model-proposed setup Bash such as nested `cargo init` before artifact
  completion had a target.
- Rust integration-test import failures (`unresolved import <crate>`) were not
  classified as a framework/test-artifact finding, so repair target selection
  over-preferred implementation files.
- Artifact completion treated exact test target paths too strictly even though
  test artifact completion is role-level. A compatible same-family test file
  should satisfy the test role.
- Behavior coverage was too brittle for multilingual prompts: Japanese
  requirements and English API/test names caused implementation/test artifacts
  to be re-edited even after real files existed.
- Structured verifier `Missing` conflated two cases:
  - no owned tests exist: safe stop is appropriate
  - owned tests exist but no verifier/manifest is available: this is recoverable
    via MissingVerifierJob/setup edit

Applied adjustments:

- Added generic implementation-request vocabulary and Rust/cargo build-target
  recognition.
- Preinstalled the initial artifact recovery target before the first model turn.
- Added Rust cargo diagnostic framework finding for test-only unresolved crate
  imports.
- Allowed same-family test artifact edits to satisfy the active test role.
- Relaxed behavior completion for implementation/test artifacts:
  implementation now blocks only obvious placeholder bodies; test semantic
  validation is delegated to structured verifier binding.
- Expanded README surface markers to cover generic/Japanese usage examples,
  common manifest/dependency files, and common verifier command names.
- Routed structured verifier `Missing` with owned tests to `NoVerifier` so
  MissingVerifierJob can request setup/config repair in the same run.

Latest Rust slug smoke:

- Reached artifact completion for `src/lib.rs`, `tests/lib.rs`, and `README.md`.
- Detected missing verifier and asked for setup/config repair.
- The model created `Cargo.toml`.
- Verifier ran `cargo test`.
- Remaining failure is verifier repair convergence: repeated patch proposals
  were rejected or did not converge on the failing Rust slug implementation.

Follow-up fixes after that smoke:

- Added a duplicate-binding repair guard. When verifier output reports a
  duplicate/redefined symbol, an implementation patch is rejected if it leaves
  the named binding duplicated or makes the duplicate count worse. This blocks
  invalid repairs that claim to remove a duplicate function but actually insert
  another copy.
- Tightened `MissingVerifierJob` setup guidance. Missing verifier recovery now
  asks for exactly one project-local verifier/setup metadata edit and includes a
  request-derived hint such as `Cargo.toml` for Rust/Cargo work. This keeps the
  setup recovery owned by `MissingVerifierJob` rather than falling back to vague
  prose retry.

Latest Rust slug smoke after the follow-up fixes:

- Missing verifier recovery reached `Cargo.toml` successfully.
- Repair dispatch stayed inside `RepairJob`; it did not fall through to generic
  retry or focused edit recovery.
- The remaining failure was `repair_exhausted` after repeated assertion
  failures for Japanese transliteration examples generated by the model's own
  tests:
  - `こんにちは世界` expected `konnnichiwa-sekai`
  - `Hello こんにちは World` expected `hello-konnnichiwa-world`
  - `日本語ドキュメント` expected `nihongo-dokyument`

Updated assessment:

- The control flow has moved forward materially: false-positive done,
  wrong-stack verifier, initial unrestricted nested setup, over-strict
  multilingual artifact completion, and recoverable missing verifier safe-stop
  have been reduced.
- The current blocker is narrower: `RepairJob` can now reach real verifier
  failure, but patch-provider repair still does not reliably converge on
  ordinary assertion failures.
- This is not a FastAPI-specific or Rust-specific adaptation. The fixes are
  structural except for framework-diagnostic recognition, which is scoped to
  generic Rust/cargo failure classes rather than a business use case.

### Remaining Tasks After Follow-up Smoke

1. Separate broad behavior-contract metadata from repair authority.
   - Domain terms and capability labels are useful context, but they should not
     automatically authorize exact-value implementation changes.
   - Exact assertion expectations should become implementation authority only
     when backed by explicit user request, behavior examples in docs, a verified
     interface, or a high-confidence diagnostic explaining why the test is not
     generated speculation.
2. Add a generic ambiguous-expectation terminal path.
   - If all remaining failures are generated assertion literals without external
     authority, stop with an actionable ambiguity report.
   - Do not keep mutating implementation solely to satisfy hallucinated or
     typo-like expected literals.
3. Keep implementation repair available for objective failures.
   - Compile errors, import errors, missing public symbols, dependency/config
     failures, and clearly connected runtime errors should still repair
     implementation/setup normally.
   - Test isolation/setup/import bugs should still target tests when the failure
     shape proves the test is disconnected from the system under test.
4. Improve diagnostic output use for assertion authority.
   - Ask the diagnostic LLM to classify whether expected literals are backed by
     request/docs/interface evidence or are generated-test assumptions.
   - The controller must validate that classification against bounded artifact
     excerpts before accepting it.

### Follow-up Authority Separation Implementation

Applied after the latest smoke:

- Split behavior-contract projection from behavior-contract repair authority.
  `project_behavior_contract()` can still expose broad domain/runtime labels to
  diagnostic prompts, but `behavior_contract_has_repair_authority()` is narrower
  and does not treat domain terms alone as authority for exact assertion
  repair.
- Kept CRUD-style operation labels authoritative by expanding the closed
  operation set from `crud` into create/read/update/delete. This is a generic
  software capability signal, not a FastAPI-specific template.
- Strengthened repair authority validation:
  - a diagnostic self-claim of `user_request` now requires controller-detected
    explicit user-request evidence;
  - a diagnostic self-claim of `behavior_contract` now requires controller
    behavior-contract repair authority;
  - implementation changes for observed/expected assertion pairs without
    external authority are rejected as ambiguous.

Verification after implementation:

- `cargo fmt --check`: pass
- targeted repair-authority tests: pass
- `cargo test --lib -q`: pass, `2985 passed`
- `cargo build --release`: pass

Smoke after authority separation:

- Prompt: `文字列スラッグ生成用のRustライブラリを開発してください。README.mdとcargo testで動くテストも実装してください。`
- Result: not `done`, but improved terminal behavior.
- Anvil generated `src/lib.rs`, `tests/lib.rs`, `README.md`, and `Cargo.toml`.
- Verifier ran and failed.
- Diagnostic was accepted.
- Controller rejected invalid/ambiguous repair proposals and stopped inside
  `RepairJob` with:
  - `repair_exhausted`
  - `verifier repair safe stop: patch_rejected_repeatedly`

Assessment:

- This is the desired direction for ambiguous generated-test expectations:
  prefer controlled safe stop over repeated mutation toward weak authority.
- The remaining quality gap is not dispatch ownership. It is producing a more
  actionable safe-stop report that clearly says which assertion expectations
  lacked external authority and what the user should specify next.
