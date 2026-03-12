# Repository Layout

## Goal

Define the initial filesystem and module layout for the Rust implementation.

## Recommended Initial Layout

```text
Anvil/
├─ Cargo.toml
├─ src/
│  ├─ main.rs
│  ├─ app/
│  ├─ contracts/
│  ├─ config/
│  ├─ provider/
│  ├─ agent/
│  ├─ tooling/
│  ├─ session/
│  ├─ state/
│  ├─ tui/
│  ├─ extensions/
│  └─ metrics/
├─ tests/
├─ workspace/
│  ├─ 1_ProductDefinition/
│  └─ 2_RustFoundation/
└─ ANVIL.md
```

## Why One Crate First

Start with one binary crate because:

- the team is still defining the real seams
- compile-time isolation is less valuable than iteration speed at this stage
- disciplined module boundaries are enough initially

Do not split into multiple crates until one of these becomes true:

- compile times become painful
- ownership of subsystems diverges strongly
- test isolation benefits materially from crate boundaries
- public internal APIs stabilize enough to justify crate extraction

## Module Responsibilities

### `app`

- startup orchestration
- dependency assembly
- lifecycle control

### `config`

- CLI args
- config file loading
- environment overrides
- derived runtime settings

### `contracts`

- typed shared interfaces
- event definitions
- provider-facing normalized types
- tool request and result DTOs
- cross-module view models where needed

### `provider`

- local model backend abstraction
- Ollama integration
- provider capability normalization

### `agent`

- main orchestration loop
- turn handling
- malformed-output recovery
- provider/runtime coordination

### `tooling`

- tool registry
- typed tool protocol
- execution policy
- permission flow
- tool execution

### `session`

- history persistence
- context accounting
- compaction and summarization hooks

### `state`

- explicit runtime state model
- transitions
- state snapshots for UI

### `tui`

- rendering
- input handling
- slash commands
- state-driven display

### `extensions`

- MCP
- custom slash commands
- skills and future extension points

### `metrics`

- timings
- counters
- comparison-axis instrumentation

## Test Placement Strategy

Use both module tests and integration tests.

Module tests should cover:

- config merge and validation
- state transitions
- tool policy classification
- error mapping

Integration tests should cover:

- bootstrap path
- state-driven TUI shell behavior
- approval flow behavior
- interruption and recovery flow behavior

## Immediate Rule

No module should reach into another module's internals when a typed interface can be defined instead.
