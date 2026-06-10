# WP8 Actor Loop Phase Refactor Validation

Date: 2026-06-10

## Scope

WP8 is a refactor-only slice:

- added `LoopPhaseEvent` builders in `loop_phase.rs`;
- moved the contract-admitted / generation-prepared / tool-execution enter transition construction out of `run_actor_loop`;
- moved prepared tool/repo-edit counter updates into `LoopState::record_prepared_tool_batch`;
- did not add new semantic branches, recovery branches, or terminal states.

## Deterministic Validation

Commands:

- `cargo fmt --check`
- `cargo test --lib loop_phase`
- `cargo test --lib loop_state`
- `cargo build`

Result:

- all passed.

Covered unit assertions:

- loop phase labels remain unchanged;
- event-builder snapshot payloads keep the same structured labels and detail keys;
- `LoopState` owns tool-call and repo-edit counter updates and reset behavior.

## Real LLM Validation

Model:

- `qwen3.6:27b-coding-nvfp4`

### Case 1: Docs Runbook

Workdir:

- `/private/tmp/anvil-wp8-smoke/docs-1`

Prompt:

- create `docs/runbook.md`;
- include Overview, Prerequisites, Deploy, Rollback, Validation.

Result:

- `done`;
- 1 iteration;
- edited `docs/runbook.md`.

### Case 2: Python Neutral Coding

Workdir:

- `/private/tmp/anvil-wp8-smoke/python-1`

Prompt:

- create `stats.py` and `tests/test_stats.py`;
- implement `mean` and `median`;
- raise `ValueError` for empty list;
- include pytest tests and verify them.

Result:

- `done`;
- 4 iterations;
- edited `stats.py`, `tests/test_stats.py`;
- verified with `python3 -m pytest -q -p no:cacheprovider`.

### Case 3: Hard Node Coding

Workdir:

- `/private/tmp/anvil-wp8-smoke/node-1`

Prompt:

- create `src/normalize_records.js` and `tests/normalize_records.test.js`;
- implement `normalizeRecords(records)`;
- verify with `node --test tests/normalize_records.test.js`;
- do not create README files.

Result:

- `done`;
- 11 iterations;
- edited `package.json`, `src/normalize_records.js`, `tests/normalize_records.test.js`;
- went through verifier failure and bounded repair;
- verified with `npm test`.

## Log Verification

Observed `agent.loop_phase.transition` events in the hard Node run:

- `contract_admitted` / `exit` with `contract_present=true`;
- `generation_prepared` / `enter` with `reply_tool_call_count`;
- `generation_prepared` / `exit` with `current_reply_tool_call_count` and `prepared_tool_call_count`;
- `tool_execution` / `enter` with `prepared_tool_call_count`;
- `tool_execution` / `exit` with `repo_edit_calls_made_this_turn` and `tool_calls_made_this_turn`.

## Assessment

WP8 reduced `run_actor_loop` responsibility for phase-event construction and counter mutation without changing runtime behavior. The real LLM smoke covered docs, neutral Python coding, and a hard Node repair path. No new regression was observed.

The remaining actor-loop complexity is still high. Further reduction should continue by extracting local transition/data builders, not by adding new task-kind-specific branches to `run_actor_loop`.
