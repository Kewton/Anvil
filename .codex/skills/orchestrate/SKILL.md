---
name: orchestrate
description: Plan and run Anvil issue orchestration through CommandMate and git worktrees.
---

# Anvil Codex Orchestrate

Use this skill when the user asks to run `/orchestrate <issue...>` for this repository.

## Operating Rules

- Start from the Anvil `develop` integration worktree.
- Keep Issue enhancement lightweight; ask only blocking questions.
- Use `origin/develop` as the base for planned worktrees.
- Prefer repository-local artifacts under `workspace/management/runs/`.
- Do not delete, reset, or overwrite existing worktrees without explicit user approval.
- Do not merge PRs with failing CI unless the user explicitly approves.
- Do not start/stop CommandMate or kill port processes unless the user explicitly asks.
- Treat `commandmatedev` read failures such as `Server is not running` as "unreachable"
  until verified outside the sandbox; sandboxed localhost access can fail even when
  the user's terminal and CommandMate server are healthy.
- Include manual UAT steps when GUI, TTY behavior, release flow, or real-device confirmation is needed.

## First Action

Run the dry-run planner first:

```bash
python3 scripts/codex_orchestrate.py <issue...> --dry-run
```

Review the generated:

- `workspace/management/runs/<run_id>/manifest.md`
- `workspace/management/runs/<run_id>/issue-analysis.md`
- `workspace/management/runs/<run_id>/dependency-plan.md`

Proceed to worktree creation, CommandMate dispatch, PR creation, or merge only
after the plan is coherent and any blocking questions have been answered.
