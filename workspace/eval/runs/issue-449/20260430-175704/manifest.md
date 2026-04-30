# Issue 449 Evaluation Manifest

Run ID: `20260430-175704`
Issue: `#449` Epic F: Observability / Evaluation / Training Export
Evaluation commit: `be1fae4534a3ac162f8f8de2dd72993a51a2d05a`
Branch: `develop`

## Included Implementation Commits

- `63c1fc5` feat(eval-log): introduce structured evaluation log per turn (#471)
- `76838c3` feat(eval-harness): introduce local model A/B evaluation harness (#472)
- `bb34448` feat(dataset-export): introduce fine-tuning dataset export (#473)

## Models

- Live E2E: `qwen3.6:27b-coding-nvfp4`
- Sidecar: `qwen3.5:9b`
- A/B harness dry-run matrix: `qwen3.5:122b`, `qwen3.6:27b-coding-nvfp4`

## Commands

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- `cargo test --test eval_log_smoke`
- `cargo test --test eval_harness_smoke`
- `cargo test --test dataset_export_smoke`
- `python3 -m unittest tests/test_compare_security.py`
- `target/debug/anvil ... --oneshot ...`
- `target/debug/anvil sessions export ...`
- `scripts/bench.sh heavy-space-invaders --models qwen3.5:122b,qwen3.6:27b-coding-nvfp4 --runs 1 --dry-run --no-precautions --no-case-memory --no-auto-test`

## Scenarios

- P6-01: successful live turn writes `logs/eval.jsonl`
- P6-01-scrub: successful live turn with `ANVIL_EVAL_SCRUB_PATHS=1`
- P6-03: A/B harness interface and matrix report
- P6-04: dataset export JSONL shape, filters, and redaction behavior

## Notes

GitHub Issue `#449` and child issues `#471`, `#472`, `#473` are still open at evaluation time, but the corresponding implementation commits are present in `develop`.
