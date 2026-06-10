# WP2b Authoring Style Contract Packet Validation

Date: 2026-06-10

## Scope

WP2 の追加検証として、`AuthoringStyleDecision` を raw user prompt ではなく sealed `TaskContract` / `TaskExecutionContract` 経由で Contract-Bound Generation packet に渡す最小実装を検証した。

狙いは次の 3 点。

- evidence runner flavor を test authoring style と混同しない
- 曖昧な新規 Python テストでは、local LLM が壊しにくい pytest function style を model robust default として示す
- 明示された `unittest` 要求は pytest default で上書きしない

## Implementation Summary

- `src/agent/loop_run/authoring_style.rs` を追加し、runner flavor と authoring style decision を分離した。
- `TaskContract` に `authoring_style_decision` を追加し、`TaskExecutionContract` へ投影した。
- Contract-Bound Generation packet に `authoring_style_decision` と `authoring_style_policy` を含めた。
- 実 LLM で style decision は pytest へ寄ったが、CLI subprocess test が temp working directory から relative `main.py` を呼ぶ別失敗を露出した。
- そのため、benchmark 固有名ではなく runtime test binding policy として、repo-local CLI entrypoint は repo root または absolute script path に束縛し、temp dirs は input data 用にする最小ガードを追加した。

## Rust Validation

Passed:

- `cargo fmt --check`
- `cargo test --lib authoring_style`
- `cargo test --lib contract_bound_generation`
- `cargo test --lib project_unit`
- `cargo test --lib` outside the sandbox: 4137 passed
- `cargo build`

Note:

- `cargo test --lib` inside the sandbox failed because local `mockito` test servers could not bind sockets (`Operation not permitted`). Re-running the same command outside the sandbox passed, so this is not attributed to the WP2b change.

## Real LLM Validation

Model:

- `qwen3.6:27b-coding-nvfp4`

Command shape:

- `target/debug/anvil -m qwen3.6:27b-coding-nvfp4 --max-iterations 14 --chat-timeout-secs 180 --oneshot -y --no-footer --fresh-session`

### Case 1: Python CSV CLI, Ambiguous Test Style

Prompt:

- Create `main.py` and `tests/test_main.py`.
- CLI accepts a CSV path with columns `item,amount`.
- Print total amount.
- Include tests and run them.

Result:

- Anvil completed in 4 iterations.
- Verified with `python3 -m pytest -q -p no:cacheprovider`.
- Generated pytest function-style tests.
- Tests used a temporary CSV input file, but called repo-local `main.py` via a path derived from `tests/__file__`.
- Local verification: `4 passed`.

### Case 2: Python JSON CLI, Ambiguous Test Style

Prompt:

- Create `main.py` and `tests/test_main.py`.
- CLI accepts a JSON file path containing objects with `name` and `active`.
- Print active names sorted alphabetically.
- Include tests and run them.

Result:

- Anvil completed in 3 iterations.
- Verified with `python3 -m pytest -q -p no:cacheprovider`.
- Generated pytest function-style tests.
- Tests used temporary JSON data and repo-root-bound `main.py`.
- Local verification: `6 passed`.

### Case 3: Explicit Python unittest

Prompt:

- Create `calc.py` and `tests/test_calc.py`.
- Implement `add(a, b)` and `multiply(a, b)`.
- Use Python `unittest` style tests explicitly.
- Verify with `python -m unittest discover -s tests`.

Result:

- Anvil completed in 8 iterations.
- It hit invalid tool-call / missing artifact retries, but continued to tool actions instead of final-answering early.
- Generated `unittest.TestCase` tests.
- Verified with `python3 -m unittest discover -s tests`.
- Local verification: `Ran 2 tests OK`.

## Insight

The typed decision path is working better than prompt-only guidance:

- Ambiguous Python tasks no longer inherited `unittest` style merely because a verifier can run `unittest discover`.
- Explicit `unittest` remained authoritative.
- The first follow-up failure was not style selection, but CLI test binding: pytest generated a subprocess test that changed cwd away from the repo-local script.

The useful architectural boundary is therefore:

- LLM decides semantic intent where ambiguity exists.
- Controller admits the result as a typed `AuthoringStyleDecision`.
- Generation packet carries that decision and runtime binding policy as contract facts.
- Runtime-specific compatibility guidance stays generic and capability-based, not benchmark-specific.

## Remaining Risk

The latest LLM runs are positive but still small. Do not claim broad success-rate improvement yet.

Next validation should include:

- 10-20 mixed Python coding / TDD / feature-change tasks before claiming improvement.
- At least one hard repair case where initial pytest tests fail and repair must preserve the admitted authoring style.
- Non-Python sanity checks to ensure runtime test binding policy does not leak Python-specific assumptions into Rust, Node, docs, data, or research tasks.
