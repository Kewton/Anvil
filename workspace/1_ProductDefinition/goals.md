# Anvil Goal Definition

## Vision

Anvil is a coding agent implemented in Rust and optimized for local LLMs.
It targets users who want flagship-class coding workflows without depending on hosted frontier models.

## Product Goals

1. Build a Rust-based coding agent specialized for local LLM operation.
2. Assume a local LLM baseline as of March 2026 with the following profile:
   - Model size: around 20 GB
   - Quality: slightly better than GPT-5 mini
   - Speed: about half the speed of GPT-5 mini
   - Context length: more than 200,000 tokens
3. Exceed `vibe-local` in speed, quality, and stability.
4. Design an extensible architecture that can keep up with the UX evolution of the latest coding agents.
5. Deliver a UX comparable to Claude Code, and at minimum better than `vibe-local`.

## UX Requirements

- Include an ASCII-art-like logo.
- Support custom slash commands.
- Make user input and agent output visually distinguishable at a glance.
- Keep the terminal interaction model fast, legible, and low-friction for long coding sessions.

## Architecture Principles

- Rust-first implementation focused on predictable performance and operational stability.
- Local-first design: optimize for on-device inference constraints rather than cloud assumptions.
- Extensible internal boundaries so new UX features, tools, and model backends can be added without large rewrites.
- Long-context aware workflows that take advantage of 200k+ token windows without degrading responsiveness.
- Reliable execution model with strong error handling, cancellation, and recovery behavior.

## Competitive Bar

Anvil should be judged against:

- `vibe-local` for performance, response quality, and robustness.
- Claude Code for interaction quality, usability, and overall operator experience.

## Success Criteria

- A user can complete end-to-end coding tasks locally with a UX that feels modern and efficient.
- Typical coding workflows are measurably faster or more stable than `vibe-local`.
- The system architecture supports iterative adoption of new agent UX patterns without structural rework.
- Core terminal output remains immediately scannable, with clear separation between user actions and agent actions.

## Non-Goals For Now

- Optimizing primarily for remote-hosted flagship models.
- Building a generic chat assistant unrelated to coding workflows.
- Chasing benchmark wins at the expense of day-to-day usability and stability.
