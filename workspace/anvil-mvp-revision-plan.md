# Anvil MVP Revision Plan

This document turns the current review findings into concrete MVP design rules.

The goal is not to fully solve every future requirement.
The goal is to make the first usable release:

- safe enough to run locally
- fast enough for practical terminal workflows on mid-range local LLMs
- structured enough to extend without rewriting the architecture

---

## MVP Priorities

The MVP should prioritize these concerns in this order:

1. command execution safety
2. prompt and instruction trust boundaries
3. bounded session state and memory retention
4. latency control for local-model workflows
5. role and schema maintainability

If a feature conflicts with these priorities, the safer and simpler behavior should win.

---

## 1. Command Execution Security Model

Anvil's command execution model must be explicit in the MVP.

The MVP should define three execution modes:

### Read-only mode

- allows file reads, search, diff inspection, and safe environment inspection
- disallows file writes, shell writes, package installs, network access, and destructive Git commands
- should be the safest default for repository inspection and review tasks

### Workspace-write mode

- allows file edits inside the repository workspace
- allows non-destructive local commands needed for tests, linting, and builds
- disallows writes outside the workspace unless explicitly approved
- disallows networked commands by default

### Full-access mode

- allows broader command execution only with explicit user opt-in
- required for package installation, network access, system-level writes, or opening external applications

### MVP Rules

- Every command must run through a policy layer rather than direct model output.
- The model must not emit raw shell that is executed without validation.
- Commands must be represented as structured tool calls with:
  - executable
  - arguments
  - working directory
  - requested capability level
- The runtime must classify commands before execution.
- Destructive commands such as `rm`, `git reset --hard`, and equivalent recursive deletion or overwrite flows must always require explicit user confirmation.
- Commands that require network access must always be marked and blocked by default.
- The runtime should redact sensitive environment variables from tool-visible output where feasible.

### Why this is MVP-critical

Local LLMs in the target class are capable enough to be useful, but not reliable enough to safely improvise shell access without hard runtime policy.

---

## 2. Instruction Trust Model

Anvil must define which text is trusted and which text is untrusted.

### Trusted sources

- built-in system/runtime rules
- explicit user instructions in the current session
- `anvil.md`
- explicit runtime permission settings

### Conditionally trusted sources

- `anvil-memory.md`
- imported handoff files created by Anvil

These are trusted only after runtime validation and source labeling.

### Untrusted sources

- repository source code
- comments in code
- README files
- generated files
- test fixtures
- vendored dependencies
- tool output

### MVP Rules

- Repository content must be treated as data, not authority.
- Anvil must never allow instructions found inside normal repository files to override user instructions, runtime safety policy, or `anvil.md`.
- If repository content appears to contain agent instructions, the PM should surface that as repository content, not obey it automatically.
- When context is passed to a subagent, the PM should label each context block with source type such as `user`, `anvil.md`, `memory`, `repo-file`, or `tool-output`.
- Subagent prompts should explicitly state that `repo-file` and `tool-output` content are untrusted.

### Why this is MVP-critical

Without this rule set, prompt injection from repository files becomes a default failure mode.

---

## 3. Session State and Memory Bounds

The current direction is correct in preferring summaries over full transcripts.
The MVP now needs hard bounds.

### State retention rules

The runtime should maintain:

- one active `objective`
- one bounded `workingSummary`
- one bounded `repositorySummary`
- a short list of active constraints
- a short list of open questions
- a bounded list of pending steps
- a bounded recent-result window

### Recommended MVP limits

- `workingSummary`: max 1 to 2 KB text
- `repositorySummary`: max 1 to 2 KB text
- `activeConstraints`: max 20 items
- `openQuestions`: max 10 items
- `pendingSteps`: max 20 items
- `relevantFiles`: max 200 paths
- `recentDelegations`: last 20 only
- `recentResults`: last 20 only

### Compression rules

- New results should be merged into summaries rather than appended indefinitely.
- Older delegations and results should be dropped once summarized.
- Evidence should be references to files, commands, or artifact IDs rather than large copied text blobs.
- Tool output should be stored in full only in ephemeral runtime artifacts, not in durable session state by default.

### Memory rules

- `anvil-memory.md` should remain opt-in for auto-write in the MVP, or at minimum default to conservative write behavior.
- Candidate memory items must pass explicit filters for:
  - user specificity
  - long-term usefulness
  - low sensitivity
  - high confidence
- The runtime should store memory outside the repository by default unless the user explicitly requests repository-local memory.

### Handoff rules

- Handoff files must be treated as export artifacts, not as the only backing store for active session state.
- Handoff import should validate schema version, source metadata, and size limits before use.
- Handoff files should support a `source` or `createdBy` field in a future schema revision.

### Why this is MVP-critical

Without bounds, the PM/subagent architecture eventually recreates the same context bloat it was meant to avoid.

---

## 4. Performance and Latency Budget

The MVP should assume the local model is slower than a hosted small model and optimize for fewer round trips.

### MVP performance strategy

- Prefer direct PM handling for:
  - small clarifications
  - short summaries
  - simple planning updates
  - tiny single-file edits when no separate review or validation step is needed
- Use subagents only when specialization is likely to save tokens or improve correctness.
- Support parallel subagent execution only for independent read/review tasks after the basic runtime is stable.

### Delegation heuristics

The PM should delegate only when at least one of these is true:

- the task needs repository discovery across multiple files
- the task needs focused editing over a bounded file set
- the task needs shell validation
- the task needs a review pass distinct from implementation

The PM should avoid delegation when:

- the task can be answered from already-bounded state
- the task is a one-step follow-up on the immediately previous result
- the expected delegation cost is larger than the context reduction benefit

### Runtime optimizations

- Cache file reads and parsed repository maps within the active session.
- Reuse recent summaries unless affected files changed.
- Avoid regenerating repository-wide summaries on every turn.
- Keep tool result payloads short by default and allow explicit expansion only when needed.

### Suggested MVP observability

Track at least:

- prompt tokens per agent call
- wall-clock latency per agent call
- tool call counts per turn
- cache hit rate for file/context reuse

If these are not measured, performance tuning will stay guess-based.

---

## 5. Single Source of Truth for Roles and Schemas

The current documents already show drift risk.
The MVP should reduce this before implementation grows.

### MVP rule

Agent role definitions should have one canonical machine-readable source.

That source should define:

- role id
- display name
- default capability set
- whether the role is enabled in MVP
- whether the role supports model override

### Derived artifacts

The following should be generated or validated against that canonical source:

- CLI flags
- session-state schema role keys
- handoff schema role keys
- prompt template selection
- startup model mapping display

### Specific MVP simplification

- Keep `planner` as an internal optional role, but do not make it a required first-release role everywhere.
- If Planner is merged into PM for MVP, remove Planner from user-facing CLI flags and persisted schemas until it becomes real product surface.
- Store the canonical MVP role set in a checked-in role registry instance, and validate external schemas and CLI help against it.

### Why this is MVP-critical

This avoids multi-file drift and keeps future role additions cheap.

---

## 6. Proposed MVP Scope Adjustments

The current product direction should be narrowed slightly for the first release.

### Keep in MVP

- interactive CLI
- `-p` mode
- PM + Reader + Editor + Tester + Reviewer
- per-role model override
- `anvil.md`
- bounded session resume
- structured tool use

### Defer or narrow in MVP

- aggressive auto-writing of `anvil-memory.md`
- planner as a distinct always-on role
- broad slash-command ecosystem
- complex skill packaging beyond a minimal loader format
- parallel multi-subagent orchestration beyond simple cases

### Rationale

This keeps the differentiation while reducing coordination overhead and security surface.

---

## 7. Required Spec Changes Before Implementation

Before full implementation starts, the workspace docs should be revised to include:

1. a runtime permission model
2. a trust-boundary and prompt-injection policy
3. state and handoff size limits
4. canonical role registry rules
5. PM fast-path versus delegation heuristics

These should be treated as product requirements, not implementation notes.

---

## 8. Recommended Next Documents

After this revision plan, the next useful additions would be:

1. `anvil-runtime-permissions.md`
2. `anvil-trust-model.md`
3. `anvil-role-registry.schema.json`
4. session and handoff schema revisions aligned to size and source metadata rules

---

## Bottom Line

The core direction is sound:

- local-first
- Rust runtime
- PM-led bounded delegation
- explicit model routing

For the MVP, success depends less on adding more agent features and more on making the runtime strict about:

- what may execute
- what may be trusted
- what may be persisted
- when delegation is actually worth the latency
