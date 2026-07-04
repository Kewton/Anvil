---
name: release
description: Create an Anvil release branch, version bump, changelog update, tag, and GitHub Release.
---

# Release

Use this skill when the user invokes:

- `/release patch`
- `/release minor`
- `/release major`
- `/release <version>`

Anvil releases from `main`. Feature work may flow through `develop`, but release
tags must point at code already merged to `main` or at a release PR merged into
`main`.

## Safety Rules

- Do not start or stop CommandMate unless the user explicitly asks.
- Do not infer that CommandMate is down from sandboxed localhost failures.
- Do not release with uncommitted tracked changes in the release worktree.
- Do not tag before the release PR is merged to `main`.
- Do not force-push, delete tags, or use destructive cleanup unless explicitly approved.
- Keep `.env`, local runtime data, workspace run artifacts, and raw logs out of release commits.

## Phase 1: Prepare Release PR

### 1. Preflight

Run from the main integration worktree or a clean release worktree:

```bash
git branch --show-current
git status --short
git fetch origin main --tags
```

If the current branch is not `main`, verify that a `main` worktree exists:

```bash
git worktree list
```

Use `main` as the release branch base.

### 2. Determine Version

Read the current version from `Cargo.toml`:

```bash
python3 - <<'PY_INNER'
import re
from pathlib import Path

cargo = Path("Cargo.toml").read_text()
print(re.search(r'^version = "([^"]+)"', cargo, re.M).group(1))
PY_INNER
```

Calculate the next version:

- `patch`: increment PATCH.
- `minor`: increment MINOR and reset PATCH to 0.
- `major`: increment MAJOR and reset MINOR/PATCH to 0.
- explicit version: use the provided `X.Y.Z`.

Reject non-SemVer versions.

### 3. Check Tag Availability

```bash
git rev-parse "v$new_version"
git ls-remote --tags origin "v$new_version"
```

If either command finds an existing tag, stop and report the collision.

### 4. Create Release Worktree

```bash
WORKTREE_DIR="../Anvil-release-v$new_version"
git worktree add -b "release/v$new_version" "$WORKTREE_DIR" origin/main
```

### 5. Update Release Files

In the release worktree:

- Update `Cargo.toml` package version.
- Run `cargo generate-lockfile` so `Cargo.lock` is consistent.
- Update `CHANGELOG.md`:
  - move relevant `[Unreleased]` entries under `[X.Y.Z] - YYYY-MM-DD`;
  - leave an empty `[Unreleased]` section;
  - update compare links when present.
- Update README release/download text if the current version is mentioned.

### 6. Run Checks

Run:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo build --release
```

If a check fails, fix it in the release worktree or stop with the failure.

### 7. Commit, Push, PR

```bash
git add Cargo.toml Cargo.lock CHANGELOG.md README.md
git commit -m "chore: release v$new_version"
git push -u origin "release/v$new_version"
```

Create a PR to `main` with:

- title: `chore: release v$new_version`
- body containing the changelog section and the checks run.

Prefer the GitHub connector for PR creation. Use `gh pr create` only when the
connector cannot create the PR and `gh` is authenticated.

## Phase 2: Tag After Merge

After the user confirms the release PR was merged:

```bash
git fetch origin main --tags
git checkout main
git pull --ff-only origin main
git tag "v$new_version"
git push origin "v$new_version"
```

The tag push triggers `.github/workflows/release.yml`, which builds Anvil
binaries and attaches release artifacts to a GitHub Release.

## Cleanup

Only after tag push and release workflow success:

```bash
git worktree remove "../Anvil-release-v$new_version"
git branch -d "release/v$new_version"
git push origin --delete "release/v$new_version"
```

Use only non-forced deletion by default.

## Verification

Confirm:

```bash
git tag -l "v$new_version"
gh release view "v$new_version"
gh run list --limit 3
```

If `gh` is unavailable or unauthenticated, use the GitHub connector or report
that local release verification could not be completed.
