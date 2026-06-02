# Design Note

Issue #898 is the parent epic for the no-PAM authority-chain failures observed in v0.4.33.

This PR implements a focused consolidation slice rather than a full rewrite:

- keep `TaskContract` / `CompletionEvidence` as deterministic completion authority,
- close the observed no-PAM false-positive/false-negative gaps,
- add generated-test preflight before owned tests can bind verifier authority,
- add machine-readable eval taxonomy fields for PAM variant, task kind, outcome agreement, and failure authority.

Non-goals:

- no provider abstraction,
- no old Anvil architecture revival,
- no broad verifier orchestration rewrite.

