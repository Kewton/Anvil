# Issue #867 Design

## Goal

Keep PAM as an advisory prompt-context mechanism only. Completion authority remains owned by `TaskContract` artifact/verifier evidence and verifier repair state.

## Boundary

- Do not pass PAM decisions into `TaskContract::evaluate*` or `plan_artifact_recovery`.
- Treat PAM impact as observability: record which advisory target was affected (`prompt_context`, `task_contract`, or `repair_hint`) and why.
- When PAM is not used in a turn, record the reason in the eval-log projection instead of silently omitting `pam_eval`.

## Implementation Plan

- Extend `PamEvalSummary` additively with bounded `affected_targets` and optional `unused_reason`.
- Derive target attribution from the existing `PamAdvisoryDecision.candidate_decisions` and `active_job_role`.
- Track a turn-local PAM skip reason on `Agent` for disabled, duplicate, photon-unavailable, shadow/canary/plan/no-injection cases.
- Populate `record.pam_eval` from either the advisory decision or the skip reason at eval-log write time.
- Add focused tests for advisory-only completion invariance and eval-log serialization of used/unused PAM.
