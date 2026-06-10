# WP4 SemanticCandidate Shadow Validation

Date: 2026-06-10

## Scope

WP4 の最小 slice として、sealed `TaskContract` から deterministic shadow `SemanticCandidate` を作成し、per-turn structured log に出す接続を追加した。

この WP では candidate を completion / repair / tool policy の authority にしない。

## Implementation Summary

- 既存 `task_contract_semantic_candidate.rs` を再利用し、新規の重複 semantic layer は作らなかった。
- `SemanticCandidate` に次を追加した。
  - `authoring_style`
  - `compatibility_risks`
- `populate_task_contract_authority()` で deterministic shadow candidate を生成し、`agent.semantic_candidate.shadow` を emit する。
- admission は `allow_equivalent_current_behavior=false` のため、equivalent candidate でも status は `ignored` / reason は `shadow_only`。

## Rust Validation

Passed:

- `cargo fmt --check`
- `cargo test --lib task_contract_semantic_candidate`
- `cargo test --lib task_contract_admission`
- `cargo build`

## Real LLM Validation

Model:

- `qwen3.6:27b-coding-nvfp4`

### Docs

Prompt:

- Create `README.md` with sections Overview, Usage, and Validation.

Result:

- Completed in 1 iteration.
- Shadow event:
  - `objective_kind=docs`
  - `deliverable_kind=document_sections`
  - `evidence_kind=content_check`
  - `status=ignored`
  - `reasons=["shadow_only"]`

### Data

Prompt:

- Generate `output.csv` with columns `id,total` and rows `1,100` and `2,250`.

Result:

- Completed in 1 iteration.
- Shadow event:
  - `objective_kind=data`
  - `deliverable_kind=output_file`
  - `evidence_kind=schema_check`
  - `status=ignored`
  - `reasons=["shadow_only"]`

### Python Coding

Prompt:

- Create `main.py` and `tests/test_main.py`.
- Implement `slugify(text)` and run tests.

Result:

- Completed in 4 iterations.
- Verified with `python3 -m pytest -q -p no:cacheprovider`.
- Local verification: `6 passed`.
- Shadow event:
  - `objective_kind=coding`
  - `deliverable_kind=source_files`
  - `evidence_kind=test_run`
  - `authoring_style=pytest_function_style`
  - `style_authority=model_robust_default`
  - `compatibility_risks=["evidence_required_without_explicit_runner_hint"]`
  - `status=ignored`
  - `reasons=["shadow_only"]`

## Insight

WP4 is correctly behavior-preserving:

- The candidate vocabulary is visible in logs across docs/data/coding.
- The candidate carries authoring style and compatibility risk without driving success.
- Non-coding tasks use the same event shape, which supports Anvil's general-purpose direction.

Known issue:

- This is still deterministic shadow, not true LLM semantic candidate admission. It improves observability and prepares WP5, but it does not by itself improve task success rate.
