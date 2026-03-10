# Directory Structure

```text
Anvil/
├─ Cargo.toml
├─ README.md
├─ LICENSE
├─ .gitignore
├─ docs/
│  └─ directory-structure.md
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
│  └─ role_registry.rs
└─ workspace/
   └─ design and product drafts
```

## Notes

- `schemas/role-registry.json` is the canonical checked-in role registry instance.
- `schemas/*.schema.json` define the machine-readable contracts used by runtime state.
- `workspace/` remains the design area until implementation-facing docs are promoted into `docs/`.
