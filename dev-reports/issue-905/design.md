Issue #905 Design Note

Goal: make the PAM boundary explicit: PAM may shape prompt/candidate context, but it must not be accepted as completion authority.

Current state:
- `CompletionEvidence` has deterministic variants only: repo edits, verifier exits, deliverable checkers, and answer-only.
- `TaskContract` decides completion from an `EvidenceSet` plus artifact/verifier state.
- PAM advisory decisions live in `last_pam_decision_this_turn`, memory reports, and eval summaries; they are not part of `EvidenceSet`.

Design:
- Add a small deterministic-authority predicate on `CompletionEvidence` and route `TaskContract` evidence acceptance through it.
- Keep the predicate conservative and closed over the existing enum variants so PAM cannot be smuggled in as an accepted evidence class.
- Add focused regression coverage proving that a live PAM advisory alone cannot make a repository task complete and that PAM eval metadata remains advisory-only.
