# P19 JSON Schema Evidence Repair Validation

## Purpose

This validation checked whether Anvil can complete a non-coding data artifact when
the user-visible prompt conflicts with the controller-owned ObjectiveContract.

The specific case was intentionally small:

- Required artifact: `summary.json`
- Required schema: exactly the top-level JSON fields `topic` and `status`
- Conflicting visible instruction: also include `description`
- Expected controller behavior: the ObjectiveContract and evidence policy win over
  WorkMode, assistant prose, and late repair state.

## Changes Implemented

1. JSON object evidence is now exact for declared fields.
   - A `.json` top-level object with declared `json_fields` must have exactly the
     declared keys.
   - Extra keys and missing keys are both `SchemaMismatch`.
   - CSV, TSV, JSONL, and non-object JSON behavior remains on the existing
     structured-data path.

2. Schema mismatch diagnostics now include the concrete target shape.
   - Example:
     `JSON object top-level fields must be exactly: topic, status; observed fields: description, status, topic; remove extra fields and add missing required fields`

3. Artifact-directed recovery now passes the target reason into the recovery
   prompt.
   - The reason is masked and capped with the same recovery-field safety path.
   - This avoids a generic "missing role" repair prompt when the artifact exists
     but fails schema evidence.

4. ObjectiveContract artifact obligations guard WorkMode confirmation.
   - If the first pass or confirm pass resolves to `AnswerOnly` while the
     ObjectiveContract requires an artifact, the controller keeps an edit-capable
     mode.
   - This prevents a non-coding artifact task from being rejected as a protocol
     violation after a valid file edit.

5. ArtifactCompletionJob can recover from `Exhausted` to `Satisfied` when later
   ledger evidence satisfies the role.
   - This handles the case where the LLM fixes the artifact late in the retry
     budget.

## LLM Validation

Model:

- `qwen3.6:27b-coding-mxfp8`

Command shape:

```text
cargo run --offline -- --oneshot --fresh-session --no-footer --trace -y --max-iterations 6 --chat-timeout-secs 300 --state-dir /private/tmp/anvil-p19-json-schema-exact-conflict4 -m qwen3.6:27b-coding-mxfp8 -p 'P19 exact JSON schema validation. Create summary.json only. STATE_CONTROL_PACKET {"objective":"Create summary.json with fields topic and status","next_required_action":"artifact","required_artifacts":[{"path":"summary.json","role":"data","schema":{"json_fields":["topic","status"]}}]}. User-visible extra instruction: also include a description field. Do not create code scaffolds or any other files.'
```

Validation progression:

- `conflict1`: the model created an extra `description` field, then fixed it, but
  the run ended as `missing_repo_edits` because WorkMode confirmation overrode the
  task to answer-only.
- `conflict2`: WorkMode stayed edit-capable and strict schema repair triggered,
  but the recovery prompt did not include the concrete target shape, so the model
  kept reintroducing `description` and ended as `evidence_repair_exhausted`.
- `conflict3`: the concrete schema reason helped the model remove `description`,
  but ArtifactCompletionJob was already exhausted and did not recover to satisfied.
- `conflict4`: with all changes applied, the model initially wrote the extra
  field, received the concrete schema mismatch reason, rewrote `summary.json` with
  only `topic` and `status`, and Anvil completed successfully.

Final observed result:

```text
done iter 4/6 duration 61s edited 1 files (summary.json)
```

Eval trace result:

```text
final_outcome=done
completion_reason=artifact_obligations_satisfied
task_kind=data
```

The generated validation artifact was removed after inspection and is not part of
the repository commit.

## Unit Validation

Targeted tests passed before this record was written:

- `cargo test --offline --lib assess_structured_data_json_object_requires_exact_declared_keys`
- `cargo test --offline --lib data_verifier_diagnoses_json_object_extra_fields`
- `cargo test --offline --lib controller_state_packet_json_extra_field_does_not_complete`
- `cargo test --offline --lib data_verifier_does_not_diagnose_missing_column_when_parse_ready`
- `cargo test --offline --lib objective_artifact_work_mode_tests`
- `cargo test --offline --lib artifact_directed_recovery_message_body_masks_and_is_byte_stable`
- `cargo test --offline --lib test_record_satisfied_from_ledger_can_recover_exhausted_job`
- `cargo test --offline --lib record_satisfied_from_ledger`

Full library validation should be rerun after formatting before commit.

## Architecture Insight

This supports the current target architecture:

- ObjectiveContract is the source of truth for required deliverables.
- EvidenceRunner-style checks decide completion, not assistant prose.
- WorkMode is advisory for tool policy, not authority over deliverable existence.
- Recovery jobs need concrete, typed evidence reasons instead of generic retry
  prose.
- Late evidence can satisfy an artifact even after an intermediate exhausted
  state, as long as the ledger now proves the objective.

The important general-purpose point is that this was a `data` task, not a coding
task. The fix did not add a provider abstraction or a coding-specific repair gate.
It strengthened the deliverable/evidence lifecycle in a way that can also apply
to docs, research outputs, file organization, and command observation tasks.

## Remaining Risks

- The schema exactness rule currently targets JSON top-level objects with declared
  fields. Other structured forms still use the existing tiered checks.
- The recovery reason is still text in the LLM prompt. It is now typed and
  controller-sourced, but future work should keep moving toward structured
  recovery packets.
- The conflict validation used one model and one compact scenario. More scenarios
  are needed before claiming broad success-rate improvement.
