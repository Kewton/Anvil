# Issue 835 Design Note

## Scope

Investigate why a docs-only README task can terminate as
`safe_stop_verifier_missing` after the README artifact exists and an external
postcheck passes.

## Approach

1. Inspect the request classifiers that decide docs-only vs test-bearing work:
   `TaskContract::from_request`, `request_asks_for_test_artifact`, and
   `RequiredBehaviorContract.test_execution_required`.
2. Trace every production path that maps `VerifierMissing` to the terminal
   `safe_stop_verifier_missing` exit reason.
3. Add focused tests for the README prompt shape so future changes cannot
   silently reclassify docs-only wording as a test-execution task.
4. Record the source file and line that emits the observed terminal reason,
   plus the handoff recommendation for the CompletionPolicy work under #833.

## Expected Change Shape

This is an investigation issue, not the policy fix. Code changes should be
limited to regression tests and dev-report documentation unless the source
trace reveals a small, clearly safe classifier bug.
