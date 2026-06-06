# P12 Evidence Repair Exhausted Terminal Projection Validation

Date: 2026-06-06

## Hypothesis

P11 で invalid artifact writes は `ArtifactCompletionJob` の `EvidenceFailed` budget に入るようになった。しかし eval log の `terminal_diagnostics` は `final_outcome = missing_repo_edits` だけを見て旧来の `model_output_failure` に分類していた。

これは根本原因の見え方を歪める。実際には repo edit は存在し、成果物も存在する。失敗しているのは「成果物 evidence が obligation を満たさず、修復 budget が尽きた」こと。

今回の仮説は、legacy `final_outcome` は互換用に残しつつ、structured terminal diagnostics だけを `evidence_repair_exhausted` に projection すれば、coding 以外の failure authority を正しく観測できる、というもの。

## Implementation Slice

最小変更:

- `EvalRecord::mark_artifact_evidence_repair_exhausted(...)`
  - `terminal_diagnostics.classification = evidence_repair_exhausted`
  - non-coding artifact write があれば `repo_edit = satisfied`
  - `artifact_evidence = unsatisfied` obligation を追加
  - `repair_convergence = unsatisfied` を `evidence_repair_exhausted` domain に更新
  - verifier environment/evidence は not applicable にする
- `failure_authority_for_eval(...)`
  - `evidence_repair_exhausted -> artifact_evidence`
- agent boundary:
  - `ArtifactCompletionStatus::Exhausted` かつ最後の attempt が `EvidenceFailed` のときだけ、この projection を適用。

設計判断:

- `final_outcome` は `missing_repo_edits` のまま残す。互換評価や既存 exit reason を壊さない。
- terminal diagnostics / evaluation taxonomy を generic evidence authority に寄せる。
- DataOutput / docs などの task-specific terminal label は増やさない。

## Deterministic Tests

Commands:

```bash
cargo test --offline --lib terminal_diagnostics_project_artifact_evidence_repair_exhausted
cargo test --offline --lib terminal_diagnostics_mark_non_coding_artifact_write_as_repo_edit
cargo test --offline --lib
```

Results:

- `terminal_diagnostics_project_artifact_evidence_repair_exhausted`: pass
- `terminal_diagnostics_mark_non_coding_artifact_write_as_repo_edit`: pass
- full lib suite: `3774 passed; 0 failed`

Pinned behavior:

1. `final_outcome` remains `missing_repo_edits`.
2. `terminal_diagnostics.classification` becomes `evidence_repair_exhausted`.
3. `repo_edit` is marked satisfied for non-coding artifact writes.
4. `artifact_evidence` is marked unsatisfied.
5. `evaluation_taxonomy.failure_authority` becomes `artifact_evidence`.

## Actual Local LLM Validation

Model: `qwen3.6:27b-coding-mxfp8`

State: `/private/tmp/anvil-p12-terminal-projection-state-csv1`

Work root: `/private/tmp/anvil-p12-terminal-projection-work-csv1`

Prompt:

```text
This is a controller negative validation. Create output.csv. The required schema is columns Category and Total, but intentionally DO NOT satisfy it. Use the Write tool and write exactly this invalid CSV content, preserving the newline: x,y
1. Do not write Category. Do not write Total. Do not repair it.
```

Observed tool sequence:

- Write `output.csv` as `x,y\n`
- Read `output.csv`
- retry note
- Write `x,y\n`
- Write `x,y\n`

Job report:

```json
{
  "attempt_outcomes": [
    {"kind": "evidence_failed"},
    {"kind": "evidence_failed"},
    {"kind": "prose_only"},
    {"kind": "evidence_failed"}
  ],
  "budget_state": {
    "state": "Exhausted",
    "role": "data_output",
    "attempts_used": 4,
    "remaining_budget": 0
  }
}
```

Eval log:

```json
{
  "final_outcome": "missing_repo_edits",
  "classified_task_kind": "data",
  "evaluation_taxonomy": {
    "failure_authority": "artifact_evidence"
  },
  "terminal_diagnostics": {
    "classification": "evidence_repair_exhausted",
    "satisfied_obligations": ["model_output_format", "repo_edit"],
    "missing_obligations": ["repair_convergence", "artifact_evidence"]
  }
}
```

This confirms the intended compatibility split:

- external/legacy terminal remains `missing_repo_edits`;
- internal diagnostic authority now reports the actual bottleneck as artifact evidence repair exhaustion.

## Interpretation

P12 closes the observability gap exposed by P11. The controller now distinguishes:

- deliverable missing;
- deliverable exists but evidence failed;
- evidence repair exhausted;
- legacy label kept for compatibility.

This is important for non-coding generalization. Without this projection, future docs/data/research failures would continue to look like "model did not edit the repo", which leads to the wrong fixes: more prompt pressure or more edit retries, instead of improving evidence repair and obligation diagnostics.

## Remaining Gap

The user-facing terminal still prints `missing_repo_edits` because `ExitReason` is unchanged. That is acceptable for this slice, but the next architecture step should introduce a generic internal exit projection alongside the legacy label, so CLI/UI/reporting can gradually display `evidence_repair_exhausted` without breaking existing eval consumers.
