# Repository Hygiene

The public repository should contain source, tests, deterministic fixtures, CI,
and documentation. Local run outputs, model transcripts, generated screenshots,
and scratch planning files should stay out of git.

## Commit These

- Product source under `src/`
- Deterministic tests under `tests/`
- Stable fixtures under `tests/fixtures/` or `tests/golden/`
- Public documentation under `docs/`
- Release and CI automation under `.github/`, `scripts/`, and root metadata

## Do Not Commit These

- `.anvil/`: local runtime state, logs, sessions, plans
- `.commandmate/`: local command tool attachments and screenshots
- `workspace/`: local planning notes, UAT workdirs, eval runs, temporary reports
- `dev-reports/`: issue-development scratch reports
- `sandbox/`: generated test sandboxes
- `.env*`, caches, local logs, and temporary files

`workspace/eval/runs/` is intentionally local. It is the right place for raw
E2E/UAT results during development, but those run directories are not stable
release artifacts. When an evaluation result should be public, summarize it in
`docs/e2e-uat-evaluation.md`, `docs/metrics.md`, or a new focused document
under `docs/`.

## Evaluation Artifacts

Use these locations:

- Raw local E2E/UAT runs: `workspace/eval/runs/<run-id>/`
- Durable evaluation design: `docs/e2e-uat-evaluation.md`
- Durable metric summaries: `docs/metrics.md`
- Deterministic harness fixtures: `tests/golden/`
- Reusable scenario/test inputs: `tests/fixtures/`

Before promoting local output into git, remove machine-specific paths, model
transcripts that may include private prompts, screenshots with local data, and
anything that depends on a local username or filesystem layout.

## Check

Run:

```bash
bash scripts/check_repo_hygiene.sh
git status --short --ignored
```

The hygiene script fails if transient top-level artifact directories are
tracked. It is intentionally conservative: if a generated artifact is useful
enough to publish, move it to a documented public location first.
