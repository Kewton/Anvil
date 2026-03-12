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

- [ ] Create initial Cargo project structure
- [ ] Create module skeleton under `src/`
- [ ] Create shared `contracts` layer
- [ ] Add module tests and integration-test skeleton

## Phase 2: Build the Core App Foundation

- [ ] Implement config loading and app bootstrap
- [ ] Implement `EffectiveConfig`
- [ ] Implement provider capability model
- [ ] Implement top-level app error model
- [ ] Implement structured event definitions

## Phase 3: Build State and Session Primitives

- [ ] Implement state primitives and state snapshots
- [ ] Implement state transition rules
- [ ] Implement message model
- [ ] Implement session persistence
- [ ] Implement interruption-safe session normalization

## Phase 4: Build the First TUI Vertical Slice

- [ ] Implement TUI skeleton
- [ ] Implement startup screen
- [ ] Implement visual separation for `[U]`, `[A]`, `[T]`
- [ ] Implement status/footer area
- [ ] Implement `Ready`, `Thinking`, `AwaitingApproval`, `Working`, `Interrupted`, and `Done` views
- [ ] Implement plan display during thinking
- [ ] Implement reasoning-log display during thinking

## Phase 5: Build the Mock Runtime Slice

- [ ] Implement mock runtime loop
- [ ] Implement one-tool-call approval flow
- [ ] Implement interruption flow from `Thinking -> Interrupted -> Ready`
- [ ] Demonstrate one full mock turn through the visible states

## Phase 6: Add Real Runtime Integration

- [ ] Implement provider abstraction for local LLM backends
- [ ] Implement Ollama-first backend
- [ ] Implement basic agent loop
- [ ] Implement message/context handoff between session and provider
- [ ] Implement interruption and cancellation handling for live runtime

## Phase 7: Add the Typed Tool System

- [ ] Implement typed tool registry
- [ ] Implement tool input and result schemas
- [ ] Implement execution policy classes
- [ ] Implement permission flow
- [ ] Implement parallel-safe tool execution path
- [ ] Implement rollback and checkpoint support

## Phase 8: Reach the First Useful Version

- [ ] Support startup and interactive prompt
- [ ] Support user input -> agent output flow
- [ ] Support tool execution display
- [ ] Support follow-up instructions in one session
- [ ] Support session save and resume
- [ ] Implement slash command framework
- [ ] Reach a usable local-first CLI prototype

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
