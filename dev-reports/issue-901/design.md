# Design Note

Issue #901 focuses on generated tests becoming verifier authority before validation.

Plan:

- add a small generated-test preflight module,
- reject or downgrade owned tests that are missing, contain known racy shared-file patterns, use brittle Rust integration-test binary path construction, or assert unsupported non-ASCII slug behavior,
- filter owned test artifacts before they are passed to verifier binding.

