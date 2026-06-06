# P11 Evidence-Failed Artifact Budget Validation

Date: 2026-06-06

## Hypothesis

P10 で `STATE_CONTROL_PACKET.required_artifacts` の schema を obligation へ流せるようになったが、schema mismatch のような「成果物は存在するが品質証跡が失敗した」状態は、まだ artifact completion budget に入っていなかった。

そのため、不正 CSV を繰り返し書くケースでは `AwaitingEdit` のまま `max_iterations` へ落ちる可能性があった。これは coding 以外へ広げる際に問題になる。docs の required section 欠落、data schema mismatch、research source coverage 不足などはすべて「deliverable exists, evidence failed」であり、task-kind 別の retry counter を増やすべきではない。

今回の仮説は、obligation diagnostic を generic `EvidenceFailed` attempt として `ArtifactCompletionJob` に記録すれば、DataOutput / docs / future research で同じ bounded lifecycle を共有できる、というもの。

## Implementation Slice

最小変更として、artifact completion attempt outcome に `EvidenceFailed` を追加した。

- `ArtifactAttemptOutcomeKind::EvidenceFailed`
  - wire label: `evidence_failed`
  - job report schema versionは維持。`attempt_outcomes[].kind` の additive label として扱う。
- `artifact_completion_record::record_artifact_completion_evidence_failure(...)`
  - obligation diagnostic reason から deterministic `FailureClusterKey` を生成。
  - raw reason は既存 sanitize/cap/hash projection を通る。
  - domain-specific 判定は verifier / obligation 側に閉じる。
- `task_contract::blocking_obligation_diagnostic_for_role(...)`
  - 既存の `recovery_target_hint_for_blocking_obligation_diagnostic(...)` を structured code 付き helper に分離。
  - `MissingFile` と semantic evidence failure を文字列 pattern ではなく `VerifierDiagnosticCode` で区別する。
- `actor_loop_flow::sync_post_tool_contract_recovery_target(...)`
  - post-tool cleanup のみで evidence failure attempt を記録。
  - pre-reply / reply 後の再評価では記録しないため、同じ diagnostic の重複 budget 消費を避ける。

設計上のポイント:

- DataOutput 専用 branch ではない。
- docs required section failure も同じ path を通る。
- provider abstraction は増やしていない。
- WorkMode ではなく、ObjectiveContract/obligation diagnostic の結果が lifecycle を動かす。

## Deterministic Tests

Commands:

```bash
cargo test --offline --lib satisfied_data_artifact_job_still_blocks_on_schema_diagnostic
cargo test --offline --lib docs_required_section_failure_records_generic_evidence_failed_attempt
cargo test --offline --lib test_artifact_attempt_outcome_kind_enum_is_5_variants_closed
cargo test --offline --lib attempt_outcome_category_label_fixed_enum
cargo test --offline --lib
```

Results:

- `satisfied_data_artifact_job_still_blocks_on_schema_diagnostic`: pass
- `docs_required_section_failure_records_generic_evidence_failed_attempt`: pass
- `test_artifact_attempt_outcome_kind_enum_is_5_variants_closed`: pass
- `attempt_outcome_category_label_fixed_enum`: pass
- full lib suite: `3773 passed; 0 failed`

Pinned behavior:

1. DataOutput schema mismatch does not become `Satisfied`.
2. After a repo edit, the schema mismatch records `EvidenceFailed`.
3. The attempt carries a `FailureClusterKey`.
4. Remaining budget decreases from 4 to 3 after the first evidence failure.
5. With `repo_edit_calls_made_this_turn = 0`, no semantic attempt is recorded.
6. Docs required section failure records the same `EvidenceFailed` outcome.

## Actual Local LLM Validation

Model: `qwen3.6:27b-coding-mxfp8`

All runs used current source via `cargo run --manifest-path ... --offline -- ...`; localhost Ollama required sandbox escalation.

### Negative CSV: Repeated Invalid Data Artifact

State: `/private/tmp/anvil-p11-evidence-failed-state-csv2`

Work root: `/private/tmp/anvil-p11-evidence-failed-work-csv2`

Prompt:

```text
This is a controller negative validation. Create output.csv. The required schema is columns Category and Total, but intentionally DO NOT satisfy it. Use the Write tool and write exactly this invalid CSV content, preserving the newline: x,y
1. Do not write Category. Do not write Total. Do not repair it.
```

Observed:

- Iteration 1: Write `output.csv` as `x,y\n`
- Iteration 2: Read `output.csv`
- Iteration 3: Write `x,y\n`
- Iteration 4: Read `output.csv`
- Iteration 5: Write `x,y\n`
- Final outcome: `missing_repo_edits`
- User-facing terminal text included `artifact completion role-specific retry budget exhausted`

Job report:

```json
{
  "attempt_outcomes": [
    {"kind": "evidence_failed"},
    {"kind": "evidence_failed"},
    {"kind": "evidence_failed"},
    {"kind": "evidence_failed"}
  ],
  "budget_state": {
    "state": "Exhausted",
    "role": "data_output",
    "attempts_used": 4,
    "attempts_limit": 4,
    "remaining_budget": 0
  },
  "task_kind": "data"
}
```

This is the main P11 success condition: repeated invalid data artifact writes now converge through a bounded evidence-failed artifact lifecycle instead of drifting as `AwaitingEdit` until `max_iterations`.

### Positive JSON Regression

State: `/private/tmp/anvil-p11-evidence-failed-state-json1`

Work root: `/private/tmp/anvil-p11-evidence-failed-work-json1`

Prompt:

```text
STATE_CONTROL_PACKET
{"objective":"Create summary.json as a data output. Fill sensible values for every required field.","next_required_action":"artifact","required_artifacts":[{"path":"summary.json","role":"data","schema":{"json_fields":["status","duration_seconds","warnings"]}}]}
```

Observed:

- Iteration 1 wrote a valid JSON object:
  - `status`
  - `duration_seconds`
  - `warnings`
- Final outcome: `done`
- `completion_reason`: `artifact_obligations_satisfied`
- Job report:
  - `budget_state.state = Satisfied`
  - `attempts_used = 0`
  - no `evidence_failed` attempt outcomes

The evidence-failed path does not penalize valid artifacts.

### Positive CSV Note

An initial negative attempt using a schema packet asked the model to write invalid CSV, but the model chose to satisfy the schema and wrote `Category,Total\n`. That run completed successfully. This is useful as a behavioral note: when schema is explicit in `STATE_CONTROL_PACKET`, the local LLM may prioritize the contract over contradictory natural-language text.

## Interpretation

P11 moves the architecture closer to the intended general-purpose controller:

- `MissingDeliverable` and `EvidenceFailed` are now distinguishable in the artifact lifecycle.
- The distinction is based on structured obligation diagnostics, not string rules.
- The retry budget is attached to the deliverable/evidence lifecycle, not coding-only verifier repair.
- Data and docs already share the same route.
- Future research/source coverage and shell observation failures can reuse the same outcome kind if their obligation diagnostic reports a non-`MissingFile` failure.

## Remaining Gap

Terminal projection still reports the legacy `missing_repo_edits` class for the exhausted negative CSV run, even though the structured job report now shows the more accurate `EvidenceFailed` budget exhaustion.

The next slice should align terminal diagnostics and active job taxonomy:

- project `ArtifactCompletionStatus::Exhausted` with last outcome `EvidenceFailed` to a generic terminal such as `evidence_repair_exhausted`;
- keep legacy labels only as evaluation-compatible projection;
- avoid introducing data/docs-specific terminal states.
