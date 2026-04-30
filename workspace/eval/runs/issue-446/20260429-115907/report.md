# Issue 446 Evaluation Report

Run ID: `20260429-115907`
Commit: `9b38a14dcd98d9ff430327b2709e4afd8e19fb0e`

## Verdict

Judgement: `Mostly Positive / Memory Works, Quality Control Still Needed`

Issue 446 is valuable. It adds real cross-session memory: successful sessions persist compact CaseRecords, similar future tasks can retrieve `Relevant Local Cases`, and repeated failures become `Avoid Patterns`. The practical E2E runs confirmed all three behaviors.

The main limitation is that memory improves context and sometimes speed, but it does not guarantee better edits. qwen3.5 completed faster with retrieved cases, yet produced a lower-quality README edit with duplicate `Usage` sections. Retrieval also skipped a semantically related task because the score stayed below the 0.40 threshold.

## Static Verification

| Check | Result | Notes |
| --- | --- | --- |
| `cargo fmt --check` | PASS | no formatting drift |
| `cargo clippy --all-targets -- -D warnings` | PASS | no warnings |
| `cargo test` | PASS | all unit, integration, and doc tests passed |

Issue 446-specific coverage observed in `cargo test`:

- `tests/case_record_extraction.rs`: successful CaseRecord extraction, persistence, caps, secret masking, git remote sanitization.
- `tests/case_retrieval_smoke.rs`: lexical retrieval, prompt rendering, threshold behavior, corrupt file handling, secret masking.
- `tests/anti_pattern_extraction.rs`: failed-case creation, repeat increment, retrieval, prompt rendering, retirement, corrupt file handling.
- In-module tests cover deterministic IDs, Jaccard scoring, path/symlink rejection, env disable/dry-run gates, and prompt caps.

## Practical E2E Results

| Scenario | Model | Result | Iter | Duration | Key Observation |
| --- | --- | --- | --- | --- | --- |
| P3-01a CaseRecord creation | qwen3.6 | PASS | 5/50 | 11s | README edited; `agent.case_record.extracted`; case file persisted |
| P3-01b Related retrieval | qwen3.6 | MIXED | 10/50 | 31s | candidate found but `below_threshold`; no `Relevant Local Cases` injection |
| P3-01c Near-identical retrieval | qwen3.6 | PASS | 6/50 | 18s | `agent.case_retrieval.completed`; prompt included `Relevant Local Cases` |
| P3-01d qwen3.5 with retrieval | qwen3.5:122b | MIXED | 3/50 | 25s | retrieval injected and task completed faster, but README quality regressed with duplicated Usage |
| P3-02 AntiPattern repeat | qwen3.6 | PASS | 3 runs | 11s / 11s / 9s | unsafe command anti-pattern created, incremented to x2, then injected as `Avoid Patterns` |
| P3-03 Secret-bearing task | qwen3.6 | FAIL/MIXED | 4-5/50 | 5s / 7s | no CaseRecord created because model stalled after Read; logs masked secret, but raw session.json retained user message |

## What Improved

- CaseRecord persistence works in the real turn loop.
- CaseRetrieval can inject compact prior cases without raw conversation history.
- qwen3.5 benefited on speed: a repeated README task finished in 3 iterations where qwen3.6 similar runs took 5-10 iterations.
- AntiPattern memory works end to end: creation, repeat increment, retrieval, and prompt injection were all observed.
- Static test coverage is broad and safety-focused.

## Remaining Gaps

- Retrieval threshold is conservative. A related Widget CLI troubleshooting task found the prior case but skipped injection as `below_threshold`.
- Retrieved memory is not yet quality-aware. It can speed execution while still allowing duplicated or lower-quality edits.
- Documentation-only edits are not reflected as `user_visible_artifact=true` in AnvilScore, even though CaseRecord extraction succeeds through the non-auto-test path.
- `changed_files` telemetry still includes `.anvil-state` artifacts.
- Secret handling is split: logs mask secrets and CaseRecord tests verify masking, but raw `session.json` still stores the original user message. That matters for future dataset export and observability.
- The secret-bearing live E2E did not reach CaseRecord extraction because qwen3.6 stalled after Read twice.

## Recommendation

Treat Issue 446 as successful infrastructure, not as final quality improvement. It should feed Issue 447 and 449:

1. Use CaseMemory through a skill/permission layer so retrieved memory can drive specific next actions.
2. Add quality filters to retrieved cases: prefer cases that reduced iterations without introducing duplicate sections or messy edits.
3. Extend AnvilScore/file classification to recognize documentation artifacts for documentation-mode tasks.
4. Redact or exclude raw user messages from future export paths, even if session storage remains raw for replay.
5. Keep the Issue 445 follow-up issues active because telemetry noise and verifier mismatch still affect evaluation quality.
