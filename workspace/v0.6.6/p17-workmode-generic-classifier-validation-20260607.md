# P17 WorkMode generic classifier validation

## Objective

WorkMode を汎用用途の弱い補助信号へ寄せる。

P16 の実LLM trace で、`STATE_CONTROL_PACKET.required_artifacts` の `required` 内にある `ui` 断片が first-pass classifier の `typescript-ui` 候補を立てていた。second-pass LLM が補正したため実害はなかったが、WorkMode が tool policy や fallback に影響する以上、制御パケット由来のノイズを mode signal として扱うのは危険。

## Changes

- `src/modes/plan_act.rs`
  - `ui` / `ux` の短い ASCII mode token を substring ではなく token boundary で検出するようにした。
  - `required`, `build`, `music` など通常単語内の断片では UI 候補を立てない。
  - `UI`, `ui-guidelines.md` など明示 token は引き続き検出する。
  - `GenericCode` の互換ラベルは維持しつつ、intent / reason を `generic-edit` / generic artifact wording に変更した。
- `src/agent/loop_run/work_mode_confirm.rs`
  - confirm prompt の persona を `local coding agent` から `local-first agent` に変更した。
  - `generic-code` を互換ラベルとして残し、説明を `generic file-edit or artifact creation work` に変更した。

## Unit Validation

- `cargo test --offline --lib mode_classifier_`
  - 5 passed.
- `cargo test --offline --lib build_prompt_uses_generic_agent_wording_for_compatibility_modes`
  - 1 passed.
- `cargo test --offline --test work_mode_confirm_smoke`
  - 14 passed.
- `cargo test --offline --lib`
  - 3791 passed.

## Actual LLM Validation

Model:

- `qwen3.6:27b-coding-mxfp8`

Command shape:

```text
cargo run --offline -- --oneshot --fresh-session --no-footer --trace -y --max-iterations 5 --chat-timeout-secs 300 --state-dir /private/tmp/anvil-p17-workmode-generic-state-json1 -m qwen3.6:27b-coding-mxfp8 -p '... Create summary.json only ... required_artifacts ... role=data ...'
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

- `agent.work_mode.classified`
  - `work_mode`: `generic-code`
  - `intent`: `generic-edit`
  - alternatives: only `generic-code=0.70`
  - no `typescript-ui` candidate from `required_artifacts`
- confirm prompt
  - contained `WorkMode classifier for a local-first agent`
  - contained `compatibility labels`
  - contained `generic file-edit or artifact creation work`
  - did not contain `local coding agent`
- raw LLM tool call used `path:"summary.json"` and wrote valid JSON.

Created file:

```json
{
  "topic": "P17 work mode classifier validation",
  "status": "completed"
}
```

## Remaining Risk

This does not make WorkMode the source of truth. It only reduces noisy first-pass mode signals. The stronger architecture remains: ObjectiveContract / required artifacts / evidence obligations must continue to outrank WorkMode when selecting tools and deciding completion.
