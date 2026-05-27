# Cyclomatic Complexity Implementation Log

## Completed In This Pass

### Phase 1: Complexity Report Script

Implemented:

- `scripts/complexity_report.py`
  - approximate Rust function-level complexity report
  - default scope: `src/**/*.rs`
  - supports explicit Rust file/directory inputs
  - emits text or JSON
  - reports file metrics and top function hotspots
  - safely ignores files outside the repository root
  - masks comments, normal string literals, raw string literals, and simple char
    literals before counting branch-like tokens

Tests:

- `tests/test_complexity_report.py`
  - JSON reporting
  - comments/string/raw-string masking
  - path escape rejection

Result:

- The report now correctly identifies `turn.rs::run_actor_loop` as the dominant
  hotspot.
- A focused baseline is recorded in `workspace/v0.4.25/complexity-baseline.md`.

### Phase 3: Model Request Boundary, First Slice

Implemented:

- `src/agent/loop_run/model_request.rs`
  - `should_use_streaming_transport`
  - `non_streaming_assistant_reply_timeout_secs`
  - `effective_non_streaming_timeout_secs`
  - `focused_edit_timeout_override_secs`
  - `focused_edit_max_predict_override`
  - `request_non_streaming_assistant_reply`

Changed:

- `turn.rs` now calls `request_non_streaming_assistant_reply` instead of owning
  non-streaming request execution directly.
- focused-edit request sizing and streaming-transport selection no longer live
  in `turn.rs`.

Result:

- `turn.rs` function count dropped from 1042 to 1036 in the focused report.
- `model_request.rs` has low current complexity:
  - functions: 9
  - max rough CC: 8
  - functions with rough CC >= 15: 0

## Verification

Commands run:

```bash
python3 -m unittest tests/test_complexity_report.py
cargo fmt --check
cargo test model_request --lib -q
cargo test focused_edit_overrides --lib -q
cargo test --lib -q
cargo clippy --all-targets -- -D warnings
cargo build --release
git diff --check
```

Notes:

- The first sandboxed `cargo test --lib -q` failed because `mockito` could not
  start local test servers under sandbox restrictions:
  `Operation not permitted (os error 1)`.
- The same command passed when rerun with local server binding allowed:
  `3058 passed; 0 failed`.

## Remaining Work

Highest priority:

1. Extract message composition from `build_request_messages`.
2. Continue extracting model request retry orchestration from
   `request_assistant_reply_with_retry`.
3. Extract verifier execution into a `verifier_driver`.
4. Extract verifier repair orchestration into a `repair_driver`.
5. Normalize tool execution through typed `ToolExecutionOutcome`.
6. Split `RepairJob` internals only after the driver boundary is stable.
7. Extract the final turn driver to shrink `run_actor_loop`.

Important constraint:

- Do not attempt to reduce complexity by adding business-specific repair
  branches. The current work is structural and generic.
