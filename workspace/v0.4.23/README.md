# v0.4.23 State Control Consolidation

## Purpose

This workspace tracks the effort to simplify Anvil's state control so one controller decides the next action for each phase.

The immediate target is verifier repair. When a verifier failure has created a `RepairJob`, production dispatch must go through `RepairJob::next_action()` and `RepairJob::begin_next_repair_step()` only. Focused edit recovery, generic retry, and legacy deterministic repair must not independently decide repair progress.

The full unification plan is maintained in `full-unification-work-plan.md`.
The remaining completion work is tracked in `completion-work-plan.md`.
The execution plan for reaching the final completion criteria is maintained in `complete-achievement-execution-plan.md`.
The latest remaining work breakdown is maintained in `remaining-completion-work-plan.md`.

## Current Dispatch Sources

### Production path to keep

- `task_contract::plan_artifact_recovery`
  - Owns pre-verifier artifact completion decisions.
  - Should decide whether required artifact roles are still missing, whether verifier can run, or whether the task can safe stop before verification.
- `project_probe::probe_completion`
  - Owns generic workspace readiness probing.
  - Should return facts/decision for verifier readiness, not repair policy.
- `repair_job::RepairJob::next_action`
  - Owns verifier repair phase decisions after a verifier failure.
  - Should produce diagnostic, patch, verifier rerun, safe stop, or done.
- `active_job_arbiter::select_active_job`
  - Owns tool policy projection for one write-owner job.
  - Should not decide semantic progress.

### Transitional / fallback path to restrict

- `MissingVerifierJob`
  - Transitional missing-verifier bootstrap. It may remain until project verifier discovery is generalized.
- focused edit recovery
  - Useful for non-repair protocol recovery, but must not control verifier repair when `RepairJob` exists.
- local-LLM small edit fallback
  - Should remain a protocol fallback, not a completion or repair authority.
- deterministic repair/scaffold paths
  - Should be telemetry, validator assist, or explicit fallback only. They should not override the main state machine.

## Change Made In This Pass

### Phase 1: remove duplicate verifier repair dispatch

`turn.rs` had a new top-level `dispatch_repair_job_step()` path, but old verifier repair branches still existed below it:

- direct `DiagnosticUnavailable` handling via `scope_safeguarded_verifier_repair_decision`
- direct diagnostic pass dispatch
- direct controller-applied patch dispatch

Those branches duplicated the same responsibilities already handled by `dispatch_repair_job_step()`.

This pass removes the duplicate lower branches. With this change, when all of the following are true:

- mode is not Plan
- `task_contract_verifier_repair_pending == true`
- `repair_job.is_some()`

then the loop enters `dispatch_repair_job_step()` at the start of the iteration and the selected repair step is derived from `RepairJob::begin_next_repair_step()`.

### Phase 2: project active repair policy from `RepairJob::next_action()`

The active-job policy branch still projected verifier repair through the legacy `VerifierRepairDecision` compatibility shape. That kept one more semantic dispatch source in production code.

This pass adds a direct mapping:

```text
RepairJob::next_action()
  -> RepairNextAction
  -> EffectiveToolPolicy
  -> DesiredAction::VerifierRepair
```

When an active `RepairJob` exists, `build_arbiter_candidates()` now reads `RepairJob::next_action()` directly. For `RequestPatch`, the target hint is converted into a least-privilege read/edit/write policy for that specific workspace-relative path. For diagnostic, replan, rerun verifier, safe stop, and done states, the LLM tool surface is closed because those steps are controller-owned.

### Phase 3: introduce `LoopControlAction`

This pass adds a small pre-model controller decision:

```text
LoopControlInputs
  -> determine_loop_control_action()
  -> LoopControlAction
```

The top-level verifier dispatch now goes through this boundary:

```text
ContinueRepairJob -> dispatch_repair_job_step()
RunVerifier       -> drive_task_contract_verifier()
```

`build_arbiter_candidates()` also projects verifier repair / missing verifier ownership through `LoopControlAction`. A stale `task_contract_verifier_repair_pending` flag without either `RepairJob` or `MissingVerifierJob` no longer creates a verifier-repair active job.

### Phase 4: demote legacy verifier repair projection

`VerifierRepairDecision` and `repair_job::verifier_repair_decision()` are now test-only compatibility helpers. Production recovery notes and verifier repair policy messages read `RepairJob::next_action()` directly.

## Remaining Structural Problems

1. `VerifierRepairDecision` still exists in test-only compatibility code.
   - It is no longer used as a production dispatch source.
   - Remaining work is to delete or isolate old compatibility tests once the new transition-table tests fully cover the same behavior.

2. `MissingVerifierJob` is still outside `RepairJob`.
   - It is now projected through `LoopControlAction`, but the state struct remains separate.
   - A future pass should expose a dedicated bootstrap next-action enum and remove any remaining missing-verifier special casing.

3. Artifact completion can still fail before verifier repair starts.
   - This is separate from verifier repair. It needs an artifact-completion state machine or clearer project-unit probing.

4. `project_probe` is still conservative.
   - It detects safe verifier readiness but does not yet provide a full project-unit model for every stack.

5. Legacy deterministic repair candidate generation has been removed from the
   verifier repair production path.
   - Deterministic scaffold/fallback code still needs to stay outside repair
     authority, but it no longer chooses verifier repair patches.

## Quality Plan

Run in this order:

1. `cargo fmt --check`
2. targeted verifier repair tests
3. `cargo test task_contract --lib`
4. `cargo test repair_job --lib`
5. `cargo test --lib`
6. `cargo clippy --all-targets -- -D warnings`
7. `cargo build --release`

E2E evaluation should wait until the state-transition tests pass.

## Verification Result

Completed in this pass:

- `cargo fmt --check`: pass
- `cargo test loop_control_action_tests --lib`: pass, 17 tests
- `cargo test synthetic_verifier_e2e --lib`: pass, 4 tests
- `cargo test production_repair_dispatch_does_not_call_legacy_decision_bridge --lib`: pass, 1 test
- `cargo test missing_verifier_job --lib`: pass, 6 tests
- `cargo test repair_job --lib`: pass, 133 tests
- `cargo test project_probe --lib`: pass, 5 tests
- `cargo test task_contract --lib`: pass, 80 tests
  - First sandboxed run failed because mock server creation was blocked by the sandbox.
  - Re-run outside the sandbox passed.
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo test --lib`: pass, 2925 tests
- `cargo build --release`: pass

Latest smoke evaluation, 2026-05-25:

| Group | Result | Notes |
|---|---:|---|
| FastAPI CRUD no-PAM | 4/5 `done` | 1 run ended `verifier_failed`; failure showed `.anvil-state/verifier-python/site` leaking into runtime import behavior (`State.items` missing). |
| FastAPI CRUD PAM live | 4/5 `done` | 1 run ended `verifier_failed`; delete endpoint expectation mismatch remained after repair attempts. |
| Generality smoke: Rust CLI | 0/1 `done` | Ended `transport_error` because auto test command timed out after 300s. |
| Generality smoke: Python data script | 0/1 `done` | Ended with verifier repair safe stop: `patch_rejected_repeatedly`. |
| Generality smoke: Node utility | 0/1 `done` | Ended with verifier repair safe stop: `patch_rejected_repeatedly`. |

Smoke logs are under `workspace/v0.4.23/smoke-20260525/`.

Interpretation:

- The main verifier-control regression is improved: observed verifier-owned runs did not fall back to generic `missing_repo_edits`.
- FastAPI CRUD success is much better than the earlier baseline, but not stable enough to call complete.
- Generality is not complete. The controller no longer looks FastAPI-specific, but verifier discovery / verifier execution / repair proposal quality still do not generalize reliably across Rust, Python data, and Node utility tasks.
- Remaining work should focus on verifier environment isolation and patch proposal convergence, not on adding more framework-specific repair branches.

## Implementation Notes

Production verifier repair dispatch is now narrower:

```text
task_contract_verifier_repair_pending && repair_job.is_some()
  -> dispatch_repair_job_step()
  -> RepairJob::begin_next_repair_step()
  -> RunDiagnostic | RunPatchProvider | RunVerifier | SafeStop | Done
```

The removed lower branches were not the desired authority anymore. Their responsibilities are now owned by `dispatch_repair_job_step()`.

The active-job policy projection is now:

```text
LoopControlAction::ContinueRepairJob
  -> verifier_repair_policy_for_next_action()
  -> EffectiveToolPolicy
```

New regression tests cover:

- direct target-hint policy mapping
- active-job branch reading `RepairJob::next_action()`
- active repair job winning over verifier run, artifact flow, and missing-verifier state
- stale pending flag without an owner not creating legacy repair dispatch

Test-only compatibility remains for `verifier_weak` safe-stop emission because the existing safe-stop E2E tests still use the seam.

## 2026-05-25 Follow-Up Execution

This follow-up implements the first narrow slice of the final achievement plan.

### Implemented

- Structured Python verifier dependency state remains under
  `.anvil-state/verifier-python/site`, but it is asserted to be controller-owned
  ignored workspace state.
- Structured Python verifier `PYTHONPATH` is explicitly built as:

```text
<workspace-root>:<.anvil-state/verifier-python/site>
```

  This keeps task code ahead of verifier helper dependencies while avoiding
  parent-process `PYTHONPATH` leakage.
- Verifier timeout errors from `AutoTestRunner` are now converted into verifier
  failure evidence instead of raw transport errors.
- Timeout evidence is masked and routed into the existing verifier repair flow,
  so `RepairJob` can replan, retry, or safe stop through its normal budgeted
  path.
- The legacy shell verifier path is now bounded by the same auto-test timeout
  helper as the structured verifier path, so unstructured verifier execution
  cannot hang indefinitely.

### Verification

- `cargo fmt --check`: pass
- `cargo test auto_test --lib`: pass, 148 tests after bounding the legacy shell verifier path
- `cargo test loop_control_action_tests --lib`: pass, 17 tests
- `cargo test repair_job --lib`: pass, 133 tests
- `cargo test task_contract --lib`: pass, 80 tests
- `cargo test --lib`: pass, 2929 tests
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass

### Remaining Gap

This does not yet implement a full `ProjectUnit` model. It closes the immediate
timeout-routing bug and locks down the verifier dependency path invariant, but
project-wide verifier selection still needs the broader Phase 2 work before the
generality smoke set can be expected to stabilize.

## 2026-05-26 Follow-Up Execution

This pass implements the next planning slice from
`remaining-completion-work-plan.md`.

### Implemented

- Added a minimal `ProjectUnit` fact model to `project_probe`.
- `CompletionProbeDecision::RunVerifier` now carries:
  - project root
  - manifest evidence
  - current artifact roles
  - verifier candidates
  - observed stacks
  - verifier timeout class
- Completion-probe logs now include a bounded project-unit summary.
- Added `VerifierTimeoutKind` to classify bounded verifier timeout evidence:
  - generated test hang
  - dependency setup timeout
  - environment stall timeout
  - build command timeout
  - long-running verifier
  - unknown timeout

### Verification

- `cargo test project_probe --lib`: pass, 8 tests
- `cargo test verifier_timeout --lib`: pass, 2 tests
- `cargo test auto_test --lib`: pass, 148 tests
- `cargo test task_contract --lib`: pass, 80 tests
- `cargo test repair_job --lib`: pass, 133 tests
- `cargo test loop_control_action_tests --lib`: pass, 17 tests
- `cargo test --lib`: pass, 2933 tests
- `cargo fmt --check`: pass
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass

### Remaining Gap

`ProjectUnit` is not yet the source of truth for `AutoTestRunner` verifier
selection. It is now observable and tested at the completion-probe boundary,
which is the next safe step toward moving verifier command selection behind the
same model.

## 2026-05-26 Follow-Up Execution: ProjectUnit-Aware Selection

This pass connects the minimal `ProjectUnit` model to task-contract verifier
selection without adding a new dispatch source.

### Implemented

- Added `project_probe::probe_project_unit()` so verifier execution can rebuild
  the current task unit from scoped, edited workspace facts.
- Added `ProjectUnit::allows_verifier_source()` and an
  `AutoTestRunner::detect_with_owned_test_artifacts_and_project_unit()` entry
  point.
- Task-contract verifier execution now filters AutoTest candidates to verifier
  sources admitted by the current `ProjectUnit` when one is available.
- Added a regression test for a mixed workspace where root `Cargo.toml` exists
  but the current owned test artifact is Python. The filtered path selects the
  Python structured verifier instead of the unrelated Cargo candidate.

### Remaining Gap

`ProjectUnit` is still an optional verifier-selection filter. The next broader
slice is to make verifier discovery itself produce project units directly and
remove duplicated source-specific ranking outside that model.

### Verification

- `cargo fmt --check`: pass
- `cargo test project_unit --lib`: pass, 4 tests
- `cargo test project_probe --lib`: pass, 8 tests
- `cargo test auto_test --lib`: pass, 149 tests
- `cargo test task_contract --lib`: pass, 80 tests
  - Sandboxed run hit the known mockito local-server bind restriction.
  - Re-run outside the sandbox passed.
- `cargo test --lib`: pass, 2934 tests
  - Sandboxed run hit the same local-server bind restriction.
  - Re-run outside the sandbox passed.
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `git diff --check`: pass

## 2026-05-26 Follow-Up Execution: ProjectUnit Authority And Typed Timeout Evidence

This pass tightens the previous ProjectUnit-aware verifier selection and adds
typed timeout evidence to the diagnostic packet.

### Implemented

- When task-contract verifier execution has a `ProjectUnit`, AutoTest selection
  is now built from that unit's verifier candidates instead of first running
  root-level stack detection and filtering afterward.
- A `ProjectUnit` with no verifier candidates now yields verifier missing for
  that task path instead of falling back to an unrelated root-level verifier.
- `FailurePacket` now carries `timeout_kind` as a typed
  `FailurePacketTimeoutKind` field and includes it in the diagnostic JSON.
- Deterministic verifier-repair candidates are no longer called by the
  production patch-provider main path. The old candidate builder is confined
  to test/quarantine code, and the deterministic admission bridge was removed.
- While `RepairJob` or `MissingVerifierJob` owns verifier repair dispatch,
  `build_arbiter_candidates()` now returns that verifier candidate set
  immediately. Lower-priority focused edit, artifact recovery, setup bootstrap,
  and local small-edit candidates are not built for that model turn.
- AutoTest verifier detection now filters controller-owned changed files at
  the entrypoint, so `.anvil-state` paths cannot create Python/Rust/Node
  verifier candidates or surface as Python script evidence.
- RepoEdit observation and ArtifactLedger seed helpers now reject
  controller-owned paths before they can become edited-path evidence,
  artifact evidence, verifier observations, or repair-target admission
  signals.

### Remaining Gap

`RepairJob::next_action()` now consumes typed timeout evidence for the first
safe-stop route: environment-stall timeouts stop as verifier-timeout safe stops
instead of entering patch repair. Other timeout classes still go through
diagnostic planning so generated-test hangs, dependency setup, build commands,
and long-running verifier cases can be repaired when a safe target exists.

The deterministic repair candidate builder has now been deleted. Remaining
coverage validates accepted `VerifierRepairIntent` edits and `RepairJob`
state transitions instead of preserving legacy candidate-generation behavior.

### Verification

- `cargo test project_unit --lib`: pass, 5 tests
- `cargo test failure_packet --lib`: pass, 6 tests
- `cargo fmt --check`: pass
- `cargo test auto_test --lib`: pass, 151 tests
- `cargo test repair_job --lib`: pass, 138 tests
- `cargo test loop_control_action_tests --lib`: pass, 17 tests
- `cargo test issue660_phase_d --lib`: pass, 8 tests
- `cargo test active_job_arbiter --lib`: pass, 40 tests
- `cargo test v0421_repair_runner_contract_tests --lib`: pass, 5 tests
- `cargo test artifact_ledger_phase2 --lib`: pass, 22 tests
  - Sandboxed run hit the known mockito local-server bind restriction.
  - Re-run outside the sandbox passed.
- `cargo test repo_edit_observation_ignores_controller_owned_state --lib`: pass, 1 test
- `cargo clippy --all-targets -- -D warnings`: pass after removing the production
  deterministic-repair admission bridge and quarantining the legacy helper.
- `cargo test task_contract --lib`: pass, 80 tests
  - Sandboxed run hit the known mockito local-server bind restriction.
  - Re-run outside the sandbox passed.
- `cargo test --lib -q`: pass, 2949 tests
  - Sandboxed run hit the same local-server bind restriction.
  - Re-run outside the sandbox passed.
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `git diff --check`: pass

### 2026-05-26 Follow-Up Execution: RepairJob Timeout Routing

This pass moves timeout evidence one step further into the repair-state
machine.

Implemented:

- `RepairJob` now stores `timeout_kind` as typed controller state.
- `verifier_repair_context_from_failure()` extracts the timeout kind when a
  verifier failure is converted into a repair job.
- The duplicated timeout enum in `turn.rs` was removed; timeout labels and
  repair hints now use the shared `FailurePacketTimeoutKind` type.
- `FailurePacket::from_repair_job()` preserves the typed timeout kind even when
  the bounded excerpt no longer contains the original `timeout_kind=...` token.
- `RepairJob::next_action()` safe-stops environment-stall verifier timeouts
  before patch repair. This is intentionally narrow: repairable timeout classes
  still go through diagnostic planning.

Verification:

- `cargo fmt --check`: pass
- `cargo test repair_job_environment_timeout_safe_stops_without_patch --lib`: pass
- `cargo test repair_job_verifier_passed_wins_over_stale_timeout_evidence --lib`: pass
- `cargo test failure_packet_preserves_repair_job_timeout_kind_when_excerpt_lacks_label --lib`: pass
- `cargo test verifier_repair_context_captures_typed_timeout_kind --lib`: pass
- `cargo test failure_packet --lib`: pass, 6 tests
- `cargo test repair_job --lib`: pass, 138 tests
- `cargo test verifier_timeout --lib`: pass, 2 tests
- `cargo test auto_test --lib`: pass, 151 tests
- `cargo test loop_control_action_tests --lib`: pass, 17 tests
- `cargo test task_contract --lib`: pass, 80 tests
  - Sandboxed run hit the known mockito local-server bind restriction.
  - Re-run outside the sandbox passed.
- `cargo test --lib -q`: pass, 2944 tests
  - Sandboxed run hit the same local-server bind restriction.
  - Re-run outside the sandbox passed.
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `git diff --check`: pass

### 2026-05-26 Follow-Up Execution: Final Dispatch Cleanup

This pass finishes the remaining verifier-dispatch cleanup from
`remaining-completion-work-plan.md`.

Implemented:

- Post-loop verifier execution now receives the current `ProjectUnit` and uses
  project-unit-bound verifier discovery instead of root-level fallback guesses.
- `ProjectUnit` now carries a bounded confidence label. Docs-only/no-code units
  are low-confidence and cannot trigger unrelated verifier selection.
- Repairable timeout classes now have explicit state-machine coverage:
  generated-test hang, dependency setup timeout, build-command timeout,
  long-running verifier, and unknown timeout all continue to diagnostic repair
  rather than safe-stopping prematurely.
- The old deterministic verifier-repair candidate builder and its dedicated
  candidate-generation tests were removed from `turn.rs`.
- The production patch-provider source guard remains: verifier repair patches
  must flow through the accepted semantic repair context and validated
  `VerifierRepairIntent` path.
- Python structured dependency inference now treats `types` as a Python
  standard-library module. This fixes a smoke-discovered verifier setup bug
  where `import types` was converted into `pip install types`.

Verification:

- `cargo test project_unit --lib`: pass, 6 tests
- `cargo test verifier_skill --lib`: pass, 8 tests
- `cargo test repair_job --lib`: pass, 139 tests
- `cargo test auto_test --lib`: pass, 151 tests
- `cargo test task_contract --lib`: pass, 80 tests
  - Sandboxed run hit the known mockito local-server bind restriction.
  - Re-run outside the sandbox passed.
- `cargo test loop_control_action_tests --lib`: pass, 17 tests
- `cargo test --lib -q`: pass, 2942 tests
  - Sandboxed run hit the same local-server bind restriction.
  - Re-run outside the sandbox passed.
- `cargo fmt --check`: pass
- `cargo clippy --all-targets -- -D warnings`: pass
- `cargo build --release`: pass
- `git diff --check`: pass

Small smoke gate:

| Case | Result | Notes |
|---|---:|---|
| FastAPI CRUD | `done` | First run exposed the `types` stdlib dependency inference bug. After the fix, rerun completed and verified with `python3 -B -m pytest -p no:cacheprovider tests/test_main.py`. |
| Rust text counter CLI | `done` | Completed and verified with `cargo test --test main`. |
| Python CSV sales analyzer | `verifier_failed` | Controlled verifier-repair flow, but did not converge. Final failure was one generated integration test mismatch around output totals after several implementation repairs. |

Interpretation:

- The controller no longer falls into generic `missing_repo_edits` in these
  smoke runs.
- The `types` verifier setup bug was a real generic Python verifier issue and
  is fixed.
- Repair convergence is still not fully solved for arbitrary Python CLI tasks.
  The remaining problem is patch quality / convergence, not dispatch ownership.

- Do not start another large PAM/no-PAM evaluation cycle until the remaining
  Python CLI repair-convergence gap has a targeted fix or a controlled safe
  stop acceptance criterion.

### Genericity Evaluation: 20 Case Smoke

Run root:
`/Users/maenokota/share/work/localwork/anvilv0.4/anvilwork/generic-eval-20260526-004603`

Command:
`anvildev -m qwen3.6:27b-coding-nvfp4 --sidecar-model qwen3.5:9b -y --fresh-session --no-footer --deterministic-fallback full --max-iterations 50`

Summary:

- Total: 20
- Done: 5
- Actionable safe stop / verifier safe stop: 9
- Failure without safe stop: 2
- External harness timeout at 420s: 4

Successful cases:

- FastAPI CRUD
- FastAPI ToDo backend
- Python sales CSV analysis
- Rust stack library
- Rust line filter CLI

Observed remaining failure modes:

- Verifier repair often reaches a safe stop after repeated rejected patches.
  This is safer than forcing a weak patch, but it is not completion.
- Some non-FastAPI Python and Node cases exceed the 420s external harness
  timeout, which means repair convergence remains too slow or cyclic.
- A small number of cases still fail before editing all required artifact roles,
  especially when the model creates only setup or implementation artifacts.
- The repaired flow is no longer limited to FastAPI, but generic completion is
  not yet stable enough to call the original goal fully achieved.

### Follow-up Generic Smoke After Repair-Pass Budget

Run root:
`/private/tmp/anvil-v0423-postfix-generic`

No-PAM, 5 mixed cases, `target/release/anvil`, `--max-iterations 50`:

| Case | Result | Notes |
|---|---:|---|
| FastAPI CRUD | `done` | Verified, but slow: 384s and many repair cycles. |
| Python CSV CLI | `repair_safe_stop` | Controlled stop on repair-plan role mismatch; no uncontrolled timeout. |
| Rust word-count CLI | `missing_repo_edits` | `cargo init` scaffold was not enough to satisfy requirement-specific implementation evidence. |
| Node JSON formatter CLI | `repair_exhausted` | Implementation/tests/docs were generated, but repair did not converge. Target path normalization still needs work. |
| docs-only README | `done` | Correctly handled as documentation-only work. |

Conclusion:

- The repair-pass wall-clock budget improves control stability by preventing
  the controller from waiting indefinitely inside one repair provider call.
- Generic performance is still not acceptable for broad unattended use.
  The next structural work should focus on bootstrap-artifact evidence,
  workspace-relative path normalization in repair targets, and repair patch
  convergence rather than adding domain-specific templates.

### Expanded Generic Smoke: Additional 10 No-PAM Cases

Run root:
`/private/tmp/anvil-v0423-generic-expanded`

Command shape:
`target/release/anvil -m qwen3.6:27b-coding-nvfp4 --sidecar-model qwen3.5:9b -y --fresh-session --oneshot --no-footer --deterministic-fallback full --max-iterations 50`

| Case | Terminal | Duration | Quality note |
|---|---:|---:|---|
| Python TOML config CLI | `repair_exhausted` | 222s | Thin initial artifacts; repair proposals were repeatedly rejected. |
| Rust slug library | `done` | 23s | False positive: only `README.md` and `tests/test_main.py` were created; no Rust implementation or Cargo project. |
| Node CSV-to-JSON CLI | `repair_exhausted` | 150s | Generated implementation/tests/docs, but repair did not converge. |
| Python file-renamer CLI | `repair_exhausted` | 355s | Repeated implementation repairs; final verifier still had 3 failing tests. |
| Rust JSONL counter CLI | `repair_exhausted` | 239s | Generated Rust project and tests, then exhausted after rejected patches. |
| FastAPI notes API | `done` | 147s | Valid completion; verifier passed after repair. |
| Python Markdown lint CLI | `repair_exhausted` | 416s | Near miss: 13/14 tests passed, but repair exhausted on code-block handling. |
| Node ToDo JSON CLI | `repair_exhausted` | 283s | Generated artifacts; repeated patch rejection led to safe stop. |
| Rust JSON config merge CLI | `done` | 147s | Valid completion; verified with `cargo test --test main`. |
| Docs-only SRE runbook | `missing_repo_edits` | 190s | No file edit; artifact completion retry budget exhausted. |

Aggregate:

- Raw terminal `done`: 3/10
- Quality-adjusted valid `done`: 2/10
- False positive `done`: 1/10
- Controlled repair exhaustion / safe stop: 6/10
- Artifact completion failure before any edit: 1/10
- Uncontrolled timeout: 0/10

Interpretation:

- The latest repair-pass budget change is effective for control stability:
  none of the additional cases hung indefinitely inside repair.
- Generic task completion remains weak. The dominant failure is no longer
  dispatch falling into a random recovery path, but repair quality and
  artifact-evidence correctness.
- The Rust slug false positive is a serious correctness gap: verifier
  selection and artifact completion accepted a Python dummy test for a Rust
  library request. This needs priority over more repair heuristics.
- Docs-only handling improved in one earlier smoke but still has an initial
  edit-stability gap; documentation artifacts need the same strict-but-simple
  target execution guarantee as implementation/test artifacts.
