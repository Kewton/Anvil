# Security Model

`anvilminimal` is a local-first coding agent for trusted workspaces and trusted
goals. It is designed to reduce accidental damage from model tool calls, not to
execute hostile projects or hostile instructions safely.

## Trust Boundary

- The active workspace is trusted. Do not run the agent on a repository whose
  scripts, package hooks, or source tree you would not run manually.
- The requested goal is trusted. A malicious prompt can still ask the agent to
  run dangerous commands or edit sensitive files.
- Bash commands are policy-checked before execution, but Bash is not a sandbox.
  Shell commands still run as the current OS user.

## `--yes`

`--yes` skips interactive approval for mutating tools. Use it only in a trusted
workspace after checking the goal and current branch. It does not make command
execution safer and does not auto-kill unrelated local processes.

## Child Process Environment

Bounded child processes start from `env_clear` and receive only a small
allowlist such as `PATH`, `HOME`, locale variables, `TERM`, selected Node/npm
settings, and explicit verifier/dev-server variables. Provider API keys and
unrelated parent environment variables must not be inherited by child commands.

## Symlink Policy

Write/Edit reject a symlink as the final target, re-check existing components
under the workspace root, and use no-follow open flags where the platform
supports them. Existing intermediate symlinks are allowed only when their
canonical target remains under the workspace root.

This narrows symlink escape risk but is not a complete kernel-level sandbox.
There remains a TOCTOU window on intermediate path components; hostile
workspaces require a separate containment layer.

## Backlog

- Container-sandboxed execution track: run Bash/verifier commands inside a
  per-workspace container or equivalent OS sandbox with a mounted workspace,
  minimal environment, and explicit network policy.
