# WP3 Style / Runner Logging Validation

Date: 2026-06-10

## Scope

WP3 の最小 slice として、success authority を変えずに `agent.task_contract.verifier.selection` の structured payload へ style / runner 観測情報を追加した。

追加した fields:

- `authoring_style`
- `style_authority`
- `python_project_unit_verifier_flavor`
- `runner_style_mismatch`

## Implementation Summary

- `PythonProjectUnitVerifierFlavor::label()` を追加した。
- Python verifier command だけを flavor に分類する `python_project_unit_verifier_flavor_if_python()` を追加した。
- `python_runner_style_mismatch_label()` を追加し、pytest function style を unittest discover runner で実行する危険を観測できるようにした。
- `select_task_contract_verifier_once()` の既存 log event に fields を追加した。
- verifier selection / success 判定 / repair lifecycle は変更していない。

## Rust Validation

Passed:

- `cargo fmt --check`
- `cargo test --lib authoring_style`
- `cargo test --lib verifier_command_policy`
- `cargo test --lib verifier_selection`
- `cargo test --lib` outside the sandbox: 4139 passed
- `cargo build`

## Real LLM Validation

Model:

- `qwen3.6:27b-coding-nvfp4`

Prompt:

- Create a Python CLI in `main.py` and `tests/test_main.py`.
- CLI accepts one text file path and prints the number of non-empty lines.
- Include tests and run them.

Result:

- Anvil completed in 5 iterations.
- Verified with `python3 -m pytest -q -p no:cacheprovider`.
- Local verification: `7 passed`.
- Generated tests used pytest style and did not require unittest discovery.

Observed structured log:

```json
{
  "event": "agent.task_contract.verifier.selection",
  "payload": {
    "authoring_style": "pytest_function_style",
    "style_authority": "model_robust_default",
    "python_project_unit_verifier_flavor": "pytest_stdlib",
    "runner_style_mismatch": "none",
    "selection": "structured_runnable",
    "test_execution_required": true
  }
}
```

## Insight

This is the right level for WP3:

- The controller can now observe runner/style separation without adding a new semantic branch.
- The log is tied to verifier selection, where runner binding actually becomes concrete.
- Non-Python commands are not mislabeled as Python verifier flavor.

Do not expand this into broad pattern matching yet. The next useful step is to log concrete style mismatch only when source/test artifact inspection or failed evidence provides a typed reason, not from raw prompt fragments.
