# ANVIL.md

## Purpose

This file defines stable project instructions for Anvil contributors and agents.

## Development Rules

- Use TDD by default.
- For every new behavior change:
  - add or update a failing test first
  - implement the smallest change to make it pass
  - refactor only after the test suite is green
- Do not merge untested behavior.
- Prefer integration tests for cross-module behavior.
- Prefer focused module tests for local state or parsing logic.

## Current Testing Policy

- `tests/config_bootstrap.rs`
  - config loading
  - provider bootstrap
  - startup event expectations
- `tests/state_session.rs`
  - state transitions
  - session persistence
  - interruption normalization
- `tests/tui_console.rs`
  - startup screen
  - operator console rendering
  - working and done views

## Product Guardrails

- Preserve explicit state transitions.
- Preserve session integrity on interruption.
- Keep `[U]`, `[A]`, and `[T]` visually distinct.
- Keep the footer/status area visible and compact.
- Avoid regressing local-first startup clarity.
