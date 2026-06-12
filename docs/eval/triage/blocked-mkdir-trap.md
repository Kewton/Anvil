# Blocked mkdir Trap Triage

Date: 2026-06-12 JST

Basis:

- [Cycle 2 loss triage](cycle2-loss-triage.md)
- Minimal+M001 root: `.anvil/benchmarks/20260612T174200-32077`
- Model: `qwen3.6:27b-coding-nvfp4`

## Findings

### 1. `Write` already creates parent directories

`src/tools/write.rs::run` calls `fs::create_dir_all(parent)` before
`fs::write`. Therefore `Write("src/dateRange.js", ...)` is already sufficient
when `src/` does not exist.

Task21-A is not needed.

### 2. `mkdir -p` is blocked by offline Bash policy

The observed error text is:

```text
ERROR: offline mode only allows read-only, build-test, or local script-run shell commands: mkdir -p src
```

The rejection comes from `src/tools/bash.rs::enforce_offline_policy`.
`mkdir -p` is classified as a general/mutating shell command, and offline mode
allows only read-only, build-test, or local script-run commands. This is not a
dangerous-command safety block and not a bench-only rule; it is the global
offline Bash policy.

The policy is defensible for local-first offline runs because mutating shell is
a broad surface. The trap is that the error did not name the deterministic
replacement: `Write` already performs the needed parent-directory creation.

### 3. Bench-wide blocked command distribution

Run-root `llm-io.jsonl` files in the Task15 minimal root:

- logs inspected: 122
- unique blocked Bash commands observed: 45

By policy:

| policy | count |
|---|---:|
| general/mutating offline block | 44 |
| network offline block | 1 |

By command token:

| command token | count |
|---|---:|
| `mkdir` | 40 |
| `cd ... && <script>` | 4 |
| `npx` | 1 |

By scenario:

| scenario | count |
|---|---:|
| `fix-python-slugify` | 8 |
| `multi-file-python-package` | 7 |
| `long-session-large-component` | 5 |
| `fix-rust-parser-error` | 4 |
| `fix-css-token-doc` | 3 |
| `fix-js-date-helper` | 3 |
| `new-rust-cli-small` | 3 |
| `long-session-read-edit` | 2 |
| `fix-json-normalizer` | 1 |
| `long-session-data-report` | 1 |
| `multi-file-docs-and-examples` | 1 |
| `multi-file-node-package` | 1 |
| `multi-file-rust-library` | 1 |
| `new-large-react-kanban` | 1 |
| `new-python-csv-small` | 1 |
| `non-coding-runbook` | 1 |
| `scaffold-next-dashboard` | 1 |
| `scaffold-rust-cli` | 1 |

Interpretation:

- The dominant block is not package install or network activity. It is
  directory creation before file writing.
- The current bench does not primarily reveal a model capability limit here; it
  reveals a tool-affordance mismatch. The model does not know that `Write`
  handles parent directories, then offline policy blocks the unnecessary Bash
  fallback without giving the replacement action.

## Fixes Selected

Task21-B and Task21-C apply.

Task21-B in this PR:

- `Write` tool description now states: parent directories are created
  automatically.

Task21-C is a separate PR:

- Offline policy rejection for general/mutating Bash should add: call `Write`
  directly for file creation, because `Write` creates parent directories
  automatically.

Prompt/token impact for Task21-B:

- Tool catalog change adds one sentence to the `Write` description:
  `Parent directories are created automatically.`
- Increment: 51 characters, 5 whitespace-delimited words. Token count is
  tokenizer-dependent, but this is a small factual tool-description correction.

## Deferred

The following are intentionally not changed in this PR:

- No M002 mechanism is added.
- `mkdir -p` is not allowed through offline Bash policy. That would broaden
  mutating shell permissions and is unnecessary while `Write` can do the
  deterministic operation.
- Semantic verifier continuation for `fix-js-date-helper` run-5 remains out of
  scope.

## Recommended Follow-Up

After Task21-B, Task21-C, and Task22 are merged, run a narrow seeded matrix on:

- `fix-js-date-helper`
- `new-python-csv-small`
- `non-coding-runbook`
- `fix-python-slugify`
- `fix-python-retry-policy`

Measure whether `M001 -> Bash mkdir blocked -> no Write` disappears before
considering any M002 feedback mechanism.
