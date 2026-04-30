# Issue 448 Evaluation Manifest

Run ID: `20260430-070803`
Issue: `448`
Epic: `Epic E - Repo Graph & Domain Context`
Branch: `develop`
Evaluation commit: `8176bdbaa1100249e455c05649125d6f4d06b37e`
Epic E feature commits: `1421ea0`, `c035493`, `8176bdb`

## Models

| Role | Model |
| --- | --- |
| Main A | `qwen3.6:27b-coding-nvfp4` |
| Main B | `qwen3.5:122b` |
| Sidecar | `qwen3.5:9b` |

## Static Checks

| Command | Log | Result |
| --- | --- | --- |
| `cargo fmt --check` | `raw/cargo-fmt-check.log` | PASS |
| `cargo clippy --all-targets -- -D warnings` | `raw/cargo-clippy.log` | PASS |
| `cargo test` | `raw/cargo-test.log` | PASS |
| `cargo test --test repo_graph_smoke` | `raw/cargo-test-repo-graph-smoke.log` | PASS |
| `cargo test repo_context` | `raw/cargo-test-repo-context-filter.log` | PASS |
| `cargo test path_scoped` | `raw/cargo-test-path-scoped-filter.log` | PASS |

Notes:

- `cargo test --test repo_context_ranking_smoke` and `cargo test --test anvil_md_path_aware_smoke` were attempted because the orchestration summary listed those names, but they are not standalone integration test targets. Their behavior is covered by in-module filtered tests in `src/agent/prompting.rs`.

## Practical E2E

| Scenario | Workdir | Model | Log |
| --- | --- | --- | --- |
| P5-01 Rust graph / qwen3.5 | `e2e/rust-graph-qwen35` | `qwen3.5:122b` | `raw/e2e-rust-graph-qwen35.log` |
| P5-01 Rust graph / qwen3.6 fixed fixture | `e2e/rust-graph-qwen36-r2` | `qwen3.6:27b-coding-nvfp4` | `raw/e2e-rust-graph-qwen36-r2.log` |
| P5-02 Node graph | `e2e/node-graph-qwen36` | `qwen3.6:27b-coding-nvfp4` | `raw/e2e-node-graph-qwen36.log` |
| P5-03 path-scoped ANVIL.md | `e2e/path-anvil-qwen36` | `qwen3.6:27b-coding-nvfp4` | `raw/e2e-path-anvil-qwen36.log` |
| P5-03 path-scoped ANVIL.md R2 | `e2e/path-anvil-qwen36-r2` | `qwen3.6:27b-coding-nvfp4` | `raw/e2e-path-anvil-qwen36-r2.log` |
| P5-04 graph disabled fallback | `e2e/graph-disabled-qwen36` | `qwen3.6:27b-coding-nvfp4` | `raw/e2e-graph-disabled-qwen36.log` |

## Scope Note

This run focused on Issue 448-specific P5 behavior plus static coverage. It did not rerun the full P0-P5 matrix with three repetitions per model.

