# Design Note

Issue #899 focuses on split completion authority caused by raw keyword checks.

Plan:

- make `quality::request_explicitly_requires_tests` delegate to the `TaskContract` test-artifact predicate,
- make `request_asks_for_test_artifact` negation-aware for English and Japanese test/code prohibition phrases,
- add regression coverage for `Do not create code or tests` and docs-only README tasks.

