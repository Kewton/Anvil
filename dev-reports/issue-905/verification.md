Issue #905 Verification

Commands run:

- `cargo test issue905` - passed.
- `cargo fmt --check` - initially failed on formatting in the new regression file.
- `cargo fmt` - applied formatting.
- `cargo fmt --check` - passed.
- `cargo test issue905` - passed after formatting.
- `cargo clippy --all-targets` - passed.
- `cargo test` - failed under the sandbox because mockito-based tests could not bind local test servers (`Operation not permitted`).
- `cargo test` with escalated local permissions - passed.
- After a final comment-only ASCII cleanup: `cargo fmt --check` and `cargo test issue905` - passed.

Focused regression coverage:

- `agent::loop_run::issue905_pam_completion_tests::pam_advisory_without_evidence_cannot_produce_task_contract_done`
- `agent::loop_run::task_contract::tests::issue905_completion_authority_predicate_lists_deterministic_evidence_only`

Result: acceptance criteria are satisfied. Completion remains driven by deterministic `TaskContract` evidence; PAM advice is retained as advisory/eval metadata and cannot produce `done` without verifier or artifact evidence.
