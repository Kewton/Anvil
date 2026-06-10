# WP5 SemanticCandidate Limited Adoption Validation

Date: 2026-06-10

## Scope

WP5 の最小 slice として、equivalent deterministic `SemanticCandidate` を低リスク範囲で `admitted` として扱うログ経路を追加した。

この WP でも sealed `TaskContract` は mutation しない。admission は observability / future adoption boundary のみ。

## Implementation Summary

- `limited_semantic_candidate_adoption_enabled()` を追加した。
- docs / data / research / ops / authoring は equivalent candidate を limited adoption 可能にした。
- coding は `AuthoringStyleDecision` に `Unknown` 以外の authority がある場合のみ limited adoption 可能にした。
- `agent.semantic_candidate.shadow` に `admission_mode` を追加した。

## Rust Validation

Passed:

- `cargo fmt --check`
- `cargo test --lib task_contract_semantic_candidate`
- `cargo build`

## Real LLM Validation

Model:

- `qwen3.6:27b-coding-nvfp4`

### Docs

Prompt:

- Create `README.md` with sections Overview and Usage.

Result:

- Completed in 1 iteration.
- Shadow event:
  - `admission_mode=limited_adoption`
  - `status=admitted`
  - `objective_kind=docs`

### Data

Prompt:

- Generate `summary.json` with fields `topic`, `status`, and `count`.

Result:

- Completed in 1 iteration.
- Shadow event:
  - `admission_mode=limited_adoption`
  - `status=admitted`
  - `objective_kind=data`

### Python Coding

Prompt:

- Create `main.py` and `tests/test_main.py`.
- Implement `is_palindrome(text)` ignoring case and spaces.

Result:

- Completed in 3 iterations.
- Verified with `python3 -m pytest -q -p no:cacheprovider`.
- Local verification: `8 passed`.
- Shadow event:
  - `admission_mode=limited_adoption`
  - `status=admitted`
  - `objective_kind=coding`
  - `authoring_style=pytest_function_style`
  - `style_authority=model_robust_default`

## Insight

This moves the architecture one step closer to a single semantic admission surface:

- Non-coding tasks and coding style authority now share the same candidate/admission event shape.
- WorkMode is still not completion authority.
- Memory and raw request helpers do not gain authority.

Known issue:

- This is still equivalent deterministic candidate adoption. It does not yet consume true LLM semantic candidates, so it should not be counted as a success-rate improvement by itself.
