# P5 Event Summary

## Static / Unit Coverage

- `cargo test` passed.
- `repo_graph_smoke` passed.
- `repo_context` filtered tests passed, including graph-aware ranking fallback behavior.
- `path_scoped` filtered tests passed, including scoped match, no-match, global-only, env gate, and invalid pattern handling.

## Live E2E

| Scenario | Result | Observation |
| --- | --- | --- |
| P5-01 Rust / qwen3.5 | FAIL | RepoGraph built and `src/pricing.rs` was selected, but the model ran Cargo with an absolute `cd`, hit a workspace-membership error, then failed focused edit recovery. No repo edit remained. |
| P5-01 Rust / qwen3.6 R2 | PASS/MIXED | The model fixed `src/pricing.rs` and `cargo test` passed. However, RepoContext initially selected only `src/pricing.rs`; the adjacent test was found later by `Glob`, not by graph ranking. |
| P5-02 Node / qwen3.6 | FAIL | RepoGraph built, but RepoContext emitted `no_candidates`; the model ran `ls` then produced prose-only "I need to read files" responses and stopped without edits. |
| P5-03 path-scoped ANVIL.md / qwen3.6 | FAIL | Initial prompt was classified as read-only, so Edit was blocked. Project instructions contained only the global ANVIL.md rule. |
| P5-03 path-scoped ANVIL.md R2 / qwen3.6 | FAIL/MIXED | After the first edit, path-scoped docs instruction appeared in the prompt. The file was edited, but the required `DOC-SCOPE-PASS` marker was not written, and the turn failed the TypeScript UI protocol. |
| P5-04 graph disabled fallback / qwen3.6 | PASS | `ANVIL_NO_REPO_GRAPH=1` emitted `agent.repo_graph.disabled`; RepoContext fell back to lexical ranking and README edit completed. |
| P5-02 Node explicit paths / qwen3.6 | PASS | With `src/math.js` and `test/math.test.js` named explicitly, RepoContext selected both files, the model read both, edited only `src/math.js`, and the Node test passed. |
| P5-Python explicit paths / qwen3.6 | FAIL/MIXED | RepoGraph selected both `tests/test_calc.py` and `src/calc.py`; the implementation fix was correct, but the Python test policy demanded a new test artifact despite an existing test file, so the turn failed as `missing_repo_edits`. Local `python3 -m pytest` also failed because pytest is not installed in this interpreter. |
| P5-03 src-scoped ANVIL.md / qwen3.6 | PASS/MIXED | Explicit `src/app.py` produced RepoContext hit and the initial prompt injected the `src/**` scoped ANVIL.md rule. The model wrote `SRC-SCOPE-PASS` and verified behavior, but its verification command used an absolute `cd` and generated `src/__pycache__`. |

## Key Signals

- RepoGraph build events are present and stable: `agent.repo_graph.completed` with node/edge counts.
- Disabled fallback is safe: `agent.repo_graph.disabled` followed by `agent.repo_context.completed { repo_graph_state: "absent" }`.
- Graph-aware ranking is observable, but live candidate quality is uneven:
  - Rust: implementation file ranked, adjacent test not ranked.
  - Node: no candidates for a generic "Node tests" task.
- Path-scoped ANVIL.md works in unit tests and can appear after `touched_files` includes the matching path, but it did not reliably influence the first live prompt or final edit content.
- When the user explicitly names the target source path, path-scoped ANVIL.md can influence the first live prompt and final edit content.
- Existing mode/protocol issues still dominate some tasks: answer-only/read-only misclassification and TypeScript UI protocol false failure on documentation edits.
- Python quality gating is currently over-strict for tasks with an existing test file: it can demand a new test artifact after a correct implementation edit instead of accepting read-and-run of the existing test.
