# Changelog

## [0.1.0] - 2026-04-16

- Rebuilt Anvil from scratch as an Ollama-first local coding agent.
- Removed the old generic agent architecture and its large state-machine surface.
- Added a minimal tool-first loop with `Bash`, `Read`, `Write`, `Edit`, `Glob`, and `Grep`.
- Added session persistence, deterministic compaction, Plan / Act mode, and git checkpoint / rollback.
- Added `<think>` stripping and XML tool-call fallback for local models.
- Preserved the existing CI and GitHub Releases process with the `anvil` binary name unchanged.
