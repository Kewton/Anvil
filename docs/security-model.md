# Security Model

Anvil is a local-first coding agent for Ollama. The model runs locally, but the
agent can still perform powerful local actions through tools. This document
defines the security boundary for users and contributors.

## Trust Boundary

Trusted:

- the local user account running Anvil
- the selected workspace contents
- the local Ollama server on localhost

Not fully trusted:

- model output
- shell output interpreted by the model
- repository files from untrusted sources
- instructions embedded in issues, logs, docs, or source files

## Tool Capabilities

Anvil may:

- read workspace files
- write or edit workspace files
- run shell commands through Bash
- store sessions and logs outside the workspace state root

The Bash tool is the highest-risk capability. Approval prompts and command
classification reduce accidental execution, but they do not turn model output
into trusted code.

## Hardening Rules

Security-sensitive code should stay deterministic where possible:

- path confinement
- localhost URL validation
- dangerous command blocking
- approval prompts
- secret masking in logs
- parser recovery for malformed tool calls

Heuristic or model-assisted judgment is appropriate for ambiguous product
decisions, but not as the only guard for security boundaries.

## Safer Operation

Recommended defaults for untrusted repositories:

```bash
anvil --fresh-session
```

Avoid `--yes` unless you trust the repository and prompt. Review proposed Bash
commands before approving them. Keep secrets out of prompts and fixture files.

## Known Limits

- A local model can still suggest unsafe changes or commands.
- Logs may contain file contents or command output.
- `--yes` increases automation and reduces interactive review.
- Anvil does not sandbox arbitrary commands beyond its own approvals and path
  checks.
