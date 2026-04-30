# Issue 448 Evaluation Report

Run ID: `20260430-070803`
Evaluation commit: `8176bdbaa1100249e455c05649125d6f4d06b37e`

## Verdict

Judgement: `Mixed / RepoGraph Foundation Works, Explicit Paths Work, Generic Discovery Still Weak`

Issue 448 was worth doing as infrastructure. RepoGraph now builds for small Rust, Node, Python/documentation-like repositories, emits runtime events, persists graph JSON, and falls back safely when disabled. The static test coverage is strong.

The practical quality improvement is not yet stable. In live runs, RepoContext works when explicit paths are available, but it does not reliably infer candidates from generic project-level requests such as "make Node tests pass". Path-scoped ANVIL.md can work on explicit source paths, but documentation cases still became entangled with mode/protocol misclassification.

## Static Verification

| Check | Result | Notes |
| --- | --- | --- |
| `cargo fmt --check` | PASS | no formatting drift |
| `cargo clippy --all-targets -- -D warnings` | PASS | no warnings |
| `cargo test` | PASS | full suite passed |
| `cargo test --test repo_graph_smoke` | PASS | RepoGraph smoke passed |
| `cargo test repo_context` | PASS | graph-aware prompt tests passed |
| `cargo test path_scoped` | PASS | path-scoped ANVIL.md parser/filter tests passed |

The orchestration summary mentions `tests/repo_context_ranking_smoke.rs` and `tests/anvil_md_path_aware_smoke.rs`, but those are not standalone test targets in the current tree. The behavior is currently covered by in-module tests in `src/agent/prompting.rs`.

## Practical E2E Results

| Scenario | Model | Result | Iter | Duration | Key Observation |
| --- | --- | --- | ---: | ---: | --- |
| P5-01 Rust graph | `qwen3.5:122b` | FAIL | 8/50 | 82s | Graph built and ranked `src/pricing.rs`, but qwen3.5 ran absolute-path Cargo, hit workspace error, then failed focused edit recovery |
| P5-01 Rust graph R2 | `qwen3.6:27b-coding-nvfp4` | PASS/MIXED | 8/50 | 36s | Fixed `src/pricing.rs`; `cargo test` passed; adjacent test was found by `Glob`, not initial graph ranking |
| P5-02 Node graph | `qwen3.6:27b-coding-nvfp4` | FAIL | 4/50 | 9s | Graph built, but RepoContext returned `no_candidates`; model stopped after prose-only messages |
| P5-03 path-scoped ANVIL.md | `qwen3.6:27b-coding-nvfp4` | FAIL | 3/50 | 7s | Edit request was classified read-only; scoped docs rule was not injected in initial prompt |
| P5-03 path-scoped ANVIL.md R2 | `qwen3.6:27b-coding-nvfp4` | FAIL/MIXED | 3/50 | 7s | Scoped docs rule appeared after touched file, but marker was not written; TypeScript UI protocol caused final failure |
| P5-04 graph disabled fallback | `qwen3.6:27b-coding-nvfp4` | PASS | 3/50 | 7s | `agent.repo_graph.disabled`; lexical fallback selected README; edit completed |
| P5-02 Node explicit paths | `qwen3.6:27b-coding-nvfp4` | PASS | 6/50 | 17s | Explicit `src/math.js` and `test/math.test.js` were both ranked; model edited `src/math.js`; Node test passed |
| P5-Python explicit paths | `qwen3.6:27b-coding-nvfp4` | FAIL/MIXED | 5/50 | 14s | RepoContext ranked both source and test; source fix was correct, but Python test policy required a new test artifact and failed |
| P5-03 src-scoped ANVIL.md | `qwen3.6:27b-coding-nvfp4` | PASS/MIXED | 5/50 | 20s | Initial prompt included the `src/**` scoped rule; model wrote `SRC-SCOPE-PASS`; verification used absolute `cd` and generated `__pycache__` |

## Additional Targeted Tests

The extra tests sharpen the verdict rather than overturning it.

- Explicit path mention makes a major difference. In the Node rerun, RepoContext ranked both the implementation and test file, and qwen3.6 completed the fix with a passing Node test. This means the graph/context plumbing works when candidate seeding has concrete paths.
- Python graph ranking also worked: both `tests/test_calc.py` and `src/calc.py` were selected, including a `graph_neighbor` reason for the test file. The failure was downstream quality policy, not target discovery. The agent corrected `return a + b` to `return a * b`, but then failed because the Python test policy demanded a new test artifact even though an existing test was already read.
- Path-scoped ANVIL.md can work on the first prompt when the user explicitly names a matching source file. The `src/**` rule was injected without waiting for a touched file, and the marker appeared in `src/app.py`.

## What Improved

- RepoGraph builds and persists successfully in real sessions.
- Runtime has visible graph events: `agent.repo_graph.completed`, `agent.repo_graph.disabled`, `agent.repo_context.completed`, and `agent.repo_context.skipped`.
- Graph disabled fallback is safe and usable.
- Explicit path seeding is effective: Node and Python follow-up tests ranked the named source/test files and avoided broad search.
- Rust implementation targeting improved enough for qwen3.6 to complete a test-driven fix.
- Path-scoped ANVIL.md has a tested parser and filter, and the scoped block can appear on the first prompt when the user mentions a matching path.

## Remaining Gaps

- Target-file ranking is too lexical. In Rust, initial context ranked `src/pricing.rs` but not `tests/pricing_test.rs`, even though the task was "make pricing tests pass".
- Generic project requests such as "Node tests pass" can produce `repo_context.skipped { reason: "no_candidates" }`.
- Path-scoped ANVIL.md is still sensitive to mode/protocol classification. It worked for explicit `src/app.py`, but not for the earlier docs task.
- Mode classification can overpower repo-context improvements: one docs edit became answer-only/read-only; another became TypeScript UI and failed a UI protocol despite editing a Markdown file.
- Python test policy is too blunt. A task with an existing test file should accept reading and running that test; it should not always require a newly added test artifact.
- qwen3.5 still struggles with focused edit recovery and absolute-path Bash behavior on Rust tasks.
- `changed_files` still includes `.anvil-state` and generated artifacts such as `Cargo.lock` for tiny fixture runs.

## Recommendation

Treat Issue 448 as a good repo-context foundation, not as a completed quality improvement.

1. Make RepoContext ranking use graph relationships more aggressively for test-to-implementation tasks.
2. Seed candidate discovery from project structure and package scripts, not only lexical task terms.
3. Preserve the explicit-path behavior observed in the follow-up tests, and extend it to less direct wording such as "Node tests" or "pricing tests".
4. Fix mode classification so documentation edits do not become read-only or TypeScript UI tasks.
5. Refine Python quality gating to distinguish "existing test read and verified" from "no test coverage exists".
6. Keep `ANVIL_NO_REPO_GRAPH` fallback behavior as-is; it worked cleanly.
