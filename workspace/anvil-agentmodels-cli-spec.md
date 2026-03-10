# Anvil `agentModels` CLI Specification

## Purpose

This document defines how Anvil should expose per-agent model configuration through the CLI.

The key rule is:

- the PM agent model is the session default
- subagents inherit the PM model unless explicitly overridden

This specification is designed to keep model routing:

- explicit
- reproducible
- understandable to users

---

## Design Goals

- easy to use for the common case
- explicit for advanced users
- works in both interactive mode and `-p` mode
- supports persistent session defaults and one-off overrides
- remains readable in shell scripts
- composes cleanly with runtime permission settings

---

## Model Resolution Rule

Effective model for a role is resolved in this order:

1. explicit role override from CLI
2. stored session role override
3. PM model for the current invocation or session
4. global default model

If a role has no override, it inherits the PM model.

For the MVP:

- `planner` may remain internal even if supported in config structures
- user-facing CLI should not expose role overrides for roles that are not active product surface
- public role flags and displays should be derived from the canonical role registry instance

---

## Core Flags

### Global / PM model

```bash
anvil --model <model>
anvil --pm-model <model>
```

Semantics:

- `--model <model>` is shorthand for `--pm-model <model>`
- this sets the PM model
- all subagents inherit this model unless they have explicit overrides

### Role-specific overrides

```bash
anvil --reader-model <model>
anvil --editor-model <model>
anvil --tester-model <model>
anvil --reviewer-model <model>
```

These flags override the PM default for a specific role.

### Runtime permission flags

Model selection must not be confused with runtime permissions.

Recommended companion flags:

```bash
anvil --permission-mode <read-only|workspace-write|full-access>
anvil --network <disabled|local-only|enabled-with-approval>
```

Semantics:

- permission flags control what the runtime may execute
- model flags control which model reasons about the task
- neither flag family overrides the other

---

## Interactive Mode Examples

### Use one model everywhere

```bash
anvil --model qwen-coder-14b
```

Effect:

- PM = `qwen-coder-14b`
- Reader = inherit
- Editor = inherit
- Tester = inherit
- Reviewer = inherit
- runtime permissions unchanged

### Override one role

```bash
anvil --pm-model qwen-coder-14b --reader-model qwen-coder-7b
```

Effect:

- PM = `qwen-coder-14b`
- Reader = `qwen-coder-7b`
- Others = inherit PM

### Override multiple roles

```bash
anvil --pm-model qwen-coder-14b --reader-model qwen-coder-7b --reviewer-model deepseek-coder-14b
```

### Combine model and permission settings

```bash
anvil --model qwen-coder-14b --permission-mode workspace-write --network local-only
```

Effect:

- PM = `qwen-coder-14b`
- subagents inherit unless overridden
- filesystem writes allowed only inside workspace policy
- network limited to approved local model endpoints

---

## `-p` Mode Examples

### Simple

```bash
anvil -p "inspect the repo" --model qwen-coder-14b
```

### With role overrides

```bash
anvil -p "review the current diff" \
  --pm-model qwen-coder-14b \
  --reader-model qwen-coder-7b \
  --reviewer-model deepseek-coder-14b
```

### Read-only inspection

```bash
anvil -p "inspect this repo and summarize the auth flow" \
  --model qwen-coder-14b \
  --permission-mode read-only \
  --network disabled
```

---

## Resume Behavior

### Default resume

```bash
anvil resume <session-id>
```

Behavior:

- load stored PM model
- load stored role overrides
- load stored permission mode
- load stored network policy
- continue with that configuration

### Resume with override

```bash
anvil resume <session-id> --reviewer-model qwen-coder-14b
```

Behavior:

- load stored session configuration
- override reviewer model for the resumed session

### Resume with safer permissions

```bash
anvil resume <session-id> --permission-mode read-only --network disabled
```

Behavior:

- load stored model routing
- lower active permissions for the resumed session
- do not silently preserve one-off destructive confirmations

---

## Config File Representation

Anvil should support a config representation equivalent to:

```json
{
  "pmModel": "qwen-coder-14b",
  "permissionMode": "workspace-write",
  "networkPolicy": "local-only",
  "agentModels": {
    "reader": "qwen-coder-7b",
    "editor": null,
    "tester": null,
    "reviewer": "deepseek-coder-14b"
  }
}
```

Where:

- `null` means "inherit PM model"
- permission settings belong to runtime policy, not to model routing itself

---

## Recommended UX Rules

### 1. Show effective model mapping

When a session starts, Anvil should be able to show the effective mapping:

```text
PM: qwen-coder-14b
Reader: qwen-coder-7b
Editor: qwen-coder-14b (inherited)
Tester: qwen-coder-14b (inherited)
Reviewer: deepseek-coder-14b
```

When relevant, Anvil should also show runtime policy:

```text
Permission mode: workspace-write
Network: local-only
```

### 2. Avoid silent magic

If Anvil changes model assignment, it should be visible in state or output.

The same applies to permission changes and resume-time overrides.

### 3. Keep the common case simple

Most users should only need:

```bash
anvil --model <model>
```

### 4. Make role overrides optional

Role-specific model routing is an advanced feature, not a required setup step.

### 5. Keep authority boundaries clear

CLI configuration for models, permissions, and network access should be explicit user input.
Repository content and tool output must not mutate these settings implicitly.

---

## Validation Rules

- `--model` and `--pm-model` should not conflict silently
- invalid model names should fail clearly
- unknown agent roles should fail clearly
- role overrides should be persisted only when intended
- invalid permission modes should fail clearly
- invalid network policy values should fail clearly
- CLI resume should not silently elevate permissions above stored or explicitly requested policy

---

## Future Extensions

Possible future additions:

- `anvil models recommend`
- `anvil models bench`
- `anvil session set-model <role> <model>`
- task-based routing profiles

But the initial version should stay simple and explicit.
