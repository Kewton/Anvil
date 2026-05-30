use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::agent::prompting;
use crate::agent::recovery;
use crate::config::Config;
use crate::format_model_banner;
use crate::logging::log_llm_event;
use crate::model_registry::RuntimeModels;
use crate::modes::plan_act::{ExecutionMode, ModePolicy};
use crate::ollama::client::{AssistantReply, OllamaClient, should_use_native_tool_calls};
use crate::repo_graph::{
    BuildOptions as RepoGraphBuildOptions, BuildOutcome, RepoGraph, RepoGraphError,
    build_repo_graph,
};
use crate::session::compact::{
    approximate_token_count, compact_messages, compact_messages_with_strategy,
};
use crate::session::store::{ConversationMessage, SessionSnapshot, SessionStore};
use crate::stdin_prompt;
use crate::tools::registry::ToolRegistry;

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
// Issue #660: active-job arbitration SSOT. Pure function
// (`select_active_job`) + `EffectiveToolPolicy` projection
// (`project_policy`) used by `turn.rs::effective_tool_policy()` to pick
// at most one write-owner job per turn. Module is intentionally *not*
// re-exported (DR3-001) — `turn.rs` is the only in-crate consumer via
// `super::active_job_arbiter::*`.
mod active_job_arbiter;
// Issue #681 (parent #680, Phase 1): actor loop control-flow data types
// (`PostReplyRecovery*` / `ActorLoop*Args` / `ActorLoop*Outcome`) +
// 2 small Outcome constructor helpers extracted from `turn.rs`. Module
// is intentionally *not* re-exported (DR3-001) — `turn.rs` is the only
// in-crate consumer via `super::actor_loop_flow::*`.
mod actor_loop_flow;
// Anti-pattern extraction + retrieval flow extracted from `turn.rs`
// (parent #680). Hosts `maybe_extract_anti_pattern` and
// `try_inject_anti_pattern_message` (free fns over `&mut Agent`) plus
// their private logging / feedback helpers. `pub(super)` limited / no
// facade re-export (DR3-001).
mod anti_pattern_flow;
// Case-record extraction + retrieval flow extracted from `turn.rs`
// (parent #680). Hosts `maybe_extract_case_record` and
// `try_inject_case_retrieval_message` (free fns over `&mut Agent`) plus
// `persist_case_record` / `finish_case_record_extraction` private
// helpers. `pub(super)` limited / no facade re-export (DR3-001).
mod case_record_flow;
// Work-mode / feedback-kind / quality second-pass confirmation flow
// extracted from `turn.rs` (parent #680). Hosts 4 production entry
// points + 4 private attempt/resolution helpers, all free fns over
// `&mut Agent`. `pub(super)` limited / no facade re-export (DR3-001).
mod classify_confirm_flow;
// Tester invocation flow extracted from `turn.rs` (parent #680). Hosts
// `try_invoke_tester` (pub(super)) and 5 private helpers as free fns
// over `&mut Agent` / `&Agent`. `pub(super)` limited / no facade
// re-export (DR3-001).
mod tester_invocation;
// Python package-marker materialization extracted from `turn.rs`
// (parent #680). Hosts 2 entry points + a shared materializer, all free
// fns over `&mut Agent`. `pub(super)` limited / no facade re-export
// (DR3-001).
mod python_markers;
// Verifier event emitters extracted from `turn.rs` (parent #680). Hosts
// `emit_agent_verifier_invoked_if_new` (per-turn payload-digest dedup)
// + `emit_agent_verifier_external_import_rejected_if_first` (single
// per-turn cap). Both apply `mask_payload_inplace` as final defence.
// `pub(super)` limited / no facade re-export (DR3-001).
mod emit_verifier_events;
// Task-contract verifier observation hooks extracted from `turn.rs`
// (parent #680). Hosts `record_task_contract_verifier_invocation`,
// `observe_task_contract_verifier_exit_zero`, and
// `observe_task_contract_verifier_exit_zero_bound` — all free fns over
// `&mut Agent`. `pub(super)` limited / no facade re-export (DR3-001).
mod verifier_observation;
// Assistant-reply retry orchestration extracted from `turn.rs` (parent
// #680). Hosts the retry loop entry point + 10 branch-by-branch error
// handlers (format-error / timeout / transport / native-tool downgrade /
// generic retry + actual Ollama dispatch). `current_assistant_model`
// stays on Agent (5+ external call sites). `pub(super)` limited / no
// facade re-export (DR3-001).
mod reply_retry;
// Repair-job dispatch flow extracted from `turn.rs` (parent #680). Hosts
// `dispatch_repair_job_step`, `dispatch_missing_verifier_job_step`,
// `repair_rejection_next_action` (pub(super)) + 11 private branch
// handlers as free fns over `&mut Agent`. `pub(super)` limited / no
// facade re-export (DR3-001).
mod repair_job_dispatch;
// Request-message assembly extracted from `turn.rs` (parent #680).
// Hosts `build_request_messages` (pub(super) entry point) + 4 private
// helpers (general context / context-pack / common / focused-edit
// message appenders). All free fns over `&mut Agent` / `&Agent`.
// `pub(super)` limited / no facade re-export (DR3-001).
mod build_request_messages;
// Effective-tool-policy + arbiter candidate selection extracted from
// `turn.rs` (parent #680). Hosts `effective_tool_policy` (pub(super)
// entry point), `build_arbiter_candidates` (pub(super)), and 5 private
// helpers as free fns over `&Agent`. Replaces former `impl Agent`
// methods. `pub(super)` limited / no facade re-export (DR3-001).
mod effective_tool_policy_flow;
// Per-turn entry point extracted from `turn.rs` (parent #680). Hosts
// `handle_user_message` as a free fn over `&mut Agent`. Owns the per-
// turn state reset (CLAUDE.md per-turn rule) + dispatch to
// `Agent::run_turn` + post-turn ledger refresh + job report emit.
// `pub(super)` limited / no facade re-export (DR3-001).
mod handle_user_message;
// Per-actor-loop-turn state initializer extracted from `turn.rs`
// (parent #680). Hosts `prepare_actor_loop_turn_state` as a free fn
// over `&mut Agent`. Resets ~25 per-actor-loop caps / dedup carriers
// and computes the initial TaskContract. `pub(super)` limited / no
// facade re-export (DR3-001).
mod prepare_actor_loop_state;
// Per-turn driver extracted from `turn.rs` (parent #680). Hosts
// `run_turn` as a free fn over `&mut Agent`. Pushes user message +
// runs work-mode classify second-pass + plan stage refresh + photon
// context-pack hook + actor loop entry. `pub(super)` limited / no
// facade re-export (DR3-001).
mod run_turn;
// Active-job + behavior-contract event emit helpers extracted from
// `turn.rs` (parent #680). Hosts `emit_active_job_selected_if_changed`,
// `emit_behavior_contract_projected_if_changed`, and
// `current_active_job_selection` as free fns over `&mut Agent` /
// `&Agent`. `pub(super)` limited / no facade re-export (DR3-001).
mod active_job_emit;
// Working-memory + repo-context prompt messages extracted from
// `turn.rs` (parent #680). Hosts `refresh_working_memory`,
// `working_memory_message`, `answer_only_fallback_response`, and
// `repo_context_message` as free fns over `&mut Agent` / `&Agent`.
// `pub(super)` limited / no facade re-export (DR3-001).
mod working_memory_messages;
// Artifact-recovery target + completion-job lifecycle extracted from
// `turn.rs` (parent #680). Hosts `clear_artifact_recovery_target`,
// `maybe_install_artifact_completion_job_for_hint`,
// `maybe_emit_artifact_completion_failed_diagnostic`, and
// `artifact_recovery_target_path` as free fns over `&mut Agent` /
// `&Agent`. `pub(super)` limited / no facade re-export (DR3-001).
mod artifact_recovery_flow;
// Artifact-recovery target installer (projection-write half)
// extracted from `turn.rs` (parent #680). Hosts
// `set_artifact_recovery_target_for_decision`,
// `set_artifact_recovery_target_for_action`,
// `set_artifact_recovery_target_from_hint`, and 2 private alignment /
// synthesis helpers. Free fns over `&mut Agent` / `&Agent`.
// `pub(super)` limited / no facade re-export (DR3-001).
mod set_artifact_recovery_target;
// Per-turn artifact-ledger state management (parent #680). Hosts the
// per-turn lifecycle of `Agent::artifact_ledger`: reset + observability
// stamp, end-of-turn summary emit, four seed helpers (Existing /
// Scaffold / RepoEdit / VerifierObservation), dual-source divergence
// assertion + release-mode emitter. Free fns over `&mut Agent` /
// `&Agent`. `pub(super)` limited / no facade re-export (DR3-001).
mod artifact_ledger_state;
// TaskContract artifact-state projection extracted from `turn.rs` (parent
// #680). Hosts the Phase-3 (Issue #659 Task 3.2) production helper
// `task_contract_artifact_states` (ledger authority + legacy shadow +
// divergence emit), the legacy / ledger derivations, the masked
// observability emit, and three `#[cfg(test)]` test seams. Free fns
// over `&mut Agent` / `&Agent`. `pub(super)` limited / no facade
// re-export (DR3-001).
mod artifact_state_projection;
// `agent.safe_stop.report` emit cluster extracted from `turn.rs` (parent
// #680). Hosts the Issue #654 emit lifecycle: per-StopReason dedup +
// emit-4-job-reports + SafeStopReport build + bounded payload render +
// final-defence-masked log_llm_event, the §6.4 current-role priority
// chain, six emit shells, the shared repair-job emit helper, and the
// owned-test-artifact collector. Free fns over `&mut Agent` / `&Agent`.
// `pub(super)` limited / no facade re-export (DR3-001).
mod safe_stop_emit;
// Verifier-diagnostic pass flow extracted from `turn.rs` (parent #680).
// Hosts the Issue #637 / #638 / #654 lifecycle: prepare → request →
// failure → record_failure / record_unavailable, plus the stale-state
// reset helper. Free fns over `&mut Agent` / `&Agent`. `pub(super)`
// limited / no facade re-export (DR3-001).
mod verifier_diagnostic_flow;
// Verifier-repair pass flow extracted from `turn.rs` (parent #680).
// Hosts the per-attempt repair-plan-driven targeted-edit loop:
// prepare (admission + accepted-plan build + behavior projection emit +
// prompt render) → handle_attempt (per-reply dispatcher → progress
// outcome) → handle_reply (parse + shadow validation + intent admission
// + legacy comparison + apply via `apply_verifier_repair_pass_edit`),
// plus wall-clock-timeout error + bounded timeout-event log helper.
// Free fns over `&mut Agent` / `&Agent`. `pub(super)` limited / no
// facade re-export (DR3-001).
mod verifier_repair_pass_flow;
// Artifact-completion attempt-recording cluster extracted from `turn.rs`
// (parent #680). Hosts `push_artifact_directed_recovery_note` (system
// note push gated by focused-edit target) + `record_artifact_completion_attempt`
// + `record_artifact_completion_bash_violation` + the shared
// `record_artifact_completion_outcome` core (append outcome + trigger
// turn-local `artifact_completion_failed` diagnostic on Exhausted
// transition). Free fns over `&mut Agent`. `pub(super)` limited / no
// facade re-export (DR3-001).
mod artifact_completion_record;
// Recovery / verifier-repair policy message builders extracted from
// `turn.rs` (parent #680). Hosts the prompt-text builders that drive
// the focused-edit / artifact-directed / verifier-repair recovery
// policies: focused_edit_no_tool_note_for_{target,policy},
// artifact_directed_recovery_message, verifier_repair_policy_message
// (RepairNextAction dispatcher) + 2 private repair-policy sub-builders,
// artifact_directed_policy_violation_message,
// push_deterministic_ui_recovery_continuation_note. Free fns over
// `&mut Agent` / `&Agent`. `pub(super)` limited / no facade re-export
// (DR3-001).
mod recovery_messages;
// TaskContract recovery action / target planners extracted from
// `turn.rs` (parent #680). Hosts `task_contract_recovery_action` (the
// production chokepoint that routes between `RunVerifier` / `Continue`
// / `Done` per artifact state + repair state + completion probe) +
// `task_contract_recovery_target` (`Continue { missing }` →
// scaffold-candidate / existing-Owned / synthesised-implementation
// target hint) + the private `task_contract_repair_state` adapter.
// Free fns over `&mut Agent` / `&Agent`. `pub(super)` limited / no
// facade re-export (DR3-001).
mod task_contract_recovery;
// `owned_test_artifacts_for_verifier` projection extracted from
// `turn.rs` (parent #680). Hosts the Phase-3 (Issue #659 Task 3.3)
// verifier-binding Owned-test-artifacts projection: legacy + ledger
// derivations with masked `agent.artifact_ledger.divergence_detected`
// emit when they disagree; returns the ledger projection as the v0.4.8
// production authority. Free fns over `&mut Agent` / `&Agent`.
// `pub(super)` limited / no facade re-export (DR3-001).
mod owned_test_projection;
// Tool-call execution dispatch extracted from `turn.rs` (parent #680).
// Hosts the per-tool-call execution lifecycle: production chokepoint
// `execute_tool_call` (policy gates → Bash vs. non-Bash dispatch) +
// `tool_context` builder + 4 dispatch helpers
// (`{handle_tool_execution_rejection,execute_bash_tool_call,capture_pre_tool_hash_if_needed,execute_non_bash_tool_call}`)
// + Issue #606 T-1.6 `observe_evidence_from_bash_outcome` (Bash exit-0
// → VerifierExitZero evidence + last_verifier_invocation record). Free
// fns over `&mut Agent` / `&Agent`. `pub(super)` limited / no facade
// re-export (DR3-001).
mod tool_call_execution;
// Post-Edit/Write repo-edit evidence observation extracted from
// `turn.rs` (parent #680). Hosts the Issue #606 (T-1.7)
// `observe_evidence_from_repo_edit` chokepoint: ignored-top-dir gate
// → scaffold-delta gate → no-op-hash gate →
// `turn_edited_relative_paths` write-through + per-turn evidence +
// task-contract evidence + bounded post-edit excerpt + Issue #659
// Task 2.5 ArtifactLedger seed. Free fn over `&mut Agent`.
// `pub(super)` limited / no facade re-export (DR3-001).
mod repo_edit_observation;
// Focused-edit / repo-change / verifier-repair recovery target +
// note builders extracted from `turn.rs` (parent #680). Hosts
// `focused_edit_recovery_target` (focused-edit chain),
// `repo_change_no_edit_recovery_target` (private repo-change chain),
// `push_repo_change_no_edit_recovery_note`, and
// `push_verifier_repair_recovery_note` (RequestDiagnostic /
// RequestPatch branches). Free fns over `&mut Agent` / `&Agent`.
// `pub(super)` limited / no facade re-export (DR3-001).
mod recovery_targets;
// Playable-UI quality gate decision helpers extracted from `turn.rs`
// (parent #680). Hosts `current_request_needs_playable_ui_quality_gate`
// (Act + policy + request gate), `accepted_repo_change_quality_issue`
// (Issue #580 second-pass classifier), `accepted_repo_change_polish_target`
// (deterministic polish target with quality-suppression), and the
// private `unsupported_ui_framework_context` predicate. Free fns over
// `&mut Agent` / `&Agent`. `pub(super)` limited / no facade re-export
// (DR3-001).
mod quality_gate;
// Per-Agent tool-policy decision helpers extracted from `turn.rs`
// (parent #680). Hosts `answer_only_mode_active` (Issue #576 / DR3-001
// SSOT: second-pass work_mode only) + `script_execution_requested` +
// `answer_only_policy_error` (read-only gate with Read/Glob/Grep
// allowlist + Bash-script allowlist branch) +
// `effective_tool_policy_error` (no-explicit-policy fallback via
// `effective_tool_policy_flow`). Free fns over `&Agent`. `pub(super)`
// limited / no facade re-export (DR3-001).
mod tool_policy_decisions;
// Python request inspection helpers extracted from `turn.rs` (parent
// #680). Hosts the Python-specific signals that drive completion /
// scaffold gating: `active_python_request_requires_tests`,
// `python_verifier_available_for_requested_tests`, and
// `python_test_artifact_exists` (top-level `test_*.py` / `*_test.py` /
// `tests.py` scan). Free fns over `&Agent`. `pub(super)` limited / no
// facade re-export (DR3-001).
mod python_request_helpers;
// Per-Agent `#[cfg(test)]` test seams extracted from `turn.rs` (parent
// #680). Hosts the test-only `pub(super)` seams that drive the
// production wiring without widening visibility (Issue #664 / CB2-003):
// effective_tool_policy_pub / build_arbiter_candidates_pub /
// drive_policy_error / artifact_directed_policy /
// last_attempt_bash_policy_violation / artifact_completion_job_attempts_len.
// Free fns over `&mut Agent` / `&Agent`. `#![cfg(test)]` module guard
// excludes from production binary. `pub(super)` limited / no facade
// re-export (DR3-001).
#[cfg(test)]
mod test_seams;
// Forced-small-edit recovery target / note builders extracted from
// `turn.rs` (parent #680). Hosts the focused-edit policy's
// "previous tool call truncated → force a small follow-up edit"
// branch: `forced_small_edit_recovery_target` (Act mode + truncated +
// no successful non-plan edit since) + `forced_small_edit_recovery_message`
// (renders the recovery note). Free fns over `&Agent`. `pub(super)`
// limited / no facade re-export (DR3-001).
mod forced_small_edit;
// Issue #636 bounded post-edit excerpt reader extracted from `turn.rs`
// (parent #680). Hosts `bounded_post_edit_excerpt`: workspace-confined
// + O_NOFOLLOW + 8 KiB cap + NUL/UTF-8 guards + mask_secrets +
// mask_header_family + post-mask char-boundary re-truncation. Used by
// `repo_edit_observation::observe_evidence_from_repo_edit` to capture
// a behavior-coverage snippet for `plan_artifact_recovery`. Free fn
// over `&Agent`. `pub(super)` limited / no facade re-export (DR3-001).
mod post_edit_excerpt;
// Tool-prep helpers extracted from `turn.rs` (parent #680). Hosts
// `tool_specs_for_policy` (filter registered tool specs by policy
// allowlist), `local_llm_small_edit_target` (small-edit Edit target
// for local LLMs), and `mode_policy_message` (per-`WorkMode`
// `[Mode Policy]` system note; Auto returns None). Free fns over
// `&Agent`. `pub(super)` limited / no facade re-export (DR3-001).
mod tool_prep;
// Per-Agent misc lifecycle helpers extracted from `turn.rs` (parent
// #680). Hosts `refresh_artifact_completion_satisfied` (Issue #663
// SSOT for ledger-driven Satisfied transition),
// `tool_policy_violation_exit_reason` (RecoveryOwner → ExitReason
// mapping), `record_missing_verifier_setup_failure` (invalid setup
// attempt → SafeStop or task_contract_no_verifier_note push), and
// `current_assistant_model` (mode + plan-model override picker). Free
// fns over `&mut Agent` / `&Agent`. `pub(super)` limited / no facade
// re-export (DR3-001).
mod agent_misc;
// Per-tool-call argument normalization extracted from `turn.rs`
// (parent #680). Hosts `prepare_tool_call`: per-tool argument sanitise
// + Read/Write/Edit workspace-confined path resolution +
// focused-edit Read directory→target redirect. Free fn over `&Agent`.
// `pub(super)` limited / no facade re-export (DR3-001).
mod tool_call_prepare;
// Read-tool path lookup helpers extracted from `turn.rs` (parent
// #680). Hosts `last_read_tool_path` (most recent `Read` path across
// all turns) and `latest_turn_preferred_read_edit_target` (latest user
// turn's `Read`s, resolved + filtered to existing files, prefers
// `is_preferred_read_edit_target` matches). Free fns (no Agent
// dependency). `pub(super)` limited / no facade re-export (DR3-001).
mod read_target_helpers;
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
// Focused-edit recovery helpers extracted from `turn.rs` (parent #680).
// Hosts the guidance-note builders, page-component anchor extractors, and
// conversation history shapers used by the focused-edit recovery flow.
// `pub(super)` limited / no facade re-export (DR3-001).
mod focused_edit_recovery;
// File excerpt + content-hash helpers extracted from `turn.rs` (parent
// #680). Hosts `open_excerpt_file_nofollow`, `utf8_prefix_respecting_cap`,
// `truncate_on_char_boundary`, `current_file_hash_for_relative_path`,
// `sha256_hex`. `pub(super)` limited / no facade re-export (DR3-001).
mod file_excerpt;
// v0.4.13 Phase 1: bounded verifier-failure packet used as the shared
// controller/LLM input for the new repair pipeline. Private module; no facade
// re-export (DR3-001 pattern).
mod failure_packet;
mod interrupt;
mod lifecycle;
// v0.4.25: model request policy helpers. Keeps transport and focused-edit
// request sizing decisions out of the actor-loop dispatcher as they are
// extracted toward a dedicated request boundary.
mod model_request;
pub mod photon_user_feedback;
// Issue #639: ProjectVerifier capability. Module is intentionally *not*
// re-exported (DR3-001) — `turn.rs` is the only in-crate consumer via
// `super::project_verifier::*`.
mod project_verifier;
// v0.4.22: generic completion probe. Private helper that can advance to
// verifier execution when current-turn artifacts are physically present even
// if the legacy artifact projection is still asking for another edit.
mod progress_text;
mod project_probe;
mod protocol;
mod quality;
pub(crate) mod quality_confirm;
pub(crate) mod reminder;
// v0.4.13 Phase 4: controller-owned action projected from a validated
// RepairBrief. Private module; consumed by the repair pipeline as it is wired.
mod repair_action;
// v0.4.15 MVP: provenance/authority boundary for verifier repair proposals.
// Private module; `turn.rs` is the only production consumer.
mod repair_authority;
// v0.4.16: accepted repair-plan boundary. Diagnostic output remains a proposal
// until this module validates it against FailurePacket + authority evidence.
mod repair_plan;
// v0.4.25: verifier-repair admission boundary. Keeps RepairPlan acceptance and
// admission error transition mapping out of the turn dispatcher.
mod repair_plan_admission;
// v0.4.13 Phase 2: small diagnostic schema returned by the short-lived
// diagnostic LLM. Private module; no direct provider abstraction.
mod repair_brief;
mod repair_job;
// Issue #653: `RepairAttemptOutcome` lifecycle ledger. Module is intentionally
// *not* re-exported (DR3-001) — `turn.rs` and `repair_job.rs` are the only
// in-crate consumers via `super::repair_attempt_outcome::*`.
mod repair_attempt_outcome;
// v0.4.26: repair-pass driver boundary. Keeps verifier-repair pass outcome
// typing, retry timing, and retry advice out of the actor-loop dispatcher as
// repair execution moves behind a dedicated owner.
mod repair_driver;
// v0.4.25: pure assertion/output analysis helpers shared by verifier repair
// diagnostics and generated-test semantic weakening filters.
mod repair_assertion_analysis;
// Issue #635: deterministic RequiredBehaviorContract extractor. Module is
// intentionally *not* re-exported (DR3-001) — `task_contract.rs` is the only
// in-crate consumer via `super::required_behavior::*`.
mod required_behavior;
// v0.4.13 Phase 5: bounded patch proposal schema for the patch shaper LLM.
mod patch_proposal;
// v0.4.16: patch-provider admission boundary. Providers propose concrete
// edits only after the controller has accepted a repair plan.
mod patch_provider;
// v0.4.25: applies already validated verifier-repair patches with preimage
// protection. This is intentionally separate from patch validation.
mod repair_patch_executor;
// v0.4.25: pure patch-admission checks shared by verifier repair validation.
mod repair_patch_validation;
// v0.4.25: verifier-repair target ownership admission SSOT. Keeps the
// path-local ownership gate out of the actor loop dispatcher.
mod repair_target_admission;
// v0.4.25: bounded Python local import-contract evidence for verifier repair
// validation. Keeps filesystem probing out of the turn dispatcher.
mod repair_python_import_evidence;
// v0.4.25: Python pytest/test-fragment analysis shared by verifier framework
// diagnostics and semantic test-repair validation.
mod repair_python_test_analysis;
// v0.4.25: semantic test weakening admission filter. Keeps verifier repair
// authority decisions out of the actor loop.
mod repair_test_weakening_filter;
// v0.4.25: objective framework/test-runner findings used as bounded evidence
// for verifier diagnostics.
mod repair_framework_findings;
// v0.4.25: diagnostic LLM assessment JSON boundary. Keeps schema-shape
// tolerance and enum mapping out of the actor loop dispatcher.
mod verifier_assessment_parser;
// v0.4.26: task-contract verifier outcome normalization boundary. Keeps
// pass/fail/transport classification separate from the actor-loop dispatcher
// while command execution and state transitions remain in turn.rs.
mod verifier_driver;
// v0.4.25: diagnostic LLM attempt schedule and timeout constants.
mod verifier_diagnostic_attempt;
// v0.4.25: verifier failure fingerprint/signature helpers. Keeps log
// summarization out of the actor loop dispatcher.
mod verifier_failure_signature;
// v0.4.25: verifier-repair shadow telemetry and legacy brief projection.
// Keeps observational payload shaping out of the actor loop dispatcher.
mod verifier_repair_shadow;
// v0.4.25: verifier-repair target/path helper boundary. Keeps diagnostic
// path safety and Python module/dependency candidate parsing out of the actor
// loop dispatcher.
mod verifier_repair_targeting;
// Issue #682 (parent #680, Phase 2): verifier orchestration data types
// extracted from `turn.rs`. Hosts 7 pub(super) types
// (JobInstallOutcome / VerifierDiagnosticPassOutcome /
// PreparedVerifierDiagnosticPass / PreparedVerifierRepairPass /
// VerifierRepairAttemptProgress / StructuredTaskContractVerifierRun /
// TaskContractVerifierFlowArgs). Module is intentionally *not*
// re-exported (DR3-001) — `turn.rs` is the only in-crate consumer.
// Dispatch methods on `impl Agent` stay in turn.rs and will be migrated
// in follow-up PRs (mirrors Phase 1 / actor_loop_flow pattern).
mod verifier_orchestration;
// Issue #683 (parent #680, Phase 3): scaffold / deterministic-scaffold
// pipeline data types + telemetry-event-name constants extracted from
// `turn.rs`. Hosts ScaffoldFramework / PlanExplorationKey /
// ScaffoldFallbackResult / DeterministicScaffoldSpec + 5 const
// (EVENT_DETERMINISTIC_FASTAPI_SCAFFOLD / _PYTHON_CLI /
// _FORMAT_ERROR_SMALL_EDIT / _PYTHON_TEST_FALLBACK /
// CREATE_NEXT_APP_PACKAGE_VERSION). Module is intentionally *not*
// re-exported (DR3-001) — `turn.rs` is the only in-crate consumer.
// Scaffold install / framework detection / fallback dispatch methods
// on `impl Agent` stay in turn.rs for now and will be migrated in
// follow-up PRs.
mod scaffold_pipeline;
// Issue #684 (parent #680, Phase 4): reminder dispatch orchestration extracted
// from `turn.rs`. Hosts `ReminderCallContext` + the impl that materialises a
// `ReminderInputs<'_>` view and emits the `agent.reminder.*` log event. The
// reminder helper (`reminder.rs` sibling) keeps its public SSOT (Inputs /
// Outcome / build_log_payload); only the orchestration that assembles the
// context lives here. `pub(super)` limited / no facade re-export (DR3-001) —
// `turn.rs` is the only in-crate consumer.
mod reminder_pipeline;
// Issue #685 (parent #680, Phase 5): streaming reply render state + chunk-
// handling flow extracted from `turn.rs`. Hosts
// `StreamingReplyRenderState` + `handle_streaming_assistant_chunk` /
// `finish_streaming_assistant_reply` + the prefix/trailing-newline
// predicates. The renderer is `None` when markdown is fully disabled;
// otherwise a fresh `tui::markdown::MarkdownRenderer` is built per
// stream. `pub(super)` limited / no facade re-export (DR3-001) —
// `turn.rs` is the only in-crate consumer.
mod streaming_reply;
// Issue #688 (parent #680, Phase 8): photon feedback derive core
// (`PhotonOutcomeInputs` / `PhotonFeedbackOutcome` / `case_f_condition_met`
// + the static-allowlist `PHOTON_OUTCOME_DETAIL_NO_PROGRESS_DESPITE_INJECT`
// const) extracted from `turn.rs`. `pub(super)` for the types (turn.rs
// internal); the const stays `pub` via the existing `pub use` re-export
// below so `tests/photon_evaluate_signal_smoke.rs` keeps working without
// path changes.
mod photon_feedback_derive;
// Answer-only mode shell-command allowlist + script-execution fallback
// response builder extracted from `turn.rs` (parent #680). Hosts
// `answer_only_script_command_allowed` + the prefix / blocked-contains
// / blocked-prefix const lists + `truncate_for_answer` +
// `answer_only_script_execution_fallback_response`. `pub(super)`
// limited / no facade re-export (DR3-001) — `turn.rs` is the only
// in-crate consumer.
mod answer_only_mode;
// `FeedbackFrame` builder helpers + path-extraction utilities used by
// the bash / edit failure pipelines, extracted from `turn.rs` (parent
// #680). Hosts `build_feedback_for_{bash,unsafe_block_reason,edit_failure}`
// + `bash_outcome_primary_error` + `extract_{suspected_files,path_tokens,current_request}_*`.
// `pub(super)` limited / no facade re-export (DR3-001) — `turn.rs` is
// the only in-crate consumer.
mod feedback_builders;
// Case-record extraction helpers + supporting agent-layer projections
// (`derive_language_stack` for `RepoFingerprint`, `build_anvil_test_summary`
// orchestration boundary) extracted from `turn.rs` (parent #680).
// Hosts `case_record_auto_test_active` / `case_record_extraction_succeeded`
// / `case_record_initial_feedback` / `derive_language_stack` /
// `build_anvil_test_summary`. `pub(super)` limited / no facade re-export
// (DR3-001).
mod case_record_extract;
// Issue #453: per-prompt precaution selection pipeline extracted from
// `turn.rs` (parent #680). Hosts `select_precautions_for_prompt` +
// `normalize_relevance_key` / `relevance_keyset_from_*` /
// `relevance_score` / `sort_precautions_for_prompt` /
// `apply_budget_caps`. `select_precautions_for_prompt` is re-exported
// below via `pub use` (pre-existing surface), the rest is `pub(super)`
// (DR3-001).
mod precaution_relevance;
// Issue #576 / #579 / #580 confirmation-flow plumbing extracted from
// `turn.rs` (parent #680). Hosts `should_writeback_first_pass`,
// `effective_turn_index_for_stage`, `preflight_*_skip_reason`,
// `quality_confirm_cached_result`, `work_mode_confirm_parse_status`,
// `log_*_confirm_outcome`, `override_feedback_kind_from_outcome`.
// `pub(super)` limited / no facade re-export (DR3-001).
mod confirmation_flow;
// Deterministic fallback-plan generator extracted from `turn.rs`
// (parent #680). Used by `scaffold_pipeline.rs::maybe_materialize_plan_after_timeout`
// when the main planning model fails to produce a usable plan.
// Hosts `deterministic_timeout_fallback_plan` + 3 supporting helpers
// (`extract_requested_port`, `fallback_plan_request_label`,
// `fallback_plan_platform_label`). `pub(super)` limited / no facade
// re-export (DR3-001).
mod deterministic_fallback_plan;
// Workspace-relative path normalization + package.json/lock sync
// helpers extracted from `turn.rs` (parent #680). Hosts
// `normalize_memory_path`, `normalize_exploration_path`,
// `sync_package_json_with_existing_lock`. `pub(super)` limited / no
// facade re-export (DR3-001).
mod path_helpers;
// Plan-section progress helpers extracted from `turn.rs` (parent #680).
// Hosts `join_sections_for_progress`, `plan_sections_with_content`,
// `plan_section_body_for_progress` + the private heading normalization
// SSOT (`plan_section_has_content`, `normalize_plan_heading_for_progress`).
// `pub(super)` limited / no facade re-export (DR3-001).
mod plan_sections;
// Plan-mode helpers extracted from `turn.rs` (parent #680). Hosts
// `should_materialize_plan_after_timeout`,
// `should_materialize_plan_after_tool_call_format_error`,
// `should_fallback_plan_model_after_timeout`, `assistant_model_for_mode`,
// `plan_file_alias`, `prune_plan_mode_messages`,
// `is_plan_mode_only_system_note`. `pub(super)` limited / no facade
// re-export (DR3-001).
mod plan_mode_helpers;
// Small standalone helpers extracted from `turn.rs` (parent #680). Hosts
// `rfc3339_now_utc`, `masked_path_hash_bounded_list`,
// `latest_tool_result_since_last_user`, `raw_mode_safe_text`,
// `user_interrupt_result`, `anti_pattern_failed_action_summary`.
// `pub(super)` limited / no facade re-export (DR3-001).
mod small_helpers;
// v0.4.13 Phase 6: verifier rerun progress classifier.
mod repair_progress;
mod safe_stop_payload;
// Issue #647 (Phase A.1): semantic repair planning — bounded failure-report
// schema and deterministic cluster-key generation. Module is intentionally
// *not* re-exported (DR3-001) — future consumers (`turn.rs`, `repair_job.rs`)
// reach in via `super::semantic_failure::*`.
mod semantic_failure;
// v0.4.25: semantic verifier-repair planning bridge. Converts parsed
// diagnostic reports and admitted legacy assessments into SemanticRepairPlan
// values outside the actor loop dispatcher.
mod semantic_repair_planning;
pub mod slash_commands;
// Issue #647 (Phase A.2): spec-authority enum + tie-break scoring + test/impl
// weakening detectors. Module is intentionally *not* re-exported (DR3-001) —
// future consumers (`turn.rs`, `repair_job.rs`) reach in via
// `super::spec_authority::*`.
mod spec_authority;
mod spinner;
mod success;
// Per-iteration progress line rendering extracted from `turn.rs` (parent
// #680). Hosts `ProgressDisplay`, `progress_path_display`, `tool_display`
// (entry point) and the per-tool projection helpers (write/edit/read/bash/
// search/default plus plan_read/workspace_read shells). `pub(super)` limited
// / no facade re-export (DR3-001).
mod tool_display;
// v0.4.25: tool-call history projection helpers. Keeps conversation evidence
// lookup out of both the actor loop dispatcher and RepairJob state machine.
mod tool_history;
// v0.4.26: typed tool execution outcome boundary. Keeps artifact evidence
// interpretation separate from low-level tool dispatch as turn.rs is reduced
// toward a coordinator.
mod tool_execution;
// Issue #654 (CB-001): in-crate `#[cfg(test)]` E2E suite for the bounded
// safe-stop-report pipeline. The seam set above is `#[cfg(test)]`-only, so
// the cross-crate `tests/bounded_safe_stop_report_e2e.rs` integration file
// has been migrated here to keep the seams unreachable from release builds.
#[cfg(test)]
mod safe_stop_e2e_tests;
// Repair-runner contract tests extracted from `turn.rs` (parent #680).
// Hosts `repair_lifecycle_event_tests` (pure-fn timeout / shadow validation)
// and `v0421_repair_runner_contract_tests` (source-string grep assertions
// pinning production invariants). #[cfg(test)] only; production binary
// excludes this mod. No facade re-export (DR3-001).
#[cfg(test)]
mod repair_runner_contract_tests;
// Photon-feedback derive unit tests extracted from `turn.rs` (parent
// #680). Hosts `derive_photon_feedback_outcome_tests`,
// `is_rerun_trigger_tests`, `rerun_hint_eligibility_tests`, and
// `prepare_adopted_ids_for_evaluate_tests`. #[cfg(test)] only; production
// binary excludes this mod. No facade re-export (DR3-001).
#[cfg(test)]
mod photon_feedback_derive_tests;
// turn.rs `mod tests` extracted to a sibling file (parent #680).
// Inner mod accesses turn.rs items via `super::X` (resolved through
// `use super::turn::*;` at the wrapper file scope) and sibling modules
// / loop_run items via `super::super::X` (depth preserved).
// #[cfg(test)] only. No facade re-export (DR3-001).
#[cfg(test)]
mod turn_tests;
// turn.rs `mod progress_tests` extracted to a sibling file (parent #680).
// Inner mod accesses turn.rs items via `super::X` and sibling modules /
// loop_run items via `super::super::X` — same pattern as `turn_tests`.
// #[cfg(test)] only. No facade re-export (DR3-001).
#[cfg(test)]
mod progress_tests;
// turn.rs `mod truncate_tests` extracted to a sibling file (parent #680).
// Same pattern as turn_tests / progress_tests. #[cfg(test)] only.
// No facade re-export (DR3-001).
#[cfg(test)]
mod truncate_tests;
// Issue #666: structured per-turn job reports
// (ArtifactCompletion/Verification/Repair/Memory). Private mod, no
// facade re-export (DR3-001). `turn.rs` is the only in-crate consumer
// via `super::job_report::*`.
mod job_report;
// Issue #666: in-crate `#[cfg(test)]` E2E suite (CB-001 fix pattern,
// safe_stop_e2e_tests.rs precedent). Production binary does not include
// this module.
#[cfg(test)]
mod job_report_e2e_tests;
// Issue #664: in-crate `#[cfg(test)]` E2E suite for Bash/Setup policy
// wiring (CB-001 fix pattern, `safe_stop_e2e_tests.rs` /
// `job_report_e2e_tests.rs` / `behavior_contract_projection_e2e_tests.rs`
// precedent). Production binary does not include this module (DR3-001).
#[cfg(test)]
mod bash_policy_e2e_tests;
// Issue #667: PAM advisory adapter SSOT (pure functions + decision types).
// Module is intentionally *not* re-exported (DR3-001) — `turn.rs` is the
// only behavioral in-crate consumer via the `record_pam_advisory_decision`
// thin shell on `impl Agent`.
mod pam_advisory;

/// Issue #667 (DR2-004 Tier-2 / CB-001 precedent): drive
/// `Agent::record_pam_advisory_decision` from `pam_advisory_e2e_tests`
/// without widening the production API. `#[cfg(test)]` keeps the seam out
/// of release builds entirely (matches the precedent set by
/// `emit_safe_stop_report_*_for_test` and `maybe_emit_job_reports_for_test`).
#[cfg(test)]
pub(in crate::agent::loop_run) fn record_pam_advisory_decision_for_test(
    agent: &mut Agent,
    resp: &crate::photon::schema::ContextPackResponse,
    blocked_ids: &std::collections::HashSet<String>,
    shadow: bool,
) {
    let _ = agent.record_pam_advisory_decision(resp, blocked_ids, shadow);
}

#[cfg(test)]
impl Agent {
    /// Issue #667 test-only accessor: snapshot of the PAM decision carrier.
    pub(in crate::agent::loop_run) fn last_pam_decision_this_turn(
        &self,
    ) -> Option<&pam_advisory::PamAdvisoryDecision> {
        self.last_pam_decision_this_turn.as_ref()
    }

    /// Issue #667 test-only accessor: deep clone of the active job selection
    /// so a test can assert "adapter did not mutate selection state".
    pub(in crate::agent::loop_run) fn last_active_job_selection_clone(
        &self,
    ) -> Option<active_job_arbiter::ActiveJobSelection> {
        self.last_active_job_selection.clone()
    }

    /// Issue #667 iteration-2 test-only seam: simulate the turn-boundary
    /// reset that production `handle_user_message` performs at the top of
    /// each turn. Lets the T5 boundary test drive multiple turns without
    /// constructing a full request lifecycle. `#[cfg(test)]` keeps this out
    /// of release binaries.
    pub(in crate::agent::loop_run) fn reset_last_pam_decision_for_test(&mut self) {
        self.last_pam_decision_this_turn = None;
    }
}

/// Issue #667 (CB-001): in-crate test helpers exposed via a thin shim
/// module so the e2e tests can reference schema-pin sample payloads and
/// constants without exporting them across the crate boundary.
#[cfg(test)]
pub(in crate::agent::loop_run) mod tests_export {
    use crate::agent::loop_run::pam_advisory::{
        MAX_PAM_DECISION_LIST_LEN, PamAdvisoryDecision, PamAdvisoryDecisionPayload,
        PamAdvisoryMode, ShadowVsLiveDiff, SuppressedSummary, SuppressionReason,
    };

    /// Re-export of `MemoryReport::PAYLOAD_SCHEMA_VERSION` for the
    /// regression test (T17 schema-pin sibling). Issue #667 must remain 1.
    pub const MEMORY_REPORT_SCHEMA_VERSION: u32 = {
        use super::job_report::JobReport;
        <super::job_report::MemoryReport as JobReport>::PAYLOAD_SCHEMA_VERSION
    };

    /// Schema-pin shape: build a sample `PamAdvisoryDecisionPayload`
    /// matching the documented JSON wire format for the "live" mode.
    pub struct PamAdvisoryDecisionPayloadShape;

    impl PamAdvisoryDecisionPayloadShape {
        pub fn sample_live() -> serde_json::Value {
            let decision = PamAdvisoryDecision {
                mode: PamAdvisoryMode::Live,
                injected_summary_ids: vec!["s1".to_string()],
                injected_summary_ids_truncated: false,
                suppressed_summary_ids: vec![SuppressedSummary {
                    summary_id: "s2".to_string(),
                    reason: SuppressionReason::RoleMismatch,
                }],
                suppressed_summary_ids_truncated: false,
                shadow_vs_live_diff: None,
                active_job_role: "ArtifactRecovery:test".to_string(),
            };
            decision.to_json_value()
        }

        pub fn sample_shadow() -> serde_json::Value {
            let decision = PamAdvisoryDecision {
                mode: PamAdvisoryMode::Shadow,
                injected_summary_ids: Vec::new(),
                injected_summary_ids_truncated: false,
                suppressed_summary_ids: Vec::new(),
                suppressed_summary_ids_truncated: false,
                shadow_vs_live_diff: Some(ShadowVsLiveDiff {
                    would_inject_in_live: vec!["sX".to_string()],
                    would_inject_in_live_truncated: false,
                }),
                active_job_role: ":".to_string(),
            };
            decision.to_json_value()
        }
    }

    /// Schema-pin sample for the per-list cap (T7): build a payload with
    /// 16 injected ids and the truncated flag set so the cap is observable
    /// in the wire format without spinning up a full adapter call.
    pub fn sample_truncated_decision_payload() -> serde_json::Value {
        let ids: Vec<String> = (0..MAX_PAM_DECISION_LIST_LEN)
            .map(|i| format!("s{i:02}"))
            .collect();
        let decision = PamAdvisoryDecision {
            mode: PamAdvisoryMode::Live,
            injected_summary_ids: ids,
            injected_summary_ids_truncated: true,
            suppressed_summary_ids: Vec::new(),
            suppressed_summary_ids_truncated: false,
            shadow_vs_live_diff: None,
            active_job_role: ":".to_string(),
        };
        decision.to_json_value()
    }

    // Type-export so the wire wrapper struct stays reachable for
    // PamAdvisoryDecisionPayload-direct serde verification, if needed.
    #[allow(dead_code)]
    pub(in crate::agent::loop_run) type Payload = PamAdvisoryDecisionPayload;

    /// Issue #667 T24 boundary regression: expose the per-list cap constant
    /// so the e2e test can pin the documented value (16) without piercing
    /// `pam_advisory.rs::pub(super)` visibility.
    pub const MAX_PAM_DECISION_LIST_LEN_FOR_TEST: usize = MAX_PAM_DECISION_LIST_LEN;

    /// Issue #667 iteration-3 BND-002 boundary regression: expose the
    /// envelope cap so the e2e proof can pin the documented value (8 KiB)
    /// without piercing `job_report.rs::pub(super)` visibility.
    pub const MAX_REPORT_PAYLOAD_BYTES_FOR_TEST: usize =
        super::job_report::MAX_REPORT_PAYLOAD_BYTES;

    /// Issue #667 iteration-3 BND-002: build the design's section 6 decision-3
    /// worst-case `PamAdvisoryDecision` — all three per-list caps fully
    /// saturated at `MAX_PAM_DECISION_LIST_LEN = 16` entries, every
    /// `summary_id` synthesised at exactly the renderer SSOT cap
    /// (`MAX_BLOCKED_SUMMARY_ID_BYTES = 256` bytes) — and return the
    /// `PamAdvisoryDecisionPayload::to_json_value()` projection that flows
    /// into `MemoryReport.pam_decision`. The caller pairs this with
    /// `enforce_envelope_bounds_with_pam_decision_for_test` to verify the
    /// envelope cap proof (`section 6 decision 3`).
    pub fn build_worst_case_pam_decision_payload_for_test() -> serde_json::Value {
        // Each id is exactly 256 bytes of ASCII `a` — the exact SSOT cap.
        let max_id: String = "a".repeat(crate::photon::prompt::MAX_BLOCKED_SUMMARY_ID_BYTES);
        let injected: Vec<String> = (0..MAX_PAM_DECISION_LIST_LEN)
            .map(|i| {
                // Vary one suffix char so ids are distinct but stay at the
                // 256-byte cap exactly.
                let mut id = max_id.clone();
                let prefix_len = format!("{i:02}").len();
                id.replace_range(0..prefix_len, &format!("{i:02}"));
                id
            })
            .collect();
        let suppressed: Vec<SuppressedSummary> = (0..MAX_PAM_DECISION_LIST_LEN)
            .map(|i| {
                let mut id = max_id.clone();
                let prefix = format!("s{i:02}");
                id.replace_range(0..prefix.len(), &prefix);
                SuppressedSummary {
                    summary_id: id,
                    reason: SuppressionReason::RoleMismatch,
                }
            })
            .collect();
        let would_inject: Vec<String> = (0..MAX_PAM_DECISION_LIST_LEN)
            .map(|i| {
                let mut id = max_id.clone();
                let prefix = format!("w{i:02}");
                id.replace_range(0..prefix.len(), &prefix);
                id
            })
            .collect();
        let decision = PamAdvisoryDecision {
            mode: PamAdvisoryMode::Shadow,
            injected_summary_ids: injected,
            injected_summary_ids_truncated: true,
            suppressed_summary_ids: suppressed,
            suppressed_summary_ids_truncated: true,
            shadow_vs_live_diff: Some(ShadowVsLiveDiff {
                would_inject_in_live: would_inject,
                would_inject_in_live_truncated: true,
            }),
            active_job_role: "ArtifactRecovery:test".to_string(),
        };
        decision.to_json_value()
    }

    /// Issue #667 iteration-3 BND-002 envelope-cap proof helper. Wraps the
    /// given `PamAdvisoryDecision` JSON into a `MemoryReport`, builds the
    /// envelope via the production `build_envelope::<MemoryReport>` chokepoint
    /// and runs `enforce_bounds`. Returns
    /// `(serialized_len_after_enforce, overflowed, truncated)` so the e2e
    /// proof can assert `serialized_len <= MAX_REPORT_PAYLOAD_BYTES` and the
    /// payload was NOT replaced with `"<dropped:overflow>"`.
    pub fn enforce_envelope_bounds_with_pam_decision_for_test(
        pam_decision: serde_json::Value,
    ) -> (usize, bool, bool) {
        use super::job_report::{MemoryReport, build_envelope, enforce_bounds};
        let report = MemoryReport {
            turn_index: 1,
            pam_decision: Some(pam_decision),
            context_pack_binding: None,
            adopted_item_count: 0,
            injection_skipped_reason: None,
        };
        let mut envelope = build_envelope(&report);
        let (overflowed, truncated) = enforce_bounds(&mut envelope);
        let serialized_len = serde_json::to_string(&envelope)
            .map(|s| s.len())
            .unwrap_or(usize::MAX);
        (serialized_len, overflowed, truncated)
    }
}
// Issue #667: in-crate `#[cfg(test)]` E2E suite for the PAM advisory
// pipeline (CB-001 fix pattern, `safe_stop_e2e_tests.rs` /
// `job_report_e2e_tests.rs` precedent). Production binary does not include
// this module (DR3-001).
#[cfg(test)]
mod pam_advisory_e2e_tests;

/// Test seam (#[cfg(test)] only): drive
/// `Agent::maybe_emit_job_reports_with_linkage` from
/// `job_report_e2e_tests` without widening the production API.
///
/// CB-004 fix: empty turns produce no Reports by design. The test seam
/// passes a synthetic linkage reason so the 3 linked Reports (Artifact,
/// Verification, Repair) are forced observable for channel-wiring
/// tests, and pre-seeds Memory observability via
/// `last_injected_summary_ids`. Production callers go through
/// `maybe_emit_job_reports` (no linkage override).
#[cfg(test)]
pub(crate) fn maybe_emit_job_reports_for_test(agent: &mut Agent) {
    if agent.last_injected_summary_ids.is_empty() {
        agent
            .last_injected_summary_ids
            .push("test-seed-summary-id".to_string());
    }
    agent.maybe_emit_job_reports_with_linkage(Some("test_synthetic".to_string()));
}

// Issue #665 (Phase 8 / S7-001 / CB-001): in-crate E2E test module for
// `BehaviorContractProjection` consumer wiring + `agent.behavior_contract.projected`
// observability event. `#[cfg(test)]` ensures production binary does NOT
// include the module. Rust does not auto-discover sibling test files, so
// this explicit `mod` declaration is REQUIRED.
#[cfg(test)]
mod behavior_contract_projection_e2e_tests;
mod summary;
mod task_contract;
// Issue #646: task workspace scope detection. Module is intentionally
// *not* re-exported (DR3-001) — `turn.rs` is the only in-crate consumer
// via `super::task_workspace_scope::*`.
mod task_workspace_scope;
mod tester;
// Workspace walker helpers extracted from `turn.rs` (parent #680).
// Hosts `workspace_appears_empty`, `meaningful_workspace_files`,
// `collect_meaningful_workspace_files`. `pub(super)` limited / no
// facade re-export (DR3-001).
mod tool_policy;
mod turn;
pub(crate) mod verifier_skill;
pub(crate) mod work_mode_confirm;
mod workspace_walk;
// Workspace candidate lookup helpers extracted from `turn.rs` (parent
// #680). Hosts `existing_workspace_candidate_for_role_in_scope` (the
// scope-aware lookup used by `task_contract_artifact_states` /
// `task_contract_recovery_target`) plus the test-only legacy un-scoped
// variant and `target_path_in_scope`. `pub(super)` limited / no facade
// re-export (DR3-001).
mod workspace_candidates;

// Public re-exports so `lib.rs::run_cli` can hand a `FooterHandle` into
// `Agent::new` and own the matching `FooterLease` for its scope (issue #430).
pub use footer::{FooterHandle, FooterLease};

// Re-export env helpers so `src/tui/markdown.rs` can reuse the
// existing POSIX-compliant NO_COLOR and UTF-8 locale logic without duplicating
// it. `mod turn;` itself stays private; only these two fns leak out (issue #431).
pub(crate) use progress_text::no_color_requested;
pub(crate) use progress_text::unicode_supported;

// Issue #453: expose the precaution prompt selector so integration tests in
// `tests/` (and any future callers) can validate the Act-mode prompt
// selection pipeline without requiring a live Ollama call.
pub use precaution_relevance::select_precautions_for_prompt;

// Issue #556: expose pure helper functions so `tests/photon_turn_hook_smoke.rs`
// can verify truncation and injection-message building without constructing
// a full Agent (Ollama-free).
pub use photon_feedback_derive::{
    MAX_PHOTON_CONTEXT_PACK_PROMPT_BYTES, build_photon_injection_message,
    truncate_photon_context_pack,
};

// Issue #601: expose the Case F `outcome_detail` static-allowlist literal so
// `tests/photon_evaluate_signal_smoke.rs` can grep / assert against the same
// SSOT used by production. `mod turn;` is private, so a `pub const` alone is
// not reachable from integration tests; this re-export widens visibility.
pub use photon_feedback_derive::PHOTON_OUTCOME_DETAIL_NO_PROGRESS_DESPITE_INJECT;

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

#[doc(hidden)]
pub fn acquire_footer_with_terminal_flag_for_test(
    config: &crate::config::Config,
    stdout_is_terminal: bool,
) -> FooterLease {
    footer::FooterLease::acquire_with_terminal_flag_for_test(config, stdout_is_terminal)
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
    let evidence = verifier_orchestration::build_verifier_exit_zero_evidence(outcome)?;
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
    crate::agent::loop_run::safe_stop_emit::emit_safe_stop_report_for_diagnostic_target_missing(
        agent,
    );
}

/// Issue #654 (E.3) test seam: invoke the `verifier_failed_safe_stop` emit
/// shell. In production this fires at `drive_task_contract_verifier`'s
/// attempt-limit branch.
#[cfg(test)]
pub(crate) fn emit_safe_stop_report_verifier_failed_safe_stop_for_test(agent: &mut Agent) {
    if agent.repair_job.is_none() {
        agent.repair_job = Some(repair_job::RepairJob::empty_synthetic());
    }
    crate::agent::loop_run::safe_stop_emit::emit_safe_stop_report_for_verifier_failed_safe_stop(
        agent,
    );
}

/// Issue #654 (E.4) test seam: invoke the `verifier_weak` emit shell. In
/// production this fires when `VerifierRepairPassOutcome::Invalid` exhausts
/// the controller repair-pass retry budget.
#[cfg(test)]
pub(crate) fn emit_safe_stop_report_verifier_weak_for_test(agent: &mut Agent) {
    if agent.repair_job.is_none() {
        agent.repair_job = Some(repair_job::RepairJob::empty_synthetic());
    }
    crate::agent::loop_run::safe_stop_emit::emit_safe_stop_report_for_verifier_weak(agent);
}

/// Issue #662 test seam: invoke the `repair_exhausted` emit shell. In
/// production this fires when `record_repair_attempt_outcome` reports
/// `PromotionResult.all_clusters_exhausted = true` at either the Applied
/// path (`turn.rs::drive_task_contract_verifier`) or the Invalid path
/// (`record_controller_verifier_repair_invalid`).
///
/// `#[cfg(test)] pub(crate)` scoping matches the existing 5 stop-reason
/// in-crate E2E seams introduced by Codex review v1 / CB-001 (the
/// `safe_stop_e2e_tests.rs` precedent — production binary excludes the test
/// mod, the seam is invisible to release builds, and DR3-001 holds because
/// no internal `pub(super)` type leaks across the `pub(crate)` boundary).
#[cfg(test)]
pub(crate) fn emit_safe_stop_report_repair_exhausted_for_test(agent: &mut Agent) {
    if agent.repair_job.is_none() {
        agent.repair_job = Some(repair_job::RepairJob::empty_synthetic());
    }
    verifier_orchestration::emit_safe_stop_report_for_repair_exhausted(agent);
}

/// Issue #662 (Codex CB-002): production-path test seam that drives the
/// full Applied / Invalid caller observation pipeline. The Codex review v1
/// CB-002 finding was that the Applied caller in `drive_task_contract_verifier`
/// and the Invalid caller in `record_controller_verifier_repair_invalid`
/// independently observed `PromotionResult` and called the emit shell — a
/// CB-002-style regression in either site would leak past the unit test
/// surface. This seam pairs the **production** `record_repair_attempt_outcome`
/// and `maybe_emit_repair_exhausted_from_promotion` helpers so the in-crate
/// E2E suite exercises the actual production observation/emit pair.
///
///   1. seed `Agent.repair_job` with a `RepairJob` whose `semantic_plan`
///      carries a single cluster bound to `cluster_label` + `role_label` so
///      the `record_repair_attempt_outcome` precondition (`semantic_plan =
///      Some`) is satisfied;
///   2. push two outcomes (the caller picks the variants as `kind_labels`
///      so the test can cover Applied caller paths
///      (`applied_no_progress` / `applied_worsened`) or Invalid caller
///      paths (`rejected_noop` / `rejected_malformed` / `rejected_duplicate`));
///   3. after each push, invoke the **production**
///      `Agent::maybe_emit_repair_exhausted_from_promotion` helper — the
///      very same observation/emit pair the Applied caller (in
///      `drive_task_contract_verifier`) and the Invalid caller (in
///      `record_controller_verifier_repair_invalid`) call in production.
///
/// **`#[cfg(test)] pub(crate)` scoping (DR3-001 / `private_interfaces`)**:
/// only `&mut Agent` and string literals cross the `pub(crate)` boundary,
/// so the internal types (`ArtifactRole` / `RepairAttemptOutcomeKind` /
/// `SemanticRepairPlan`) stay `pub(super)` to the `loop_run` module,
/// mirroring `emit_safe_stop_report_artifact_completion_failed_for_test`'s
/// `role_label: &str` convention. The seam is invisible to release builds.
///
/// Unknown labels are mapped to a deterministic fallback (Implementation
/// role / RejectedNoop kind) so a typo never silently produces a different
/// behavioural path than the test intended.
#[cfg(test)]
pub(crate) fn drive_record_repair_attempt_outcomes_for_test(
    agent: &mut Agent,
    cluster_label: &str,
    role_label: &str,
    kind_labels: &[&str],
) {
    use repair_attempt_outcome::{RepairAttemptOutcome, RepairAttemptOutcomeKind};
    use repair_job::{RepairJob, SemanticRepairPlan};
    use semantic_failure::{cluster_key_for_test, parse_semantic_failure_report};
    use spec_authority::SpecAuthority;

    let role = match role_label {
        "test" => task_contract::ArtifactRole::Test,
        "setup" => task_contract::ArtifactRole::Setup,
        "usage_docs" => task_contract::ArtifactRole::UsageDocs,
        // Implementation is the deterministic fallback for unknown labels.
        _ => task_contract::ArtifactRole::Implementation,
    };

    // Build a minimal semantic_report with exactly one cluster bound to
    // `cluster_label` so `next_repairable_cluster` returns `None` (= all
    // clusters exhausted) as soon as the `(cluster, role)` lands in
    // `exhausted_attempts`. The cluster's `cluster_key` is overridden to
    // match `cluster_key_for_test(cluster_label)`, mirroring the existing
    // `semantic_report_fixture_with_cluster` repair_job.rs helper.
    let json = serde_json::json!({
        "failure_kind": "assertion_mismatch",
        "confidence": 0.7,
        "preferred_repair_role": role_label,
        "repair_hypothesis": "hypothesis text",
        "failure_clusters": [
            {
                "observed": cluster_label,
                "expected": "exp",
                "input_shape": "shape",
                "assertion_shape": "AssertEq",
                "involved_artifacts": ["test"],
                "affected_cases": ["case1"],
            }
        ],
    });
    let mut report = parse_semantic_failure_report(&json).expect("fixture parses");
    let cluster_key = cluster_key_for_test(cluster_label);
    if let Some(cluster) = report.failure_clusters.get_mut(0) {
        cluster.cluster_key = cluster_key.clone();
        cluster
            .admitted_cluster_targets
            .push(task_contract::RecoveryTargetHint {
                role,
                path: format!("tests/{cluster_label}_smoke.rs"),
                reason: "CB-002 fixture".to_string(),
            });
    }
    let plan = SemanticRepairPlan {
        semantic_report: report,
        failure_cluster_id: cluster_key.clone(),
        semantic_cause: crate::agent::loop_run::VerifierDiagnosticFailureKind::AssertionMismatch,
        spec_authority: SpecAuthority::BehaviorContract,
        preferred_repair_role: role,
        repair_hypothesis: "h".to_string(),
        expected_improvement: None,
        assessment_generation_at_creation: 0,
    };
    let job = RepairJob {
        semantic_plan: Some(plan),
        ..RepairJob::new_for_test()
    };
    agent.repair_job = Some(job);

    // Drive each pushed outcome through the production observation/emit
    // pair (`record_repair_attempt_outcome` + `maybe_emit_repair_exhausted_
    // from_promotion`). The second push (count >= 2 for the same (cluster,
    // role)) is what flips `all_clusters_exhausted` to `true` for this
    // single-cluster fixture.
    for label in kind_labels {
        // Map the test-supplied label to the internal kind. Unknown labels
        // fall through to `RejectedNoop` (safe default — Invalid caller
        // path, no weakening metadata required).
        let kind = match *label {
            "applied_no_progress" => RepairAttemptOutcomeKind::AppliedNoProgress,
            "applied_worsened" => RepairAttemptOutcomeKind::AppliedWorsened,
            "rejected_noop" => RepairAttemptOutcomeKind::RejectedNoop,
            "rejected_duplicate" => RepairAttemptOutcomeKind::RejectedDuplicate,
            "rejected_malformed" => RepairAttemptOutcomeKind::RejectedMalformed,
            _ => RepairAttemptOutcomeKind::RejectedNoop,
        };
        let outcome = RepairAttemptOutcome::for_test(cluster_key.clone(), role, kind);
        let promotion = agent
            .repair_job
            .as_mut()
            .map(|job| job.record_repair_attempt_outcome(outcome));
        verifier_orchestration::maybe_emit_repair_exhausted_from_promotion(agent, promotion);
    }
}

/// Issue #654 (E.5) test seam: invoke the `verifier_missing` emit shell
/// (`FromMissingVerifier` builder).
#[cfg(test)]
pub(crate) fn emit_safe_stop_report_verifier_missing_for_test(agent: &mut Agent) {
    if agent.missing_verifier_job.is_none() {
        agent.missing_verifier_job = Some(repair_job::MissingVerifierJob::new(1, 0));
    }
    crate::agent::loop_run::safe_stop_emit::emit_safe_stop_report_for_verifier_missing(agent);
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
    crate::agent::loop_run::safe_stop_emit::emit_safe_stop_report_for_artifact_completion_failed(
        agent,
        role,
        expected_target,
    );
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
    crate::agent::loop_run::artifact_ledger_state::seed_artifact_ledger_repo_edit(
        agent, &path, role, &scope,
    );
}

// Issue #664 — in-crate `#[cfg(test)]` test seams for the Bash/Setup
// policy E2E suite. The seams follow the precedent established by
// #654 (`emit_safe_stop_report_*_for_test`) / #659
// (`seed_artifact_ledger_repo_edit_for_test`) / #666
// (`maybe_emit_job_reports_for_test`): `#[cfg(test)] pub(crate)`-only,
// signature accepts primitives + `&{,mut} Agent`, never leaks an
// internal `pub(super)` type across the boundary (DR3-001 / AD19 /
// S7-003 / DR2-005). Production code MUST NOT reach into these.

/// Issue #664 test seam: install a freshly built `ArtifactCompletionJob`
/// for the requested role on the agent so the structured report
/// projection pipeline (Phase 4 `attempt_outcome_to_json_value` +
/// `job_report.rs::maybe_emit_job_reports`) can be exercised without
/// driving the full recovery target plumbing. Accepts a role label
/// string + workspace-relative path (primitives only — `ArtifactRole`
/// stays `pub(super)`).
///
/// The seam:
/// 1. Maps the role string to `ArtifactRole` via the same vocabulary
///    used by `emit_safe_stop_report_artifact_completion_failed_for_test`.
/// 2. Materialises the path as a real file under `agent.work_root` so
///    `ArtifactCompletionJob::new` succeeds (it canonicalises the path
///    via `classify_ownership`).
/// 3. Installs the resulting job at `agent.artifact_completion_job`.
#[cfg(test)]
pub(crate) fn seed_artifact_completion_job_pending_for_test(
    agent: &mut Agent,
    role_label: &str,
    relative_path: &str,
) {
    use std::fs;
    let role = match role_label {
        "test" => task_contract::ArtifactRole::Test,
        "usage_docs" => task_contract::ArtifactRole::UsageDocs,
        "setup" => task_contract::ArtifactRole::Setup,
        _ => task_contract::ArtifactRole::Implementation,
    };

    // Materialise the file under work_root so `ArtifactCompletionJob::new`
    // accepts the target. Parent directories are created best-effort.
    let target_full = agent.work_root.join(relative_path);
    if let Some(parent) = target_full.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&target_full, "// seeded for bash_policy_e2e_tests\n");

    let hint = task_contract::RecoveryTargetHint {
        role,
        path: relative_path.to_string(),
        reason: "664 e2e seed".to_string(),
    };

    let scope = agent.current_workspace_scope();
    let work_root = agent.work_root.clone();
    if let Ok(job) = artifact_completion_job::ArtifactCompletionJob::new(
        &work_root, &scope, hint, true,  // edited_this_session
        false, // scaffold_changed
    ) {
        agent.artifact_completion_job = Some(job);
    }
}

/// Issue #664 iteration-2 (CB-001) test seam: drive the per-turn Stage A
/// observation flag (`owned_test_verifier_missing_observed_this_turn`)
/// so `bash_policy_e2e_tests.rs` can exercise both halves of the
/// SetupBootstrap decision tree without spinning up a real verifier
/// run. Production code sets the flag in
/// `run_task_contract_verifier_once`'s `OwnedTestVerifierPlan::Missing`
/// arm; this seam mirrors that producer for unit-level E2E.
///
/// The seam takes a `bool` so it can also un-set the flag — useful for
/// pinning the Stage-A-false branch (false-positive suppression
/// regression) from the same fixture.
#[cfg(test)]
pub(crate) fn seed_owned_test_verifier_missing_for_test(agent: &mut Agent, observed: bool) {
    agent.owned_test_verifier_missing_observed_this_turn = observed;
}

/// Issue #664 iteration-3 (CB2-001) test seam: drive the cross-turn
/// carryover for the Stage A observation. Mirrors the production
/// setter inside `run_task_contract_verifier_once`'s
/// `OwnedTestVerifierPlan::Missing` arm without requiring a live
/// verifier run + SafeStop cycle.
///
/// Issue #664 iteration-4 (CB3-001): the seam takes an
/// `Option<&str>` so tests can bind the carryover to a specific
/// originating request text (`Some`) or clear it (`None`). The raw
/// text is immediately piped through `mask_secrets` +
/// `stable_path_hash` inside `RequestCarryoverKey::from_request`, so
/// the seam never persists the raw string on the Agent.
#[cfg(test)]
pub(crate) fn seed_owned_test_verifier_missing_carryover_for_test(
    agent: &mut Agent,
    originating_request_text: Option<&str>,
) {
    agent.owned_test_verifier_missing_observed_carryover =
        originating_request_text.map(task_contract::RequestCarryoverKey::from_request);
}

/// Issue #664 iteration-3 (CB2-001) test seam: simulate the next-turn
/// entry into `run_actor_loop` so unit tests can drive the carryover
/// → `_this_turn` promotion without spinning up the full actor loop.
///
/// Issue #664 iteration-4 (CB3-001): the seam now takes
/// `current_request_text: Option<&str>` to mirror the
/// request-bound consume semantics implemented in
/// `run_actor_loop`'s head. The carryover promotes into
/// `_this_turn` iff the current-turn key equals the stored key;
/// otherwise the carryover is cleared without promotion (topic
/// switch / unrelated request).
#[cfg(test)]
pub(crate) fn consume_carryover_at_actor_loop_head_for_test(
    agent: &mut Agent,
    current_request_text: Option<&str>,
) {
    let promoted = match (
        agent.owned_test_verifier_missing_observed_carryover.take(),
        current_request_text,
    ) {
        (Some(stored), Some(current)) => {
            let current_key = task_contract::RequestCarryoverKey::from_request(current);
            stored == current_key
        }
        // No stored carryover OR no current request text → no promotion.
        _ => false,
    };
    agent.owned_test_verifier_missing_observed_this_turn = promoted;
}

/// Issue #664 iteration-3 (CB2-001) test seam: read both Stage A
/// flags as a primitive `(observed_this_turn, carryover_present)`
/// tuple. Used by `bash_policy_e2e_tests` to assert the consume-once
/// invariant.
///
/// Issue #664 iteration-4 (CB3-001): the second tuple slot is now
/// `carryover.is_some()` rather than a raw bool field — the seam
/// never returns the underlying `RequestCarryoverKey` so the hash
/// digest stays inside the `pub(super)` boundary (DR3-001 /
/// forgeability-safe).
#[cfg(test)]
pub(crate) fn owned_test_verifier_missing_flags_for_test(agent: &Agent) -> (bool, bool) {
    (
        agent.owned_test_verifier_missing_observed_this_turn,
        agent
            .owned_test_verifier_missing_observed_carryover
            .is_some(),
    )
}

/// Issue #664 iteration-4 (CB3-001) test seam: assert that the raw
/// request text is never directly stored in the carryover field.
/// Returns `true` iff a carryover is present AND its 16-hex
/// `originating_request_hash` is NOT equal to the raw `needle`
/// text — proving the carryover key passed through
/// `mask_secrets` + `stable_path_hash` (the hash digest differs
/// from any raw input).
///
/// Returns `false` (= "raw text NOT detected"; the safe outcome) if
/// the carryover is absent.
#[cfg(test)]
pub(crate) fn carryover_key_raw_text_not_stored_for_test(agent: &Agent, needle: &str) -> bool {
    match &agent.owned_test_verifier_missing_observed_carryover {
        Some(key) => {
            let hash = key.originating_request_hash_for_test();
            !hash.contains(needle) && hash != needle
        }
        None => true,
    }
}

/// Issue #664 test seam: project the production `build_arbiter_candidates`
/// output into a primitive 4-field DTO array (`kind` /
/// `desired_action_label` / `allowed_tool_names` / `budget_kind`).
///
/// The seam never returns the internal `pub(super)` types
/// (`JobCandidate` / `EffectiveToolPolicy` / `Budget` etc.) so DR3-001 /
/// AD19 / DR2-005 are upheld at the type level (the return type itself
/// is the contract — see `test_seam_return_type_is_primitive_only`).
///
/// Each element shape:
/// ```jsonc
/// {
///   "kind": "VerifierRepair" | "ForcedSmallEditRecovery" | ... | "SetupBootstrap",
///   "desired_action_label": "verifier_repair" | ... | "setup_bash",
///   "allowed_tool_names": ["Bash"]   // None policy → empty array
///   "budget_kind": "unbounded" | "bounded"
/// }
/// ```
#[cfg(test)]
pub(crate) fn build_arbiter_candidates_for_test(agent: &Agent) -> Vec<serde_json::Value> {
    let candidates =
        crate::agent::loop_run::test_seams::build_arbiter_candidates_pub_for_test(agent);
    candidates
        .into_iter()
        .map(|c| {
            let allowed_tool_names: Vec<String> = c
                .policy
                .allowed_tool_names_for_prompt()
                .map(|tools| tools.iter().map(|s| (*s).to_string()).collect())
                .unwrap_or_default();
            let budget_kind = match c.budget {
                active_job_arbiter::Budget::Unbounded => "unbounded",
                active_job_arbiter::Budget::Bounded { .. } => "bounded",
            };
            serde_json::json!({
                "kind": c.kind.as_str(),
                "desired_action_label": c.desired_action.label(),
                "allowed_tool_names": allowed_tool_names,
                "budget_kind": budget_kind,
            })
        })
        .collect()
}

/// Issue #664 test seam: project the production `effective_tool_policy()`
/// into a primitive `(Vec<String>, String)` tuple
/// (`(allowed_tool_names, reason_label)`).
///
/// As with `build_arbiter_candidates_for_test`, the return type carries
/// only primitives so the internal `EffectiveToolPolicy` /
/// `EffectiveToolPolicyReason` types stay private to the `loop_run`
/// module (DR3-001 / DR2-005).
#[cfg(test)]
pub(crate) fn effective_tool_policy_for_test(agent: &Agent) -> (Vec<String>, String) {
    let policy = crate::agent::loop_run::test_seams::effective_tool_policy_pub_for_test(agent);
    let allowed_tool_names: Vec<String> = policy
        .allowed_tool_names_for_prompt()
        .map(|tools| tools.iter().map(|s| (*s).to_string()).collect())
        .unwrap_or_default();
    let reason_label = policy.reason().as_str().to_string();
    (allowed_tool_names, reason_label)
}

/// Issue #664 iteration-3 (CB2-003) test seam: drive the
/// `effective_tool_policy_error_for_call_with_scope` rejection +
/// `record_artifact_completion_bash_violation` chokepoint under an
/// `artifact_directed_from_job` policy that was built from the currently-
/// installed `ArtifactCompletionJob`.
///
/// Returns
/// `Some((error_string, bash_policy_violation_marker, attempts_after))`
/// — `bash_policy_violation_marker` is the marker observed on the
/// **latest** attempt (`None` when no attempt was recorded). Returns
/// `None` when no `ArtifactCompletionJob` is installed (the seam refuses
/// to fabricate a policy without a real job).
///
/// All return values are primitive types (`String` / `bool` / `usize`)
/// so `EffectiveToolPolicy` / `ArtifactAttemptOutcome` stay private to
/// the `loop_run` module (DR3-001 / AD19 / DR2-005).
#[cfg(test)]
pub(crate) fn drive_artifact_directed_policy_error_for_test(
    agent: &mut Agent,
    name: &str,
    arguments: serde_json::Value,
) -> Option<(Option<String>, Option<bool>, usize)> {
    let policy = crate::agent::loop_run::test_seams::artifact_directed_policy_for_test(agent)?;
    let (err, _delta) = crate::agent::loop_run::test_seams::drive_policy_error_for_test(
        agent, &policy, name, &arguments,
    );
    let marker =
        crate::agent::loop_run::test_seams::last_attempt_bash_policy_violation_for_test(agent);
    let attempts_after =
        crate::agent::loop_run::test_seams::artifact_completion_job_attempts_len_for_test(agent)
            .unwrap_or(0);
    Some((err, marker, attempts_after))
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
    /// Issue #660 (Phase C / DD-4): per-turn diff-based dedup state for the
    /// `agent.active_job.selected` structured log event. Holds the
    /// `ActiveJobSelection` most recently passed to
    /// `emit_active_job_selected_if_changed`; the helper re-emits only when
    /// the new selection differs. Reset at the head of every
    /// `handle_user_message` adjacent to `safe_stop_report_emitted.clear()`
    /// (per DR1-007 — per-turn reset group locality).
    ///
    /// NOT serialized — `SessionSnapshot` / `CaseRecord` / `EvalTurnRecord`
    /// persistence schemas are unchanged by Issue #660 (in-memory only,
    /// same pattern as `safe_stop_report_emitted` and `artifact_ledger`).
    pub(in crate::agent::loop_run) last_active_job_selection:
        Option<active_job_arbiter::ActiveJobSelection>,
    /// Issue #665 (Phase 6 / S5-006 / S7-002): per-turn diff-based dedup state
    /// for the `agent.behavior_contract.projected` structured log event. Holds
    /// the **payload-shaped key** (NOT the raw `BehaviorContractProjection`)
    /// most recently emitted. The emit helper re-emits only when the new key
    /// differs from this one. Reset at the head of every `handle_user_message`
    /// adjacent to `last_active_job_selection.take()`.
    ///
    /// **Why payload-shaped, not raw projection (S5-006)**: dedup state must
    /// not retain attacker-controlled `label` / `excerpt` content across turns.
    /// `BehaviorProjectionEventKey` contains only the `schema_version` /
    /// `consumer` / `confidence_bucket` / `fields_used` metadata, which is
    /// what the emitted event payload actually keys on.
    ///
    /// NOT serialized — in-memory only, same pattern as
    /// `last_active_job_selection`.
    pub(in crate::agent::loop_run) last_behavior_contract_projection_event:
        Option<required_behavior::BehaviorProjectionEventKey>,
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
    /// Issue #666: per-turn fire-once dedup keys for the four new
    /// `agent.{artifact_completion,verification,repair,memory}.report`
    /// events. Single namespaced HashSet — keys are
    /// `"{event_name}::{report_dedup_key}"`. Reset at the head of every
    /// `handle_user_message` adjacent to `safe_stop_report_emitted.clear()`
    /// (CLAUDE.md per-turn rule).
    ///
    /// NOT serialized — same pattern as `safe_stop_report_emitted` and
    /// `last_active_job_selection`. `SessionSnapshot` / `CaseRecord` /
    /// `EvalTurnRecord` persistence schemas remain unchanged by Issue #666.
    pub(in crate::agent::loop_run) job_report_dedup_keys: std::collections::HashSet<String>,
    /// Issue #661 Task 2.6 (DR1-004): per-turn dedup state for the
    /// `agent.verifier.invoked` structured log event. Holds the 8-byte digest
    /// of the most recent emit's canonical-JSON payload (`mask_payload_inplace`
    /// applied first — same masking as the value the log consumer sees).
    /// Reset to `None` at the head of every `handle_user_message`, alongside
    /// `last_active_job_selection` / `safe_stop_report_emitted` (per-turn
    /// reset group locality, DR1-010).
    ///
    /// NOT serialized — `SessionSnapshot` / `CaseRecord` / `EvalTurnRecord`
    /// persistence schemas are unchanged by Issue #661 (in-memory only).
    /// Iteration-3 wires the producer (see design 5-2 DR1-005):
    /// `turn.rs::run_task_contract_verifier_once` and
    /// `verifier_skill.rs::execute_with_invocation_observer`.
    #[allow(dead_code)]
    // Iteration-3 wires production update sites; until then only the per-turn reset semantics are exercised.
    pub(in crate::agent::loop_run) last_verifier_invoked_payload_digest: Option<[u8; 8]>,
    /// Issue #661 Task 2.6 (DR1-010): per-turn cap for the
    /// `agent.verifier.external_import_rejected` structured log event.
    /// Set to `true` after the first emit so duplicate detections inside
    /// the same turn do not amplify event cardinality. Reset to `false`
    /// at the head of every `handle_user_message`, mirroring
    /// `last_verifier_invoked_payload_digest`.
    ///
    /// NOT serialized (same rationale as
    /// `last_verifier_invoked_payload_digest`).
    #[allow(dead_code)]
    // Iteration-3 wires the production producer (external_import detection in run_structured); until then only the per-turn reset semantics are exercised.
    pub(in crate::agent::loop_run) external_import_rejected_emitted_this_turn: bool,
    /// Issue #664 iteration-2 (CB-001): per-turn observation flag set when
    /// `run_task_contract_verifier_once` observes
    /// `OwnedTestVerifierPlan::Missing`. Read by
    /// `build_arbiter_candidates` as Stage A of the
    /// `VerifierPrerequisiteSignal` (the live observation path,
    /// distinguished from Stage B label fallback).
    ///
    /// Reset to `false` at the head of every `handle_user_message`
    /// (alongside `verifier_safe_stop_emitted_this_turn`) so a previous
    /// turn's verifier-missing observation cannot leak into the current
    /// turn's SetupBootstrap decision tree (CLAUDE.md per-turn rule).
    ///
    /// NOT serialized (per-turn runtime state only).
    pub(in crate::agent::loop_run) owned_test_verifier_missing_observed_this_turn: bool,
    /// Issue #664 iteration-3 (CB2-001): cross-turn carryover for the
    /// Stage A `OwnedTestVerifierPlan::Missing` observation. The
    /// iteration-2 flag was reset at `run_actor_loop` head, but the
    /// only production setter is the `Missing` arm in
    /// `run_task_contract_verifier_once` which immediately returns
    /// `TaskContractVerifierOutcome::SafeStop` — the same turn never
    /// reaches a subsequent `build_arbiter_candidates` cycle that
    /// could consume the signal. Without a carryover, the next user
    /// message starts a fresh turn that resets the flag before any
    /// arbiter call can see it (`Stage A live observation set only on
    /// a terminal SafeStop path` — Codex CB2-001).
    ///
    /// The carryover preserves the Stage A observation across exactly
    /// one turn boundary so the next `handle_user_message` can promote
    /// it into `owned_test_verifier_missing_observed_this_turn` at the
    /// head of `run_actor_loop`, before the per-turn reset would
    /// otherwise drop it. The carryover is consumed exactly once and
    /// reset on the same turn so it never accumulates across multiple
    /// safe-stop cycles.
    ///
    /// Issue #664 iteration-4 (CB3-001): the carryover is now a
    /// `RequestCarryoverKey` (16-hex digest of `mask_secrets(originating
    /// request text)`) instead of a plain `bool`. The actor-loop head
    /// consumer promotes the carryover into
    /// `owned_test_verifier_missing_observed_this_turn` only when the
    /// **current** turn's request still hashes to the same key. A topic
    /// switch (different request text) clears the carryover and does
    /// NOT promote, closing the false-positive grant of the
    /// `setup_bootstrap` Bash-only policy to unrelated requests.
    ///
    /// NOT serialized (cross-turn runtime state only; #659 / #663 ledger
    /// is the authority for any persisted ownership / completion data).
    pub(in crate::agent::loop_run) owned_test_verifier_missing_observed_carryover:
        Option<task_contract::RequestCarryoverKey>,
    /// Issue #667 (DR1-004): per-turn PAM advisory decision carrier. `is_some()`
    /// is synonymous with "adapter has produced a decision this turn"; the
    /// old design's separate `pam_advisory_decided_this_turn: bool` flag is
    /// intentionally absent (SRP violation + double-write footgun).
    ///
    /// Reset to `None` at the head of every `handle_user_message` adjacent
    /// to `last_active_job_selection = None`. NOT serialized — in-memory
    /// only (same per-turn pattern as `last_active_job_selection` and
    /// `last_behavior_contract_projection_event`).
    pub(in crate::agent::loop_run) last_pam_decision_this_turn:
        Option<pam_advisory::PamAdvisoryDecision>,
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
    /// Issue #662: control-flow failure_type emitted when `StopReason::RepairExhausted`
    /// fires. `SafeStopReport::build_from` sets this variant directly via the
    /// same `if stop_reason == ...` upgrade pattern used for
    /// `DiagnosticTargetMissing`; classifier helpers never map to this variant.
    RepairExhausted,
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
            Self::RepairExhausted => "repair_exhausted",
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
            last_active_job_selection: None,
            last_behavior_contract_projection_event: None,
            artifact_ledger: artifact_ledger::ArtifactLedger::new(),
            job_report_dedup_keys: std::collections::HashSet::new(),
            // Issue #661 Task 2.6: per-turn dedup state for
            // `agent.verifier.invoked` + `agent.verifier.external_import_rejected`.
            // Reset at handle_user_message head; producers land in iteration-3.
            last_verifier_invoked_payload_digest: None,
            external_import_rejected_emitted_this_turn: false,
            // Issue #664 iteration-2 (CB-001): per-turn Stage A observation.
            owned_test_verifier_missing_observed_this_turn: false,
            // Issue #664 iteration-4 (CB3-001): request-bound carryover.
            owned_test_verifier_missing_observed_carryover: None,
            // Issue #667: PAM advisory per-turn carrier.
            last_pam_decision_this_turn: None,
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

    /// Issue #667 (DR1-008): SSOT helper that resolves the adapter's
    /// per-turn inputs (S3-008 ordering priorities (1)(2)(3) + `#665`
    /// behavior projection). Called once per turn by
    /// `record_pam_advisory_decision` so the 2 chokepoints never inline
    /// the resolution logic (DR1-005).
    pub(in crate::agent::loop_run) fn pam_advisory_inputs(
        &self,
    ) -> pam_advisory::PamAdvisoryInputs {
        // Priority (1): active-job arbiter selected kind.
        let role_hint_from_active = self
            .last_active_job_selection
            .as_ref()
            .and_then(|sel| sel.selected.as_ref())
            .and_then(|c| pam_advisory::job_kind_to_artifact_role(c.kind));

        // Priority (2): artifact_completion_job's role (SSOT for
        // role-specific completion targets).
        let role_hint_from_completion = self.artifact_completion_job.as_ref().map(|job| job.role());

        // Priority (3): TaskContract derived from active request text.
        // `first_missing_required_role` is evidence-aware and not available
        // here, so we fall back to the first required role hint
        // (DR3-004 — same pattern as `refresh_artifact_completion_satisfied`).
        let task_contract = self
            .active_request_text()
            .map(|text| task_contract::TaskContract::from_request(&text));
        let role_hint_from_contract = task_contract
            .as_ref()
            .and_then(|tc| tc.required_artifacts.first().copied());

        let role_hint = role_hint_from_active
            .or(role_hint_from_completion)
            .or(role_hint_from_contract);

        let behavior = task_contract
            .as_ref()
            .and_then(required_behavior::project_behavior_contract);

        pam_advisory::PamAdvisoryInputs {
            role_hint,
            behavior,
        }
    }

    /// Issue #667 (DR1-005): adapter SSOT entry point. The 2 production
    /// chokepoints (`turn.rs::invoke_photon_context_pack` and the path-b
    /// branch in `build_request_messages`) call this shell instead of
    /// touching the adapter directly. Performs:
    ///
    /// 1. `config.pam_advisory_enabled` gate (early `None`).
    /// 2. Per-turn dedup via `last_pam_decision_this_turn.is_some()`.
    /// 3. Builds `PamAdvisoryInputs` once.
    /// 4. Invokes `evaluate_pam_advisory` (pure fn).
    /// 5. Writes `last_pam_decision_this_turn = Some(decision)` exactly once.
    pub(in crate::agent::loop_run) fn record_pam_advisory_decision(
        &mut self,
        resp: &crate::photon::schema::ContextPackResponse,
        blocked_ids: &std::collections::HashSet<String>,
        shadow_input: bool,
    ) -> Option<pam_advisory::PamAdvisoryOutcome> {
        if !self.config.pam_advisory_enabled {
            return None;
        }
        if self.last_pam_decision_this_turn.is_some() {
            return None;
        }
        let inputs = self.pam_advisory_inputs();
        let active_ref = self.last_active_job_selection.as_ref();
        let outcome = pam_advisory::evaluate_pam_advisory(
            resp,
            blocked_ids,
            active_ref,
            inputs.role_hint,
            inputs.behavior.as_ref(),
            pam_advisory::PamAdvisoryModeInput {
                shadow: shadow_input,
            },
        );
        self.last_pam_decision_this_turn = Some(outcome.decision.clone());
        Some(outcome)
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
