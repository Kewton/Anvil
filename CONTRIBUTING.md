# Contributing

Thanks for improving Anvil. The project is intentionally small and local-first:
changes should keep the Ollama-only path simple, testable, and understandable
for local LLMs.

## Development Setup

Prerequisites:

- Rust stable with edition 2024 support
- Ollama for live E2E tests
- `cargo fmt`, `cargo clippy`, and `cargo test`

Useful commands:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo build --release
```

## Design Expectations

- Prefer simple protocol changes over broad state-machine additions.
- Do not add provider abstractions; Anvil is Ollama-first.
- Treat pattern matching as evidence, not final truth, for intent, quality, and
  verifier decisions.
- Keep safety guards deterministic and easy to audit.
- Add focused tests for new behavior, especially local LLM failure modes.

## Pull Requests

Each pull request should include:

- a short problem statement
- the behavioral change
- tests or a clear explanation of why tests were not practical
- any user-facing CLI, config, or documentation updates

For E2E-sensitive changes, include the model, prompt set, number of runs, and
pass/fail summary in the PR description.
