# Anvil

Local-first coding agent runtime in Rust.

Anvil is a self-hosted agent runtime for code inspection, bounded editing, validation, review, and resumable CLI sessions on top of local model servers.

## Current Status

Implemented today:

- role-registry-driven model selection
- schema-validated session and handoff persistence
- permission-gated runtime tools for read, write, search, exec, diff, and env inspection
- trust-aware prompt context with `anvil.md`
- PM fast path plus Reader, Editor, Tester, and Reviewer delegation
- Ollama HTTP adapter
- LM Studio HTTP adapter
- `anvil -p`, `anvil resume`, `anvil resume -p`
- interactive multi-turn CLI sessions with `/help`, `/status`, `/snapshot`, `/models`, `/history`, `/exit`

Still in progress:

- broader fixture-based end-to-end coverage
- richer step lifecycle semantics
- documentation promotion beyond the current core docs

## Supported Model Providers

- Ollama
- LM Studio

Ollama defaults to `http://127.0.0.1:11434` and can be overridden with `ANVIL_OLLAMA_ENDPOINT`.
LM Studio defaults to `http://127.0.0.1:1234/v1/chat/completions` and can be overridden with `ANVIL_LM_STUDIO_ENDPOINT`. LM Studio model names should use the form `lmstudio/<model-id>`.

## Implemented CLI Surface

```bash
anvil
anvil -p "inspect the repository layout"
anvil resume <session-id>
anvil resume <session-id> -p "review the current diff"
anvil handoff export <session-id>
anvil handoff import <file>
```

Useful flags:

```bash
--model
--pm-model
--reader-model
--editor-model
--tester-model
--reviewer-model
--permission-mode read-only|workspace-write|full-access
--network disabled|local-only|enabled-with-approval
```

## Interactive Commands

Inside `anvil` interactive mode or `anvil resume <session-id>`:

- `/help`
- `/status`
- `/snapshot`
- `/models`
- `/history`
- `/exit`

## Example

```bash
anvil --model qwen3.5:35b --permission-mode workspace-write
```

```text
interactive mode
session: session-...
interactive commands: enter a prompt, or `exit` to finish
inspect the repository layout
apply update file sample
/history
/exit
```

Non-interactive mode:

```bash
anvil -p "inspect the repository layout" --model qwen3.5:35b
```

## Runtime Behavior

- repository instructions are loaded from `anvil.md`
- repository content and tool output are treated as lower-trust evidence
- runtime permissions gate writes, validation commands, networked commands, and destructive commands
- subagent results are persisted into bounded session state
- sessions can be resumed or exported as handoff files

## Development

```bash
cargo fmt
cargo test
```

The test suite includes CLI integration tests, session/handoff roundtrips, permission and trust tests, PM/model routing tests, and a small fixture-based resume/edit/review flow.

Manual smoke examples:

```bash
anvil -p "Reply with exactly: OK" --model qwen3.5:35b --network local-only
anvil -p "Reply with exactly: OK" --model lmstudio/<your-model-id> --network local-only
```

Optional live LM Studio test:

```bash
ANVIL_LM_STUDIO_MODEL=lmstudio/<your-model-id> \
  cargo test --test pm_and_models -- --ignored lm_studio_live_smoke_test
```

## Docs

- [Runtime Overview](/Users/maenokota/share/work/github_kewton/Anvil/docs/runtime-overview.md)
- [Agent Architecture](/Users/maenokota/share/work/github_kewton/Anvil/docs/agent-architecture.md)
- [Runtime Permissions](/Users/maenokota/share/work/github_kewton/Anvil/docs/runtime-permissions.md)
- [Memory Policy](/Users/maenokota/share/work/github_kewton/Anvil/docs/memory-policy.md)
- [Trust Model](/Users/maenokota/share/work/github_kewton/Anvil/docs/trust-model.md)
- [Directory Structure](/Users/maenokota/share/work/github_kewton/Anvil/docs/directory-structure.md)
- [Current Implementation Plan](/Users/maenokota/share/work/github_kewton/Anvil/workspace/anvil-implementation-plan.md)
