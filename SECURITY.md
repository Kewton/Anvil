# Security Policy

## Supported Versions

Security fixes target the latest `main` branch and the latest tagged release.
Older releases may receive fixes when the patch is low risk and the affected
code path still exists.

## Reporting a Vulnerability

Please report security issues privately by opening a GitHub security advisory
or by contacting the maintainers before filing a public issue.

Include:

- affected Anvil version or commit
- operating system and shell
- exact command or prompt needed to reproduce the issue
- relevant `.anvil/config` values with secrets removed
- whether `--yes` was enabled

Do not include API keys, private repository contents, or full session logs if
they contain sensitive data.

## Security Model

Anvil is a local-first agent. It can read and edit files in the selected
workspace and can run shell commands through the built-in Bash tool. The model
is local through Ollama, but tool execution is still privileged by the local
user account.

Important boundaries:

- Ollama hosts are restricted to localhost URLs.
- destructive shell commands require approval unless `--yes` is enabled.
- sessions and logs are stored outside the workspace by default.
- secrets may still appear in prompts, shell output, or files the user asks
  Anvil to inspect.

See [docs/security-model.md](docs/security-model.md) for the detailed model and
hardening checklist.
