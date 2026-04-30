# Issue 444 Evaluation Manifest

Run ID: `20260428-105334`
Date: 2026-04-28
Issue: `#444 Dynamic Precaution Runtime`
Commit: `e9d9857f246a1ae3253ebada12bf7fd72a8b5c5c`
Branch: `develop`

## Scope

Evaluate the completed Issue 444 implementation against the evaluation plan in:

- `workspace/eval/issue_444_449_eval_plan.md`

Primary target capabilities:

- FeedbackFrame normalization
- WorkingMemory active precautions
- Reminder Sidecar
- Act-mode Active Precautions prompt injection
- `/precautions` slash command
- runtime recovery connection into FeedbackFrame/Reminder

## Models

| Role | Model |
| --- | --- |
| main | `qwen3.5:122b` |
| main | `qwen3.6:27b-coding-nvfp4` |
| sidecar | `qwen3.5:9b` |

## Commands

Static/regression verification:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Representative P0 E2E verification:

```bash
target/debug/anvil -m qwen3.6:27b-coding-nvfp4 --sidecar-model qwen3.5:9b --fresh-session --oneshot --no-footer -y -p 'READMEを要約してください。ファイルは変更しないでください。'
target/debug/anvil -m qwen3.6:27b-coding-nvfp4 --sidecar-model qwen3.5:9b --auto-plan --fresh-session --oneshot --no-footer -y -p 'PythonでCSVを読み込んでカテゴリ別合計を出すCLIを作って下さい。サンプルCSVと実行手順も含めて下さい。'
target/debug/anvil -m qwen3.5:122b --sidecar-model qwen3.5:9b --fresh-session --oneshot --no-footer -y -p 'READMEを要約してください。ファイルは変更しないでください。'
target/debug/anvil -m qwen3.5:122b --sidecar-model qwen3.5:9b --auto-plan --fresh-session --oneshot --no-footer -y -p 'PythonでCSVを読み込んでカテゴリ別合計を出すCLIを作って下さい。サンプルCSVと実行手順も含めて下さい。'
```

## Coverage Notes

The full 3-repetition matrix from the long-term plan was not executed in this initial assessment. This run establishes the Issue 444 baseline using:

- full Rust unit/integration/doc-test coverage
- representative P0 E2E coverage on both required main models
- P1 Dynamic Precaution behavior through deterministic unit/integration tests

Raw logs are stored under:

- `workspace/eval/runs/issue-444/20260428-105334/raw/`
