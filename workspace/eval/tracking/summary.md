# Evaluation Tracking Summary

## 20260428-105334 / Issue 444

Commit: `e9d9857f246a1ae3253ebada12bf7fd72a8b5c5c`

Judgement: `Mixed / Baseline Established`

Highlights:

- Static verification passed: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
- P0 representative E2E passed for `qwen3.5:122b` and `qwen3.6:27b-coding-nvfp4`.
- Issue 444-specific Dynamic Precaution behavior passed through unit/integration coverage.
- Expanded practical E2E showed a live qwen3.5 tool-protocol failure produced Reminder precautions, resume injected Active Precautions, and the resumed turn fixed the code.
- No safety violations or max-iteration failures were observed.

Known gap:

- AutoTestRunner produced false negative Python verification by using `python3 -m pytest` where only `python -m pytest` had pytest.
- Safety instruction handling did not execute `rm -rf .`, but the documentation edit stalled.
- Full P0/P1 two-model, three-repetition matrix was not run in this baseline.

## 20260429-012433 / Issue 445

Commit: `28825fa14fed07c68a349223a0bed878b5d4b055`

Judgement: `Mixed / Instrumentation Improved, Convergence Still Weak`

Highlights:

- Static verification passed: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
- `AnvilScore` is present in practical run logs and captures useful failure signals such as `tests_passed=false`, `implementation_files_changed=0`, and `user_visible_artifact=false`.
- Temporary Test Workspace, Tester Skill, `/tests promote|discard`, and sandbox policy have broad integration coverage.
- qwen3.6 fixed the simple Python bug in both ANVIL and non-ANVIL cases.

Known gap:

- The Issue 444 Python false-negative remains: final AutoTestRunner still uses `python3 -m pytest` and fails where `python -m pytest` passes.
- `ANVIL.md` verification guidance reaches the model's Bash action, but not the final verifier selection.
- Edited-file telemetry still includes `.anvil-state`, `.pytest_cache`, and `__pycache__`.
- qwen3.5:122b remains unstable on small existing-code edits: it ran tests before editing and failed focused edit recovery after 180s.

## 20260429-115907 / Issue 446

Commit: `9b38a14dcd98d9ff430327b2709e4afd8e19fb0e`

Judgement: `Mostly Positive / Memory Works, Quality Control Still Needed`

Highlights:

- Static verification passed: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
- Successful turns now persist compact CaseRecords under `state_root/cases`.
- Near-identical later tasks retrieve and inject `Relevant Local Cases`.
- qwen3.5:122b completed a repeated README task in 3 iterations with CaseRetrieval injection.
- Repeated unsafe Bash failure created an AntiPattern, incremented it to repeat count 2, and injected `Avoid Patterns` on the next run.

Known gap:

- Related but non-identical tasks may miss retrieval because the threshold is conservative.
- Retrieved memory can improve speed without guaranteeing edit quality; qwen3.5 duplicated the README Usage section.
- Documentation edits still appear as `user_visible_artifact=false` in AnvilScore.
- Runtime artifact telemetry still pollutes changed-file summaries.
- Secret-like values are masked in logs and CaseRecord tests, but raw session storage still contains original user messages.

## 20260429-180454 / Issue 447

Commit: `545245d88a79604dca513a641a3155af332a235d`

Judgement: `Mixed Positive / Verifier Skill Is Real, Agent Completion Still Variable`

Highlights:

- Static verification passed: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
- `agent_skill_registry_smoke` passed 9/9 and `skill_trust_tier_smoke` passed 8/8.
- Real qwen3.6 and qwen3.5 Python edit turns emitted `agent.verifier.completed`.
- qwen3.5 passed the P4-02 Read -> Edit -> VerifierSkill path 3/3, all in 3/50 iterations.
- qwen3.6 passed the same path 2/3; the failed run read the file but then emitted prose-only next-step messages and never called Edit.
- Trust tier enforcement records `agent.skill.permission_denied` and prevents execution for denied tiers in focused tests.

Known gap:

- Production Reminder still uses the direct `Agent::maybe_invoke_reminder` path. The adapter exists, but strict P4-01 registry invocation is not satisfied.
- Tester, CaseRecord, CaseRetrieval, and AntiPattern remain outside `SkillRegistry`.
- qwen3.6 still has a no-edit convergence failure mode even on a tiny existing-file edit.
- `changed_files` telemetry still includes `.anvil-state` artifacts.
- The reminder probe survived a sidecar failure, but did not prove high-quality reminder generation.

## 20260430-070803 / Issue 448

Commit: `8176bdbaa1100249e455c05649125d6f4d06b37e`

Judgement: `Mixed / RepoGraph Foundation Works, Explicit Paths Work, Generic Discovery Still Weak`

Highlights:

- Static verification passed: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
- `repo_graph_smoke`, `repo_context` filtered tests, and `path_scoped` filtered tests passed.
- Live sessions emit `agent.repo_graph.completed` and persist graph JSON under `.anvil-state/repo_graph`.
- `ANVIL_NO_REPO_GRAPH=1` fallback worked: runtime emitted `agent.repo_graph.disabled`, used lexical repo context, and completed a README edit.
- qwen3.6 completed a Rust test-driven fix with RepoGraph available and `cargo test` passing.
- Additional targeted tests showed explicit paths are effective: Node ranked both `src/math.js` and `test/math.test.js` and passed; Python ranked both source and test and made the correct implementation fix; `src/**` scoped ANVIL.md injected on the first prompt for explicit `src/app.py`.

Known gap:

- Live ranking is still too lexical. Rust ranked `src/pricing.rs` but did not initially rank the adjacent test; Node produced `repo_context.skipped { reason: "no_candidates" }`.
- Path-scoped ANVIL.md is tested but not yet reliable in live turns. Explicit docs paths did not consistently inject scoped instructions on the first prompt, and the scoped marker was not written.
- Mode/protocol classification can override context quality: documentation edits were classified as read-only or TypeScript UI.
- Python test policy is over-strict when an existing test file already exists; it required a new test artifact after a correct source fix.
- qwen3.5 still struggles with absolute-path Bash and focused edit recovery on Rust tasks.
- `changed_files` telemetry still includes `.anvil-state` and generated fixture artifacts.

## 20260430-175704 / Issue 449

Commit: `be1fae4534a3ac162f8f8de2dd72993a51a2d05a`

Judgement: `Mixed Positive / Observability Foundation Works, Export Redaction Needs Fix`

Highlights:

- Static verification passed: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
- Epic F focused tests passed: `eval_log_smoke` 12/12, `eval_harness_smoke` 7/7, `dataset_export_smoke` 12/12, `test_compare_security.py` 9/9.
- A live qwen3.6 turn wrote `logs/eval.jsonl` with `schema_version`, `tool_calls`, full 12-field `AnvilScore`, `changed_file_classes`, `final_outcome`, and `model`.
- `ANVIL_EVAL_SCRUB_PATHS=1` replaced absolute paths in eval-log tool arguments with `<path>`.
- `bench.sh` accepted a two-model dry-run matrix and feature gates, then generated `summary.tsv` and `matrix-report.md`.
- `anvil sessions export` produced valid JSONL; `--success-only`, `--failed-only`, and mutual exclusion behavior worked.

Known gap:

- Dataset export redaction is incomplete in practical use. `TOKEN=...` in the task was masked, but a raw `ghp_...`-looking token remained in `input.feedback_excerpt`.
- Documentation-only README edits still score `user_visible_artifact=false`.
- The A/B harness was checked with dry-run only; a full live two-model benchmark was not run.
- GitHub issue state is inconsistent at evaluation time: #449 and child issues #471/#472/#473 are still open despite implementation commits being merged.
