# Anvil Architecture

## Purpose

This document defines the high-level architecture of Anvil.
It translates the product concept into explicit subsystem boundaries for implementation in Rust.

## Design Goals

- Optimize for local LLM coding workflows first
- Exceed `vibe-local` in speed, stability, and extensibility
- Preserve strong terminal clarity during long sessions
- Support imperfect tool-calling and interruption as normal operating conditions
- Make future UX and backend evolution possible without structural rewrites

## Architectural Principles

- Separate runtime concerns from UI concerns
- Separate model-provider concerns from agent-loop concerns
- Use typed internal contracts instead of string-heavy implicit flows
- Treat execution policy and safety as core systems, not wrappers
- Treat session continuity and context shaping as core systems, not utilities

## Top-Level Subsystems

### 1. App Layer

Responsibilities:

- process startup
- config loading
- dependency wiring
- command-line mode selection
- lifecycle management

Key outputs:

- initialized runtime
- initialized UI
- initialized session environment

### 2. Provider Layer

Responsibilities:

- communicate with local model runtimes
- normalize model responses into Anvil's internal format
- stream tokens, tool calls, and metadata
- expose model capabilities and limits

Initial focus:

- Ollama-first backend

Future extension:

- additional local runtimes through the same provider contract

### 3. Agent Runtime

Responsibilities:

- run the main agent loop
- manage turns and iterations
- consume provider outputs
- recover from malformed tool calls
- coordinate tool execution and follow-up model calls
- integrate planning, execution, interruption, and retry behavior

The agent runtime is the orchestration core.
It should not own rendering details or persistence details directly.

### 4. Tool System

Responsibilities:

- register tools
- validate tool inputs
- classify execution policy
- execute tools
- collect typed results
- support parallel-safe and sequential execution modes

Sub-concerns:

- tool registry
- tool schema and validation
- execution policy
- permission checks
- rollback hooks

### 5. Session System

Responsibilities:

- persist conversation history
- maintain structured message history
- estimate and track context usage
- compact or summarize old state
- support resume and recovery

The session system should own message persistence and shaping, but not the provider protocol itself.

### 6. State Machine

Responsibilities:

- model current app state
- track transitions between idle, thinking, working, waiting, interrupted, and done states
- expose UI-readable state snapshots
- support interruption and recovery

This is a separate subsystem because UI clarity depends on explicit state, not inferred behavior.

### 7. TUI Layer

Responsibilities:

- render the operator console
- display user, agent, tool, and status regions clearly
- render plans, reasoning logs, and tool execution
- accept input and slash commands
- show current state and active step without owning orchestration logic

The TUI should observe state rather than re-derive it from runtime internals.

### 8. Extension Layer

Responsibilities:

- support custom slash commands
- support skills or instruction modules
- support MCP integrations
- support future UX feature expansion

This layer should extend the core without creating new coupling between unrelated subsystems.

## Data Flow

Primary interactive flow:

1. App starts and initializes config, provider, runtime, session, state machine, and TUI.
2. User submits input.
3. Input becomes a typed user message in the session system.
4. Agent runtime requests a provider turn.
5. Provider streams normalized events.
6. Agent runtime updates state machine and emits progress events.
7. If tool calls are produced, tool system validates and executes them.
8. Tool results are recorded in session and fed back into the next provider turn.
9. TUI renders state snapshots and event streams throughout.
10. Session saves incrementally.

## Event Model

Core internal events should be typed.
Representative examples:

- `UserInputReceived`
- `PlanCreated`
- `StepStarted`
- `ReasoningUpdated`
- `ProviderTokenDelta`
- `ProviderToolCallProposed`
- `ToolExecutionStarted`
- `ToolExecutionFinished`
- `PermissionRequested`
- `PermissionResolved`
- `SessionCompacted`
- `Interrupted`
- `TurnCompleted`

These events should drive UI updates and logging.

## Execution Policy Boundary

Execution policy must not be embedded ad hoc inside the agent loop.
It should be a first-class subsystem with explicit answers to:

- is this tool safe
- is this tool confirmation-required
- is this tool network-bound
- is this tool parallel-safe
- is this tool allowed in plan mode
- what happens on interruption
- what rollback hook applies

## Reasoning Visibility Boundary

Anvil should distinguish between:

- internal reasoning artifacts
- user-visible reasoning summaries
- plan steps
- tool execution logs

The runtime may maintain richer internal reasoning state, but the TUI should receive compact user-facing reasoning summaries designed for readability.

## Context Strategy

Anvil should assume 200k+ context is available, but should still manage context actively.

Core policies:

- preserve recent high-value interaction state
- summarize low-value older state
- separate planning artifacts from execution artifacts
- keep tool results structured
- prefer selective context shaping over blunt truncation

## Recovery and Stability Strategy

Anvil should treat failure as a normal path.

Core requirements:

- interrupted turns should leave session state valid
- malformed tool calls should be repairable or rejectable with explicit policy
- tool crashes should not corrupt UI or session continuity
- provider disconnects should degrade to recoverable state
- partial progress should remain visible after interruption

## Initial Module Layout

Suggested initial Rust module grouping:

- `app`
- `config`
- `provider`
- `agent`
- `tooling`
- `session`
- `state`
- `tui`
- `extensions`
- `metrics`

This does not require separate crates initially.
A single workspace crate with disciplined module boundaries is acceptable at the beginning.

## Non-Goals at This Stage

- supporting all model providers equally from day one
- building a full-screen IDE-like interface first
- maximizing feature breadth before core runtime quality is proven

## Definition of Architectural Success

The architecture is successful if:

- UI evolution does not require rewriting the agent loop
- new tools do not require changing session internals
- provider changes do not require rewriting the TUI
- interruption, permission, and recovery behavior remain explicit and testable
- the implementation can reach a useful local-first CLI prototype quickly without accruing monolith-style coupling
