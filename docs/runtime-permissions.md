# Runtime Permissions

Anvil applies runtime permissions at execution time, not at model-output time. The model may request an action, but only the runtime decides whether that action is allowed, blocked, or requires explicit confirmation.

## Permission Levels

### `read-only`

Use for inspection, explanation, and review.

Allowed:

- reading files inside the workspace
- listing directories
- searching files and file contents
- reading diffs and Git status
- reading safe environment metadata

Blocked:

- file creation, deletion, rename, or modification
- commands that write to disk
- package installation
- network access
- destructive Git commands

### `workspace-write`

Use for normal coding-agent work inside the repository.

Allowed:

- everything from `read-only`
- editing files inside the workspace
- creating new files inside the workspace
- running non-destructive local validation commands
- writing bounded temporary artifacts in approved locations

Still gated:

- writes outside the workspace
- network access outside approved local model endpoints
- destructive commands
- system-wide configuration changes

### `full-access`

Use only for actions the user has explicitly approved.

Allowed:

- broader filesystem writes
- networked commands
- dependency installation
- external application launch within platform limits

Still confirmation-gated:

- bulk deletion
- irreversible overwrite
- Git history rewrite
- commands with unclear or destructive intent

## Command Classification

Before execution, Anvil classifies each command into one of these broad groups:

- safe read commands
- local validation commands
- networked commands
- destructive commands

Examples:

- safe read: `pwd`, `ls`, `rg`, `git status`, `git diff`
- local validation: `cargo check`, `cargo test`, `pytest`, `npm test`
- networked: `curl`, package install, remote Git operations
- destructive: `rm`, `git clean -fd`, `git reset --hard`

The classification result is then combined with the session permission mode and path/network policy.

## Network Policy

Network is off by default for general tool execution.

Allowed without extra approval:

- loopback access to configured local model endpoints

Requires explicit approval:

- public internet access
- private LAN access outside approved model endpoints
- dependency download
- remote Git operations

Configured model endpoints remain inspectable, and tool results should make network use visible.

## Filesystem Scope

Writable scope is bounded to:

- the current workspace
- approved temp directories
- any extra user-approved path

Blocked by default:

- home-directory paths outside approved locations
- system directories
- sibling repositories
- credential and secret directories

Path checks operate on canonicalized paths so symlink tricks do not bypass policy.

## Confirmation Rules

Some commands require explicit user confirmation even when the session has broad permissions.

This applies to:

- destructive commands
- commands with irreversible effects
- commands whose intent cannot be classified confidently

The runtime should surface the exact command, affected scope, and reason for the confirmation requirement.
