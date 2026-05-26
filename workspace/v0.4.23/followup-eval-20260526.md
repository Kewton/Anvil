# Follow-up Evaluation - 2026-05-26

## Harness Correction

The first follow-up smoke run was invalid because Anvil was launched from the
Anvil repository root while per-case directories were only passed as shell
variables. FastAPI logs showed the agent reading the Anvil repository README,
so those results were excluded.

The corrected run launched `target/release/anvil` with each case directory as
the process cwd and placed logs outside the evaluated workspace. This avoids
both wrong-root evaluation and `run.log` contamination.

## Clean Smoke Before Additional Fixes

| Mode | Case | Terminal | Notes |
|---|---|---|---|
| no-PAM | FastAPI CRUD | `repair_exhausted` | Controlled safe stop: repeated invalid repair proposals. |
| no-PAM | Rust word counter | `done` | Generated implementation, tests, README, and passed `cargo test`. |
| no-PAM | Python CSV | timeout | Controller repair pass stalled around iter 20; no wall-clock budget at repair-pass level. |
| no-PAM | Node ToDo CLI | `missing_repo_edits` | Request was misclassified as docs; only setup/package work happened. |
| no-PAM | docs-only README | `tool_call_format_error` | Docs mode conflicted with SetupBootstrap Bash-only policy. |
| PAM | FastAPI CRUD | `done` | PAM helped this case converge after repair. |
| PAM | Rust word counter | `missing_repo_edits` | Model wrote nested `pam-2-rust_word_counter/src/main.rs`; test role never completed. |
| PAM | Python CSV | timeout | Same repair-pass stall pattern as no-PAM. |
| PAM | Node ToDo CLI | `missing_repo_edits` | Same package-only artifact completion failure as no-PAM. |
| PAM | docs-only README | `tool_call_format_error` | Same SetupBootstrap/docs conflict as no-PAM. |

## Bugs Found And Fixed In This Slice

1. Work-mode classification treated any README mention as primary docs work.
   Code tasks that also requested README/tests, such as Node CLI + README,
   were routed as `Docs`.
2. `SetupBootstrap` could own a docs-only empty workspace because words such as
   install/test appeared inside requested README content. That gave the model a
   Bash-only policy when the correct next action was `Write README.md`.
3. Task contract treated `テスト方法` as a required test artifact. Docs-only
   README tasks could pass by creating placeholder tests, which is overreach.
4. Repair-job safe stops after rejected patches could still surface as
   `verifier_failed`, hiding the fact that the controller reached a controlled
   safe-stop terminal.

Implemented fixes:

- Secondary docs references are now weakened whenever a primary code task is
  present, not only for Python/UI tasks.
- Docs/answer-only work modes suppress `SetupBootstrap`; artifact recovery or
  the task contract must select the documentation target instead.
- Test artifact detection no longer treats bare Japanese `テスト` or English
  `test` as sufficient. It requires explicit test-artifact phrasing such as
  test code, tests, add/write/implement test, `テストコード`, `テストを追加`, or
  `テストを実装`.
- Added `repair_safe_stop` as a distinct terminal label for controlled repair
  safe stops that are not budget exhaustion.

## Post-fix Spot Checks

| Case | Result | Notes |
|---|---|---|
| docs-only README | `done` | Only `README.md` was created; no placeholder test was generated. |
| Node ToDo CLI | progressed to verifier repair | No longer stopped after `package.json`; generated implementation, tests, and README. The observed run then stopped on a repair safe-stop path. The current code maps this class to `repair_safe_stop` instead of raw `verifier_failed`. |

## Remaining Structural Gaps

- Controller repair pass needed its own wall-clock budget. The actor-level
  iteration budget did not help when one repair-pass LLM call stalled.
  This slice adds a repair-pass wall-clock cap and classifies provider
  timeouts as `provider_timeout` instead of conflating them with malformed
  patches.
- Repair convergence is still weak for non-trivial Python/Node failures.
  The controller now reaches safer terminals, but patch quality and target
  switching still need improvement.
- PAM is advisory, not reliably beneficial. It improved one FastAPI case, but
  worsened one Rust case by nudging a nested path shape.
- Full 20/20 + 20/20 evaluation should remain blocked until the small smoke
  gate no longer shows uncontrolled timeouts or artifact-contract overreach.

## Post Repair-Pass Budget Generic Smoke

Run root:
`/private/tmp/anvil-v0423-postfix-generic`

Command shape:
`target/release/anvil -m qwen3.6:27b-coding-nvfp4 --sidecar-model qwen3.5:9b -y --fresh-session --oneshot --no-footer --deterministic-fallback full --max-iterations 50`

| Mode | Case | Terminal | Duration | Notes |
|---|---|---:|---:|---|
| no-PAM | FastAPI CRUD | `done` | 384s | Completed, but required many repair cycles. Repair state returned through re-diagnostic instead of falling into generic recovery. |
| no-PAM | Python CSV CLI | `repair_safe_stop` | 123s | Controlled stop after repair plan `role_mismatch`; no uncontrolled timeout. |
| no-PAM | Rust word-count CLI | `missing_repo_edits` | 68s | `cargo init` created setup files, but artifact completion still stopped before a requirement-specific implementation edit. |
| no-PAM | Node JSON formatter CLI | `repair_exhausted` | 269s | Generated implementation/tests/docs, then exhausted repair budget. Target display exposed workspace-relative path normalization weakness. |
| no-PAM | docs-only README | `done` | 50s | Correctly created only `README.md`; no test-artifact overreach. |

Interpretation:

- The specific uncontrolled repair-pass timeout seen in the earlier Python CSV
  smoke is mitigated.
- Completion remains too slow and too brittle outside simple docs work.
- Remaining generic gaps are now clearer: artifact completion after bootstrap
  tools, repair target/path normalization, and repair patch quality/convergence.

## Expanded Generic Smoke: Additional 10 No-PAM Cases

Run root:
`/private/tmp/anvil-v0423-generic-expanded`

| Case | Terminal | Duration | Notes |
|---|---:|---:|---|
| Python TOML config CLI | `repair_exhausted` | 222s | Thin artifacts; repeated rejected patches. |
| Rust slug library | `done` | 23s | False positive: no Rust implementation or Cargo project, only README and Python dummy test. |
| Node CSV-to-JSON CLI | `repair_exhausted` | 150s | Full artifacts generated; repair did not converge. |
| Python file-renamer CLI | `repair_exhausted` | 355s | Final verifier still had extension/hidden-file failures. |
| Rust JSONL counter CLI | `repair_exhausted` | 239s | Rust project generated; repeated rejected patches. |
| FastAPI notes API | `done` | 147s | Valid completion after repair. |
| Python Markdown lint CLI | `repair_exhausted` | 416s | Near miss: 13/14 tests passed. |
| Node ToDo JSON CLI | `repair_exhausted` | 283s | Generated artifacts; patch rejection loop ended safely. |
| Rust JSON config merge CLI | `done` | 147s | Valid completion with cargo test. |
| Docs-only SRE runbook | `missing_repo_edits` | 190s | No edit; docs artifact retry budget exhausted. |

Quality-adjusted aggregate:

- Valid done: 2/10
- False-positive done: 1/10
- Controlled repair exhaustion / safe stop: 6/10
- Pre-edit artifact completion failure: 1/10
- Uncontrolled timeout: 0/10

What this adds to the diagnosis:

1. The repair-pass budget is doing its job: no uncontrolled repair-provider
   hang appeared in the expanded set.
2. The main blocker is now correctness, not only control flow. A request for a
   Rust library can still be "verified" by an unrelated Python test artifact.
3. Repair quality is the dominant convergence problem: several tasks reached
   verifier repair with reasonable files, then exhausted after repeated invalid
   or ineffective patch proposals.
4. Documentation-only work is not fully stable: the initial target is correct
   in some runs but can still exhaust without a write.
