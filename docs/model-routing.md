# Model Routing

Anvil resolves models per role, with the PM model acting as the default for the whole session.

## Supported Flags

Global PM selection:

- `--model`
- `--pm-model`

Role-specific overrides:

- `--reader-model`
- `--editor-model`
- `--tester-model`
- `--reviewer-model`

`--model` is shorthand for `--pm-model`.

## Resolution Order

Effective model selection follows this order:

1. explicit CLI override for that role
2. stored session override for that role
3. current PM model
4. built-in default model

If a role has no explicit override, it inherits the PM model.

## Role Registry

Public role overrides are derived from the checked-in role registry.

Current user-facing roles:

- `pm`
- `reader`
- `editor`
- `tester`
- `reviewer`

The runtime does not maintain a separate handwritten public-role list for model routing.

## Resume Behavior

`anvil resume <session-id>` loads:

- stored PM model
- stored role overrides
- stored permission mode
- stored network policy

CLI flags on resume can still override the loaded values for the current invocation.

Examples:

```bash
anvil resume <session-id>
anvil resume <session-id> --editor-model qwen3.5:35b
anvil resume <session-id> --permission-mode read-only --network disabled
```

## Display Behavior

Interactive startup and session status expose the effective mapping so inheritance stays visible.

Typical shape:

```text
PM: qwen3.5:35b
Reader: qwen3.5:35b (inherited)
Editor: custom-editor-model
Tester: qwen3.5:35b (inherited)
Reviewer: qwen3.5:35b (inherited)
```

This is shown alongside active permission and network settings.
