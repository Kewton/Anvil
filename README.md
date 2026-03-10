# Anvil

Anvil is a local-first coding agent runtime for local LLM workflows, intended to be implemented in Rust.

## Current Scope

- interactive CLI and `-p` mode
- PM-led coordination with bounded subagents
- explicit runtime permissions
- explicit trust boundaries
- resumable session state and handoff files
- per-role model routing derived from a canonical role registry

## Repository Layout

- `src/`: Rust implementation
- `schemas/`: canonical JSON schemas and registry data
- `prompts/`: prompt templates
- `docs/`: implementation-facing documentation
- `workspace/`: product and design drafts

See [docs/directory-structure.md](docs/directory-structure.md) for the current layout plan.

## Development

```bash
cargo run -- --help
```

```bash
cargo test
```

## Status

This repository currently contains the initial Rust project skeleton and supporting design documents.
