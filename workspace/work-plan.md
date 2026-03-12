# Anvil Work Plan

## Planning

- [x] Define project goals
- [x] Analyze `vibe-local`
- [x] Refine `vibe-local` analysis into `Facts / Inferences / Implications`
- [x] Define the Anvil concept
- [x] Create usage-image mockups

## Product Definition

- [x] Create `anvil-architecture.md`
- [x] Create `anvil-cli-spec.md`
- [x] Create `anvil-state-model.md`
- [x] Create `anvil-tool-protocol.md`
- [x] Define measurable comparison axes against `vibe-local`

## Rust Foundation

- [ ] Decide repository layout for the Rust implementation
- [ ] Create initial Cargo project structure
- [ ] Define core crates or modules
- [ ] Implement config loading and app bootstrap
- [ ] Implement structured logging and error model

## Core Runtime

- [ ] Implement provider abstraction for local LLM backends
- [ ] Implement Ollama-first backend
- [ ] Implement session persistence
- [ ] Implement message and context model
- [ ] Implement basic agent loop
- [ ] Implement interruption and cancellation handling

## Tool System

- [ ] Implement typed tool registry
- [ ] Implement tool input and result schemas
- [ ] Implement execution policy classes
- [ ] Implement permission flow
- [ ] Implement parallel-safe tool execution path
- [ ] Implement rollback and checkpoint support

## Terminal UX

- [ ] Implement TUI skeleton
- [ ] Implement visual separation for `[U]`, `[A]`, `[T]`
- [ ] Implement status/footer area
- [ ] Implement plan display during thinking
- [ ] Implement reasoning-log display during thinking
- [ ] Implement slash command framework

## First Useful Version

- [ ] Support startup and interactive prompt
- [ ] Support user input -> agent output flow
- [ ] Support tool execution display
- [ ] Support follow-up instructions in one session
- [ ] Support session save and resume
- [ ] Reach a usable local-first CLI prototype

## Competitive Validation

- [ ] Compare startup and first-use flow against `vibe-local`
- [ ] Compare iteration latency against `vibe-local`
- [ ] Compare interruption and recovery behavior against `vibe-local`
- [ ] Compare long-session context handling against `vibe-local`
- [ ] Identify remaining gaps versus Claude Code-level UX

## Later Expansion

- [ ] Add custom slash command extensions
- [ ] Add richer planning and execution flows
- [ ] Add improved large-repo retrieval
- [ ] Add additional local model backends if needed
- [ ] Add more advanced UX features without breaking core clarity
