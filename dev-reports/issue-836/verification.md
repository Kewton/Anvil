# Issue 836 Verification

## Commands

- `cargo fmt --check`
  - Passed.
- `cargo test project_intent_classifies --lib`
  - Passed: 2 focused tests.
- `cargo clippy --all-targets --all-features`
  - Passed.
- `cargo test`
  - First sandbox run failed because mockito-backed tests could not bind local servers (`Operation not permitted` / closed channel).
  - Escalated rerun passed: full suite passed.
- `cargo build --release`
  - Passed.

## Acceptance Notes

- Existing failure prompt shapes now have `ProjectIntent` unit coverage for Rust CLI word counter and Node CLI JSON formatter.
- `TaskContract::from_request` scrubs nested behavior `required_artifacts` / `verification`, leaving operational artifact and verification state on `TaskContract`.
- No new `request_*` helper was added; `request_asks_for_verification` was removed and replaced by `VerificationRequirement` projection.
