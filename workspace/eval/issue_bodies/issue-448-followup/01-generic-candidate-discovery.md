## Summary

Issue 448 added RepoGraph and graph-aware RepoContext, but live E2E still fails when the user gives a project-level request without explicit file paths.

Example: "Node のテストを通して" built a RepoGraph, but RepoContext returned `no_candidates`, and the model stopped after prose-only messages.

## Problem

RepoGraph exists, but candidate seeding is still too lexical and depends heavily on explicit paths in the prompt.

When a task mentions "Node tests", "pricing tests", or similar project-level targets, Anvil should inspect project structure and likely test/script locations before falling back to broad model inference.

## Acceptance Criteria

- For Node projects, requests like "tests pass" consider `package.json`, declared test scripts, `test/**`, `tests/**`, and likely `src/**` files.
- For Rust projects, requests like "pricing tests pass" consider `Cargo.toml`, `tests/**`, and matching `src/**` files.
- For Python projects, requests like "tests pass" consider `pyproject.toml`, `pytest.ini`, `tests/**`, and matching source files.
- `agent.repo_context.completed` should avoid `no_candidates` for small projects with recognizable test structure.
- Add focused E2E fixtures for Node, Rust, and Python project-level wording without explicit source paths.

## Evidence

- Issue 448 run: `workspace/eval/runs/issue-448/20260430-070803`
- Failing Node generic run: `raw/e2e-node-graph-qwen36.log`
- Passing Node explicit-path rerun: `raw/e2e-node-explicit-qwen36.log`
