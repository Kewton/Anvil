## Design

Issue #863 needs `TaskContract` to describe non-coding work without changing
the existing role-based completion gates. The smallest coherent change is to
add an explicit generic layer to `task_contract.rs`:

- `TaskKind` classifies the prompt as `coding`, `docs`, `data`, `research`, or
  `ops`.
- `TaskDeliverable` records the artifact role, target path when known, and
  required sections for documentation deliverables.
- `CompletionPolicy` stores the `task_kind` alongside the existing
  `CompletionProjectIntent` so completion semantics are explicit per generic
  task class.

The existing `required_artifacts`, `required_artifact_identities`, and
`required_behavior` fields remain the execution-facing contract for the agent
loop. The new fields are derived in `TaskContract::from_request`, then tests
pin representative prompts for coding, docs, data, research, and ops.
