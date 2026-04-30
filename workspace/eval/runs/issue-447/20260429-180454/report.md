# Issue 447 Evaluation Report

Run ID: `20260429-180454`
Evaluation commit: `545245d88a79604dca513a641a3155af332a235d`

## Verdict

Judgement: `Mixed Positive / Verifier Skill Is Real, Agent Completion Still Variable`

Issue 447 was worth doing. It creates a concrete `AgentSkill` / `SkillRegistry` foundation, moves the post-loop verifier path into that foundation, and adds enforceable trust tiers. The most important practical result is that real agent turns now emit `agent.verifier.completed` before auto-test and AnvilScore events.

The follow-up repetitions make the evaluation more nuanced. `qwen3.5:122b` was stable on the small Read -> Edit -> Verify case, passing 3/3. `qwen3.6:27b-coding-nvfp4` passed 2/3; the failed run read the file but then returned prose-only messages and never called Edit. That failure is not a VerifierSkill regression, but it shows the actor loop still needs stronger enforcement when the model describes an edit without executing it.

The other limitation is scope: the skill architecture is not yet the unified internal-harness layer described by the Epic goal. `ReminderSkill` exists and is tested as an adapter, but production reminder dispatch still uses the legacy `Agent::maybe_invoke_reminder` path. Tester, CaseRecord, CaseRetrieval, and AntiPattern also remain outside the registry.

## Static Verification

| Check | Result | Notes |
| --- | --- | --- |
| `cargo fmt --check` | PASS | no formatting drift |
| `cargo clippy --all-targets -- -D warnings` | PASS | no warnings |
| `cargo test` | PASS | full suite passed |
| `cargo test --test agent_skill_registry_smoke` | PASS | 9 passed |
| `cargo test --test skill_trust_tier_smoke` | PASS | 8 passed |

## Practical E2E Results

| Scenario | Model | Result | Iter | Duration | Key Observation |
| --- | --- | --- | ---: | ---: | --- |
| P4-02 Verifier after repo edit | `qwen3.6:27b-coding-nvfp4` | PASS | 3/50 | 9s | `agent.verifier.completed { dispatched: "autotest" }`; `py_compile` passed |
| P4-02 Verifier after repo edit R2 | `qwen3.6:27b-coding-nvfp4` | FAIL | 4/50 | 7s | Read succeeded, but model emitted prose-only next-step messages and never called Edit; verifier dispatched `skip` |
| P4-02 Verifier after repo edit R3 | `qwen3.6:27b-coding-nvfp4` | PASS | 3/50 | 8s | `agent.verifier.completed { dispatched: "autotest" }`; `py_compile` passed |
| P4-02 Verifier after repo edit | `qwen3.5:122b` | PASS | 3/50 | 18s | Same registry-backed verifier event and passing `py_compile` |
| P4-02 Verifier after repo edit R2 | `qwen3.5:122b` | PASS | 3/50 | 15s | Same registry-backed verifier event and passing `py_compile` |
| P4-02 Verifier after repo edit R3 | `qwen3.5:122b` | PASS | 3/50 | 15s | Same registry-backed verifier event and passing `py_compile` |
| P4-01 Reminder after failure | `qwen3.6:27b-coding-nvfp4` | PARTIAL/FAIL | 6/50 | 18s | Reminder emitted `agent.reminder.failed/skipped`, but no `agent.skill.*` event |
| P4-01 Reminder after failure R2 | `qwen3.6:27b-coding-nvfp4` | FAIL | 4/50 | 8s | No `agent.skill.*` event; stopped with missing repo edits after reporting command failure |
| P4-03 Permission enforcement | unit/integration | PASS | n/a | n/a | Permission-denied events and tier policy covered by focused tests |

## Follow-up Stability

| Model | P4-02 Pass Rate | Notes |
| --- | ---: | --- |
| `qwen3.6:27b-coding-nvfp4` | 2/3 | Skill event observable in all runs, but one run never called Edit after Read |
| `qwen3.5:122b` | 3/3 | Stable Read -> Edit -> VerifierSkill -> AutoTest across all three runs |

## What Improved

- Verifier dispatch is now visible as a skill-level runtime event, including no-edit failure cases where it dispatches `skip`.
- AutoTestRunner still executes after successful edits, and its result is linked to AnvilScore.
- Permission tiers are explicit and enforced at the registry boundary.
- Permission-denied events are structured as `agent.skill.permission_denied`.
- qwen3.5 handled the small Read -> Edit protocol cleanly in all three repetitions and reached verification in 3 iterations each time.

## Remaining Gaps

- Reminder is not yet production-routed through `SkillRegistry`; strict P4-01 is not satisfied.
- The "Agentic Skills Layer" is currently foundation + Verifier + tier policy, not a complete migration of Reminder/Tester/CaseRecall.
- qwen3.6 still sometimes describes the needed edit instead of making it. Existing retry prompts did not force an Edit call in that run.
- Skill permission enforcement is well tested, but live production exposure is limited because most skills are still in-process built-ins rather than registry-dispatched tools.
- `changed_files` telemetry still includes `.anvil-state` runtime artifacts.
- The reminder probe also exposed a sidecar instability: the reminder LLM call failed and was recorded recoverably, but this means the run proves loop survivability more than reminder quality.

## Recommendation

Treat Issue 447 as a successful architectural step, not the end state. The next useful work is to migrate the remaining internal harness components into the same registry path:

1. Route production Reminder/Precaution through `SkillRegistry::invoke`.
2. Add Tester, CaseRecord, CaseRetrieval, and AntiPattern as permissioned skills.
3. Keep registry events as the single observability surface for internal skills.
4. Filter runtime artifacts from edited-file telemetry before quality scoring.
5. Strengthen the no-edit retry path: when a model says it will edit but emits no tool call, inject a stricter next-turn protocol or force a small Edit/Write decision.
6. Add one live permission-denied scenario once an externally disabled or write-capable skill can be exercised through a production-facing path.
