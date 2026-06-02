# Design Note

Issue #902 asks for task-kind verifier adapters and structured diagnostics.

The current tree already contains `DocsVerifier`, `DataVerifier`, structured data evidence, and terminal diagnostics. This PR adds a focused `GeneratedTestPreflightDiagnostic` surface for the observed no-PAM generated-test failures and leaves the broader adapter architecture intact.

