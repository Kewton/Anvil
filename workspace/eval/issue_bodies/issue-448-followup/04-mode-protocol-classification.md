## Summary

Mode/protocol classification can override otherwise useful RepoGraph and ANVIL.md context.

Issue 448 path-scoped docs tasks were classified as read-only or TypeScript UI work, causing failures unrelated to RepoGraph itself.

## Problem

RepoGraph and path-scoped ANVIL.md can only help if the execution protocol is correct. In the live docs runs:

- one edit request became answer-only/read-only and blocked Edit;
- another Markdown edit was evaluated through a TypeScript UI protocol and failed despite editing a Markdown file.

## Acceptance Criteria

- Documentation edit requests should enter an edit-capable documentation protocol, not read-only mode.
- Markdown/doc tasks should not be evaluated by TypeScript UI criteria unless the user explicitly asks for a UI.
- Path-scoped ANVIL.md should be injected based on explicit user-mentioned paths before protocol-specific quality checks.
- Add E2E coverage for docs-only edits with scoped ANVIL.md markers.

## Evidence

- `raw/e2e-path-anvil-qwen36.log`
- `raw/e2e-path-anvil-qwen36-r2.log`
- `raw/e2e-path-src-anvil-qwen36.log` shows the same mechanism can work when mode classification is correct.
