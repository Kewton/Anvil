## Context

Follow-up of #447.

Parent follow-up:
- TBD

Issue 447 created the SkillRegistry foundation and moved VerifierSkill into the production post-loop path. The evaluation found that other internal harness components still remain outside the skill layer.

## Problem

Tester, CaseRecord, CaseRetrieval, and AntiPattern are still dispatched through bespoke code paths. That means the runtime has multiple internal control surfaces with different logging, permission, and applicability behavior.

This limits the value of SkillTrustTier and makes future observability/evaluation harder.

## Desired Direction

- Model Tester, CaseRecord, CaseRetrieval, and AntiPattern as AgentSkill implementations or a closely related protocol abstraction.
- Give each component an explicit trigger, applicability rule, trust tier, and event payload.
- Keep payloads compact and deterministic for local LLM evaluation.
- Avoid making the runtime more provider-generic; keep the local-first Ollama implementation simple.

## Acceptance Criteria

- At least Tester and CaseRetrieval can be invoked through SkillRegistry or a shared permissioned skill protocol.
- Skill invocation logs identify which internal harness component ran, skipped, or failed.
- Existing CaseMemory and Tester tests continue to pass.
- The migration reduces direct branching in `turn.rs` rather than adding another parallel path.

