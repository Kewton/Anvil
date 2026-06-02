# Issue 899 Design Note

## Problem

Completion and verifier dispatch still have paths that consult raw request
keywords after `TaskContract::from_request` has built a final
`CompletionPolicy`. Negated instructions such as "Do not create code or tests"
must be resolved once in the contract and not reinterpreted by later gates.

## Approach

- Keep the existing deterministic parsing in `TaskContract` as the authority.
- Expose the final policy booleans needed by downstream gates instead of
  rereading `RequiredBehaviorContract` or `quality` keyword helpers.
- Build `RequestContext` and post-loop verifier binding from one
  `TaskContract::from_request` value.
- Add focused regressions for docs-only negated code/test requests and for
  coding requests that explicitly require tests.

## Scope

No provider or loop architecture changes. The change is limited to completion
contract/policy consumers and regression tests.
