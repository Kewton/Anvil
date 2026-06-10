# WP9 TurnState Ownership Validation

Date: 2026-06-10

## Scope

WP9 moved remaining verifier event dedup carriers from `Agent` into `TurnState`:

- `last_verifier_invoked_payload_digest`
- `external_import_rejected_emitted`

The change is refactor-oriented. It reduces direct per-turn reset fields in `handle_user_message` and keeps verifier event caps under `TurnState::reset_dedup_state()`. No new semantic branch, terminal state, or recovery policy was added.

## Deterministic Validation

Commands:

- `cargo fmt --check`
- `cargo test --lib turn_state`
- `cargo test --lib issue661`
- `cargo build`

Result:

- all passed.

Covered assertions:

- fresh `Agent` initializes verifier dedup state through `TurnState`;
- `TurnState::reset_dedup_state()` clears job report dedup, behavior projection dedup, verifier invoked digest, and external-import cap together;
- verifier invoked event still dedups identical snapshots within a turn;
- verifier invoked event re-emits after a turn-state reset;
- external-import rejected event still emits once per turn and re-emits after reset.

## Real LLM Validation

Model:

- `qwen3.6:27b-coding-nvfp4`

Command shape:

- `target/debug/anvil -m qwen3.6:27b-coding-nvfp4 --max-iterations ... --chat-timeout-secs 180 --oneshot -y --no-footer --state-dir /private/tmp/anvil-wp9-smoke/state -p "..."`

Note:

- `--resume` cannot be combined with `--oneshot` / `-p`.
- For two-turn smoke, the second command reused the same state directory and project workdir without `--fresh-session`; the CLI continued the latest session.

### Case 1: Docs -> Coding

Workdir:

- `/private/tmp/anvil-wp9-smoke/docs-coding`

Turn 1 prompt:

- create `README.md`;
- include Overview and Usage sections.

Turn 1 result:

- `done`;
- 1 iteration;
- edited `README.md`.

Turn 2 prompt:

- create `math_utils.py` and `tests/test_math_utils.py`;
- implement `square(n)`;
- include pytest tests and verify them;
- do not edit `README.md`.

Turn 2 result:

- continued session;
- `done`;
- 4 iterations;
- edited `math_utils.py` and `tests/test_math_utils.py`;
- did not edit `README.md`.

Assessment:

- Basic docs-to-coding turn reuse did not show cross-turn contamination.
- Turn-local verifier dedup reset did not block the second turn.

### Case 2: Coding -> Data

Workdir:

- `/private/tmp/anvil-wp9-smoke/coding-data`

Turn 1 prompt:

- create `parser.py` and `tests/test_parser.py`;
- implement `parse_int(value)`;
- include pytest tests and verify them.

Turn 1 result:

- `done`;
- 4 iterations;
- edited `parser.py` and `tests/test_parser.py`;
- verified with `python3 -m pytest -q -p no:cacheprovider`.

Turn 2 prompt:

- create `output.csv` with columns `id,total`;
- rows: `1,10` and `2,25`;
- only create the CSV file;
- do not edit `parser.py` or tests.

Turn 2 result:

- continued session;
- terminal state: `safe_stop_verifier_missing`;
- edited `tests/test_main.py`;
- did not create `output.csv`.

Observed log facts:

- the second turn still received coding-oriented verifier context;
- `Contract-Bound Generation` declared a test artifact instead of the CSV file;
- artifact-directed recovery targeted `tests/test_main.py`;
- the model followed that narrowed recovery target and wrote a test for `output.csv`, but the CSV deliverable remained missing.

Assessment:

- This is not a verifier event dedup reset bug.
- The remaining failure is semantic/session contamination: a data-only second turn can inherit coding/evidence pressure from the previous session and existing project artifacts.
- Fixing this inside WP9 would require objective contract admission / data deliverable construction changes, not just TurnState ownership cleanup.

## Architecture Assessment

WP9 moves more turn-local state behind a single reset owner and removes direct verifier dedup resets from `handle_user_message`. This is aligned with the target architecture: actor/controller code should reset grouped state through typed lifecycle owners instead of enumerating unrelated fields.

The coding-to-data smoke exposes a higher-level issue that still matters for the architecture: turn-state reset can clear per-turn counters, but it cannot by itself prevent semantic drift from conversation history or existing artifact context. The next architecture slices should make objective contract construction authoritative for the current user request, especially for non-coding deliverables, rather than relying on WorkMode or stale verifier expectations.
