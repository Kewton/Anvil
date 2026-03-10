# Anvil Role Registry Integration Plan

## Purpose

This document defines how the canonical role registry should be used during Rust implementation.

The goal is to ensure that role-related behavior is derived from one source of truth rather than re-declared across:

- CLI help and flag handling
- session and handoff persistence
- prompt template selection
- startup displays
- runtime validation

The canonical source is:

- schema: `anvil-role-registry.schema.json`
- instance: `anvil-role-registry.json`

---

## Rust Implementation Assumption

Anvil is assumed to be implemented in Rust.

That assumption is already reflected in product documents such as:

- `anvil-readme-draft.md`
- `anvil-pm-system-prompt-draft.md`

This integration plan therefore uses Rust-oriented terminology such as:

- serde
- schema validation
- build-time embedding
- startup validation

---

## Canonical Role Model

Rust code should load the role registry into a single internal type.

Suggested shape:

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RoleRegistry {
    pub format_version: String,
    pub default_session_role: String,
    pub roles: Vec<RoleDefinition>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RoleDefinition {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub enabled_in_mvp: bool,
    pub user_facing: bool,
    pub supports_model_override: bool,
    pub default_permission: PermissionLevel,
    pub capabilities: Vec<RoleCapability>,
}
```

The runtime should not maintain a second handwritten role list.

---

## Required Derivations

The following behavior should be derived from the registry instance.

### 1. Public CLI model flags

Expose `--<role>-model` only when all are true:

- the role exists
- `enabledInMvp` is `true`
- `userFacing` is `true`
- `supportsModelOverride` is `true`

For the current MVP registry, that means:

- `--reader-model`
- `--editor-model`
- `--tester-model`
- `--reviewer-model`

And not:

- `--planner-model`

### 2. Startup role display

Show effective model mapping only for:

- `pm`
- user-facing enabled roles

### 3. Session and handoff validation

Persisted `agentModels` keys should match the public persisted role set derived from the registry.

For the current MVP registry:

- `reader`
- `editor`
- `tester`
- `reviewer`

### 4. Prompt template lookup

Prompt template resolution should fail clearly if:

- a requested role is not present in the registry
- a role is disabled for the current product surface

### 5. Default permission hints

The runtime may use `defaultPermission` as a hint for planning and validation, but not as a bypass of runtime permission policy.

---

## Non-Derived Behavior

The following must not be derived only from the registry:

- actual sandbox enforcement
- network authorization
- destructive-command confirmation
- trust hierarchy

These remain governed by:

- runtime permission policy
- trust model
- explicit user approval

The registry defines role metadata, not security authority.

---

## Build-Time and Runtime Strategy

Recommended Rust approach:

### Build-time

- keep `anvil-role-registry.json` in the repository
- validate it against `anvil-role-registry.schema.json` in tests or a build script

### Runtime

- embed the validated registry with `include_str!` or equivalent
- deserialize once at startup
- fail fast if the embedded registry cannot be parsed

This avoids hidden drift between shipped binary behavior and workspace docs.

---

## Suggested Module Boundaries

One reasonable Rust layout:

```text
src/
  roles/
    mod.rs
    registry.rs
    derive.rs
  cli/
    model_flags.rs
  prompts/
    role_prompts.rs
  state/
    session.rs
    handoff.rs
```

Possible responsibilities:

- `roles/registry.rs`: serde types and loading
- `roles/derive.rs`: helper filters such as public roles and overrideable roles
- `cli/model_flags.rs`: flag registration from registry
- `prompts/role_prompts.rs`: prompt lookup with role validation
- `state/*`: persisted-role validation against registry

---

## Validation Rules

At minimum, Rust tests should assert:

1. registry JSON matches schema
2. `defaultSessionRole` exists in `roles`
3. role IDs are unique
4. all public overrideable roles appear in CLI flag generation
5. disabled non-user-facing roles do not appear in public CLI help
6. session and handoff role keys match the persisted role set

---

## Current MVP Expected Role Sets

### All defined roles

- `pm`
- `reader`
- `planner`
- `editor`
- `tester`
- `reviewer`

### Public MVP roles

- `pm`
- `reader`
- `editor`
- `tester`
- `reviewer`

### Internal-only MVP roles

- `planner`

### Persisted `agentModels` keys

- `reader`
- `editor`
- `tester`
- `reviewer`

---

## Recommended Next Step

When implementation starts in Rust, the first concrete follow-up should be:

1. create a `roles` module
2. load and validate `anvil-role-registry.json`
3. replace handwritten role lists in CLI and state code with registry-derived helpers

---

## Bottom Line

The role registry should become executable configuration for the Rust codebase, not just documentation.

If role metadata remains duplicated in Rust enums, CLI docs, and JSON schemas by hand, the current drift problem will return quickly.
