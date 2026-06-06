# P15 Non-Coding TaskKind Mode Policy Validation

Date: 2026-06-07

## Objective

Stop WorkMode from overriding an ObjectiveContract that requires a non-coding artifact.

The concrete problem came from actual LLM logs:

- A data artifact task could still receive coding-specific mode text such as `Work mode is Python`.
- Worse, a data artifact task could be second-pass classified as `answer-only`, which removed Write/Edit tools even though `STATE_CONTROL_PACKET.required_artifacts` required `summary.json`.

That violates the target architecture: WorkMode must not be the sole authority for tool policy when ObjectiveContract/TaskKind says a deliverable must be produced.

## Implementation

Changed files:

- `src/agent/loop_run/tool_prep.rs`
  - `mode_policy_message` now reads the TaskContract authority.
  - Non-coding TaskKind with an artifact requirement emits a generic non-coding policy:
    - follow objective contract
    - produce required artifacts/evidence
    - avoid code scaffolds unless explicitly requested
  - It no longer emits Python/TypeScript/GenericCode wording for non-coding tasks.
- `src/agent/loop_run/tool_policy_decisions.rs`
  - `answer_only_mode_active` now checks whether the TaskContract has required artifacts.
  - `WorkMode::AnswerOnly` remains read-only only when no artifact deliverable is required.

This keeps genuine read-only/answer-only requests protected while allowing docs/data/research/authoring artifact tasks to use write tools.

## Unit Validation

Commands:

```bash
cargo fmt
cargo test --offline --lib tool_prep::tests::
cargo test --offline --lib tool_policy_decisions::tests::
cargo test --offline --lib
```

Results:

- `tool_prep` tests: `5 passed`.
- `tool_policy_decisions` tests: `2 passed`.
- Full lib suite: `3784 passed; 0 failed`.

## Actual Local LLM Validation

Model:

- `qwen3.6:27b-coding-mxfp8`

### Pre-Fix Probe

Before the TaskContract-aware AnswerOnly gate was added, the same data artifact task failed:

```text
✘ tool_call_format_error  iter 5/5  duration 28s  edited 0 files
assistant kept calling tools outside the current tool policy
```

Relevant log facts:

- WorkMode second pass selected `answer-only`.
- Prompt said `TOOLS AVAILABLE NOW: No tools are available in this turn`.
- Mode policy said `Work mode is answer-only/read-only`.
- The model attempted `Write`, but the tool policy rejected it.
- Eval log ended with:

```json
{
  "final_outcome": "tool_call_format_error",
  "classified_task_kind": "data",
  "terminal_diagnostics": {
    "classification": "model_output_failure",
    "generic_outcome": "model_output_failure"
  }
}
```

This confirmed the failure mode was not just prompt wording; the tool policy itself was still WorkMode-owned.

### Fixed Probe

After the TaskContract-aware gate:

```bash
cargo run --offline -- --oneshot --fresh-session --no-footer --trace -y \
  --max-iterations 5 \
  --state-dir /private/tmp/anvil-p15-noncoding-mode-policy-state-json2 \
  -m qwen3.6:27b-coding-mxfp8 \
  -p 'P15 non-coding mode policy validation ... required_artifacts summary.json data schema json_fields topic,count'
```

Observed CLI summary:

```text
✔ done  iter 1/5  duration 7s  edited 1 files (summary.json)
```

Relevant request log:

```text
TOOLS AVAILABLE NOW:
- Read(path[, start_line, end_line]): read a file or list a directory
- Write(path, content): create or overwrite a file

[Mode Policy] Task kind is non-coding. Follow the objective contract and required artifacts/evidence. Do not create code scaffolds or coding-specific verification unless explicitly requested.
```

Confirmed absent from the fixed prompt:

- `Python-oriented`
- `Work mode is answer-only/read-only`

Eval log:

```json
{
  "classified_task_kind": "data",
  "completion_reason": "artifact_obligations_satisfied",
  "final_outcome": "done",
  "terminal_diagnostics": {
    "outcome": "done",
    "generic_outcome": "completed",
    "classification": "success"
  }
}
```

## Conclusion

P15 addresses a structural issue, not a one-off prompt tweak:

- WorkMode no longer unconditionally owns read/write tool policy.
- ObjectiveContract/TaskKind can keep artifact-producing non-coding tasks writable.
- LLM instructions are less contradictory: data artifact tasks no longer receive Python-specific or answer-only read-only mode text.

## Remaining Risk

The global system prompt still contains some coding-oriented defaults because Anvil is historically a coding agent. The targeted fix here only removes the most harmful WorkMode-derived contradiction. A future slice should reduce coding-specific global prompt rules when TaskProfile/TaskKind is non-coding, while preserving tool-call formatting and safety rules.
