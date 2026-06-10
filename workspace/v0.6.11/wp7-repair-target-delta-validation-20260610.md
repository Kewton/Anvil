# WP7 RepairTargetDecision Typed Delta Validation

Date: 2026-06-10

## Scope

WP7 introduced a typed repair-delta vocabulary at the repair-target boundary:

- `missing_deliverable`
- `missing_evidence`
- `evidence_failed`
- `tool_failure`
- `style_mismatch`
- `stale_evidence`

The change is intentionally additive. Existing target selection, operator dispatch order, and verifier repair execution are unchanged. The new fields are projected into:

- `RepairTargetDecision::to_json_value()`
- `RepairTargetLedgerFacts`
- `DiagnosticRepairWorkerRequest` policy/context
- `TestAuthorWorkerRequest` policy/context

## Deterministic Validation

Commands:

- `cargo fmt --check`
- `cargo test --lib repair_target_decision`
- `cargo test --lib worker_request`
- `cargo build`

Result:

- all passed.

Covered unit assertions:

- style/test-artifact mismatch projects `delta_kind=style_mismatch`;
- unbound Node runner projects `delta_kind=missing_evidence`;
- stale candidate override projects `delta_kind=stale_evidence`;
- diagnostic repair worker carries `repair_delta` and `ledger_facts`;
- test author worker carries `repair_delta=missing_evidence`;
- non-coding diagnostic targets still project docs/data roles.

## Real LLM Validation

Model:

- `qwen3.6:27b-coding-nvfp4`

Command shape:

- `target/debug/anvil -m qwen3.6:27b-coding-nvfp4 --max-iterations ... --chat-timeout-secs 180 --oneshot -y --no-footer --fresh-session --state-dir /private/tmp/anvil-wp7-smoke/state -p "..."`

### Case 1: Explicit Python unittest style

Workdir:

- `/private/tmp/anvil-wp7-smoke/style-mismatch-1`

Prompt:

- create `calc.py` and `tests/test_calc.py`;
- implement `add` and `multiply`;
- explicitly use `unittest.TestCase`;
- verify with `python -m unittest discover -s tests`.

Result:

- `done`;
- 4 iterations;
- edited `calc.py`, `tests/test_calc.py`;
- verified with `python3 -m unittest discover -s tests`.

Assessment:

- WP7 did not regress explicit unittest authoring.

### Case 2: Missing test artifact

Workdir:

- `/private/tmp/anvil-wp7-smoke/missing-test-1`

Initial fixture:

- existing `slugify.py`;
- no test artifact.

Prompt:

- add `tests/test_slugify.py`;
- cover spaces, punctuation, lowercase conversion;
- verify with `pytest tests/test_slugify.py`;
- do not rewrite `slugify.py` unless tests reveal a real bug.

Result:

- `done`;
- 3 iterations;
- edited only `tests/test_slugify.py`;
- verified with `python3 -m pytest -q -p no:cacheprovider`.

Assessment:

- Missing-deliverable/test-author path still works.

### Case 3: Existing source/test, verification-only

Workdir:

- `/private/tmp/anvil-wp7-smoke/missing-evidence-1`

Initial fixture:

- existing `calc.py`;
- existing `tests/test_calc.py`.

Prompt:

- verify existing behavior;
- if tests pass, do not change source files;
- record verification result.

Result:

- `missing_repo_edits`;
- 1 iteration;
- internal verifier passed, but completion was blocked by the fresh repo edit guard.

Assessment:

- This is not caused by WP7 delta projection.
- It exposes a pre-existing WP6 guard issue: verification-only coding tasks are still represented as coding build/modify work and require fresh edit evidence.

### Case 4: Hard Node JSON formatter

Workdir:

- `/private/tmp/anvil-wp7-smoke/hard-node-1`

Prompt:

- create `src/format_json.js` and `tests/format_json.test.js`;
- implement `formatJson(input)`;
- verify with `node --test tests/format_json.test.js`;
- do not create README files.

Result:

- `done`;
- 6 iterations;
- edited `package.json`, `src/format_json.js`, `tests/format_json.test.js`;
- MissingEvidence runner binding completed `package.json`;
- verified with `npm test`.

Assessment:

- Hard Node still completes with runner-manifest binding.

### Case 5: Evidence failed repair target

Workdir:

- `/private/tmp/anvil-wp7-smoke/repair-python-1`

Initial fixture:

- `calculator.py` returned `a - b`;
- `tests/test_calculator.py` expected `add(2, 3) == 5`.

Prompt:

- fix existing `calculator.py` so existing tests pass;
- do not rewrite tests unless clearly wrong;
- verify with `pytest tests/test_calculator.py`.

Result:

- Anvil repaired `calculator.py`;
- terminal state: `repair_safe_stop` with `verifier_unavailable`;
- manual `PYTHONPATH=. pytest -q tests/test_calculator.py` passed.

Observed typed delta log:

- event: `agent.repair_target_decision.selected`;
- `delta_kind=evidence_failed`;
- target: `calculator.py`;
- target role: `implementation`;
- ledger facts included selected target, target authority, candidate count, and rejected lower-authority test hint count.

Assessment:

- WP7 successfully emits typed repair delta and ledger facts on the live repair path.
- Existing verifier rerun binding remains weak for this Python import shape unless `PYTHONPATH=.` is supplied.

## Architecture Assessment

WP7 moves repair reporting away from raw failure text without adding a new benchmark-specific repair path. The typed delta is still conservative and does not replace the diagnostic LLM. It gives the controller and prompt layer a stable vocabulary for "what changed between contract/ledger/evidence" while keeping raw failure output auxiliary.

No success-rate improvement is claimed from WP7 alone. The improvement is observability and prompt-boundary quality: future repair work can reason over `delta_kind` and `ledger_facts` instead of repeatedly adding output-string patterns.
