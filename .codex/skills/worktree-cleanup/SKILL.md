---
name: worktree-cleanup
description: Safely remove merged or obsolete git worktrees for this repository.
---

# Worktree Cleanup

Safely clean up git worktrees for Anvil.

Use this when the user invokes:

- `/worktree-cleanup <issue_number>`
- `/worktree-cleanup all`

The default integration branch for this repository is `develop`; remote merge
checks use `origin/develop`.

## Safety Rules

- Never remove the current worktree.
- Never remove the integration worktree for `develop`.
- Never use `git worktree remove --force`.
- Never use `git branch -D`.
- Do not remove a worktree with uncommitted changes.
- Do not delete a branch unless its tip is already reachable from `origin/develop`.
- If a worktree is dirty or unmerged, stop and report the exact path, branch, and reason.

## Expected Worktree Shapes

Issue worktrees created by the Codex harness normally look like:

```text
../Anvil-issue-<number>-<slug>
../Anvil-feature-issue-<number>-<slug>
```

Branches normally look like:

```text
feature/issue-<number>-<slug>
```

Do not assume the slug. Discover candidates from `git worktree list --porcelain`
and match by branch name or path.

## Procedure

### 1. Inspect Current State

Run:

```bash
git branch --show-current
git worktree list --porcelain
git fetch origin develop --prune
```

If `git fetch` fails, continue with local information only and clearly state
that remote merge verification could not be refreshed.

### 2. Resolve Targets

For `/worktree-cleanup <issue_number>`:

- Validate that the argument is a positive integer.
- Select worktrees whose branch starts with `feature/issue-<issue_number>-` or
  whose path contains `Anvil-issue-<issue_number>-` / `Anvil-feature-issue-<issue_number>-`.
- If no target is found, report that no matching worktree exists.

For `/worktree-cleanup all`:

- Select only issue worktrees, not the current `develop` worktree.
- Prefer candidates whose branch starts with `feature/issue-`.
- Include matching paths that use the harness naming convention.

### 3. Check Each Target

For each candidate, collect:

```bash
git -C <worktree_path> status --porcelain
git -C <worktree_path> branch --show-current
git merge-base --is-ancestor <branch> origin/develop
```

Interpretation:

- `status --porcelain` has output: dirty, do not remove.
- branch is empty or detached: do not remove unless the user explicitly confirms after seeing the path and HEAD.
- `merge-base --is-ancestor` returns 0: branch is merged into `origin/develop`.
- `merge-base --is-ancestor` returns non-zero: unmerged, do not delete the branch.

### 4. Remove Safe Targets

Only for clean, merged issue worktrees:

```bash
git worktree remove <worktree_path>
git branch -d <branch>
```

If `git branch -d` reports that the branch is not found or already deleted,
record that as non-fatal. If it reports that the branch is not fully merged,
do not retry with `-D`.

### 5. Prune And Verify

Run:

```bash
git worktree prune
git worktree list
```

Report removed paths, deleted branches, skipped worktrees with reasons, and the
final `git worktree list` summary.
