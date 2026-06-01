Issue #866 Design Note

Goal: make `done` a result of evidence-based completion, not model final prose.

Current shape:
- `TaskContract` already projects required artifacts and verifier needs.
- `ArtifactRecoveryAction` already returns `Continue`, `RunVerifier`, `SafeStop`, or `Done`.
- `handle_actor_loop_completion` still turns an Act-mode final prose reply into `Done` after earlier recovery hooks pass.

Design:
- Treat `ArtifactRecoveryAction::Done` as the only Act-mode contract path that may promote final prose to `Done`.
- If a task contract exists and its action is not `Done`, earlier contract handlers keep owning recovery or safe-stop.
- If no task contract exists, keep the legacy non-contract completion path.
- Add a small eval-log field for completion reason so terminal analysis records whether `done` came from contract evidence, answer-only, plan completion, or legacy completion.

Risk control:
- Keep docs-only tasks verifier-free through the existing `CompletionPolicy` docs exemption.
- Preserve Plan-mode completion behavior.
- Add focused unit tests around contract-gated completion and eval-log serialization.
