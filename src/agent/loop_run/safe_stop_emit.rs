//! `agent.safe_stop.report` emit cluster extracted from `turn.rs`
//! (parent #680).
//!
//! Hosts the Issue #654 emit lifecycle for the structured
//! `agent.safe_stop.report` event:
//!
//! - `record_safe_stop_report` — Task D.4 chokepoint (per-StopReason
//!   dedup → emit 4 job reports → build SafeStopReport → render bounded
//!   payload → log via `log_llm_event` (final `mask_payload_inplace`
//!   defence) → mark stop_reason emitted).
//! - `resolve_current_role_for_safe_stop` — §6.4 SSOT priority chain
//!   (explicit role → semantic_plan → recovery target → target_hint →
//!   first required_artifact of reconstructed TaskContract).
//! - Six emit shells: `_for_diagnostic_target_missing` /
//!   `_for_verifier_failed_safe_stop` / `_for_verifier_weak`
//!   (#[cfg(test)]) / `_for_artifact_completion_failed` /
//!   `_for_verifier_missing` / `_for_repair_terminal`.
//! - `emit_repair_safe_stop_report` — shared helper for the
//!   repair-job-driven emit paths (E.2 / E.3 / E.4).
//! - `collect_owned_test_artifacts` — Task D.6 Owned-validated test
//!   artifact collector (≤ 8 paths, classify_ownership gated).
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent` / `&Agent`, matching the `actor_loop_flow` /
//! `reply_retry` / earlier vertical-slice precedent. `pub(super)`
//! limited / no facade re-export (DR3-001).

use super::Agent;
use super::safe_stop_payload::{build_safe_stop_payload, collect_recent_action_labels};
use super::tool_history::{
    latest_successful_read_existing_path, latest_verifier_repair_note_index,
};
use crate::logging::log_llm_event;

pub(super) fn emit_safe_stop_report_for_repair_terminal(
    agent: &mut Agent,
    reason: super::repair_job::RepairTerminalReason,
) {
    let Some(stop_reason) = reason.safe_stop_reason() else {
        return;
    };
    emit_repair_safe_stop_report(agent, stop_reason);
}

/// Issue #654 (CB-003 / Task D.5) — §6.4 SSOT priority chain for the
/// `current_role` field of `SafeStopReport`. Priority order:
/// 1. `explicit_role` (when caller has the role in hand).
/// 2. `repair_job.semantic_plan.preferred_repair_role`.
/// 3. `current_artifact_recovery_target.role`.
/// 4. `repair_job.target_hint.role`.
/// 5. First `required_artifacts` of the reconstructed `TaskContract`.
pub(super) fn resolve_current_role_for_safe_stop(
    agent: &Agent,
    explicit_role: Option<super::task_contract::ArtifactRole>,
) -> Option<super::task_contract::ArtifactRole> {
    if let Some(role) = explicit_role {
        return Some(role);
    }
    if let Some(job) = agent.repair_job.as_ref()
        && let Some(plan) = job.semantic_plan.as_ref()
    {
        return Some(plan.preferred_repair_role);
    }
    if let Some(target) = agent.current_artifact_recovery_target.as_ref() {
        return Some(target.role);
    }
    if let Some(job) = agent.repair_job.as_ref()
        && let Some(hint) = job.target_hint.as_ref()
    {
        return Some(hint.role);
    }
    let request = agent.active_request_text().unwrap_or_default();
    if !request.is_empty() {
        let contract = super::task_contract::TaskContract::from_request(&request);
        if let Some(role) = contract.required_artifacts.first().copied() {
            return Some(role);
        }
    }
    None
}

/// Issue #654 — diagnostic_target_missing path: build a
/// `SafeStopInput::FromRepair { DiagnosticTargetMissing }` and forward to
/// `record_safe_stop_report`. Pulled into a separate function so the
/// orchestration of context-collection from `Agent` state stays out of
/// the lifecycle of `record_verifier_diagnostic_unavailable`.
pub(super) fn emit_safe_stop_report_for_diagnostic_target_missing(agent: &mut Agent) {
    let Some(job) = agent.repair_job.clone() else {
        return;
    };
    let session_id = agent.session_store.session_id().to_string();
    let turn_index = agent.current_turn_index as u64;
    let scope = super::task_workspace_scope::TaskWorkspaceScope::detect(
        &agent.work_root,
        agent.active_request_text().unwrap_or_default().as_str(),
    );
    let candidates: Vec<String> = job
        .changed_file_hints
        .iter()
        .map(|hint| hint.path.clone())
        .collect();
    let latest_read = latest_successful_read_existing_path(
        &agent.session.messages,
        &agent.work_root,
        latest_verifier_repair_note_index(&agent.session.messages),
    );
    let expected = job
        .target_hint
        .as_ref()
        .map(|hint| std::path::PathBuf::from(&hint.path));
    // CB-003: route through the §6.4 SSOT fallback chain so the
    // diagnostic_target_missing path is not limited to semantic_plan.
    let current_role = resolve_current_role_for_safe_stop(agent, None);
    let input = super::repair_job::SafeStopInput::FromRepair {
        job: &job,
        stop_reason: super::repair_job::StopReason::DiagnosticTargetMissing,
        owned_test_artifacts: Vec::new(),
    };
    let ctx = super::repair_job::SafeStopContext {
        current_role,
        expected_target: expected.as_deref(),
        actual_actions_raw: collect_recent_action_labels(&agent.session.messages),
        latest_successful_read: latest_read.as_deref(),
        task_workspace_scope: &scope,
        candidates,
        session_id: &session_id,
        turn_index,
    };
    record_safe_stop_report(agent, input, ctx);
}

/// Issue #654 (Task D.4) — thin shell that dedups per StopReason, builds
/// the structured report via `SafeStopReport::build_from`, renders the
/// bounded payload via `build_safe_stop_payload`, and emits the
/// `agent.safe_stop.report` event through `log_llm_event` (which routes
/// through `mask_payload_inplace` — the final defense line).
pub(super) fn record_safe_stop_report(
    agent: &mut Agent,
    input: super::repair_job::SafeStopInput<'_>,
    ctx: super::repair_job::SafeStopContext<'_>,
) {
    let stop_reason = input.stop_reason();
    if agent.safe_stop_report_emitted.contains(&stop_reason) {
        return;
    }
    // Issue #666 (CB-001 fix): emit per-turn job reports BEFORE the
    // SafeStopReport so the llm-io event order is
    // `agent.{x}.report → agent.safe_stop.report` per design Section
    // 8-2. Per-turn dedup in `maybe_emit_job_reports` makes the
    // post-`run_turn` finalizer at `handle_user_message` a no-op for
    // any kind already emitted here. We record the stop_reason in
    // `safe_stop_report_emitted` AFTER `maybe_emit_job_reports` so
    // the SafeStopLinkage built into the job reports observes the
    // pre-stop state of the dedup set (`report_emitted=false`); the
    // job reports still carry the actual stop_reason via
    // `safe_stop.reason` once the post-run finalizer is dedup'd out.
    // Order: build linkage → emit 4 job reports → emit safe_stop.
    let linkage_reason = Some(stop_reason.as_str().to_string());
    agent.maybe_emit_job_reports_with_linkage(linkage_reason);
    let report = super::repair_job::SafeStopReport::build_from(input, ctx);
    let payload = build_safe_stop_payload(&report);
    log_llm_event("agent.safe_stop.report", payload);
    agent.safe_stop_report_emitted.insert(stop_reason);
}

/// Issue #654 (E.3) — `verifier_failed_safe_stop` emit shell. Builds a
/// `FromRepair { VerifierFailedSafeStop }` input from
/// `agent.repair_job`.
pub(super) fn emit_safe_stop_report_for_verifier_failed_safe_stop(agent: &mut Agent) {
    emit_repair_safe_stop_report(agent, super::repair_job::StopReason::VerifierFailedSafeStop);
}

#[cfg(test)]
pub(super) fn emit_safe_stop_report_for_verifier_weak(agent: &mut Agent) {
    emit_repair_safe_stop_report(agent, super::repair_job::StopReason::VerifierWeak);
}

/// Issue #654 (E.2) — `artifact_completion_failed` emit shell. Unlike
/// the other shells, this path can fire BEFORE a verifier-driven
/// `RepairJob` has been built (the role-specific retry budget exhausts
/// during pure artifact-completion attempts). When `agent.repair_job`
/// is `None` we fall back to a synthetic empty `RepairJob` whose only
/// meaningful field is `target_hint` (carried from the explicit role +
/// path the caller supplies) so the emitted payload still carries
/// `current_role` and `expected_target` for downstream `/bug-fix`
/// consumers.
pub(super) fn emit_safe_stop_report_for_artifact_completion_failed(
    agent: &mut Agent,
    role: super::task_contract::ArtifactRole,
    expected_target_path: Option<String>,
) {
    if agent
        .safe_stop_report_emitted
        .contains(&super::repair_job::StopReason::ArtifactCompletionFailed)
    {
        return;
    }
    let session_id = agent.session_store.session_id().to_string();
    let turn_index = agent.current_turn_index as u64;
    let scope = super::task_workspace_scope::TaskWorkspaceScope::detect(
        &agent.work_root,
        agent.active_request_text().unwrap_or_default().as_str(),
    );
    let owned_test_artifacts = collect_owned_test_artifacts(agent);
    // Prefer the real RepairJob (when one exists, e.g. the
    // RepairArtifact exhaustion path at the verifier-repair stage) so
    // failure_signature / command / output_excerpt remain accurate.
    // Otherwise fall back to a synthetic shell whose only carried
    // context is the target path the caller provided.
    let job = match agent.repair_job.clone() {
        Some(job) => job,
        None => {
            let mut shell = super::repair_job::RepairJob::empty_synthetic();
            if let Some(path) = expected_target_path.clone() {
                shell.target_hint = Some(super::task_contract::RecoveryTargetHint {
                    role,
                    path,
                    reason: "artifact_completion_failed".to_string(),
                });
            }
            shell
        }
    };
    let expected = expected_target_path
        .map(std::path::PathBuf::from)
        .or_else(|| {
            job.target_hint
                .as_ref()
                .map(|hint| std::path::PathBuf::from(&hint.path))
        });
    // CB-003: still preempt with the explicit role the caller supplied
    // (the artifact-completion retry budget exhausted on this role),
    // but route the no-explicit-role branches through the §6.4 helper
    // so the priority order (semantic_plan -> recovery target ->
    // target_hint -> contract) stays single-sourced.
    let current_role = resolve_current_role_for_safe_stop(agent, Some(role));
    let candidates: Vec<String> = job
        .changed_file_hints
        .iter()
        .map(|hint| hint.path.clone())
        .collect();
    let input = super::repair_job::SafeStopInput::FromRepair {
        job: &job,
        stop_reason: super::repair_job::StopReason::ArtifactCompletionFailed,
        owned_test_artifacts,
    };
    let ctx = super::repair_job::SafeStopContext {
        current_role,
        expected_target: expected.as_deref(),
        actual_actions_raw: collect_recent_action_labels(&agent.session.messages),
        latest_successful_read: None,
        task_workspace_scope: &scope,
        candidates,
        session_id: &session_id,
        turn_index,
    };
    record_safe_stop_report(agent, input, ctx);
}

/// Shared helper for repair-job-driven emit paths (E.2 / E.3 / E.4).
pub(super) fn emit_repair_safe_stop_report(
    agent: &mut Agent,
    stop_reason: super::repair_job::StopReason,
) {
    let Some(job) = agent.repair_job.clone() else {
        return;
    };
    let session_id = agent.session_store.session_id().to_string();
    let turn_index = agent.current_turn_index as u64;
    let scope = super::task_workspace_scope::TaskWorkspaceScope::detect(
        &agent.work_root,
        agent.active_request_text().unwrap_or_default().as_str(),
    );
    let owned_test_artifacts = collect_owned_test_artifacts(agent);
    let expected = job
        .target_hint
        .as_ref()
        .map(|hint| std::path::PathBuf::from(&hint.path));
    // CB-003: route through the §6.4 SSOT fallback chain.
    let current_role = resolve_current_role_for_safe_stop(agent, None);
    let candidates: Vec<String> = job
        .changed_file_hints
        .iter()
        .map(|hint| hint.path.clone())
        .collect();
    let input = super::repair_job::SafeStopInput::FromRepair {
        job: &job,
        stop_reason,
        owned_test_artifacts,
    };
    let ctx = super::repair_job::SafeStopContext {
        current_role,
        expected_target: expected.as_deref(),
        actual_actions_raw: collect_recent_action_labels(&agent.session.messages),
        latest_successful_read: None,
        task_workspace_scope: &scope,
        candidates,
        session_id: &session_id,
        turn_index,
    };
    record_safe_stop_report(agent, input, ctx);
}

/// Issue #654 (E.5) — `verifier_missing` emit shell. Uses the
/// `FromMissingVerifier` builder so empty `failure_signature` fallback
/// detection by downstream consumers does not misfire (R8).
pub(super) fn emit_safe_stop_report_for_verifier_missing(agent: &mut Agent) {
    let Some(job) = agent.missing_verifier_job.clone() else {
        return;
    };
    let session_id = agent.session_store.session_id().to_string();
    let turn_index = agent.current_turn_index as u64;
    let scope = super::task_workspace_scope::TaskWorkspaceScope::detect(
        &agent.work_root,
        agent.active_request_text().unwrap_or_default().as_str(),
    );
    let owned_test_artifacts = collect_owned_test_artifacts(agent);
    let input = super::repair_job::SafeStopInput::FromMissingVerifier {
        job: &job,
        owned_test_artifacts,
    };
    // CB-003: verifier_missing has no `RepairJob`, so the helper falls
    // through to current_artifact_recovery_target -> reconstructed
    // TaskContract.required_artifacts.first() for downstream
    // `/bug-fix`.
    let current_role = resolve_current_role_for_safe_stop(agent, None);
    let ctx = super::repair_job::SafeStopContext {
        current_role,
        expected_target: None,
        actual_actions_raw: collect_recent_action_labels(&agent.session.messages),
        latest_successful_read: None,
        task_workspace_scope: &scope,
        candidates: Vec::new(),
        session_id: &session_id,
        turn_index,
    };
    record_safe_stop_report(agent, input, ctx);
}

/// Issue #654 (DR3-002 / Task D.6) — collect Owned-validated test
/// artifact relative paths from the current turn's edits, gated by
/// `classify_ownership`. Returns at most 8 paths; the builder
/// re-applies syntactic safety as defense-in-depth.
fn collect_owned_test_artifacts(agent: &Agent) -> Vec<String> {
    let scope = super::task_workspace_scope::TaskWorkspaceScope::detect(
        &agent.work_root,
        agent.active_request_text().unwrap_or_default().as_str(),
    );
    let mut out: Vec<String> = Vec::new();
    for rel in agent.turn_edited_relative_paths.iter() {
        // Only test files qualify.
        if !crate::util::file_classify::is_test_file(std::path::Path::new(rel)) {
            continue;
        }
        let inputs = super::artifact_ownership::OwnershipInputs {
            work_root: &agent.work_root,
            relative_path: rel.as_str(),
            scope: &scope,
            edited_this_session: true,
            scaffold_changed: false,
            verifier_passed_in_scope: false,
            // Issue #661 (Task 3.1): legacy helper — not one of the 4
            // verifier-path SSOT sites. Iteration-3 may revisit if
            // this shadow consumer needs verifier-binding semantics.
            nested_test_admission: super::artifact_ownership::NestedTestAdmission::default(),
        };
        if super::artifact_ownership::classify_ownership(inputs)
            == super::artifact_ownership::ArtifactOwnership::Owned
        {
            out.push(rel.clone());
        }
    }
    out.sort();
    out.truncate(8);
    out
}
