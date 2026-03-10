# Anvil Implementation Plan

This document is the current implementation plan for Anvil based on the latest architecture, permission, trust, and role-registry decisions.

---

## Current Status

Completed:

- product and architecture drafts
- MVP revision plan
- runtime permission specification
- trust model specification
- role registry schema and canonical registry instance
- Rust project skeleton
- initial CLI, roles, runtime, state, and prompt-loading stubs
- initial repository structure and examples

Not yet completed:

- dependency expansion beyond the initial minimal crate set
- compile verification in an environment with `cargo`
- runtime permission enforcement
- trust-boundary application in prompt construction
- model adapter implementation
- tool execution implementation
- PM loop and subagent execution
- end-to-end validation

---

## Implementation Phases

### 1. Development Environment Baseline

- confirm Rust toolchain availability
- standardize on `cargo fmt`, `cargo clippy`, and `cargo test`
- confirm minimum supported Rust version for required crates
- make local development prerequisites explicit

### 2. Dependency Setup

Add the MVP dependency set:

- `tokio`
- `reqwest`
- `tracing`
- `tracing-subscriber`
- `toml`
- `jsonschema`

Add the MVP test dependency set:

- `tempfile`
- `assert_cmd`
- `predicates`

Goal:

- the project builds
- schemas can be validated
- CLI behavior can be tested

### 3. Role Registry Integration

- load `schemas/role-registry.json` as the canonical runtime role registry
- remove handwritten public-role assumptions where possible
- derive public model flags from registry metadata
- derive persisted `agentModels` role keys from registry metadata
- add tests for registry consistency

Goal:

- role metadata becomes executable configuration rather than duplicated assumptions

### 4. CLI Foundation

Implement the first useful CLI surface:

- `anvil`
- `anvil -p`
- `anvil resume`
- `anvil handoff export`
- `anvil handoff import`

Implement:

- model flags
- permission flags
- network policy flags
- startup summary output
- basic argument validation

Goal:

- the CLI shape is stable enough for the runtime loop to attach to

### 5. Session, Handoff, and Memory State

- implement `SessionState` persistence
- implement `HandoffFile` persistence
- validate both against schemas
- enforce bounded lengths and list sizes
- implement out-of-repository memory storage by default

Goal:

- resumable and inspectable state exists before agent orchestration grows

### 6. Runtime Permission Layer

Implement policy enforcement for:

- permission modes
- network policy
- writable path scope
- destructive command confirmation
- blocked action responses

Implement command classification:

- safe read
- local validation
- networked
- destructive

Goal:

- the runtime, not the model, decides what may execute

### 7. Trust Model Application

- implement source labeling for prompt context
- separate trusted and untrusted context blocks
- load and apply `anvil.md`
- treat repository files and tool output as untrusted evidence
- ensure prompt builders preserve the defined source hierarchy

Goal:

- prompt injection resistance becomes part of runtime behavior, not just documentation

### 8. Model Adapter Layer

- finalize the model client trait
- implement Ollama adapter
- implement LM Studio adapter
- implement role-aware routing from PM and subagents

Goal:

- Anvil can issue real model requests under the current routing design

### 9. Tool Layer

Implement first-party tools:

- file read
- search
- diff inspection
- file edit
- command execution
- environment inspection

Attach permission checks to all tools.

Goal:

- the PM and subagents can operate through structured tools instead of free-form shell output

### 10. PM Loop and Subagent Execution

Implement:

- PM fast path for small tasks
- bounded delegation for Reader
- bounded delegation for Editor
- bounded delegation for Tester
- bounded delegation for Reviewer

Keep Planner internal or merged into PM during the MVP.

Goal:

- the interactive execution loop becomes usable without overcommitting to unnecessary orchestration

### 11. Validation and Test Coverage

Add tests for:

- role registry loading and derivation
- CLI argument behavior
- schema roundtrips for session and handoff data
- permission policy decisions
- trust labeling and prompt construction
- small fixture-based end-to-end flows

Goal:

- high-risk architectural rules are test-backed before feature expansion

### 12. Documentation Promotion and Cleanup

- promote implementation-aligned documents from `workspace/` into `docs/`
- update top-level `README.md` to reflect the implemented feature set
- keep `workspace/` for active drafting only
- document developer setup and test workflow

Goal:

- implementation docs and code stay aligned as the project grows

---

## Recommended Immediate Next Steps

1. Add the MVP dependency set to `Cargo.toml`
2. Build in a Rust environment with `cargo`
3. Make role-derived CLI behavior test-backed
4. Implement session and handoff schema validation
5. Start the runtime permission layer before adding real command execution

---

## Ordering Rationale

The implementation order is intentionally front-loaded toward:

- correctness of structure
- safety boundaries
- role metadata consistency
- persisted-state reliability

This is more important for the MVP than immediately wiring real LLM calls, because the permission and trust model define whether the runtime is safe and maintainable.

---

## Bottom Line

The project is ready to move from design to implementation, but the first engineering focus should be:

- registry-driven role handling
- schema-backed state
- permission enforcement
- trust-aware prompt construction

Those pieces should land before broad tool coverage or more ambitious multi-agent behavior.
