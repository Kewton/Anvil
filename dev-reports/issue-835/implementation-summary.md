# Issue 835 Implementation Summary

## Conclusion

The terminal `safe_stop_verifier_missing` string is emitted from the task
contract safe-stop mapping in `src/agent/loop_run/actor_loop_flow.rs:4638`.
The actor-loop path reaches that mapping through
`handle_actor_loop_task_contract_safe_stop` at
`src/agent/loop_run/actor_loop_flow.rs:2504`.

The underlying planner producer for the same reason is
`TaskContract::evaluate_inner`:

- `src/agent/loop_run/task_contract.rs:756` gates the branch on
  `required_behavior.test_execution_required`.
- `src/agent/loop_run/task_contract.rs:759-763` returns
  `SafeStopReason::VerifierMissing` when the owned-test artifact slice is
  empty.

There is a second post-loop verifier source for the same final exit reason:

- `src/agent/loop_run/verifier_skill.rs:248` enters the structured verifier
  branch only when `test_execution_required && protocol_demands_verifier`.
- `src/agent/loop_run/verifier_skill.rs:319-324` returns
  `VerifierOutcome::Missing`.
- `src/agent/loop_run/success.rs:535-556` maps that outcome to
  `ExitReason::SafeStopVerifierMissing`.

For the README prompt shape available in-tree
(`README.md` with install/run/test-method wording), this investigation found
that the task contract is docs-only:

- `verification_required == false`
- `required_behavior.test_execution_required == false`
- `evaluate_with_owned_test_artifacts(..., &[]) == Done`

That means a run using this exact prompt should not trigger the task-contract
owned-test SafeStop producer. If run 105/115 used different wording that
contains a direct test-artifact marker such as English `tests`, then the source
is the `task_contract.rs:756-763` producer above. Otherwise the remaining
source is the post-loop verifier/CompletionPolicy path listed above.

The referenced `workspace/v0.4.27` notes were present in the sibling
`../Anvil-develop` worktree. They confirm that rows `105` and `115` are the
Docs SRE runbook cases, both ended `safe_stop_verifier_missing`, and both
passed the external README postcheck. Those notes do not include the exact raw
prompt text, so the case split above is intentionally preserved.

## Changes

- Strengthened
  `docs_readme_test_method_wording_does_not_require_test_artifact` in
  `src/agent/loop_run/task_contract.rs` to assert the production owned-test
  evaluation path returns `Done` for the docs-only README prompt.
- Added issue reports under `dev-reports/issue-835/`.

## #833 CompletionPolicy Handoff

CompletionPolicy should make one component responsible for deciding final
success vs safe-stop after artifact completion. In particular:

- Treat docs protocol completion (`UsageDocs` evidence with
  `test_execution_required == false`) as terminal success even if broad
  verifier-demand heuristics see words like `test` in README-content
  requirements.
- Reconcile the broad `quality::request_explicitly_requires_tests` predicate
  with the narrower `task_contract::request_asks_for_test_artifact` SSOT so
  docs content such as "test method" or "testing instructions" does not demand
  a bound owned-test verifier.
- Preserve a source field in safe-stop telemetry so future
  `safe_stop_verifier_missing` reports distinguish planner completion,
  task-contract verifier, and post-loop verifier origins without source
  spelunking.
