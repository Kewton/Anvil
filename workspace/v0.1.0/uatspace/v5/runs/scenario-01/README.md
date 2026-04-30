# Test Project

This project is a small local-first agent prototype.

## Architecture

- `src/config.rs` merges CLI, environment, and local config.
- `src/model_registry.rs` chooses a main and sidecar Ollama model.
- `src/agent/loop_run` manages the turn loop, recovery, deterministic fallbacks, and quality checks.
- `src/session` stores snapshots, working memory, and compaction summaries.

## Current Risks

- Recovery policy can over-assume repository edits when the user only wants analysis.
- Deterministic fallbacks must stay narrowly scoped to avoid applying UI templates to unrelated work.
- Session context should keep structured state separate from user-visible conversation history.
