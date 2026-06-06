# P10 Controller State Schema Packet Validation

Date: 2026-06-06

## Hypothesis

`STATE_CONTROL_PACKET.required_artifacts` が path / role だけを持つと、controller が DataOutput や docs の品質条件を `TaskContract` へ渡せない。

その結果、schema / required section の判定が user prompt の自然文推定に依存し、coding 以外の用途へ広げるほど task-kind 別の例外処理が増える。これは ObjectiveContract 化の目的である「成果物、証跡、修復 lifecycle の汎用化」と矛盾する。

今回の仮説は、controller packet に含めた軽量 schema を既存の `ArtifactObligation` / diagnostic 経路へ流せば、provider abstraction や task-specific gate を増やさずに docs/data の品質条件を扱える、というもの。

## Implementation Slice

最小変更として、`controller_artifact_obligation(...)` が optional schema labels を読むようにした。

- DataOutput:
  - `columns`
  - `json_fields`
  - `fields`
  - `schema_fields`
- UsageDocs:
  - `required_sections`
  - `sections`
  - `schema_sections`
- top-level と `schema.{key}` の両方を許可。
- label は trim、control-char neutralize、whitespace collapse、mask/cap、dedupe、最大32件に制限。
- DataOutput schema は `ArtifactObligation::structured_record(...)` に変換。
- docs schema は `ArtifactObligation::readme(...)` に変換。

また、P9 の `task_contract_recovery_action` だけでは不十分だった。turn end の `refresh_artifact_completion_satisfied(...)` が ledger projection だけで job report を `Satisfied` にしていたため、blocking obligation diagnostic がある場合は `Satisfied` へ更新しない guard を追加した。

この変更は DataOutput 専用の完了判定を作らず、既存の obligation diagnostic authority を job reporting 側にも反映する。

## Deterministic Tests

Commands:

```bash
cargo test --offline --lib controller_state_packet_data_schema_creates_structured_obligation
cargo test --offline --lib controller_state_packet_docs_schema_creates_required_sections_obligation
cargo test --offline --lib satisfied_data_artifact_job_still_blocks_on_schema_diagnostic
cargo test --offline --lib docs_artifact_satisfied_without_verification_returns_done
cargo test --offline --lib
```

Results:

- `controller_state_packet_data_schema_creates_structured_obligation`: pass
- `controller_state_packet_docs_schema_creates_required_sections_obligation`: pass
- `satisfied_data_artifact_job_still_blocks_on_schema_diagnostic`: pass
- `docs_artifact_satisfied_without_verification_returns_done`: pass
- full lib suite: `3772 passed; 0 failed`

Pinned behavior:

1. A controller packet can declare JSON fields for `summary.json`.
2. The resulting contract is classified as `data`.
3. The DataOutput obligation becomes `StructuredRecord` with JSON format and declared columns.
4. A malformed excerpt such as `{"x":1}` produces `schema_mismatch` and recovery `Continue`.
5. A controller packet can declare docs required sections.
6. The docs deliverable carries those required sections.
7. A schema-mismatched DataOutput job is not reported as `Satisfied`.

## Actual Local LLM Validation

Model: `qwen3.6:27b-coding-mxfp8`

All runs used current source via `cargo run --offline -- ...`; localhost Ollama required sandbox escalation.

### JSON Schema Packet Positive

State: `/private/tmp/anvil-p10-schema-packet-state-json1`

Work root: `/private/tmp/anvil-p10-schema-packet-work-json1`

Prompt:

```text
STATE_CONTROL_PACKET
{"objective":"Create summary.json as a data output. Fill sensible values for every required field.","next_required_action":"artifact","required_artifacts":[{"path":"summary.json","role":"data","schema":{"json_fields":["status","duration_seconds","warnings"]}}]}
```

Observed:

- `summary.json` was written in iteration 1.
- Logs included `schema_columns=status|duration_seconds|warnings`.
- `classified_task_kind`: `data`
- `completion_reason`: `artifact_obligations_satisfied`
- artifact completion report: `state=Satisfied`

### Docs Schema Packet Positive

State: `/private/tmp/anvil-p10-schema-packet-state-docs1`

Work root: `/private/tmp/anvil-p10-schema-packet-work-docs1`

Prompt:

```text
STATE_CONTROL_PACKET
{"objective":"Create README.md documentation.","next_required_action":"artifact","required_artifacts":[{"path":"README.md","role":"docs","schema":{"required_sections":["Setup","Usage"]}}]}
```

Observed:

- `README.md` was written in iteration 1.
- Logs included `schema_sections=Setup|Usage`.
- `classified_task_kind`: `docs`
- `completion_reason`: `artifact_obligations_satisfied`
- artifact completion report: `state=Satisfied`

### Malformed CSV Packet Negative

State: `/private/tmp/anvil-p10-schema-packet-state-csv-bad2`

Work root: `/private/tmp/anvil-p10-schema-packet-work-csv-bad2`

Prompt deliberately forced a bad artifact:

```text
STATE_CONTROL_PACKET
{"objective":"Create output.csv as a data output. It must have columns Category and Total. For the first attempt, deliberately write exactly this malformed content and nothing else: x,y newline 1. Do not include Category or Total in that first write.","next_required_action":"artifact","required_artifacts":[{"path":"output.csv","role":"data","schema":{"columns":["Category","Total"]}}]}
```

Observed after the `refresh_artifact_completion_satisfied(...)` guard:

- Final outcome: `max_iterations`
- `output.csv` remained malformed: `x,y\n1`
- Logs included `schema_mismatch`.
- `classified_task_kind`: `data`
- artifact completion report: `state=AwaitingEdit`, not `Satisfied`
- `remaining_budget=4`

This confirms the controller no longer reports schema-invalid data as a satisfied deliverable merely because an owned repo edit exists.

### JSON Schema Regression After Guard

State: `/private/tmp/anvil-p10-schema-packet-state-json2`

Work root: `/private/tmp/anvil-p10-schema-packet-work-json2`

Observed:

- Valid `summary.json` was written.
- Logs included `schema_columns=status|duration_seconds|warnings`.
- Final outcome: `done`
- artifact completion report: `state=Satisfied`

The guard did not break a valid non-coding DataOutput completion.

## Interpretation

This slice moves the architecture closer to ObjectiveContract:

- Controller state can now declare lightweight deliverable schema without relying on user prompt wording.
- Artifact existence remains ledger-owned lifecycle evidence.
- Artifact quality remains obligation-diagnostic evidence.
- Job reporting now respects the same diagnostic authority used for completion gating.
- The mechanism is reusable for docs and data now, and can be extended to research/source evidence or shell observation without adding provider abstraction.

The important maintainability point is that schema parsing is confined to controller packet normalization. The rest of the lifecycle continues to consume neutral `ArtifactObligation` and `DeliverableSchema` concepts.

## Remaining Gap

The malformed CSV run exposed a separate lifecycle gap:

`ArtifactCompletionJob` attempt budget currently tracks no-tool, wrong-target, and policy failures more strongly than semantic schema failures. A repeated bad write can therefore end in `max_iterations` while the job remains `AwaitingEdit` with budget left.

The next architectural slice should not add a CSV-specific retry counter. It should model semantic diagnostic retries as a generic `EvidenceFailedJob` or obligation-diagnostic attempt stream, so docs/data/research all share the same bounded repair lifecycle.
