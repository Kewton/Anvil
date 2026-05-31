# Issue 839 Design

## Context

The repair loop already normalizes verifier reruns into `VerifierDelta` and routes
the production controller through `RepairJob::next_action()`. The smallest safe
change is to keep the policy decision in that state machine rather than adding a
new dispatcher or provider abstraction.

## Design

- Treat repeated `VerifierDelta::Unchanged` observations as failure-signature
  iterations.
- If the same signature repeats but no controller repair diff has been applied,
  keep the existing targeted repair path alive instead of burning diagnostic
  budget.
- Stop safely on the third repeated same-signature observation.
- Keep `VerifierDelta::DifferentFailure` as a continue/replan signal so a
  changed signature does not get classified as a converged repair loop.
- Leave setup/bootstrap failures on the existing setup recovery path, where
  dependency/config verifier failures do not create a semantic repair plan.

## Tests

Focused unit coverage will pin:

- same signature plus no diff routes to targeted repair;
- three same-signature iterations safe-stop;
- changed signature continues;
- setup failure stays in setup recovery, not semantic repair.
