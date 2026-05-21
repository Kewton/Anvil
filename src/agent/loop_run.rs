use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use crate::agent::prompting;
use crate::agent::recovery;
use crate::config::Config;
use crate::format_model_banner;
use crate::logging::log_llm_event;
use crate::model_registry::RuntimeModels;
use crate::modes::plan_act::{ExecutionMode, ModePolicy};
use crate::ollama::client::{AssistantReply, OllamaClient, should_use_native_tool_calls};
use crate::ollama::xml_fallback::ToolCall;
use crate::repo_graph::{
    BuildOptions as RepoGraphBuildOptions, BuildOutcome, RepoGraph, RepoGraphError,
    build_repo_graph,
};
use crate::safety::path_guard::resolve_user_path;
use crate::session::compact::{
    approximate_token_count, compact_messages, compact_messages_with_strategy,
};
use crate::session::store::{ConversationMessage, SessionSnapshot, SessionStore};
use crate::stdin_prompt;
use crate::system_prompt::build_system_prompt;
use crate::tools::registry::{ToolContext, ToolRegistry};

// Issue #646: artifact ownership classification. Module is intentionally
// *not* re-exported (DR3-001) — `turn.rs` and `task_contract.rs` are the
// only in-crate consumers via `super::artifact_ownership::*`.
mod artifact_ownership;
// Issue #659: `ArtifactLedger` SSOT for current-turn artifact observations.
// Module is intentionally *not* re-exported (DR3-001) — `turn.rs` is the
// only in-crate consumer (Phase 2) via `super::artifact_ledger::*`.
mod artifact_ledger;
// Issue #659 Phase 2: in-crate `#[cfg(test)]` E2E tests for the Agent-level
// ArtifactLedger wiring (per-turn reset, seed call sites, dual-source
// divergence emit). Production binary does not include this module
// (DR3-001 / CB-001 fix pattern shared with `safe_stop_e2e_tests`).
#[cfg(test)]
mod artifact_ledger_phase2_tests;
// Issue #659 Phase 3: in-crate `#[cfg(test)]` tests for the
// legacy-helper internal-implementation switch
// (`task_contract_artifact_states` / `owned_test_artifacts_for_verifier`
// / `turn_edited_relative_paths` view). Production binary does not include
// this module (DR3-001 / CB-001 fix pattern shared with `safe_stop_e2e_tests`).
#[cfg(test)]
mod artifact_ledger_phase3_tests;
// Issue #659 Phase 4: in-crate `#[cfg(test)]` tests for the production-
// aligned test seam `seed_artifact_ledger_repo_edit_for_test` that
// replaces the legacy `seed_turn_edited_relative_path_for_test` (Issue
// #654 / CB-005). Production binary does not include this module
// (DR3-001 / CB-001 fix pattern shared with `safe_stop_e2e_tests`).
#[cfg(test)]
mod artifact_ledger_phase4_tests;
// Issue #659 Phase 5: in-crate `#[cfg(test)]` tests pinning the design-
// policy Section 9 acceptance checklist items not already covered by
// Phase 1-4 (nested test path Owned classification, ArtifactState
// signature, pub(super) public-API surface, mask_payload_inplace
// final-defence, event_recorded each-time emit, projection signature
// anchor). Production binary does not include this module (DR3-001 /
// CB-001 fix pattern shared with `safe_stop_e2e_tests`).
#[cfg(test)]
mod artifact_ledger_phase5_tests;
// Issue #652: `ArtifactCompletionJob` + role-specific retry budget +
// `ArtifactAttemptOutcome` 4-variant taxonomy +
// `ArtifactCompletionFailureSnapshot` for #654. Module is intentionally
// *not* re-exported (DR3-001) — `turn.rs` is the only behavioral
// in-crate consumer via `super::artifact_completion_job::*`.
mod artifact_completion_job;
pub(crate) mod auto_promote;
mod auto_test;
pub mod commands;
// Issue #606: pure data model for post-hoc completion-evidence observation.
// Internal API surfaced via `super::completion_evidence::` from `protocol.rs`,
// `success.rs`, and `turn.rs`. Two pure helpers are re-exported below with
// `#[doc(hidden)] pub` so integration tests (`tests/completion_evidence_smoke.rs`)
// can verify the SSOT classifier and the DR4-002 security gate without
// reaching into private module state — see `is_completion_verifier_command`
// / `classify_repo_edit_path` in `loop_run::completion_evidence`.
pub(crate) mod completion_evidence;
mod deterministic;
pub(crate) mod feedback_kind_confirm;
mod footer;
mod interrupt;
mod lifecycle;
pub mod photon_user_feedback;
// Issue #639: ProjectVerifier capability. Module is intentionally *not*
// re-exported (DR3-001) — `turn.rs` is the only in-crate consumer via
// `super::project_verifier::*`.
mod project_verifier;
mod protocol;
mod quality;
pub(crate) mod quality_confirm;
pub(crate) mod reminder;
mod repair_job;
// Issue #653: `RepairAttemptOutcome` lifecycle ledger. Module is intentionally
// *not* re-exported (DR3-001) — `turn.rs` and `repair_job.rs` are the only
// in-crate consumers via `super::repair_attempt_outcome::*`.
mod repair_attempt_outcome;
// Issue #635: deterministic RequiredBehaviorContract extractor. Module is
// intentionally *not* re-exported (DR3-001) — `task_contract.rs` is the only
// in-crate consumer via `super::required_behavior::*`.
mod required_behavior;
// Issue #647 (Phase A.1): semantic repair planning — bounded failure-report
// schema and deterministic cluster-key generation. Module is intentionally
// *not* re-exported (DR3-001) — future consumers (`turn.rs`, `repair_job.rs`)
// reach in via `super::semantic_failure::*`.
mod semantic_failure;
pub mod slash_commands;
// Issue #647 (Phase A.2): spec-authority enum + tie-break scoring + test/impl
// weakening detectors. Module is intentionally *not* re-exported (DR3-001) —
// future consumers (`turn.rs`, `repair_job.rs`) reach in via
// `super::spec_authority::*`.
mod spec_authority;
mod spinner;
mod success;
// Issue #654 (CB-001): in-crate `#[cfg(test)]` E2E suite for the bounded
// safe-stop-report pipeline. The seam set above is `#[cfg(test)]`-only, so
// the cross-crate `tests/bounded_safe_stop_report_e2e.rs` integration file
// has been migrated here to keep the seams unreachable from release builds.
#[cfg(test)]
mod safe_stop_e2e_tests;
mod summary;
mod task_contract;
// Issue #646: task workspace scope detection. Module is intentionally
// *not* re-exported (DR3-001) — `turn.rs` is the only in-crate consumer
// via `super::task_workspace_scope::*`.
mod task_workspace_scope;
mod tester;
mod turn;
pub(crate) mod verifier_skill;
pub(crate) mod work_mode_confirm;

// Public re-exports so `lib.rs::run_cli` can hand a `FooterHandle` into
// `Agent::new` and own the matching `FooterLease` for its scope (issue #430).
pub use footer::{FooterHandle, FooterLease};

// Re-export env helpers from `turn` so `src/tui/markdown.rs` can reuse the
// existing POSIX-compliant NO_COLOR and UTF-8 locale logic without duplicating
// it. `mod turn;` itself stays private; only these two fns leak out (issue #431).
pub(crate) use turn::{no_color_requested, unicode_supported};

// Issue #453: expose the precaution prompt selector so integration tests in
// `tests/` (and any future callers) can validate the Act-mode prompt
// selection pipeline without requiring a live Ollama call.
pub use turn::select_precautions_for_prompt;

// Issue #556: expose pure helper functions so `tests/photon_turn_hook_smoke.rs`
// can verify truncation and injection-message building without constructing
// a full Agent (Ollama-free).
pub use turn::{
    MAX_PHOTON_CONTEXT_PACK_PROMPT_BYTES, build_photon_injection_message,
    truncate_photon_context_pack,
};

// Issue #601: expose the Case F `outcome_detail` static-allowlist literal so
// `tests/photon_evaluate_signal_smoke.rs` can grep / assert against the same
// SSOT used by production. `mod turn;` is private, so a `pub const` alone is
// not reachable from integration tests; this re-export widens visibility.
pub use turn::PHOTON_OUTCOME_DETAIL_NO_PROGRESS_DESPITE_INJECT;

// Issue #594: expose the /photon-why message builder so
// `tests/photon_provenance_smoke.rs` can verify the 7 status branches and the
// per-seed rendering without constructing a full Agent (Ollama-free).
pub use commands::build_photon_why_message;

// Issue #606: integration-test-only seams for the completion-evidence
// pipeline. Marked `#[doc(hidden)]` so they do not appear in the public docs
// but are reachable from `tests/completion_evidence_smoke.rs`.
//
// `classify_repo_edit_path_for_test` exposes the SSOT path classifier so
// integration tests can pin the DR1-001 ordering invariant (`.mdx → Docs`)
// from outside the crate without reaching into `pub(crate)` types.
//
// `is_completion_verifier_command_for_test` exposes the DR4-002 security
// gate so tests can pin that shell control operators reject a verifier
// invocation without spinning up a full Agent.
#[doc(hidden)]
pub fn classify_repo_edit_path_for_test(path: &std::path::Path) -> &'static str {
    use completion_evidence::RepoEditCategory;
    match completion_evidence::classify_repo_edit_path(path) {
        RepoEditCategory::Impl => "impl",
        RepoEditCategory::Test => "test",
        RepoEditCategory::Docs => "docs",
        RepoEditCategory::Setup => "setup",
        RepoEditCategory::Other => "other",
    }
}

#[doc(hidden)]
pub fn is_completion_verifier_command_for_test(command: &str) -> bool {
    completion_evidence::is_completion_verifier_command(command)
}

/// Issue #607: integration-test seam exposing the pure projection from a
/// `BashExecutionOutcome` to an optional `VerifierExitZero` evidence record.
/// Returns a tuple `(promoted, masked_command, class_label)` where:
///   * `promoted` — true if the outcome would be pushed into `EvidenceSet`
///   * `masked_command` — the stored (mask-applied) command when promoted
///   * `class_label` — the snake_case `BashCommandClass` label
/// Used by `tests/completion_evidence_env_setup.rs` to pin BP-04 / BP-06
/// without spinning up a full Agent.
#[doc(hidden)]
pub fn build_verifier_exit_zero_evidence_for_test(
    outcome: &crate::tools::bash::BashExecutionOutcome,
) -> Option<(String, &'static str)> {
    let evidence = turn::build_verifier_exit_zero_evidence(outcome)?;
    match evidence {
        completion_evidence::CompletionEvidence::VerifierExitZero { class, command, .. } => {
            Some((command, class.as_str()))
        }
        _ => None,
    }
}

// Issue #465 / Phase 5: expose Reminder types needed by tests/agent_skill_registry_smoke.rs
// (E2E tests live outside the crate so `pub(crate) mod reminder` cannot be reached
// directly). Production code paths continue to use `super::reminder::...`; these
// `pub use` lines only widen the visibility for integration tests.
pub use reminder::{
    FailureReason as ReminderFailureReason, ReminderInputs, ReminderOutcome,
    SkipReason as ReminderSkipReason,
};

// Issue #459 / Phase 2: expose the Tester Skill orchestrator + types so the
// E2E suite under `tests/tester_skill_smoke.rs` can drive the closure-DI
// boundary directly (LLM call + Bash runner + approver) without an Ollama /
// cargo / node / python dependency. Production code paths in `turn.rs`
// continue to call these via `super::tester::...` — these `pub use` lines
// only widen the visibility for integration tests.
pub use tester::{
    AbortReason as TesterAbortReason, ApprovalMode as TesterApprovalMode,
    MAX_GENERATED_TESTS_PER_TURN as TESTER_MAX_GENERATED_TESTS_PER_TURN,
    MAX_TESTER_LLM_REPLY_BYTES, NotInvokedReason as TesterNotInvokedReason, TesterCandidate,
    TesterLlmError, TesterOutcome, TesterPrompt, TesterRun, check_invocation_gate as tester_gate,
    run_tester_with_strategy, tester_disabled,
};

// Issue #472: expose env-gate helper so `tests/eval_harness_smoke.rs` can
// drive the closure-DI boundary (ANVIL_NO_AUTO_TEST) without live Ollama.
pub use auto_test::auto_test_disabled;

// Issue #592: expose the photon user-feedback adapter so the E2E smoke suite
// under `tests/photon_user_feedback_smoke.rs` can drive the four entry-points
// directly without a live Ollama dependency.
pub use photon_user_feedback::{
    handle_correct as photon_handle_correct, handle_rule as photon_handle_rule,
    handle_thumbs_down as photon_handle_thumbs_down, handle_thumbs_up as photon_handle_thumbs_up,
    photon_feedback_disabled,
};

/// Issue #592 — test-only helper for `tests/photon_user_feedback_smoke.rs` to
/// simulate "photon injected these IDs in the previous turn" without spinning
/// up a full context-pack round-trip. Production code populates the field via
/// `invoke_photon_context_pack` (turn.rs); this seam is needed because the
/// adapter is tested end-to-end at the Agent boundary.
pub fn set_last_injected_for_test(agent: &mut Agent, ids: Vec<String>) {
    let current = agent.current_turn_index;
    agent.last_injected_summary_ids = ids;
    agent.last_injected_summary_turn_index = Some(current);
}

// Issue #601 — test-only accessors that expose the `SessionSnapshot` inside
// the `Agent` so `tests/photon_evaluate_signal_smoke.rs` (NPS-03, NPS-06)
// can force mode state / read post-turn counters without spinning up a full
// production session API. These are intentionally narrow (return &/&mut
// SessionSnapshot) and live alongside the existing `set_last_injected_for_test`
// seam.

#[doc(hidden)]
pub fn agent_session_ref(agent: &Agent) -> &SessionSnapshot {
    &agent.session
}

#[doc(hidden)]
pub fn agent_session_mut(agent: &mut Agent) -> &mut SessionSnapshot {
    &mut agent.session
}

impl Agent {
    /// Issue #601 test seam: read-only access to the inner SessionSnapshot.
    /// `pub fn` (not `pub(crate)`) so integration tests can verify per-turn
    /// counters post-process_line. Production code uses `self.session` direct.
    #[doc(hidden)]
    pub fn session_ref(&self) -> &SessionSnapshot {
        &self.session
    }

    /// Issue #601 test seam: mutable access to the inner SessionSnapshot.
    /// Used to force WorkMode / ExecutionMode in NPS-03 / NPS-06 tests
    /// without driving a real LLM classify cycle.
    #[doc(hidden)]
    pub fn session_mut(&mut self) -> &mut SessionSnapshot {
        &mut self.session
    }
}

// Issue #654 — in-crate `#[cfg(test)]` test seams for the bounded
// safe-stop-report suite.
//
// CB-001 (Codex review, Issue #654): the original implementation exposed
// these as `#[doc(hidden)] pub fn`, but `#[doc(hidden)]` is only a rustdoc
// hint — `pub fn` widens access for any external crate. The seams are now
// gated by `#[cfg(test)]` so they exist exclusively in the `cargo test` /
// `#[cfg(test)] mod` graph and are dropped entirely from release builds.
// The accompanying integration tests have been relocated to
// `src/agent/loop_run/safe_stop_e2e_tests.rs` (in-crate `#[cfg(test)] mod`)
// so they can still reach the seams while keeping production access closed.
//
// The seams are intentionally narrow and only accept what the corresponding
// wired emit point supplies in production: `current_role` + `expected_target`
// for `artifact_completion_failed`, no arguments for the four verifier-driven
// paths.
//
// Production code MUST NOT call these; the wired emit points inside `turn.rs`
// are the only legitimate callers in production.

/// Issue #654 (E.1) test seam: invoke the `diagnostic_target_missing` emit
/// shell. In production this fires from `record_verifier_diagnostic_unavailable`.
#[cfg(test)]
pub(crate) fn emit_safe_stop_report_diagnostic_target_missing_for_test(agent: &mut Agent) {
    if agent.repair_job.is_none() {
        // The shell exits early when `repair_job` is None; preserve the
        // production guarantee by ensuring callers see the same no-op path.
        agent.repair_job = Some(repair_job::RepairJob::empty_synthetic());
    }
    agent.emit_safe_stop_report_for_diagnostic_target_missing();
}

/// Issue #654 (E.3) test seam: invoke the `verifier_failed_safe_stop` emit
/// shell. In production this fires at `drive_task_contract_verifier`'s
/// attempt-limit branch.
#[cfg(test)]
pub(crate) fn emit_safe_stop_report_verifier_failed_safe_stop_for_test(agent: &mut Agent) {
    if agent.repair_job.is_none() {
        agent.repair_job = Some(repair_job::RepairJob::empty_synthetic());
    }
    agent.emit_safe_stop_report_for_verifier_failed_safe_stop();
}

/// Issue #654 (E.4) test seam: invoke the `verifier_weak` emit shell. In
/// production this fires when `VerifierRepairPassOutcome::Invalid` exhausts
/// the controller repair-pass retry budget.
#[cfg(test)]
pub(crate) fn emit_safe_stop_report_verifier_weak_for_test(agent: &mut Agent) {
    if agent.repair_job.is_none() {
        agent.repair_job = Some(repair_job::RepairJob::empty_synthetic());
    }
    agent.emit_safe_stop_report_for_verifier_weak();
}

/// Issue #654 (E.5) test seam: invoke the `verifier_missing` emit shell
/// (`FromMissingVerifier` builder).
#[cfg(test)]
pub(crate) fn emit_safe_stop_report_verifier_missing_for_test(agent: &mut Agent) {
    if agent.missing_verifier_job.is_none() {
        agent.missing_verifier_job = Some(repair_job::MissingVerifierJob::new(1, 0));
    }
    agent.emit_safe_stop_report_for_verifier_missing();
}

/// Issue #654 (E.2) test seam: invoke the `artifact_completion_failed` emit
/// shell. This path can fire without a verifier-driven `RepairJob`, mirroring
/// the production wiring at the role-specific retry budget exit points.
#[cfg(test)]
pub(crate) fn emit_safe_stop_report_artifact_completion_failed_for_test(
    agent: &mut Agent,
    role_label: &str,
    expected_target: Option<String>,
) {
    let role = match role_label {
        "test" => task_contract::ArtifactRole::Test,
        "usage_docs" => task_contract::ArtifactRole::UsageDocs,
        "setup" => task_contract::ArtifactRole::Setup,
        _ => task_contract::ArtifactRole::Implementation,
    };
    agent.emit_safe_stop_report_for_artifact_completion_failed(role, expected_target);
}

/// Issue #654 test seam: clear the per-turn dedup marker so a test can verify
/// the reset path (re-emit after `handle_user_message`-style clear).
#[cfg(test)]
pub(crate) fn clear_safe_stop_report_dedup_for_test(agent: &mut Agent) {
    agent.safe_stop_report_emitted.clear();
}

/// Issue #659 (Phase 4 / Task 4.1) test seam: simulate "agent successfully
/// edited this workspace-relative path during the current turn" by routing
/// through the **production** RepoEdit write-through adapter
/// (`Agent::seed_artifact_ledger_repo_edit`). Replaces the legacy Issue
/// #654 / CB-005 `seed_turn_edited_relative_paths_for_test` seam.
///
/// The seam:
/// 1. Infers the artifact role from the path via the production
///    `classify_repo_edit_path` → `role_from_repo_edit` chain. Paths that
///    classify into `RepoEditCategory::Other` (no role) only update the
///    legacy `turn_edited_relative_paths` HashSet — they do **not**
///    produce a ledger event, matching the production gate in
///    `observe_evidence_from_repo_edit`.
/// 2. Records the event into both the legacy set and the ArtifactLedger
///    SSOT in the same instruction (write-through divergence anchor).
/// 3. Goes through the `ArtifactLedger::record_repo_edit_event` admission
///    pipeline (`classify_ownership` workspace-relative / `..` / control-
///    char / symlink-escape / ignored-top-dir guards + role re-confirmation).
///
/// **Signature contract (DR3-001 / private_interfaces)**: only `&mut Agent`
/// and `String` cross the `pub(crate)` boundary. `ArtifactRole` /
/// `ArtifactOrigin` / `LedgerAdmissionContext` etc. remain `pub(super)` to
/// the `loop_run` module and are NOT exposed by this signature.
#[cfg(test)]
pub(crate) fn seed_artifact_ledger_repo_edit_for_test(agent: &mut Agent, path: String) {
    // Legacy set update — kept in lockstep with the ledger seed for the
    // Phase 4 adapter contract. Performed unconditionally so callers that
    // want to assert "non-test paths still pass through the legacy filter"
    // (safe_stop_e2e_tests::from_missing_verifier_excludes_*) see the
    // same set membership the old seam produced.
    agent.turn_edited_relative_paths.insert(path.clone());

    // Role inference via the production classifier chain. Paths that map
    // to `RepoEditCategory::Other` (no artifact role) do not get a ledger
    // event — same gate as `observe_evidence_from_repo_edit` (turn.rs).
    let category =
        crate::agent::loop_run::completion_evidence::classify_repo_edit_path(Path::new(&path));
    let Some(role) = crate::agent::loop_run::task_contract::role_from_repo_edit(category) else {
        return;
    };

    let scope = agent.current_workspace_scope();
    agent.seed_artifact_ledger_repo_edit(&path, role, &scope);
}

// Issue #576: expose WorkMode second-pass confirmation adapter surface so
// `tests/work_mode_confirm_smoke.rs` can drive `run_work_mode_confirm_with_strategy`
// (the closure-DI boundary) without an Ollama dependency. Production paths in
// `turn.rs` / `commands.rs` continue to call these via `super::work_mode_confirm::...`;
// these `pub use` lines only widen the visibility for integration tests.
pub use work_mode_confirm::{
    ParseStatus as WorkModeConfirmParseStatus, WORK_MODE_CONFIRM_PROMPT_INPUT_MAX_BYTES,
    WORK_MODE_CONFIRM_REASON_MAX_BYTES, WORK_MODE_CONFIRM_RESPONSE_MAX_BYTES,
    WORK_MODE_CONFIRM_TIMEOUT_SECS, WorkModeConfirmInputs, WorkModeConfirmOutcome,
    WorkModeConfirmation, WorkModeConfirmationSource, WorkModeFallbackReason, WorkModeSkipReason,
    build_work_mode_confirm_log_payload, build_work_mode_confirm_prompt,
    first_pass_has_explicit_no_edit_signal, parse_second_pass_response,
    run_work_mode_confirm_with_strategy, work_mode_confirm_disabled,
};

// Issue #579: expose FeedbackKind second-pass confirmation adapter surface so
// `tests/feedback_kind_confirm_smoke.rs` can drive
// `run_feedback_kind_confirm_with_strategy` (the closure-DI boundary)
// without an Ollama dependency. Production paths in `success.rs` / the
// `Agent::classify_with_feedback_confirm` wrapper continue to call these via
// `super::feedback_kind_confirm::...`; these `pub use` lines only widen the
// visibility for integration tests.
pub use feedback_kind_confirm::{
    FEEDBACK_KIND_CONFIRM_PROMPT_INPUT_MAX_BYTES, FEEDBACK_KIND_CONFIRM_REASON_MAX_BYTES,
    FEEDBACK_KIND_CONFIRM_RESPONSE_MAX_BYTES, FEEDBACK_KIND_CONFIRM_TIMEOUT_SECS,
    FeedbackKindConfirmInputs, FeedbackKindConfirmOutcome, FeedbackKindConfirmation,
    FeedbackKindConfirmationSource, FeedbackKindFallbackReason, FeedbackKindSkipReason,
    ParseStatus as FeedbackKindConfirmParseStatus, build_feedback_kind_confirm_log_payload,
    build_feedback_kind_confirm_prompt, feedback_kind_confirm_disabled,
    parse_second_pass_response as parse_feedback_kind_second_pass_response,
    run_feedback_kind_confirm_with_strategy, should_request_feedback_confirmation,
};

// Issue #580: expose Quality second-pass confirmation surface so
// `tests/quality_confirm_smoke.rs` can drive `run_quality_confirm_with_strategy`
// (the closure-DI boundary) without an Ollama dependency. Production paths in
// `turn.rs` continue to call these via `super::quality_confirm::...`; these
// `pub use` lines only widen the visibility for integration tests.
pub use quality::{
    QualityEarlyFailReason, QualityFirstPassGate, QualityFirstPassObservation,
    quality_first_pass_observation,
};
pub use quality_confirm::{
    ParseStatus as QualityConfirmParseStatus, QUALITY_CONFIRM_PROMPT_INPUT_MAX_BYTES,
    QUALITY_CONFIRM_REASON_MAX_BYTES, QUALITY_CONFIRM_RESPONSE_MAX_BYTES,
    QUALITY_CONFIRM_STRONG_THRESHOLD, QUALITY_CONFIRM_TIMEOUT_SECS, QualityConfirmFallbackReason,
    QualityConfirmInputs, QualityConfirmOutcome, QualityConfirmSkipReason, QualityConfirmation,
    QualityConfirmationSource, build_quality_confirm_log_payload, build_quality_confirm_prompt,
    parse_second_pass_response as parse_quality_second_pass_response, quality_confirm_disabled,
    run_quality_confirm_with_strategy, should_request_quality_confirmation,
};

/// Issue #634: Python 系特化 fallback (FastAPI / Python CLI / FizzBuzz scaffold) の
/// AND gate。`ModePolicy::allow_python_deterministic_fallback` (既存) と
/// `Config::specialized_template_fallback_enabled()` (Issue #634) の双方を満たす
/// 場合に true。Docs ブランチ (`allow_docs_deterministic_fallback`) は本 Issue で
/// touch しないため別経路で評価する。
pub(crate) fn policy_allows_python_specialized_fallback(policy: &ModePolicy, cfg: &Config) -> bool {
    policy.allow_python_deterministic_fallback && cfg.specialized_template_fallback_enabled()
}

const DEFAULT_KEEP_TAIL: usize = 24;
const LATE_TURN_KEEP_TAIL: usize = 12;

pub enum AgentEvent {
    Continue(Option<String>),
    Exit,
}

pub struct Agent {
    config: Config,
    models: RuntimeModels,
    client: OllamaClient,
    session_store: SessionStore,
    session: SessionSnapshot,
    work_root: PathBuf,
    native_tools_enabled: bool,
    plan_model_override: Option<String>,
    tool_registry: ToolRegistry,
    repo_context_cache: Option<RepoContextCache>,
    /// Fixed footer handle. Phase A: always disabled (no-op); the handle
    /// shape is plumbed now so Phase B-D can attach `publish_*` calls
    /// without re-touching `Agent::new` callers (issue #430).
    #[allow(dead_code)]
    footer: FooterHandle,
    /// Per-turn cap for the Reminder Sidecar (#452). Reset at the top of every
    /// `handle_user_message`, set to `true` only when an actual sidecar call
    /// was attempted (Completed/Failed); Skipped does not consume the cap.
    reminder_called_this_turn: bool,
    /// Issue #459: per-turn cap for the Tester Skill. Reset at the top of every
    /// `handle_user_message`. Consumed only when a Tester smoke run actually
    /// dispatched (Recorded / Aborted); NotInvoked does not consume the cap.
    pub(super) tester_called_this_turn: bool,
    /// Issue #576: per-turn cap for the WorkMode second-pass confirmation.
    /// Reset at the top of every `process_line` (DR2-002), **not**
    /// `handle_user_message` — `maybe_auto_plan_prompt` runs before
    /// `handle_user_message` and is a valid second-pass call site.
    ///
    /// CB-001 (Issue #576 follow-up): `classify_with_confirmation` reads this
    /// flag to decide whether to overwrite the previously-resolved
    /// `session.mode_state.work_mode` with a fresh first-pass result. While
    /// the cap is consumed (`true`), the value already in the session is the
    /// authoritative resolved mode and must not be clobbered.
    pub(super) work_mode_confirm_called_this_turn: bool,
    /// Issue #579: per-turn cap for the FeedbackKind second-pass confirmation.
    /// Reset at the top of every `run_turn` (DR2-005), consumed only when the
    /// orchestrator actually dispatches to the sidecar LLM (i.e.
    /// `model.is_some()`); Skip / `Fallback(SidecarUnavailable)` paths do not
    /// consume the cap. Field name mirrors `work_mode_confirm_called_this_turn`
    /// so future readers can spot the symmetry.
    pub(super) feedback_kind_confirm_called_this_turn: bool,
    /// Issue #580: per-turn cap for the Quality-gate second-pass confirmation.
    /// Reset at the top of every `run_actor_loop` (same block as
    /// `feedback_kind_confirm_called_this_turn`). Consumed only when the
    /// orchestrator actually dispatches to the sidecar LLM (`model.is_some()`).
    pub(super) quality_confirm_called_this_turn: bool,
    /// Issue #580: per-turn memoization cache for the Quality-gate
    /// second-pass adapter.
    ///
    /// **Why this is different from #576 / #579**: Quality-gate is the only
    /// adapter that may be reached from up to 5 callsites in the same turn
    /// (`accepted_repo_change_quality_issue` and
    /// `accepted_repo_change_polish_target` each invoke us from multiple
    /// host code paths). After the per-turn cap is consumed, subsequent
    /// callsites with the same `(request, content)` would otherwise revert
    /// to first-pass only, losing turn-local consistency. With memoization
    /// the cached `QualityConfirmation` is returned. #576 (WorkMode) has 2
    /// callsites with no realistic overlap; #579 (FeedbackKind) is invoked
    /// exactly once per post-loop hook.
    ///
    /// Key: `DefaultHasher::finish()` of `(request, full_content)`
    /// — collisions are per-turn-local with negligible blast radius.
    /// Value: the `QualityConfirmation` returned to the caller, preserving
    /// `source` / `reason` so cache-hit log emission stays faithful.
    /// Reset: top of every `run_actor_loop` together with
    /// `quality_confirm_called_this_turn`.
    pub(super) last_quality_confirm_result: Option<(u64, quality_confirm::QualityConfirmation)>,
    /// Issue #456: tracks whether `compute_anvil_score` has already run for
    /// the current turn. Reset at the top of every `handle_user_message`,
    /// flipped to `true` after the post-loop compute writes
    /// `session.last_anvil_score`. Used by `maybe_invoke_reminder` to pick
    /// between `AnvilScoreSnapshot::PreviousTurn` (iteration-internal hook,
    /// score is the previous turn's persisted value) and
    /// `AnvilScoreSnapshot::CurrentTurn` (post-loop hook, score is the value
    /// just computed for this turn).
    pub(super) anvil_score_computed_this_turn: bool,
    /// Issue #466: skill registry. ReminderSkill (#465) は trait 実装を直接呼び出す
    /// 既存経路を維持し、本 Issue では VerifierSkill を post-loop fence で invoke
    /// するために registry instance を hold する。dynamic skill loading は許可しない
    /// (DR4-003): static registration only.
    #[allow(dead_code)]
    pub(super) skill_registry: crate::agent::skills::SkillRegistry,
    /// Issue #468: session-lifetime cache of the repository structure graph.
    /// Built once at `Agent::new` (blocking) and never mutated afterward.
    /// Not serialized — RepoGraph state lives outside `SessionSnapshot` to
    /// avoid bloating the session JSON (DR1-004 / S3-006).
    #[allow(dead_code)]
    pub(super) repo_graph: Option<Arc<RepoGraph>>,
    /// Issue #471: per-turn cache of the last case retrieval summary for the
    /// eval log. Set by `try_inject_case_retrieval_message` when a Completed
    /// outcome is obtained. Reset at the top of `run_actor_loop`.
    pub(super) last_case_retrieval_summary: Option<crate::session::eval_log::CaseRetrievalSummary>,
    /// Issue #473: monotonically increasing counter (1-based) for the current
    /// session turn. Incremented at the top of `handle_user_message` before any
    /// per-turn logic runs. Used as a join key in `agent.reminder.completed` and
    /// `agent.anvil_score.computed` log events for dataset export.
    pub(super) current_turn_index: usize,
    /// Issue #554: optional Photon sidecar client. `None` when photon_url is
    /// unset, URL validation fails, config.offline is true, or client init fails.
    pub(super) photon: Option<crate::photon::PhotonClient>,
    /// Issue #556: masked context_pack response for the current turn.
    /// Reset to None at the top of every `handle_user_message`.
    /// Set in `run_turn` pre-hook (shadow mode=false only).
    /// Cleared after `run_actor_loop` returns.
    pub(super) photon_context_pack_response: Option<String>,
    /// Issue #558: context_pack_id extracted in `invoke_photon_context_pack`.
    /// Extracted regardless of shadow mode. Reset in `handle_user_message`.
    /// NOT reset in run_turn post-loop (must survive until invoke_photon_evaluate).
    pub(super) last_context_pack_id: Option<String>,
    /// Issue #558: photon eval summary set by `invoke_photon_evaluate`.
    /// Consumed by `build_eval_record` via `.take()`. Reset at run_turn head.
    pub(super) last_photon_eval_summary: Option<crate::session::eval_log::PhotonEvalSummary>,
    /// Live injection: number of context_pack items actually rendered into the
    /// prompt this turn. Set in `invoke_photon_context_pack` after rendering.
    /// Used by `invoke_photon_evaluate` for `adoption_status`/`items_adopted_count`.
    /// Reset at run_turn head.
    pub(super) last_photon_adopted_items: usize,
    /// Issue #591 (AS-01): sanitized `summary_id` of every item actually
    /// emitted into the prompt this turn (post-total-cap). Populated by
    /// `invoke_photon_context_pack` from `RenderStats.adopted_summary_ids`
    /// and consumed by `invoke_photon_evaluate` (which re-runs
    /// `sanitize_summary_id` defensively and applies
    /// `MAX_PHOTON_EVAL_ADOPTED_IDS=32`).
    ///
    /// **Reset at `handle_user_message` head, NOT at `run_actor_loop` head**
    /// (AS-01 / 設計判断 #2). The evaluate hook runs *after* the actor loop
    /// returns; resetting at `run_actor_loop` would clobber the ids the
    /// evaluate hook needs to read.
    pub(super) last_adopted_summary_ids: Vec<String>,
    /// Issue #594: per-turn provenance summary cache for `/photon-why`.
    /// Cleared in `handle_user_message` before `invoke_photon_context_pack`
    /// runs. Invariant (PV-01): when populated, its length equals
    /// `RenderStats.items_adopted` for the same turn.
    pub(super) last_injected_seed_provenance: Vec<crate::photon::provenance::SeedProvenanceSummary>,
    /// Issue #594: state machine for `/photon-why` dispatch. Intentionally
    /// NOT reset in `handle_user_message` so a `/plan` slash command between
    /// turns can still surface the last Act turn's lineage (S7-002).
    pub(super) last_photon_context_pack_status: PhotonContextPackStatus,
    /// Issue #592: sanitized summary IDs of the photon items that were
    /// actually injected into the previous prompt build. Populated by
    /// `invoke_photon_context_pack` from `RenderStats.adopted_summary_ids`.
    /// Consumed by the `/photon-thumbs-{up,down}` adapter to attribute user
    /// feedback to the actual injection.
    ///
    /// NOT reset in `handle_user_message`: the thumbs adapter runs on the
    /// turn AFTER the injection, so the field must survive the inter-turn
    /// boundary. Cleared on any early-return path inside
    /// `invoke_photon_context_pack` (Plan / offline / shadow / canary skip).
    pub(super) last_injected_summary_ids: Vec<String>,
    /// Issue #592: turn index when `last_injected_summary_ids` was populated.
    /// Used by the thumbs adapter to enforce a turn-staleness check (a thumbs
    /// command must arrive on the turn immediately after the injection).
    pub(super) last_injected_summary_turn_index: Option<usize>,
    /// Issue #592: per-turn cap for the photon user-feedback adapter
    /// (`/photon-thumbs-{up,down}`). Reset at the top of every `process_line`
    /// (DR2-002), consumed only when an actual `/v1/evaluate` call was
    /// attempted (i.e. inject was present and we shipped the feedback event).
    pub(super) photon_user_feedback_called_this_turn: bool,
    /// Issue #604 (DR2-006 / Issue §AP-12 S7-001): turn-local cache of the
    /// most recent `invoke_photon_auto_promote` outcome. Set by Task 5.1
    /// (`invoke_photon_auto_promote` hook) at the end of every Phase A/B
    /// path, and read by `build_eval_record(...)` to attach to
    /// `EvalRecord.auto_promote` (Task 4.1).
    ///
    /// **Distinct from `SessionSnapshot.auto_promote_called_this_turn`**:
    /// the snapshot flag is the per-turn cap guard (twice-call suppression)
    /// while this field is the EvalRecord persistence carrier. Both reset
    /// at the same `handle_user_message` head (Task 5.2 wiring) but are
    /// independent variables (Issue S7-001 SSOT).
    pub(super) last_auto_promote_outcome:
        Option<crate::agent::loop_run::auto_promote::AutoPromoteOutcomeSummary>,
    /// Issue #606 (DR1-007 / #D-06): per-turn accumulator of post-hoc
    /// completion-evidence observations. Push-only `Vec` populated by the
    /// Bash hook (`VerifierExitZero`) and the Edit/Write hook (`RepoEdit`)
    /// in `turn.rs::execute_tool_call`, consumed by
    /// `success.rs::run_post_loop_success_verifier` via
    /// `ProtocolKind::evidence_set_satisfies`. Reset at the head of
    /// `run_actor_loop` alongside the existing per-turn caps so multi-turn
    /// sessions never observe stale evidence. **Not serialized** — lives
    /// on `Agent` instead of `SessionSnapshot` because the OR-satisfaction
    /// is evaluated within the same turn the evidence was observed.
    pub(super) evidence_set_this_turn: completion_evidence::EvidenceSet,
    /// Issue #618: task-contract-specific evidence. This mirrors
    /// `evidence_set_this_turn` only for artifacts that are allowed to satisfy
    /// the currently active artifact recovery target. Generic protocol
    /// satisfaction still uses `evidence_set_this_turn` so unrelated repo edits
    /// remain visible as progress without completing the required artifact.
    task_contract_evidence_set_this_turn: completion_evidence::EvidenceSet,
    /// Issue #618 / #622: actor-loop-local artifact recovery target. Reset at
    /// the start of every user turn; while populated, task-contract recovery
    /// can constrain file tools to this artifact without forcing focused-edit
    /// mode immediately.
    ///
    /// Issue #652 (DR1-003 / design judgement #2): `ArtifactCompletionJob` is
    /// the new SSOT for the in-flight artifact completion task. While a job
    /// exists in `artifact_completion_job` and its role is `Test` (the only
    /// role driven by `requires_test_execution()` in this Issue), the
    /// `current_artifact_recovery_target` value here is kept in sync by
    /// writing the projection from `ArtifactCompletionJob::
    /// projection_recovery_target()` at the same call sites — there is no
    /// divergent independent assignment outside of `set_artifact_recovery_target_from_hint`.
    current_artifact_recovery_target: Option<crate::agent::loop_run::task_contract::RecoveryTarget>,
    /// Issue #652: in-flight artifact-completion job (role-specific retry
    /// budget, sanitized attempt history, wrong-target / no-tool / prose-only
    /// / role-policy-violation taxonomy). Reset at every
    /// `handle_user_message` head (per-turn cap pattern, DR3-003); populated
    /// by `turn.rs` when a `Test` artifact is the missing required role.
    /// Module is private (DR3-001) — `turn.rs` is the only behavioral
    /// in-crate consumer.
    artifact_completion_job:
        Option<crate::agent::loop_run::artifact_completion_job::ArtifactCompletionJob>,
    /// Issue #652 CB-002: per-turn flag set the first time
    /// `record_artifact_completion_attempt` transitions the active job to
    /// `Exhausted`. Read by the actor loop right after each `execute_tool_call`
    /// to terminate with `MissingRepoEdits` so a wrong-target rejection at
    /// the 3rd attempt does not silently continue the loop. Also gates
    /// `maybe_emit_artifact_completion_failed_diagnostic` to a single emission
    /// per turn (CB-002 duplicate diagnostic suppression). Reset at every
    /// `handle_user_message` head (per-turn cap pattern).
    pub(super) artifact_completion_exhausted_this_turn: bool,
    /// Issue #652 CB2-003: per-turn flag set by
    /// `maybe_emit_artifact_completion_failed_diagnostic` once it has emitted
    /// the `artifact_completion_failed role=<role>` system note / log /
    /// working-memory tuple in the current turn. Reset at every
    /// `handle_user_message` head (per-turn cap pattern, DR3-003).
    ///
    /// Previously the dedup relied on `working_memory.unresolved_errors`
    /// matching the `artifact_completion_failed role=<role>` prefix. That is
    /// session state — `handle_user_message` does NOT clear it — so a
    /// residual error from the prior turn would suppress the very first
    /// emission of the current turn. The turn-local boolean removes that
    /// cross-turn leakage while keeping the within-turn single-emit
    /// guarantee that CB-002 relies on.
    pub(super) artifact_completion_failed_diagnostic_emitted_this_turn: bool,
    /// Issue #623 follow-up / #625 / #627: verifier repair is a diagnostic phase.
    /// The context is turn-local control data, not conversation memory. It keeps
    /// verifier output, deterministic facts, and one bounded assessment so the
    /// tool policy can choose a repair target without polluting the assistant
    /// history with long-lived diagnostic state.
    task_contract_verifier_repair_pending: bool,
    /// Issue #647 (SF1 V3.2): mirror of the local
    /// `task_contract_verifier_passed_in_loop` bool in
    /// `handle_user_message`. Lives on `Agent` so methods invoked from
    /// inside the actor-loop (e.g. `run_verifier_diagnostic_pass`) can read
    /// the same "has the verifier already passed once in this run-actor-loop
    /// iteration?" signal that the local variable carries — without
    /// plumbing yet another `&mut bool` through six call sites. Reset at
    /// the head of every `handle_user_message`; flipped to `true` from
    /// `drive_task_contract_verifier` whenever the verifier classifies a
    /// run as Passed. **Read-only** consumer in this Issue: the
    /// `SpecAuthorityInput` builder.
    pub(super) task_contract_verifier_passed_this_actor_loop: bool,
    /// Issue #625 / #627 / #637: turn-local diagnostic context for a failed
    /// task-contract verifier. Renamed from `verifier_repair_context` to
    /// `repair_job` and consolidated under `repair_job::RepairJob` so all
    /// repair state machine field/decision logic lives in one module. This
    /// keeps verifier output as data and lets the tool policy focus the
    /// next repair turn on the most likely workspace repair file, without
    /// adding framework-specific recovery rules.
    repair_job: Option<repair_job::RepairJob>,
    /// Issue #637: artifact recovery retry counter for the `RepairArtifact`
    /// branch in `run_turn`. Replaces the turn-local
    /// `verifier_repair_retries: &mut usize` plumbing. Bounded by
    /// `TASK_CONTRACT_VERIFIER_ATTEMPT_LIMIT` (= 3); not to be confused
    /// with the wider `TASK_CONTRACT_VERIFIER_REPAIR_ATTEMPT_LIMIT` (= 6).
    repair_job_artifact_attempts: usize,
    /// Issue #638 (Task 1.4): turn-local bounded failure report produced when
    /// `record_verifier_diagnostic_unavailable` is called. Reset at the start
    /// of every user turn alongside `repair_job`. Not persisted to
    /// `self.session.messages` (design policy §5 — A-only, turn-local).
    repair_failure_snapshot: Option<repair_job::VerifierFailureSnapshot>,
    /// Issue #636: per-turn behavior-coverage excerpts keyed by artifact role.
    /// Populated by `turn.rs::observe_evidence_from_repo_edit` via
    /// `bounded_post_edit_excerpt`, read by
    /// `task_contract::plan_artifact_recovery` through
    /// `ArtifactRecoveryInputs::artifact_excerpts`. Reset at the head of
    /// `run_actor_loop` together with the other `*_this_turn` per-turn
    /// state so excerpts never bleed across user turns. Not serialized —
    /// the behavior-coverage decision is evaluated within the same turn
    /// the excerpts were observed.
    task_contract_excerpts: task_contract::ArtifactExcerpts,
    /// Issue #646 (C2 / A4): turn-scoped map of pre-tool file hashes captured
    /// immediately before each Write/Edit execution. Consumed by
    /// `observe_evidence_from_repo_edit` to detect no-op writes (content
    /// unchanged → no `Owned` promotion). `None` means the file did not exist
    /// prior to the tool call. Reset at the `handle_user_message` head.
    turn_pre_tool_file_hashes: std::collections::HashMap<String, Option<String>>,
    /// Issue #646 (A1): turn-scoped first-class state for "verifier is
    /// missing". Set by `drive_task_contract_verifier::NoVerifier`, cleared
    /// at `handle_user_message` head and on verifier success. While
    /// populated, the planner suppresses `RunVerifier` until an in-scope
    /// edit is observed (`record_in_scope_edit`).
    missing_verifier_job: Option<repair_job::MissingVerifierJob>,
    /// Issue #646: per-turn set of workspace-relative paths that were
    /// successfully written or edited during the current user turn. Consumed
    /// by `artifact_ownership::classify_ownership` to gate which existing
    /// files the active task is allowed to claim as completion evidence.
    ///
    /// **Per-turn cap pattern (CLAUDE.md "per-turn cap" §)** — reset at the
    /// head of every `handle_user_message`. The previous turn's edits do
    /// not auto-confer ownership on the new task: a new task must
    /// re-establish ownership through fresh edits or an explicit user-named
    /// scope. Fresh sessions also begin empty, so pre-existing filesystem
    /// artifacts cannot auto-promote themselves (Issue #646 §修正方針 2
    /// `Owned` rules).
    turn_edited_relative_paths: std::collections::HashSet<String>,
    /// Issue #654 (DR1-006): per-turn dedup marker for the `agent.safe_stop.report`
    /// event. `HashSet<StopReason>` provides type-safe membership tests
    /// (typo detection at compile time). Reset at the head of every
    /// `handle_user_message` so a new turn can re-emit the same StopReason.
    /// NOT serialized — `SessionSnapshot` / `CaseRecord` / `EvalTurnRecord`
    /// persistence schemas are unchanged by Issue #654.
    pub(in crate::agent::loop_run) safe_stop_report_emitted:
        std::collections::HashSet<repair_job::StopReason>,
    /// Issue #659 (Phase 2): per-turn SSOT for artifact observations
    /// (Existing / Scaffold / RepoEdit) + verifier observations bound by
    /// path. Adapter-period contract: `turn_edited_relative_paths` remains
    /// the legacy authority for caller-facing decisions (Phase 6.1); the
    /// ledger is seeded via write-through (Task 2.5) so dual-source
    /// divergence can be asserted at turn end (Task 2.7).
    ///
    /// **Per-turn cap (CLAUDE.md per-turn rule)** — reset at the head of
    /// every `handle_user_message` alongside `turn_edited_relative_paths` /
    /// `turn_pre_tool_file_hashes`. NOT serialized — Phase 1 invariant: the
    /// ledger lives only on `Agent`, never on `SessionSnapshot`.
    pub(in crate::agent::loop_run) artifact_ledger:
        crate::agent::loop_run::artifact_ledger::ArtifactLedger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerifierRepairRerunOutcome {
    Improved,
    SameFailureRemaining,
    NewFailure,
    Worsened,
}

impl VerifierRepairRerunOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Improved => "improved",
            Self::SameFailureRemaining => "same_failure_remaining",
            Self::NewFailure => "new_failure",
            Self::Worsened => "worsened",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerifierFailureType {
    CompileOrSyntax,
    ImportOrDependency,
    RuntimeError,
    AssertionFailure,
    MissingVerifierOrConfig,
    Unknown,
    /// Issue #654: control-flow stop reason emitted when diagnostic target
    /// selection failed (`record_safe_stop_report` sets this variant directly;
    /// `verifier_failure_type_for_diagnostic_kind` /
    /// `classify_verifier_failure_type` never map to this variant — DR3-005).
    DiagnosticTargetMissing,
}

impl VerifierFailureType {
    fn as_str(self) -> &'static str {
        match self {
            Self::CompileOrSyntax => "compile_or_syntax",
            Self::ImportOrDependency => "import_or_dependency",
            Self::RuntimeError => "runtime_error",
            Self::AssertionFailure => "assertion_failure",
            Self::MissingVerifierOrConfig => "missing_verifier_or_config",
            Self::Unknown => "unknown",
            Self::DiagnosticTargetMissing => "diagnostic_target_missing",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VerifierRepairAssessment {
    failure_kind: VerifierDiagnosticFailureKind,
    failure_type: VerifierFailureType,
    probable_cause_role: Option<crate::agent::loop_run::task_contract::ArtifactRole>,
    needed_reads: Vec<crate::agent::loop_run::task_contract::RecoveryTargetHint>,
    repair_target_hint: Option<crate::agent::loop_run::task_contract::RecoveryTargetHint>,
    repair_plan: Vec<crate::agent::loop_run::task_contract::RecoveryTargetHint>,
    summary: Option<String>,
    source: VerifierRepairAssessmentSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerifierRepairAssessmentSource {
    DiagnosticPass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VerifierDiagnosticFailureKind {
    DependencyMissing,
    LocalImportContractMismatch,
    CompileOrSyntaxError,
    AssertionMismatch,
    RuntimeError,
    TestBug,
    ConfigOrVerifierError,
    Unknown,
}

impl VerifierDiagnosticFailureKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::DependencyMissing => "dependency_missing",
            Self::LocalImportContractMismatch => "local_import_contract_mismatch",
            Self::CompileOrSyntaxError => "compile_or_syntax_error",
            Self::AssertionMismatch => "assertion_mismatch",
            Self::RuntimeError => "runtime_error",
            Self::TestBug => "test_bug",
            Self::ConfigOrVerifierError => "config_or_verifier_error",
            Self::Unknown => "unknown",
        }
    }

    fn allows_setup_target(self) -> bool {
        matches!(self, Self::DependencyMissing | Self::ConfigOrVerifierError)
    }
}

/// Issue #594: state machine for the `/photon-why` slash command. Lives at
/// the loop_run module level (alongside `Agent`) because the enum is only
/// consumed by `commands.rs::render_photon_why` and updated by
/// `turn.rs::invoke_photon_context_pack`; no session-layer persistence is
/// needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PhotonContextPackStatus {
    /// Initial value before any turn has executed.
    #[default]
    NoTurn,
    /// `photon_shadow_mode=true` — context_pack was not consulted for prompt.
    ShadowMode,
    /// Canary gate fired (e.g. `canary=0` or the per-session permille check).
    CanarySkipped,
    /// `run_turn` skipped the hook because the session was in Plan mode.
    PlanMode,
    /// HTTP fetch to `/v1/context/pack` failed (timeout / 5xx / fail-open).
    Failed,
    /// Fetch succeeded but all items were filtered out / capped.
    NoInjection,
    /// Fetch succeeded and at least one seed was rendered into the prompt.
    Injected,
}

#[derive(Clone)]
struct RepoContextCache {
    task: String,
    work_root: PathBuf,
    repo_graph_present: bool,
    last_feedback_kind: Option<String>,
    suspected_files_fingerprint: u64,
    touched_files_fingerprint: u64,
    message: Option<ConversationMessage>,
}

/// Issue #469 DR1-005: SSOT for path-list fingerprinting used by the
/// `RepoContextCache` key. Empty slice yields a process-stable sentinel
/// hash; non-empty path lists are sorted before hashing for stability
/// across feedback ordering.
pub(super) fn fingerprint_paths(paths: &[String]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut sorted: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
    sorted.sort_unstable();
    let mut hasher = DefaultHasher::new();
    sorted.hash(&mut hasher);
    hasher.finish()
}

impl Agent {
    pub fn new(
        config: Config,
        models: RuntimeModels,
        client: OllamaClient,
        session_store: SessionStore,
        session: SessionSnapshot,
        footer: FooterHandle,
    ) -> Self {
        let work_root = session
            .active_root
            .clone()
            .unwrap_or_else(|| config.cwd.clone());
        let native_tools_enabled =
            should_use_native_tool_calls(&models.main) && !session.native_tools_disabled;
        let mut skill_registry = crate::agent::skills::SkillRegistry::new();
        skill_registry.register(crate::agent::loop_run::verifier_skill::VerifierSkill);
        let repo_graph = ensure_repo_graph(&work_root, session_store.state_root());
        let photon = if config.offline || !config.photon_enabled {
            None
        } else {
            match crate::photon::PhotonClient::new(
                config.photon_url.clone(),
                config.photon_timeout_ms,
            ) {
                Ok(c) => Some(c),
                Err(e) => {
                    tracing::warn!("photon client init failed, disabled: {e}");
                    None
                }
            }
        };
        Self {
            config,
            models,
            client,
            session_store,
            session,
            work_root,
            native_tools_enabled,
            plan_model_override: None,
            tool_registry: ToolRegistry::default(),
            repo_context_cache: None,
            footer,
            reminder_called_this_turn: false,
            tester_called_this_turn: false,
            work_mode_confirm_called_this_turn: false,
            feedback_kind_confirm_called_this_turn: false,
            quality_confirm_called_this_turn: false,
            last_quality_confirm_result: None,
            anvil_score_computed_this_turn: false,
            skill_registry,
            repo_graph,
            last_case_retrieval_summary: None,
            current_turn_index: 0,
            photon,
            photon_context_pack_response: None,
            last_context_pack_id: None,
            last_photon_eval_summary: None,
            last_photon_adopted_items: 0,
            last_adopted_summary_ids: Vec::new(),
            last_injected_seed_provenance: Vec::new(),
            last_photon_context_pack_status: PhotonContextPackStatus::NoTurn,
            last_injected_summary_ids: Vec::new(),
            last_injected_summary_turn_index: None,
            photon_user_feedback_called_this_turn: false,
            last_auto_promote_outcome: None,
            evidence_set_this_turn: completion_evidence::EvidenceSet::new(),
            task_contract_evidence_set_this_turn: completion_evidence::EvidenceSet::new(),
            current_artifact_recovery_target: None,
            artifact_completion_job: None,
            artifact_completion_exhausted_this_turn: false,
            artifact_completion_failed_diagnostic_emitted_this_turn: false,
            task_contract_verifier_repair_pending: false,
            task_contract_verifier_passed_this_actor_loop: false,
            repair_job: None,
            repair_job_artifact_attempts: 0,
            repair_failure_snapshot: None,
            task_contract_excerpts: task_contract::ArtifactExcerpts::new(),
            missing_verifier_job: None,
            turn_pre_tool_file_hashes: std::collections::HashMap::new(),
            turn_edited_relative_paths: std::collections::HashSet::new(),
            safe_stop_report_emitted: std::collections::HashSet::new(),
            artifact_ledger: artifact_ledger::ArtifactLedger::new(),
        }
    }

    /// Issue #637: convenience helper for callers that previously consulted
    /// `task_contract_verifier_repair_pending`. After the rename, the
    /// presence of a `RepairJob` IS the pending signal; the legacy bool
    /// is kept for now until call sites are migrated, but new code should
    /// prefer this method.
    #[allow(dead_code)] // forward-facing helper; call sites migrate off the legacy bool in a follow-up.
    pub(super) fn is_verifier_repair_pending(&self) -> bool {
        self.repair_job.is_some()
    }
}

/// Issue #468 facade: build (or load from cache) the `RepoGraph` once at
/// session start. **All `agent.repo_graph.*` event emission lives here**
/// (DR1-005). Failures are non-fatal: the agent loop always continues.
fn ensure_repo_graph(work_root: &Path, state_root: &Path) -> Option<Arc<RepoGraph>> {
    let opts = RepoGraphBuildOptions::default();
    let outcome = build_repo_graph(work_root, state_root, &opts);
    match outcome {
        Ok(BuildOutcome::Built {
            graph,
            node_count,
            edge_count,
        }) => {
            log_llm_event(
                "agent.repo_graph.completed",
                serde_json::json!({
                    "reason": "built",
                    "node_count": node_count,
                    "edge_count": edge_count,
                    "fingerprint_id": graph_fingerprint_id(&graph),
                }),
            );
            Some(graph)
        }
        Ok(BuildOutcome::CacheHit { graph }) => {
            log_llm_event(
                "agent.repo_graph.completed",
                serde_json::json!({
                    "reason": "cache_hit",
                    "fingerprint_id": graph_fingerprint_id(&graph),
                }),
            );
            Some(graph)
        }
        Ok(BuildOutcome::Skipped { reason }) => {
            log_llm_event(
                "agent.repo_graph.skipped",
                serde_json::json!({"reason": reason}),
            );
            None
        }
        Err(RepoGraphError::Disabled) => {
            log_llm_event(
                "agent.repo_graph.disabled",
                serde_json::json!({"reason": "env_no_repo_graph"}),
            );
            None
        }
        Err(RepoGraphError::CwdCanonicalFailed) => {
            log_llm_event(
                "agent.repo_graph.failed",
                serde_json::json!({"reason": "cwd_canonical_failed"}),
            );
            None
        }
        Err(RepoGraphError::PersistFailed(message)) => {
            log_llm_event(
                "agent.repo_graph.failed",
                serde_json::json!({
                    "reason": "persist_failed",
                    "message": message,
                }),
            );
            None
        }
    }
}

fn graph_fingerprint_id(_graph: &Arc<RepoGraph>) -> String {
    // RepoGraph keeps the fingerprint internal; we expose a short id surface
    // here through the public node/edge counts already logged above. The id
    // itself is currently not part of the public API to keep the surface
    // minimal — a tracking entry to expose it lives with #469's seam work.
    String::new()
}

#[cfg(test)]
mod tests {
    use super::lifecycle::{format_tool_error, should_compact_late_turn};
    use crate::agent::prompting::{
        compact_tool_result, detect_created_project_root, should_skip_system_note,
    };
    use crate::session::store::ConversationMessage;
    use std::path::PathBuf;

    #[test]
    fn detects_created_next_app_root_from_success_line() {
        let output = "Success! Created sample-app at /tmp/work/sample-app";
        assert_eq!(
            detect_created_project_root(output),
            Some(PathBuf::from("/tmp/work/sample-app"))
        );
    }

    #[test]
    fn detects_created_next_app_root_from_create_line() {
        let output = "Creating a new Next.js app in /tmp/work/sample-app.";
        assert_eq!(
            detect_created_project_root(output),
            Some(PathBuf::from("/tmp/work/sample-app"))
        );
    }

    #[test]
    fn tool_errors_are_formatted_for_model_recovery() {
        assert_eq!(
            format_tool_error("failed to stat /tmp/missing: nope"),
            "Error: failed to stat /tmp/missing: nope"
        );
        assert_eq!(
            format_tool_error("Error: file not found"),
            "Error: file not found"
        );
    }

    #[test]
    fn read_results_are_compacted_for_transcript() {
        let long = "a".repeat(20_000);
        let compacted = compact_tool_result("Read", long);
        assert!(compacted.contains("[truncated"));
        assert!(compacted.len() < 13_000);
    }

    #[test]
    fn skips_duplicate_consecutive_system_note() {
        let messages = vec![ConversationMessage::system("same note".to_string())];
        assert!(should_skip_system_note(&messages, "same note"));
        assert!(!should_skip_system_note(&messages, "other note"));
    }

    #[test]
    fn skips_duplicate_system_note_with_tool_results_between() {
        let messages = vec![
            ConversationMessage::system("same note".to_string()),
            ConversationMessage::assistant(String::new(), Vec::new()),
            ConversationMessage::tool("Read".to_string(), "file contents".to_string()),
        ];
        assert!(should_skip_system_note(&messages, "same note"));
    }

    #[test]
    fn late_turn_compaction_triggers_for_edit_heavy_turns() {
        let messages = (0..18)
            .map(|index| ConversationMessage::user(format!("message {index}")))
            .collect::<Vec<_>>();
        assert!(should_compact_late_turn(&messages, 24_000, 4, 1));
        assert!(!should_compact_late_turn(&messages[..8], 24_000, 1, 0));
    }

    // -- Phase G grep / structure tests (Issue #647 acceptance closure) -- //

    #[test]
    fn no_pub_use_for_semantic_failure_or_spec_authority() {
        // S1-003 / DR3-001: `semantic_failure` and `spec_authority` are
        // private modules of `loop_run`. They must NOT be widened via
        // `pub use` re-exports — only the `turn.rs` / `repair_job.rs`
        // in-crate consumers may access them through `super::`.
        let source = include_str!("loop_run.rs");
        for line in source.lines() {
            let trimmed = line.trim_start();
            // `//` comments / `///` doc comments / `//!` module docs are
            // allowed to mention the names for documentation purposes.
            if trimmed.starts_with("//") {
                continue;
            }
            if trimmed.starts_with("pub use") {
                assert!(
                    !line.contains("semantic_failure"),
                    "DR3-001 violated: `pub use` referencing `semantic_failure` found: {line:?}",
                );
                assert!(
                    !line.contains("spec_authority"),
                    "DR3-001 violated: `pub use` referencing `spec_authority` found: {line:?}",
                );
            }
        }
    }

    #[test]
    fn verifier_diagnostic_failure_kind_has_8_variants() {
        // S1-001: SemanticFailureReport is an upper-layer wrapper that
        // **reuses** the existing 8-variant `VerifierDiagnosticFailureKind`
        // enum. Adding or removing a variant breaks the SSOT invariant
        // — this test fails to compile (non-exhaustive match) if the
        // enum surface drifts.
        use super::VerifierDiagnosticFailureKind as K;
        let all = [
            K::DependencyMissing,
            K::LocalImportContractMismatch,
            K::CompileOrSyntaxError,
            K::AssertionMismatch,
            K::RuntimeError,
            K::TestBug,
            K::ConfigOrVerifierError,
            K::Unknown,
        ];
        for v in &all {
            // Exhaustive match — extension of the enum forces this to
            // be updated (compile-time lock).
            let label: &'static str = match v {
                K::DependencyMissing => "dependency_missing",
                K::LocalImportContractMismatch => "local_import_contract_mismatch",
                K::CompileOrSyntaxError => "compile_or_syntax_error",
                K::AssertionMismatch => "assertion_mismatch",
                K::RuntimeError => "runtime_error",
                K::TestBug => "test_bug",
                K::ConfigOrVerifierError => "config_or_verifier_error",
                K::Unknown => "unknown",
            };
            assert!(!label.is_empty());
        }
        assert_eq!(all.len(), 8);
    }
}
