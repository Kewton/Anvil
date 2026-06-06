# P16 TaskProfile-aware system prompt validation

## Objective

非 coding / 汎用タスクで、system prompt と runtime context に coding 専用の前提が混ざらないようにする。

特に data/docs/research/shell などの成果物タスクで `local-first coding agent`、Next.js、`app/page.tsx`、`scaffold -> install -> implement -> verify` などの例が入り、モデルが不要な scaffold や coding workflow に寄る問題を抑える。

## Changes

- `src/system_prompt.rs`
  - `TaskProfile` ごとに persona / script rule / visual rule / multistep rule / path rule を切り替えるようにした。
  - `Generic` / `Content` / `Research` は `local-first agent` と汎用成果物例に寄せる。
  - `Coding` / `Ui` は従来の coding / UI / Next.js guidance を維持する。
- `src/agent/prompting.rs`
  - runtime context の `Current project root...` reminder にも `TaskProfile` を渡す。
  - `Generic` / `Content` / `Research` では `summary.json`, `output.csv`, `docs/runbook.md` を例示する。
  - `Coding` / `Ui` では `app/page.tsx`, `src/app/page.tsx` を維持する。
- `src/agent/loop_run/build_request_messages.rs`
  - `agent.session.mode_state.task_profile` を runtime context に渡す。

## Unit Validation

- `cargo test --offline --lib system_prompt::tests::`
  - 5 passed.
- `cargo test --offline --lib runtime_context_`
  - 3 passed.
- `cargo test --offline --lib`
  - 3788 passed.

## Actual LLM Validation

Model:

- `qwen3.6:27b-coding-mxfp8`

Command shape:

```text
cargo run --offline -- --oneshot --fresh-session --no-footer --trace -y --max-iterations 5 --chat-timeout-secs 300 --state-dir /private/tmp/anvil-p16-profile-prompt-state-json2 -m qwen3.6:27b-coding-mxfp8 -p '... Create summary.json only ... required_artifacts ... role=data ...'
```

Result:

- `✔ done iter 1/5`
- Edited file: `summary.json`
- `eval.jsonl`
  - `classified_task_kind`: `data`
  - `completion_reason`: `artifact_obligations_satisfied`
  - `final_outcome`: `done`
  - `generic_outcome`: `completed`

Trace checks:

- system prompt contained `You are Anvil, a local-first agent.`
- system prompt did not contain `local-first coding agent`.
- generic path examples were `summary.json`, `output.csv`, `docs/runbook.md`.
- runtime root reminder also used `summary.json`, `output.csv`, `docs/runbook.md`.
- runtime root reminder no longer used `app/page.tsx` / `src/app/page.tsx` for the generic run.
- raw LLM tool call used `path:"summary.json"` and wrote valid JSON.

Created file:

```json
{
  "topic": "P16 generic prompt validation rerun",
  "status": "completed"
}
```

## Observed Remaining Risk

The first-pass WorkMode classifier still briefly misclassified the JSON task as `typescript-ui`, then the LLM confirmation corrected it to `generic-code`.

This did not break the run because the objective contract / required artifact policy had higher authority than WorkMode, but it confirms the broader architecture point: WorkMode should remain a weak hint, not the source of tool policy or task semantics.
