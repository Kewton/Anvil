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
- MVP dependency expansion in `Cargo.toml`
- compile and test verification with `cargo`
- registry-driven CLI model routing and persisted role handling
- schema-backed session and handoff persistence
- out-of-repository state storage by default
- runtime permission classification and enforcement for built-in tools
- trust-aware prompt context labeling with `anvil.md` loading
- first-party tool execution for file read, file write, search, diff, exec, and env inspection
- PM fast path and bounded Reader, Editor, Tester, and Reviewer delegation
- Ollama-backed PM fast-path model execution with local validation against `qwen3.5:35b`
- CLI resume follow-up prompts and session snapshots
- structured `recentResults` capture for commands, changed files, evidence, recommendations, and completed steps
- bounded Editor file mutation through the runtime permission layer
- task-aware Tester command planning with stdout/stderr evidence capture
- interactive multi-turn loop for new and resumed sessions
- interactive slash commands for help, status, snapshot, model summary, history, approval, denial, and exit
- richer session introspection with recent delegation and recent result history
- normalized pending/completed-step lifecycle handling for semantically matching steps
- persisted pending-confirmation state plus interactive approval and denial flow for confirmation-gated tester actions
- LM Studio OpenAI-compatible HTTP adapter
- fixture-based resume/edit/test/review CLI end-to-end coverage
- CLI handoff export/import roundtrip coverage
- CLI blocked tester-path coverage in read-only mode
- CLI confirmation-required tester-path coverage for networked validation requests
- CLI confirmation-required tester-path coverage for destructive validation requests
- CLI approval and denial coverage for destructive confirmation paths
- opt-in LM Studio live smoke test coverage
- repeatable LM Studio smoke wrapper under `scripts/lm_studio_smoke.sh`
- role-aware pending-step replacement for successive recommendations
- stale pending-step cleanup through normalization-aware compaction
- handoff import followed by resumed delegated work coverage
- trust-boundary coverage showing that ordinary repo files are not loaded as repository instructions
- prompting strategy documentation promoted into stable docs
- implementation-facing documentation promoted into `README.md` and `docs/runtime-overview.md`
- initial automated test coverage for CLI, state, policy, trust, runtime/tools, and PM/model routing

Not yet completed:

- lifecycle semantics beyond normalization, role-local replacement, and stale-step compaction
- further documentation promotion from `workspace/` into stable docs where still needed
- local-LLM-first orchestration roadmap below beyond the initial planner extraction

---

## Local-First Runtime Roadmap

Goal:

- make Anvil robust with local models that are weaker, less obedient, and more tool-confused than frontier hosted models
- bias the system toward runtime guarantees, bounded execution, and compact state rather than long free-form reasoning

Workstreams:

### A. Planning Layer

Tasks:

- extract `TaskAnalyzer` from `PmAgent`
- extract `TurnPlanner` and `PlannedStep` from `PmAgent`
- keep repo-grounded work off the PM fast path by default
- add task-shape tests for conversation, inspect, analyze, change, validate, and review prompts

Status:

- `TaskAnalyzer` and `TurnPlanner` extraction is now in progress

### B. Step Executor

Tasks:

- add an explicit `StepExecutor` that owns `allowed_tools`, stop conditions, and fallback behavior per step
- stop relying on subagent summary text to decide whether the turn should halt
- represent blocked and confirmation-gated steps as structured execution outcomes

### C. Evidence Store

Tasks:

- shift session updates away from summary-centric storage toward fact/evidence-centric storage
- persist normalized facts such as branch, top directories, changed files, command exit codes, and validation outputs
- teach the PM to synthesize user-facing responses from evidence instead of raw subagent prose

### D. Memory Compaction

Tasks:

- separate `objective`, `active plan`, `open questions`, `latest evidence`, and `next steps`
- compact old turn output instead of feeding long raw summaries back into the model
- bound context by evidence relevance rather than simple recency

### E. Local Model Profiles

Tasks:

- add model capability profiles for PM, sidecar, and execution roles
- introduce sidecar routing for lightweight classification and summarization
- tune routing and prompt style for local-only providers such as Ollama and LM Studio

### F. Verification-First Editing

Tasks:

- make Editor use an explicit sequence of locate -> preview -> patch -> verify
- keep patch size and target-file scope bounded before writes are allowed
- attach focused tester passes automatically to mutation plans

### G. Failure Recovery

Tasks:

- add deterministic fallback recipes for common tool failures
- retry known safe alternatives in the runtime before asking the model to improvise
- expose failure state clearly in CLI snapshots and history

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

Status:

- model client trait is implemented
- Ollama adapter is implemented and locally validated
- LM Studio adapter is implemented through the OpenAI-compatible chat completions endpoint

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

Status:

- built-in tools exist and are permission-gated
- Reader, Tester, Editor, and Reviewer now execute through the runtime tool layer
- Editor can now apply bounded file mutations when explicitly requested

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

Status:

- PM fast path is implemented
- bounded delegation is implemented for Reader, Editor, Tester, and Reviewer
- `anvil -p` and `anvil resume <id> -p ...` execute through the PM/runtime path
- new and resumed sessions now support interactive multi-turn stdin loops
- interactive slash commands provide help, status, snapshot, model summary, history, and exit controls
- confirmation-gated tester actions can now be approved or denied from interactive sessions

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

Status:

- the listed areas have baseline automated coverage
- interactive CLI command coverage is in place
- fixture-based resume/edit/test/review and handoff CLI flows are now covered
- confirmation approval/denial flows are covered in CLI integration tests
- handoff import followed by resumed delegated work is covered
- blocked tester-path CLI coverage is in place
- confirmation-required tester-path CLI coverage is in place
- remaining gaps are broader fixture breadth only where it adds new runtime behavior, deeper step lifecycle refinement, and any final doc promotions worth keeping

### 12. Documentation Promotion and Cleanup

- promote implementation-aligned documents from `workspace/` into `docs/`
- update top-level `README.md` to reflect the implemented feature set
- keep `workspace/` for active drafting only
- document developer setup and test workflow

Goal:

- implementation docs and code stay aligned as the project grows

---

## Recommended Immediate Next Steps

1. Tighten pending/completed-step lifecycle semantics beyond the current normalization and compaction heuristics
2. Promote any remaining implementation-aligned documents from `workspace/` into stable docs
3. Expand fixture coverage only where it adds genuinely new runtime behavior
4. Add richer session inspection only where it materially improves operator clarity
5. Keep the LM Studio wrapper aligned with real operator environments as endpoint defaults evolve

---

## Remaining Work Summary

The highest-value remaining items are:

- tighten pending/completed work lifecycle semantics further
- add more end-to-end tests only where they exercise behavior not already covered by current fixture and confirmation flows
- finish promoting implementation notes into durable user/developer documentation

---

## Test Plan

### 1. Unit Tests

- keep covering role derivation, permission classification, network/path policy, and trust ordering
- add direct tests for pending/completed-step lifecycle updates
- add direct tests for command-selection heuristics in Tester

### 2. Schema and State Tests

- keep roundtrip coverage for session and handoff schemas
- add fixture tests that verify imported handoffs preserve actionable fields used by resume flows
- add negative tests for oversized lists, invalid role ids, and invalid `nextRecommendation` payloads

### 3. CLI Integration Tests

- cover `anvil -p`, `anvil resume`, and `anvil resume -p` for both PM fast-path and delegated paths
- add tests that verify startup/session snapshots include last result, pending steps, completed steps, and recommendations
- add tests for interactive slash commands and multi-turn stdin loops
- add tests for `/approve` and `/deny` flows that execute or decline confirmation-gated actions
- keep expanding fixture-based resume flows where they add new behavioral coverage
- add tests for blocked and confirmation-required tool paths surfaced through CLI output

### 4. Runtime and Tool Tests

- extend targeted tests around Editor file-write flow and diff/evidence capture
- extend targeted tests around Tester command-output summarization and blocked-command paths
- add tests that verify destructive and networked commands remain confirmation-gated

### 5. Live Adapter Verification

- keep a local Ollama smoke test using `qwen3.5:35b`
- keep the reproducible LM Studio smoke wrapper working against real operator endpoints
- separate live-adapter tests from default unit/integration runs so CI remains stable

### 6. End-to-End Validation

- create small fixture repositories for read-only inspection, bounded edit, validation, and review flows
- verify session creation, persistence, resume, and handoff import/export across those fixtures
- verify that permission and trust boundaries still hold when repository files contain misleading instructions

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

The current MVP foundation is in place:

- registry-driven role handling
- schema-backed state
- permission enforcement
- trust-aware prompt construction
- runtime-mediated subagent execution
- resumable interactive CLI sessions

The remaining work is now concentrated in provider parity, deeper session ergonomics, fixture-based end-to-end validation, and documentation cleanup.
