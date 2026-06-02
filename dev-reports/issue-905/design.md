# Design Note

Issue #905 requires PAM to stay advisory-only.

The current tree already records `advisory_only=true` and keeps PAM out of `CompletionEvidence`. This PR reinforces that by making evaluation taxonomy derive PAM as a variant/attribution field only, never as completion authority.

