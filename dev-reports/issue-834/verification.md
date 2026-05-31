# Issue 834 Verification

## Commands Run

```bash
cargo test detects_nextjs_request_from_active_task_or_plan_only
cargo test detects_explicit_scaffold_frameworks
cargo test infer_work_mode_detects_specific_edit_domains
cargo fmt --all
```

All four commands passed.

## Broader Check Note

I also ran:

```bash
cargo test scaffold
```

The edited scaffold gate tests passed within that run, but the broad filter also selected unrelated `artifact_ledger_phase2_tests` that use `mockito`. Those failed in this sandbox because the local test server could not start: `Operation not permitted (os error 1)`. I did not treat that as a regression from this issue because the exact focused tests passed and the failing tests are outside the changed gate.

## Reproduction Notes

Pure reproduction:

1. Evaluate the Node CLI prompt through `classify_work_mode_json`; it follows the existing assertion at `src/modes/plan_act.rs:741` and selects `WorkMode::GenericCode`.
2. Evaluate the same prompt through `requested_scaffold_framework`; the new assertion in `src/agent/loop_run/truncate_tests.rs` pins `None`.
3. Evaluate the same prompt through `task_or_plan_requires_nextjs_scaffold`; the new assertion pins `false`.
4. From those three facts, `mode_deterministic_scaffold_spec` has no Node spec to materialize and `maybe_apply_deterministic_nextjs_scaffold` returns `NotApplicable`, so deterministic scaffold launch cannot create `package.json`.
