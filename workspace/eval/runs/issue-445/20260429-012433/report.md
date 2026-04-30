# Issue 445 Evaluation Report

Run ID: `20260429-012433`
Commit: `28825fa14fed07c68a349223a0bed878b5d4b055`

## Verdict

Judgement: `Mixed / Instrumentation Improved, Convergence Still Weak`

Epic B added useful verification surfaces: `AnvilScore` is persisted and logged, temporary test workspace behavior is covered, Tester Skill smoke generation is covered in integration tests, and generated-test promotion/discard plus sandbox behavior are tested. However, the practical E2E run shows the same Python verification false-negative from Issue 444 remains, and project-local `ANVIL.md` verification guidance is not yet connected to final `AutoTestRunner` success criteria.

## Static Verification

| Check | Result | Notes |
| --- | --- | --- |
| `cargo fmt --check` | PASS | no formatting drift |
| `cargo clippy --all-targets -- -D warnings` | PASS | no warnings |
| `cargo test` | PASS | all unit, integration, and doc tests passed |

Issue 445-specific coverage observed in `cargo test`:

- `tests/session_tests.rs`: `AnvilScore` persistence, lossy deserialization, sanitization, prompt rendering.
- `tests/tester_skill_smoke.rs`: Tester Skill v1 smoke runs for Rust, Node, Python, malformed output, path confinement, per-turn cap, disabled and plan-mode gates.
- `tests/tmp_tests_e2e.rs`: temporary test workspace isolation, session resume, promote/discard, path traversal rejection, workspace cleanliness.
- `tests/tests_slash_command.rs`: `/tests list`, `/tests promote`, `/tests discard`, collision handling, plan-mode rejection.
- `src/tools/bash.rs` tests: generated-test environment sanitization and command sandbox policy.

## Practical E2E Results

| Scenario | Model | Result | Iter | Duration | Key Observation |
| --- | --- | --- | --- | --- | --- |
| P0-445-01 Python bug fix without `ANVIL.md` | `qwen3.6:27b-coding-nvfp4` | MIXED | 5/50 | 22s | `calculator.py` fixed; model-independent `python -m pytest` passed; final `AutoTestRunner` failed with `python3 -m pytest` missing pytest |
| P0-445-02 Python bug fix with `ANVIL.md` | `qwen3.6:27b-coding-nvfp4` | MIXED | 5/50 | 19s | LLM used `python -m pytest -q` from `ANVIL.md` and passed; final `AutoTestRunner` still re-ran `python3 -m pytest` and failed |
| P0-445-03 Python no-test edit | `qwen3.6:27b-coding-nvfp4` | FAIL | 4/50 | 7s | Read happened, but no Edit; `AnvilScore` recorded `implementation_files_changed=0` and `user_visible_artifact=false` |
| P0-445-04 Python bug fix with `ANVIL.md` | `qwen3.5:122b` | FAIL | 5/50 | 180s | First action ran tests, then Focused Edit Recovery still failed to produce Edit; code remained broken |

## Comparison To Issue 444

Improvements:

- Failures are now measurable through `agent.anvil_score.computed`.
- `implementation_files_changed`, `tests_passed`, and `user_visible_artifact` give useful signals for ranking failed turns.
- Temporary test and Tester Skill safety boundaries are substantially covered by integration tests.
- qwen3.6 still fixes simple existing-code Python bugs reliably when it reaches Edit.

Regressions or unchanged gaps:

- The Issue 444 Python false-negative remains: runtime final verification prefers `python3 -m pytest` even when `python -m pytest` is the working interpreter.
- `ANVIL.md` command guidance affects the LLM's Bash action, but not the final success verifier.
- `changed_files` telemetry still includes `.anvil-state`, `.pytest_cache`, and `__pycache__`, inflating edited-file counts.
- qwen3.5 remains fragile: it can run a verifier before editing and then fail to emit a compact Edit even under focused recovery.
- Tester Skill live invocation was not demonstrated in the practical no-test case because the main implementation did not complete.

## Quality Assessment

Epic B is a strong observability and test-harness foundation, but not yet a high-quality end-user convergence improvement. It makes failures clearer and safer, but the agent still stops in cases where a human would view the task as incomplete.

Current quality level:

- Static implementation quality: `High`
- Runtime observability: `Improved`
- Verification correctness: `Mixed`
- qwen3.6 practical coding quality: `Medium`
- qwen3.5 practical coding quality: `Low`
- Safety posture for temporary tests: `Good`

## Recommended Next Fixes

1. Make AutoTestRunner choose the same interpreter/command that succeeded in-turn, especially `ANVIL.md` preferred commands and `python -m pytest`.
2. Filter `.anvil-state`, `.pytest_cache`, `__pycache__`, and other verifier artifacts out of edited-file telemetry and no-progress gates.
3. Treat "tests executed before implementation" as setup/no-progress, not task progress.
4. For qwen3.5 focused edit recovery, provide a full exact anchor including the line to edit, not only the function signature.
5. Add one production E2E where a no-test repo completes an implementation and then Tester Skill records a temporary smoke test.
6. Add a final-verifier precedence rule: project instructions and explicit successful Bash verification should outrank generic framework detection when safe.
