# Anvil Runtime Permissions Specification

## Purpose

This document defines the MVP runtime permission model for Anvil.

The permission model exists to ensure that:

- model output is never executed as unchecked authority
- risky capabilities are explicit and inspectable
- local-first execution remains practical without becoming unsafe by default

This specification applies to:

- interactive CLI mode
- `-p` non-interactive mode
- PM and all subagents
- built-in tools and future tool extensions

---

## Design Principles

- default to the least privilege that can complete the task
- separate read access from write access from unrestricted execution
- treat command execution as a runtime policy decision, not a model decision
- require explicit escalation for destructive, networked, or system-wide actions
- make effective permissions visible to the user

---

## Permission Levels

Anvil MVP defines three runtime permission levels.

### 1. `read-only`

Intended for inspection, explanation, and review.

Allowed:

- reading files inside the workspace
- listing directories
- searching files and file contents
- reading diffs and Git status
- reading safe environment metadata such as current directory or shell

Blocked:

- file creation, deletion, rename, or modification
- commands that write to disk
- package installation
- network access
- opening GUI applications
- destructive Git commands

### 2. `workspace-write`

Intended for normal coding-agent work inside the repository.

Allowed:

- everything from `read-only`
- editing files inside the workspace
- creating new files inside the workspace
- running non-destructive local commands for tests, builds, and linting
- writing temporary artifacts inside workspace-approved temp locations

Blocked unless explicitly escalated:

- writes outside the workspace
- package installation outside the workspace
- network access
- destructive Git commands
- system configuration changes
- GUI launches

### 3. `full-access`

Intended only for actions the user has explicitly approved.

Allowed:

- broader filesystem writes
- networked commands
- dependency installation
- external application launch
- other system-level actions within platform constraints

Still blocked without explicit confirmation:

- known destructive actions with irreversible impact
- commands whose primary effect is bulk deletion or history rewrite

---

## Effective Permission Resolution

Effective permission for a tool call is resolved in this order:

1. runtime hard block rules
2. explicit per-command confirmation requirement
3. session permission mode
4. tool default capability
5. task intent

The model may request a tool action.
Only the runtime may approve and execute it.

---

## Required Command Representation

Shell-capable tool calls must be represented as structured actions.

Minimum fields:

```json
{
  "kind": "exec",
  "program": "pytest",
  "args": ["tests/auth/test_login.py"],
  "cwd": "/repo",
  "requestedPermission": "workspace-write"
}
```

The runtime must not execute:

- free-form multi-command text
- shell snippets copied directly from model prose
- concatenated command chains unless the runtime parser intentionally supports and classifies each segment

---

## Command Classification Rules

Before execution, the runtime must classify each command.

### Safe read commands

Typical examples:

- `pwd`
- `ls`
- `find`
- `rg`
- `git status`
- `git diff`

These may run in `read-only` if arguments stay within policy.

### Local validation commands

Typical examples:

- `cargo test`
- `cargo check`
- `npm test`
- `pytest`
- `ruff check`

These usually require `workspace-write` because tools may create caches or temp files even when logically read-oriented.

### Networked commands

Typical examples:

- package installs
- remote fetch or clone
- curl or wget
- commands that contact model servers outside declared local endpoints

These require explicit escalation to `full-access`.

### Destructive commands

Typical examples:

- `rm`
- `mv` or `cp` used for overwrite outside approved paths
- `git reset --hard`
- `git clean -fd`
- recursive chmod/chown on broad targets

These always require explicit user confirmation even in `full-access`.

---

## Destructive Action Policy

Anvil must treat the following as confirmation-gated:

- bulk deletion
- irreversible overwrite
- Git history rewrite
- recursive filesystem mutation outside a clearly bounded workspace target
- any command whose intent cannot be classified confidently

The runtime should surface:

- the exact command
- the affected path or scope
- why confirmation is required

The model must not be allowed to self-confirm.

---

## Network Access Policy

Network access is off by default in the MVP.

### Allowed without extra approval

- local loopback access to the configured local model provider endpoint

This may include:

- `http://127.0.0.1`
- `http://localhost`
- a user-configured equivalent local endpoint

### Requires explicit escalation

- public internet access
- private LAN access outside approved local model endpoints
- dependency download
- remote Git operations
- telemetry submission

### Additional rules

- the configured model endpoint list must be inspectable
- the runtime should distinguish local model inference traffic from general network access
- tool output should indicate when network was used

---

## Filesystem Scope Policy

The runtime must know which paths are writable.

### MVP writable scopes

- current workspace
- approved temp directories
- explicit user-approved extra paths

### Blocked by default

- home directory outside approved locations
- system directories
- sibling repositories
- SSH, cloud, and credential directories

### Path rules

- resolve symlinks before permission checks
- classify canonical paths, not just user-provided strings
- reject path traversal attempts that escape approved roots

---

## Environment and Secret Handling

Command output and tool context can expose secrets.

### MVP rules

- redact known sensitive environment variables from tool-visible logs when feasible
- do not pass full environment dumps to the model
- avoid storing raw command output containing secrets in durable session state
- never persist tokens, passwords, or private keys into memory or handoff files

### Examples of sensitive data classes

- API keys
- access tokens
- SSH material
- cloud credentials
- database passwords
- session cookies

---

## Tool Capability Mapping

Each tool must declare its minimum required permission.

Suggested MVP mapping:

- file read: `read-only`
- search: `read-only`
- diff inspection: `read-only`
- file edit: `workspace-write`
- command execution: varies by classification
- external fetch: `full-access`

The PM and subagents should request tools by capability.
The runtime decides whether the current session allows that capability.

---

## User Experience Rules

Permission behavior should be visible and predictable.

### Startup

Anvil should show effective session mode, for example:

```text
Permission mode: workspace-write
Network: blocked
Writable roots: /repo, /tmp/anvil
```

### On blocked action

Anvil should say:

- what was attempted
- why it was blocked
- what higher permission would be needed

### On escalation

Anvil should ask for approval with a short description of the action's purpose.

---

## Session and Resume Behavior

Permission mode should be part of active session state, but escalations should be treated carefully.

### MVP rules

- base session permission mode may be persisted
- one-off destructive confirmations should not silently persist across sessions
- resumed sessions should display effective permission mode again
- imported handoff files must not auto-upgrade permission level

---

## Failure Handling

If a tool action is blocked or denied:

- the runtime returns a structured refusal to the PM
- the PM must not pretend the action succeeded
- the PM should either continue with a safer fallback or surface the limitation to the user

Example structured refusal:

```json
{
  "status": "blocked",
  "reason": "network_access_disabled",
  "requiredPermission": "full-access"
}
```

---

## Implementation Notes for MVP

- start with a deny-by-default policy
- implement command classification before broad tool coverage
- prefer a small safe allowlist for `read-only`
- log permission decisions for debugging and auditability

---

## Bottom Line

In Anvil, the model can request actions.
The runtime owns permission to execute them.

That separation is a core MVP requirement, not an optional hardening step.
