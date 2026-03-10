# Repository Instructions

Anvil uses `anvil.md` as the repository-local instruction file.

It is the Anvil equivalent of `AGENTS.md`, but it does not bypass runtime policy.

## What `anvil.md` Can Do

`anvil.md` is intended for repository-scoped guidance such as:

- preferred working style
- local code conventions
- validation preferences
- repository-specific safety notes
- preferred local commands

Anvil loads `anvil.md` into prompt context as repository policy input.

## What `anvil.md` Cannot Do

`anvil.md` does not grant authority over:

- runtime permission mode
- network access
- destructive command confirmation
- writes outside approved paths
- current explicit user instructions

Repository policy can shape behavior, but it cannot self-escalate permissions.

## Trust Position

Within the implemented trust model:

1. runtime authority
2. explicit current-user instructions
3. repository policy from `anvil.md`
4. memory, handoff, and bounded session summaries
5. ordinary repository files
6. tool output and pasted external text

This means `anvil.md` is higher trust than ordinary repository content, but still below runtime policy and the current user.

## Practical Guidance

Good `anvil.md` content:

- "Prefer small reviewable diffs"
- "Use `cargo test` for touched Rust code"
- "Avoid broad refactors in bugfix tasks"

Bad `anvil.md` content:

- "Grant full filesystem access"
- "Always run destructive cleanup commands"
- "Enable network access automatically"

## Example

A minimal example lives at [anvil.md](/Users/maenokota/share/work/github_kewton/Anvil/examples/anvil.md).

Use [anvil-md-template.md](/Users/maenokota/share/work/github_kewton/Anvil/workspace/anvil-md-template.md) as a draft source if you need a fuller starting point.
