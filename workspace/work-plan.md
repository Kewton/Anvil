# Anvil Work Plan

## Completed Definition Work

- [x] Define project goals
- [x] Analyze `vibe-local`
- [x] Refine `vibe-local` analysis into `Facts / Inferences / Implications`
- [x] Define the Anvil concept
- [x] Create usage-image mockups
- [x] Create `anvil-architecture.md`
- [x] Create `anvil-cli-spec.md`
- [x] Create `anvil-state-model.md`
- [x] Create `anvil-tool-protocol.md`
- [x] Define measurable comparison axes against `vibe-local`
- [x] Decide repository layout for the Rust implementation
- [x] Define Rust Foundation plan and scope
- [x] Define bootstrap and config model
- [x] Define initial implementation slice
- [x] Define core crates or modules
- [x] Implement structured logging and error model

## Phase 1: Create the Rust Skeleton

- [x] Create initial Cargo project structure
- [x] Create module skeleton under `src/`
- [x] Create shared `contracts` layer
- [x] Add module tests and integration-test skeleton

## Phase 2: Build the Core App Foundation

- [x] Implement config loading and app bootstrap
- [x] Implement `EffectiveConfig`
- [x] Implement provider capability model
- [x] Implement top-level app error model
- [x] Implement structured event definitions

## Phase 3: Build State and Session Primitives

- [x] Implement state primitives and state snapshots
- [x] Implement state transition rules
- [x] Implement message model
- [x] Implement session persistence
- [x] Implement interruption-safe session normalization

## Phase 4: Expand the TUI Into the Intended Operator Console

- [x] Implement TUI skeleton
- [x] Implement startup screen
- [x] Implement visual separation for `[U]`, `[A]`, `[T]`
- [x] Implement status/footer area
- [x] Implement mock `Ready`, `Thinking`, `AwaitingApproval`, and `Interrupted` views
- [x] Implement mock plan display during thinking
- [x] Implement mock reasoning-log display during thinking
- [x] Implement full `Working` and `Done` views
- [x] Replace mock rendering with runtime-driven rendering

## Phase 5: Replace Mock Flow With Runtime-Driven Flow

- [x] Implement mock/runtime bridge loop
- [x] Implement one-tool-call approval flow through runtime state
- [x] Implement interruption flow from `Thinking -> Interrupted -> Ready` through runtime state
- [x] Demonstrate one full runtime-driven turn through the visible states

## Phase 6: Add Real Runtime Integration

- [x] Add provider request/response contracts and a fake-provider live-turn slice
- [x] Implement provider abstraction for local LLM backends
- [x] Implement Ollama-first backend
- [x] Implement basic agent loop
- [x] Implement message/context handoff between session and provider
- [x] Implement interruption and cancellation handling for live runtime

## Phase 7: Add the Typed Tool System

- [x] Implement typed tool registry
- [x] Implement tool input and result schemas
- [x] Implement execution policy classes
- [x] Implement permission flow
- [x] Implement parallel-safe tool execution path
- [x] Implement rollback and checkpoint support

## Phase 8: Reach the First Useful Version

- [x] Support startup and interactive prompt
- [x] Support user input -> agent output flow
- [x] Support tool execution display
- [x] Support follow-up instructions in one session
- [x] Support session save and resume
- [x] Implement slash command framework
- [x] Reach a usable local-first CLI prototype

## Phase 9: Competitive Validation

- [ ] Compare startup and first-use flow against `vibe-local`
- [ ] Compare iteration latency against `vibe-local`
- [ ] Compare interruption and recovery behavior against `vibe-local`
- [ ] Compare long-session context handling against `vibe-local`
- [ ] Identify remaining gaps versus Claude Code-level UX

## Phase 10: Later Expansion

- [ ] Add custom slash command extensions
- [ ] Add richer planning and execution flows
- [ ] Add improved large-repo retrieval
- [ ] Add additional local model backends if needed
- [ ] Add more advanced UX features without breaking core clarity
