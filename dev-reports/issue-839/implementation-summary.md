# Issue 839 Implementation Summary

## Changed

- Added a failure-signature iteration threshold to `RepairJob`.
- Routed unchanged verifier observations through a signature-aware policy:
  - same signature with no controller-applied diff keeps targeted repair active;
  - third same-signature iteration safe-stops as repair exhausted;
  - unchanged after an applied repair still replans through the existing path.
- Kept changed signatures as a continue/replan signal independent of exhausted
  diagnostic retry count.
- Recorded verifier lifecycle events after semantic rerun dispatch so cluster
  transitions that upgrade the rerun outcome to `NewFailure` are visible to
  `next_action()`.

## Tests

- Added issue-focused unit tests for:
  - same signature plus no diff targeting repair;
  - three same-signature iterations safe-stopping;
  - changed signature continuing;
  - setup dependency failure staying out of semantic repair.
