# P4 Event Summary

## P4-02 VerifierSkill

Initial single-run testing showed both main models completing the same existing-file Python edit and producing the expected SkillRegistry-backed verifier event. Follow-up testing added two more repetitions per model.

| Model | Iter | Duration | Verifier Event | Auto Test | Result |
| --- | ---: | ---: | --- | --- | --- |
| `qwen3.6:27b-coding-nvfp4` | 3/50 | 9s | `agent.verifier.completed { dispatched: "autotest" }` | `python3 -m py_compile 'app.py'`, pass | PASS |
| `qwen3.6:27b-coding-nvfp4` | 4/50 | 7s | `agent.verifier.completed { dispatched: "skip" }` | none | FAIL |
| `qwen3.6:27b-coding-nvfp4` | 3/50 | 8s | `agent.verifier.completed { dispatched: "autotest" }` | `python3 -m py_compile 'app.py'`, pass | PASS |
| `qwen3.5:122b` | 3/50 | 18s | `agent.verifier.completed { dispatched: "autotest" }` | `python3 -m py_compile 'app.py'`, pass | PASS |
| `qwen3.5:122b` | 3/50 | 15s | `agent.verifier.completed { dispatched: "autotest" }` | `python3 -m py_compile 'app.py'`, pass | PASS |
| `qwen3.5:122b` | 3/50 | 15s | `agent.verifier.completed { dispatched: "autotest" }` | `python3 -m py_compile 'app.py'`, pass | PASS |

Observed issue: `changed_files` still includes `.anvil-state/.../logs/llm-io.jsonl`, so edited-file telemetry remains noisy.

Stability after follow-up repetitions:

- `qwen3.6:27b-coding-nvfp4`: 2/3 passed. The failed run read `app.py` but then returned prose-only "I need to change..." messages and never called Edit.
- `qwen3.5:122b`: 3/3 passed. All runs followed Read -> Edit -> VerifierSkill -> AutoTest.
- VerifierSkill itself was consistently observable: pass runs dispatched `autotest`; the qwen3.6 no-edit failure dispatched `skip`.

## P4-01 Reminder / Precaution

The failure-driven reminder path emitted:

- `agent.reminder.failed`
- `agent.reminder.skipped`
- no `agent.skill.*` event

This matches the implementation note in `src/agent/skills/reminder_skill.rs`: `ReminderSkill` declares a tier and has smoke-test coverage, but the production path still goes through `Agent::maybe_invoke_reminder` directly.

Result: PARTIAL/FAIL for the strict P4-01 acceptance criterion "Reminder skill invoked through registry".

Follow-up R2 also failed strict P4-01. It ran the nonexistent command once, then stopped with no repo edits; no `agent.skill.*` event appeared.

## P4-03 Permission Enforcement

`cargo test --test skill_trust_tier_smoke` passed 8/8 cases, covering:

- `ExternalDisabled` denial
- Plan mode denial for write-capable skill
- read-only Plan mode allow
- Verifier Plan mode bypass
- permission-denied event shape
- denial does not consume per-turn cap
- `FeedbackKind::SkillPermissionDenied` serde round trip
