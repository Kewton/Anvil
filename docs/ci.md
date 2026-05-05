# CI and Release Confidence

The default CI path is deterministic and does not require Ollama. Live model
validation is manual so regular pull requests are not blocked by local model
availability, model latency, or runner hardware.

## Default CI

Default CI runs on `main`, `develop`, and `release/**` branches:

- `fmt`: `cargo fmt --all -- --check`
- `clippy`: `cargo clippy --all-targets -- -D warnings`
- `msrv`: `cargo check --all-targets --locked` on Rust `1.86.0`
- `audit`: RustSec dependency advisory check
- `cli-help`: compares `anvil --help` against `docs/cli-help.snapshot.txt`
- `test`: `cargo test --all` on Ubuntu and macOS
- `shellcheck`: shell script lint, repository hygiene, and dry-run shell smoke tests
- `python`: Python lint, snapshot, compatibility, and security tests
- `build`: release build on Ubuntu and macOS after the required checks pass

## CLI Help Snapshot

The public CLI surface is tracked in `docs/cli-help.snapshot.txt`.

When a help change is intentional, refresh it with:

```bash
cargo run --quiet -- --help > docs/cli-help.snapshot.txt
bash scripts/check_cli_help_snapshot.sh
```

This catches README / release documentation drift before a release branch is
cut.

## Repository Hygiene

`bash scripts/check_repo_hygiene.sh` fails when transient local artifact
directories such as `workspace/`, `.anvil/`, `.commandmate/`, `dev-reports/`, or
`sandbox/` are tracked. The policy is documented in
`docs/repository-hygiene.md`.

## Manual Live E2E

`.github/workflows/live-e2e.yml` is `workflow_dispatch` only. It is intended for
self-hosted runners that already have Ollama and the requested model installed.

Typical inputs:

- `runner_labels`: `["self-hosted"]` or a more specific label set
- `model`: installed Ollama model, for example `qwen3.6:27b-coding-nvfp4`
- `ollama_host`: usually `http://127.0.0.1:11434`
- `runs`: repeat count for stability-oriented ignored tests
- `test_filter`: ignored Rust test name from `tests/e2e_local_llm.rs`

Keep this workflow opt-in. Live E2E is valuable before releases, but it is not
stable enough for default CI because it depends on model availability, local
hardware, and optional frontend toolchains.
