# Directory Structure

```text
Anvil/
├─ Cargo.toml
├─ Cargo.lock
├─ README.md
├─ docs/
│  └─ directory-structure.md
│  └─ agent-architecture.md
│  └─ memory-policy.md
│  └─ model-routing.md
│  └─ repo-instructions.md
│  └─ runtime-permissions.md
│  └─ runtime-overview.md
│  └─ trust-model.md
├─ schemas/
│  ├─ role-registry.schema.json
│  ├─ role-registry.json
│  ├─ session-state.schema.json
│  └─ handoff-file.schema.json
├─ prompts/
│  ├─ pm.txt
│  ├─ reader.txt
│  ├─ editor.txt
│  ├─ tester.txt
│  └─ reviewer.txt
├─ examples/
│  ├─ anvil.md
│  ├─ config.example.toml
│  └─ handoff.example.json
├─ scripts/
│  └─ lm_studio_smoke.sh
├─ src/
│  ├─ main.rs
│  ├─ lib.rs
│  ├─ cli/
│  ├─ roles/
│  ├─ runtime/
│  ├─ agents/
│  ├─ models/
│  ├─ tools/
│  ├─ prompts/
│  ├─ state/
│  ├─ policy/
│  ├─ config/
│  ├─ slash/
│  ├─ skills/
│  ├─ git/
│  ├─ util/
│  └─ error/
├─ tests/
│  ├─ cli.rs
│  ├─ pm_and_models.rs
│  ├─ policy_and_trust.rs
│  ├─ role_registry.rs
│  ├─ runtime_and_tools.rs
│  └─ state_roundtrip.rs
└─ workspace/
   └─ implementation and design drafts
```

## Notes

- `schemas/role-registry.json` is the canonical checked-in role registry instance.
- `schemas/*.schema.json` define the machine-readable contracts used by runtime state.
- `scripts/lm_studio_smoke.sh` wraps the opt-in LM Studio live smoke test with endpoint/model env defaults.
- `docs/agent-architecture.md` describes the PM-centered delegation model and current role boundaries.
- `docs/memory-policy.md` captures the current intended policy for optional user memory.
- `docs/model-routing.md` captures PM-default model inheritance and per-role override behavior.
- `docs/repo-instructions.md` explains how `anvil.md` is loaded and where its authority stops.
- `docs/runtime-permissions.md` captures the currently implemented runtime permission model.
- `docs/runtime-overview.md` describes the currently implemented runtime surfaces.
- `docs/trust-model.md` captures the current source-precedence and prompt-injection posture.
- `workspace/` still holds active planning documents that have not yet been promoted into stable docs.
