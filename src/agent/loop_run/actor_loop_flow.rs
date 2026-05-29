//! Issue #681 (parent #680, Phase 1): actor loop control-flow data
//! types **and** the thin free-function helpers that operate over them,
//! extracted from `turn.rs`.
//!
//! Hosts:
//!
//! * `PostReplyRecovery*` / `ActorLoop*Args` / `ActorLoop*Outcome`
//!   struct/enum definitions consumed by the actor loop sub-flows
//!   (pre-reply / tool-preparation / no-tool / task-contract-reply /
//!   completion / post-tool / missing-repo-change / empty-reply /
//!   prose-only / plan-tool-followup) — 27 `pub(super)` items.
//! * 8 `pub(super) fn` helpers extracted from `impl Agent` /
//!   free-function space in `turn.rs`:
//!   * pure constructors (`repair_job_done_outcome`,
//!     `missing_repo_edits_finalize_outcome`,
//!     `missing_repo_change_budget_exhausted_outcome`,
//!     `plan_tool_followup_done_message`),
//!   * a pure predicate (`missing_repo_edit_recovery_allowed`),
//!   * three `&mut Agent` veneers
//!     (`handle_plan_progress_prose_only_fallback`,
//!     `handle_non_progress_plan_edit_fallback`,
//!     `finalize_missing_repo_edit_retry_exhausted`) that delegate
//!     state mutation back to `Agent` methods exposed at
//!     `pub(super)`.
//!
//! The `&mut Agent` veneer pattern is the **intentional transitional
//! shape** for Phase 1 — they bridge `Agent` state with the
//! `ActorLoop*Outcome` data flow so the actor-loop dispatcher can
//! migrate out of `turn.rs` incrementally. Long-term they may stay
//! (precedent: `pam_advisory::record_pam_advisory_decision`) or be
//! inlined into pure functions once `Agent` state can be untangled
//! per #680 Phase boundaries.
//!
//! DR3-001: `pub(super)` limited. `loop_run.rs` MUST NOT re-export via
//! `pub use`. `turn.rs` is the only in-crate consumer.

use std::collections::{HashMap, HashSet};

use crate::agent::orchestration::{
    RepoSnapshot, RepoVerification, capture_repo_snapshot, verify_repo_progress,
};
use crate::agent::recovery;
use crate::logging::log_llm_event;
use crate::model_capabilities::model_capabilities;
use crate::modes::plan_act::{ExecutionMode, PlanStage};
use crate::ollama::client::AssistantReply;
use crate::ollama::xml_fallback::ToolCall;
use crate::safety::path_guard::resolve_user_path;
use crate::session::feedback::{
    FeedbackFrame, FeedbackFrameDraft, FeedbackKind, build_feedback_frame,
};
use crate::session::store::ConversationMessage;
use crate::tools::registry::resolve_plan_mode_write_target;

use super::Agent;
use super::active_job_arbiter::{LoopControlAction, RecoveryDispatchGate, RecoveryOwner};
use super::interrupt::{InterruptFlag, InterruptMonitor};
use super::lifecycle;
use super::progress_text::{
    format_progress_field, paint, progress_available_width, sanitize_for_progress, tool_color,
    tool_emoji, truncate,
};
use super::progress_text::{no_color_requested, unicode_supported};
use super::spinner::Spinner;
use super::success::DETERMINISTIC_CONTENT_FALLBACK_TAG;
use super::summary::{ExitReason, LoopResult, LoopStats};
use super::tool_history::focused_edit_target_already_read;
use super::tool_history::is_plan_file_tool_call;
use super::tool_policy::EffectiveToolPolicy;
use super::turn::{
    LOG_ARGS_MAX_CHARS, PLAN_REPEATED_EXPLORATION_BLOCK_THRESHOLD, PlanExplorationKey,
    join_sections_for_progress, normalize_exploration_path, plan_section_body_for_progress,
    plan_sections_with_content, tool_display, write_stdout_rendered,
};
use crate::agent::prompting;
use crate::session::compact::approximate_token_count;
use crate::util::workspace_paths::is_ignored_workspace_display_path;
use std::io::{self, IsTerminal};
use std::path::Path;
use std::time::Instant;

/// Issue #652: `error_text` shared by the three `ArtifactCompletionJob`
/// exhaustion break-points (NoTool / ProseOnly / cross-iteration flag) in
/// `run_actor_loop`. Defined as a single constant so the three sites
/// stay aligned and any future copy survives review.
///
/// Originally defined in `turn.rs` (#652); relocated here under
/// `actor_loop_flow` for Issue #681 because the budget-exhausted
/// `error_text` is consumed exclusively by actor-loop completion
/// outcomes. Lineage: ownership remains Issue #652, location now
/// follows the actor-loop responsibility boundary.
pub(super) const ARTIFACT_COMPLETION_BUDGET_EXHAUSTED_TEXT: &str =
    "artifact completion role-specific retry budget exhausted";

pub(super) enum TaskContractVerifierFlowOutcome {
    Continue,
    Done {
        final_prose: String,
    },
    Exit {
        reason: ExitReason,
        error_text: String,
    },
}

pub(super) enum PostReplyRecoveryOutcome {
    Continue,
    Finalize {
        final_prose: String,
        exit_reason: ExitReason,
        error_text: String,
    },
}

pub(super) struct PostReplyRecoveryArgs<'a, 'b> {
    pub(super) last_iter: usize,
    pub(super) action_expectation: recovery::ActionExpectation,
    pub(super) requires_action: bool,
    pub(super) recovery_dispatch_gate: RecoveryDispatchGate,
    pub(super) repo_edit_calls_made_this_turn: usize,
    pub(super) final_reply: &'a str,
    pub(super) task_contract_action: Option<&'a super::task_contract::ArtifactRecoveryAction>,
    pub(super) interrupt_flag: &'a InterruptFlag,
    pub(super) repo_change_retries: &'b mut usize,
    pub(super) python_test_retries: &'b mut usize,
    pub(super) no_tool_retries: &'b mut usize,
    pub(super) framework_app_fallback_materialized: &'b mut bool,
}

pub(super) struct ActorLoopPreReplyArgs<'a, 'b> {
    pub(super) before_snapshot: &'a RepoSnapshot,
    pub(super) accumulated: &'a [RepoVerification],
    pub(super) task_contract: Option<&'a super::task_contract::TaskContract>,
    pub(super) repo_edit_calls_made_this_turn: &'b mut usize,
    pub(super) contract_verification_retries: &'b mut usize,
    pub(super) contract_verifier_repair_edit_count: &'b mut Option<usize>,
    pub(super) repo_change_retries: &'b mut usize,
    pub(super) verifier_repair_retries: &'b mut usize,
    pub(super) task_contract_verify_commands_collected: &'b mut Vec<String>,
    pub(super) task_contract_verifier_passed_in_loop: &'b mut bool,
    pub(super) framework_app_fallback_materialized: &'b mut bool,
    pub(super) action_expectation: recovery::ActionExpectation,
    pub(super) stream_output: bool,
    pub(super) last_iter: usize,
    pub(super) interrupt_flag: &'a InterruptFlag,
}

pub(super) enum ActorLoopPreReplyOutcome {
    Continue,
    Done {
        final_prose: String,
    },
    Exit {
        reason: ExitReason,
        error_text: String,
    },
    ReplyPrepared {
        reply: AssistantReply,
        recovery_dispatch_gate: RecoveryDispatchGate,
        missing_verifier_setup_turn: bool,
        recovery_owner: RecoveryOwner,
    },
}

#[derive(Clone)]
pub(super) struct ActorLoopPreReplyControlState {
    pub(super) loop_control_action: LoopControlAction,
    pub(super) recovery_owner: RecoveryOwner,
    pub(super) recovery_dispatch_gate: RecoveryDispatchGate,
    pub(super) missing_verifier_setup_turn: bool,
}

pub(super) struct ActorLoopToolPreparationArgs<'a, 'b> {
    pub(super) reply_tool_calls: Vec<ToolCall>,
    pub(super) task_contract: Option<&'a super::task_contract::TaskContract>,
    pub(super) tool_call_summaries: &'b mut Vec<crate::session::eval_log::ToolCallSummary>,
    pub(super) focused_policy_retries: &'b mut usize,
    pub(super) contract_completion_role_retries:
        &'b mut HashMap<super::task_contract::ArtifactRole, usize>,
    pub(super) missing_verifier_setup_turn: bool,
    pub(super) recovery_dispatch_gate: RecoveryDispatchGate,
    pub(super) recovery_owner: RecoveryOwner,
    pub(super) last_iter: usize,
}

pub(super) struct ActorLoopRejectedToolBatchArgs<'a, 'b> {
    pub(super) err: String,
    pub(super) effective_tool_policy: &'a EffectiveToolPolicy,
    pub(super) task_contract: Option<&'a super::task_contract::TaskContract>,
    pub(super) contract_completion_role_retries:
        &'b mut HashMap<super::task_contract::ArtifactRole, usize>,
    pub(super) focused_policy_retries: &'b mut usize,
    pub(super) missing_verifier_setup_turn: bool,
    pub(super) recovery_dispatch_gate: RecoveryDispatchGate,
    pub(super) recovery_owner: RecoveryOwner,
    pub(super) last_iter: usize,
}

pub(super) enum ActorLoopToolPreparationOutcome {
    Continue,
    Done {
        final_prose: String,
    },
    Exit {
        reason: ExitReason,
        error_text: String,
    },
    Prepared {
        current_reply_tool_call_count: usize,
        prepared_tool_calls: Vec<ToolCall>,
        effective_tool_policy: EffectiveToolPolicy,
    },
}

pub(super) struct ActorLoopNoToolReplyArgs<'a, 'b> {
    pub(super) last_iter: usize,
    pub(super) action_expectation: recovery::ActionExpectation,
    pub(super) requires_action: bool,
    pub(super) recovery_dispatch_gate: RecoveryDispatchGate,
    pub(super) final_reply: &'a str,
    pub(super) tool_calls_made_this_turn: usize,
    pub(super) repo_change_retries: &'b mut usize,
    pub(super) plan_progress_retries: &'b mut usize,
    pub(super) empty_retries: &'b mut usize,
    pub(super) no_tool_retries: &'b mut usize,
    pub(super) framework_app_fallback_materialized: &'b mut bool,
}

pub(super) enum ActorLoopNoToolReplyOutcome {
    NotHandled,
    Continue,
    Done {
        final_prose: String,
    },
    Exit {
        reason: ExitReason,
        error_text: String,
    },
}

pub(super) struct ActorLoopTaskContractReplyArgs<'a, 'b> {
    pub(super) before_snapshot: &'a RepoSnapshot,
    pub(super) accumulated: &'a [RepoVerification],
    pub(super) task_contract: Option<&'a super::task_contract::TaskContract>,
    pub(super) task_contract_action: Option<&'a super::task_contract::ArtifactRecoveryAction>,
    pub(super) final_reply: &'a str,
    pub(super) current_reply_tool_call_count: usize,
    pub(super) repo_edit_calls_made_this_turn: usize,
    pub(super) last_iter: usize,
    pub(super) contract_completion_retries: &'b mut usize,
    pub(super) contract_completion_role_retries:
        &'b mut HashMap<super::task_contract::ArtifactRole, usize>,
    pub(super) contract_verification_retries: &'b mut usize,
    pub(super) contract_verifier_repair_edit_count: &'b mut Option<usize>,
    pub(super) repo_change_retries: &'b mut usize,
    pub(super) verifier_repair_retries: &'b mut usize,
    pub(super) task_contract_verify_commands_collected: &'b mut Vec<String>,
    pub(super) task_contract_verifier_passed_in_loop: &'b mut bool,
    pub(super) no_tool_retries: &'b mut usize,
    pub(super) contract_deterministic_fallback_materialized: &'b mut bool,
}

pub(super) enum ActorLoopTaskContractReplyOutcome {
    Proceed,
    Continue,
    Done {
        final_prose: String,
    },
    Exit {
        reason: ExitReason,
        error_text: String,
    },
}

pub(super) struct ActorLoopTaskContractContinueArgs<'a, 'b> {
    pub(super) contract: &'a super::task_contract::TaskContract,
    pub(super) action: &'a super::task_contract::ArtifactRecoveryAction,
    pub(super) missing: &'a [super::task_contract::ArtifactRole],
    pub(super) target_hint: &'a Option<super::task_contract::RecoveryTargetHint>,
    pub(super) final_reply: &'a str,
    pub(super) current_reply_tool_call_count: usize,
    pub(super) last_iter: usize,
    pub(super) contract_completion_retries: &'b mut usize,
    pub(super) contract_completion_role_retries:
        &'b mut HashMap<super::task_contract::ArtifactRole, usize>,
    pub(super) contract_deterministic_fallback_materialized: &'b mut bool,
}

pub(super) struct ActorLoopTaskContractToolRecoveryArgs<'a, 'b> {
    pub(super) contract: &'a super::task_contract::TaskContract,
    pub(super) decision: &'a super::task_contract::CompletionDecision,
    pub(super) target_hint: Option<super::task_contract::RecoveryTargetHint>,
    pub(super) missing: &'a [super::task_contract::ArtifactRole],
    pub(super) final_reply: &'a str,
    pub(super) last_iter: usize,
    pub(super) contract_completion_retries: &'b mut usize,
    pub(super) contract_completion_role_retries:
        &'b mut HashMap<super::task_contract::ArtifactRole, usize>,
}

pub(super) struct ActorLoopTaskContractIncompleteArgs<'a, 'b> {
    pub(super) contract: &'a super::task_contract::TaskContract,
    pub(super) decision: super::task_contract::CompletionDecision,
    pub(super) target_hint: Option<super::task_contract::RecoveryTargetHint>,
    pub(super) missing: &'a [super::task_contract::ArtifactRole],
    pub(super) last_iter: usize,
    pub(super) contract_completion_retries: &'b mut usize,
    pub(super) contract_completion_role_retries:
        &'b mut HashMap<super::task_contract::ArtifactRole, usize>,
}

pub(super) struct ActorLoopPlanToolFollowupArgs<'a> {
    pub(super) last_iter: usize,
    pub(super) plan_ready_after_tool: bool,
    pub(super) plan_file_edit_calls_this_turn: usize,
    pub(super) plan_exploration_calls_this_turn: usize,
    pub(super) plan_missing_before_turn: Option<usize>,
    pub(super) plan_progress_retries: &'a mut usize,
    pub(super) plan_exploration_only_turns: &'a mut usize,
}

pub(super) enum ActorLoopPlanToolFollowupOutcome {
    Proceed,
    Done {
        final_prose: String,
    },
    Exit {
        reason: ExitReason,
        error_text: String,
    },
}

pub(super) struct ActorLoopCompletionArgs<'a, 'b> {
    pub(super) last_iter: usize,
    pub(super) final_reply: &'a str,
    pub(super) plan_progress_retries: &'b mut usize,
}

pub(super) enum ActorLoopCompletionOutcome {
    Continue,
    Done {
        final_prose: String,
    },
    Exit {
        reason: ExitReason,
        error_text: String,
    },
}

pub(super) struct ActorLoopPostToolFallbackArgs<'a> {
    pub(super) last_iter: usize,
    pub(super) action_expectation: recovery::ActionExpectation,
    pub(super) recovery_dispatch_gate: RecoveryDispatchGate,
    pub(super) emitted_bash_loop_note: bool,
    pub(super) bash_only_tool_turn: bool,
    pub(super) repo_edit_calls_made_this_turn: usize,
    pub(super) tool_calls_made_this_turn: usize,
    pub(super) logged_act_first_repo_edit: bool,
    pub(super) repo_change_retries: &'a mut usize,
}

pub(super) enum ActorLoopPostToolFallbackOutcome {
    Proceed,
    Continue,
    Exit {
        reason: ExitReason,
        error_text: String,
    },
}

pub(super) struct ActorLoopPostToolCleanupArgs<'a> {
    pub(super) task_contract: Option<&'a super::task_contract::TaskContract>,
    pub(super) contract_verifier_repair_edit_count: Option<usize>,
    pub(super) repo_edit_calls_made_this_turn: usize,
    pub(super) contract_completion_retries: usize,
    pub(super) tool_calls_made_this_turn: usize,
    pub(super) interrupt_flag: &'a InterruptFlag,
}

pub(super) enum ActorLoopPostToolCleanupOutcome {
    Continue,
    Exit {
        reason: ExitReason,
        error_text: String,
    },
}

#[derive(Clone, Copy)]
pub(super) enum ActorLoopMissingRepoChangeReplyKind {
    Empty,
    ProseOnly,
}

pub(super) struct ActorLoopMissingRepoChangeReplyArgs<'a> {
    pub(super) kind: ActorLoopMissingRepoChangeReplyKind,
    pub(super) last_iter: usize,
    pub(super) repo_change_retries: &'a mut usize,
    pub(super) framework_app_fallback_materialized: &'a mut bool,
}

pub(super) struct ActorLoopMissingRepoChangeRetryPromptArgs {
    pub(super) kind: ActorLoopMissingRepoChangeReplyKind,
    pub(super) last_iter: usize,
    pub(super) repo_change_retries: usize,
}

pub(super) struct ActorLoopMissingRepoChangeRetryExhaustedArgs<'a> {
    pub(super) last_iter: usize,
    pub(super) repo_change_retries: usize,
    pub(super) framework_app_fallback_materialized: &'a mut bool,
}

pub(super) struct ActorLoopEmptyReplyArgs<'b> {
    pub(super) last_iter: usize,
    pub(super) action_expectation: recovery::ActionExpectation,
    pub(super) recovery_dispatch_gate: RecoveryDispatchGate,
    pub(super) requires_action: bool,
    pub(super) repo_change_retries: &'b mut usize,
    pub(super) plan_progress_retries: &'b mut usize,
    pub(super) empty_retries: &'b mut usize,
    pub(super) framework_app_fallback_materialized: &'b mut bool,
}

pub(super) struct ActorLoopProseOnlyReplyArgs<'a, 'b> {
    pub(super) last_iter: usize,
    pub(super) action_expectation: recovery::ActionExpectation,
    pub(super) recovery_dispatch_gate: RecoveryDispatchGate,
    pub(super) tool_calls_made_this_turn: usize,
    pub(super) repo_change_retries: &'b mut usize,
    pub(super) plan_progress_retries: &'b mut usize,
    pub(super) no_tool_retries: &'b mut usize,
    pub(super) framework_app_fallback_materialized: &'b mut bool,
    pub(super) final_reply: &'a str,
}

pub(super) fn plan_tool_followup_done_message() -> String {
    "Plan complete. Reply yes to execute, no to revise, or provide feedback.".to_string()
}

pub(super) fn maybe_handle_answer_only_inadequate_recovery(
    agent: &mut Agent,
    args: &mut PostReplyRecoveryArgs<'_, '_>,
) -> Option<PostReplyRecoveryOutcome> {
    if !args.requires_action
        && agent.answer_only_mode_active()
        && answer_only_reply_is_inadequate(args.final_reply)
    {
        *args.no_tool_retries += 1;
        if *args.no_tool_retries >= 2 {
            agent
                .session
                .record_feedback_if_unset(build_feedback_for_no_tool_call(
                    "answer_only_inadequate_reply",
                    &agent.work_root,
                ));
            return Some(PostReplyRecoveryOutcome::Finalize {
                final_prose: agent.answer_only_fallback_response(),
                exit_reason: ExitReason::Done,
                error_text: String::new(),
            });
        }
        super::turn::write_stdout_rendered(
            &format_iteration_status(
                args.last_iter,
                agent.config.max_iterations,
                "Retry requested",
                "The model gave an underspecified answer in answer-only mode. Asked it to provide a concrete response.",
                agent.footer.current_cols(),
            ),
            true,
        );
        agent.push_system_note(
            "[Answer-only Recovery] Answer the user's request now with concrete findings from the available context. Do not output a tool call, do not edit files, and do not ask the user to run anything."
                .to_string(),
        );
        return Some(PostReplyRecoveryOutcome::Continue);
    }
    None
}

pub(super) fn maybe_handle_repo_change_quality_gate_recovery(
    agent: &mut Agent,
    args: &mut PostReplyRecoveryArgs<'_, '_>,
) -> Option<PostReplyRecoveryOutcome> {
    if !(should_apply_repo_change_quality_gate(
        args.action_expectation,
        agent.active_task_expects_repo_change(),
        agent.session.mode_state.mode,
    ) || agent.current_request_needs_playable_ui_quality_gate())
        || !args.recovery_dispatch_gate.allows_deterministic_fallback()
    {
        return None;
    }
    let (request, target_path, issue) = agent.accepted_repo_change_quality_issue()?;
    match agent.maybe_apply_deterministic_quality_fallback(&request, &target_path) {
        Ok(true) => {
            super::turn::write_stdout_rendered(
                &format_iteration_status(
                    args.last_iter,
                    agent.config.max_iterations,
                    "Quality fallback",
                    &format!("Replaced scaffold placeholder output in {target_path}."),
                    agent.footer.current_cols(),
                ),
                true,
            );
            agent.session.record_feedback_if_unset(
                build_feedback_for_deterministic_content_fallback(&agent.work_root),
            );
            agent.push_deterministic_ui_recovery_continuation_note(
                &target_path,
                (*args.repo_change_retries).saturating_add(1),
            );
            return Some(PostReplyRecoveryOutcome::Continue);
        }
        Ok(false) => {}
        Err(err) => {
            return Some(PostReplyRecoveryOutcome::Finalize {
                final_prose: String::new(),
                exit_reason: ExitReason::TransportError,
                error_text: err,
            });
        }
    }
    *args.repo_change_retries += 1;
    if *args.repo_change_retries >= 3 {
        return Some(PostReplyRecoveryOutcome::Finalize {
            final_prose: String::new(),
            exit_reason: ExitReason::MissingRepoEdits,
            error_text: issue,
        });
    }
    super::turn::write_stdout_rendered(
        &format_iteration_status(
            args.last_iter,
            agent.config.max_iterations,
            "Quality gate",
            &format!("Asked the model to replace placeholder output in {target_path}."),
            agent.footer.current_cols(),
        ),
        true,
    );
    agent.push_system_note(recovery::repo_change_quality_gate_note(
        &request,
        &target_path,
        &issue,
        *args.repo_change_retries,
    ));
    Some(PostReplyRecoveryOutcome::Continue)
}

pub(super) fn maybe_handle_repo_change_partial_progress_recovery(
    agent: &mut Agent,
    args: &mut PostReplyRecoveryArgs<'_, '_>,
) -> Option<PostReplyRecoveryOutcome> {
    if !should_apply_repo_change_partial_progress_recovery(
        args.action_expectation,
        args.repo_edit_calls_made_this_turn,
        args.final_reply,
        args.task_contract_action,
    ) || !args
        .recovery_dispatch_gate
        .allows_generic_repo_change_recovery()
    {
        return None;
    }
    *args.repo_change_retries += 1;
    if *args.repo_change_retries >= 3 {
        return Some(PostReplyRecoveryOutcome::Finalize {
            final_prose: String::new(),
            exit_reason: ExitReason::MissingRepoEdits,
            error_text: ExitReason::MissingRepoEdits
                .default_error_text()
                .to_string(),
        });
    }
    super::turn::write_stdout_rendered(
        &format_iteration_status(
            args.last_iter,
            agent.config.max_iterations,
            "Retry requested",
            "A small edit landed, but the model answered with next-step prose instead of a completed result. Asked it to keep implementing with tools.",
            agent.footer.current_cols(),
        ),
        true,
    );
    agent.push_system_note(recovery::repo_change_partial_progress_note(
        *args.repo_change_retries,
    ));
    Some(PostReplyRecoveryOutcome::Continue)
}

pub(super) fn maybe_handle_python_test_artifact_recovery(
    agent: &mut Agent,
    args: &mut PostReplyRecoveryArgs<'_, '_>,
) -> Option<PostReplyRecoveryOutcome> {
    if args.repo_edit_calls_made_this_turn == 0
        || !agent.active_python_request_requires_tests()
        || agent.python_test_artifact_exists()
        || agent.python_verifier_available_for_requested_tests()
    {
        return None;
    }
    *args.python_test_retries += 1;
    if *args.python_test_retries >= 2 {
        let (final_prose, exit_reason, error_text) = match agent
            .maybe_materialize_python_test_fallback()
        {
            Ok(Some(path)) => (
                format!(
                    "Added the requested Python test artifact with deterministic fallback: {path}."
                ),
                ExitReason::Done,
                String::new(),
            ),
            Ok(None) => (
                String::new(),
                ExitReason::MissingRepoEdits,
                "assistant did not add the requested Python test artifact".to_string(),
            ),
            Err(err) => (String::new(), ExitReason::TransportError, err),
        };
        return Some(PostReplyRecoveryOutcome::Finalize {
            final_prose,
            exit_reason,
            error_text,
        });
    }
    super::turn::write_stdout_rendered(
        &format_iteration_status(
            args.last_iter,
            agent.config.max_iterations,
            "Quality gate",
            "Asked the model to add the requested Python test file or self-test command.",
            agent.footer.current_cols(),
        ),
        true,
    );
    agent.push_system_note(
        "[Python Test Policy] The user explicitly requested tests. Add a concrete Python test artifact now, such as test_*.py, *_test.py, or a clearly runnable self-test command. Keep the edit small and verify it if possible."
            .to_string(),
    );
    Some(PostReplyRecoveryOutcome::Continue)
}

pub(super) fn maybe_handle_missing_repo_edit_recovery(
    agent: &mut Agent,
    args: &mut PostReplyRecoveryArgs<'_, '_>,
) -> Option<PostReplyRecoveryOutcome> {
    if !missing_repo_edit_recovery_allowed(args) {
        return None;
    }
    if maybe_continue_missing_repo_framework_fallback(agent, args) {
        return Some(PostReplyRecoveryOutcome::Continue);
    }
    if let Some(outcome) = maybe_continue_missing_repo_scaffold_fallback(agent, args) {
        return Some(outcome);
    }
    *args.repo_change_retries += 1;
    if *args.repo_change_retries >= 3 {
        return Some(finalize_missing_repo_edit_retry_exhausted(agent));
    }
    super::turn::write_stdout_rendered(
        &format_iteration_status(
            args.last_iter,
            agent.config.max_iterations,
            "Retry requested",
            "The turn finished without repository edits. Asked the model to continue implementing changes.",
            agent.footer.current_cols(),
        ),
        true,
    );
    push_missing_repo_edit_retry_note(agent, *args.repo_change_retries);
    Some(PostReplyRecoveryOutcome::Continue)
}

pub(super) fn maybe_continue_missing_repo_framework_fallback(
    agent: &mut Agent,
    args: &mut PostReplyRecoveryArgs<'_, '_>,
) -> bool {
    if !should_try_framework_app_fallback(args.last_iter, *args.framework_app_fallback_materialized)
        || !agent.maybe_materialize_framework_game_fallback(args.last_iter)
    {
        return false;
    }
    *args.framework_app_fallback_materialized = true;
    agent.push_system_note(framework_app_fallback_continuation_note().to_string());
    true
}

pub(super) fn maybe_continue_missing_repo_scaffold_fallback(
    agent: &mut Agent,
    args: &mut PostReplyRecoveryArgs<'_, '_>,
) -> Option<PostReplyRecoveryOutcome> {
    match agent.maybe_apply_deterministic_nextjs_scaffold(args.last_iter, args.interrupt_flag) {
        super::turn::ScaffoldFallbackResult::Applied => {
            *args.repo_change_retries = 0;
            Some(PostReplyRecoveryOutcome::Continue)
        }
        super::turn::ScaffoldFallbackResult::Failed
        | super::turn::ScaffoldFallbackResult::Skipped => {
            *args.repo_change_retries += 1;
            if *args.repo_change_retries >= 3 {
                return Some(missing_repo_edits_finalize_outcome());
            }
            agent.push_system_note(recovery::repo_change_recovery_note(
                *args.repo_change_retries,
            ));
            Some(PostReplyRecoveryOutcome::Continue)
        }
        super::turn::ScaffoldFallbackResult::NotApplicable => None,
    }
}

pub(super) fn push_missing_repo_edit_retry_note(agent: &mut Agent, attempt: usize) {
    if let Some(target) = agent.focused_edit_recovery_target() {
        let target_already_read =
            focused_edit_target_already_read(&agent.session.messages, &target, &agent.work_root);
        let note =
            agent.focused_edit_no_tool_note_for_target(&target, target_already_read, attempt);
        agent.push_system_note(note);
        return;
    }
    if !agent.push_artifact_directed_recovery_note(attempt) {
        agent.push_system_note(recovery::repo_change_recovery_note(attempt));
    }
}

pub(super) fn actor_loop_pre_reply_deterministic_fallback_allowed(
    repo_edit_calls_made_this_turn: usize,
    recovery_dispatch_gate: RecoveryDispatchGate,
) -> bool {
    repo_edit_calls_made_this_turn == 0 && recovery_dispatch_gate.allows_deterministic_fallback()
}

pub(super) fn actor_loop_pre_reply_repo_change_fallback_allowed(
    action_expectation: recovery::ActionExpectation,
    repo_edit_calls_made_this_turn: usize,
    recovery_dispatch_gate: RecoveryDispatchGate,
) -> bool {
    action_expectation == recovery::ActionExpectation::RepoChange
        && actor_loop_pre_reply_deterministic_fallback_allowed(
            repo_edit_calls_made_this_turn,
            recovery_dispatch_gate,
        )
}

pub(super) fn actor_loop_pre_reply_request_error(
    agent: &mut Agent,
    err: String,
) -> ActorLoopPreReplyOutcome {
    let reason = if err == super::turn::USER_INTERRUPT_ERROR {
        ExitReason::Interrupted
    } else if super::lifecycle::is_tool_call_format_error(&err) {
        ExitReason::ToolCallFormatError
    } else {
        ExitReason::TransportError
    };
    if err != super::turn::USER_INTERRUPT_ERROR
        && (super::lifecycle::is_native_tool_parser_failure(&err)
            || super::lifecycle::is_tool_call_format_error(&err)
            || super::lifecycle::is_native_tool_transport_failure(&err))
    {
        let frame = build_feedback_for_tool_protocol_failure(&err, &agent.work_root);
        agent.session.record_feedback(frame);
    }
    ActorLoopPreReplyOutcome::Exit {
        reason,
        error_text: err,
    }
}

pub(super) fn actor_loop_pre_reply_flow_outcome(
    outcome: TaskContractVerifierFlowOutcome,
) -> ActorLoopPreReplyOutcome {
    match outcome {
        TaskContractVerifierFlowOutcome::Continue => ActorLoopPreReplyOutcome::Continue,
        TaskContractVerifierFlowOutcome::Done { final_prose } => {
            ActorLoopPreReplyOutcome::Done { final_prose }
        }
        TaskContractVerifierFlowOutcome::Exit { reason, error_text } => {
            ActorLoopPreReplyOutcome::Exit { reason, error_text }
        }
    }
}

pub(super) fn handle_actor_loop_no_tool_reply(
    agent: &mut Agent,
    args: ActorLoopNoToolReplyArgs<'_, '_>,
) -> ActorLoopNoToolReplyOutcome {
    if args.final_reply.is_empty() {
        return handle_actor_loop_empty_reply(
            agent,
            ActorLoopEmptyReplyArgs {
                last_iter: args.last_iter,
                action_expectation: args.action_expectation,
                recovery_dispatch_gate: args.recovery_dispatch_gate,
                requires_action: args.requires_action,
                repo_change_retries: args.repo_change_retries,
                plan_progress_retries: args.plan_progress_retries,
                empty_retries: args.empty_retries,
                framework_app_fallback_materialized: args.framework_app_fallback_materialized,
            },
        );
    }

    if args.requires_action && args.tool_calls_made_this_turn == 0 {
        return handle_actor_loop_prose_only_reply(
            agent,
            ActorLoopProseOnlyReplyArgs {
                last_iter: args.last_iter,
                action_expectation: args.action_expectation,
                recovery_dispatch_gate: args.recovery_dispatch_gate,
                tool_calls_made_this_turn: args.tool_calls_made_this_turn,
                repo_change_retries: args.repo_change_retries,
                plan_progress_retries: args.plan_progress_retries,
                no_tool_retries: args.no_tool_retries,
                framework_app_fallback_materialized: args.framework_app_fallback_materialized,
                final_reply: args.final_reply,
            },
        );
    }

    ActorLoopNoToolReplyOutcome::NotHandled
}

pub(super) fn handle_actor_loop_empty_reply(
    agent: &mut Agent,
    args: ActorLoopEmptyReplyArgs<'_>,
) -> ActorLoopNoToolReplyOutcome {
    if args.action_expectation == recovery::ActionExpectation::RepoChange
        && args
            .recovery_dispatch_gate
            .allows_generic_repo_change_recovery()
    {
        return handle_actor_loop_missing_repo_change_reply(
            agent,
            ActorLoopMissingRepoChangeReplyArgs {
                kind: ActorLoopMissingRepoChangeReplyKind::Empty,
                last_iter: args.last_iter,
                repo_change_retries: args.repo_change_retries,
                framework_app_fallback_materialized: args.framework_app_fallback_materialized,
            },
        );
    }
    if args.action_expectation == recovery::ActionExpectation::PlanProgress {
        let plan_contents = agent
            .current_plan_contents()
            .ok()
            .flatten()
            .unwrap_or_default();
        let current_stage = super::lifecycle::current_plan_stage(&plan_contents);
        let next_sections = super::lifecycle::plan_next_stage_sections(&plan_contents);
        *args.plan_progress_retries += 1;
        if *args.plan_progress_retries >= 2 {
            return match agent.materialize_deterministic_fallback_plan(
                "agent.plan.progress_fallback_materialized",
            ) {
                Ok(true) => ActorLoopNoToolReplyOutcome::Done {
                    final_prose:
                        "Plan complete. Reply yes to execute, no to revise, or provide feedback."
                            .to_string(),
                },
                Ok(false) => ActorLoopNoToolReplyOutcome::Exit {
                    reason: ExitReason::PlanIncomplete,
                    error_text: ExitReason::PlanIncomplete.default_error_text().to_string(),
                },
                Err(err) => ActorLoopNoToolReplyOutcome::Exit {
                    reason: ExitReason::TransportError,
                    error_text: err,
                },
            };
        }
        super::turn::write_stdout_rendered(
            &format_iteration_status(
                args.last_iter,
                agent.config.max_iterations,
                "Retry requested",
                &format!(
                    "The model returned an empty reply. Asked it to continue the plan by writing {}.",
                    join_sections_for_progress(&next_sections)
                ),
                agent.footer.current_cols(),
            ),
            true,
        );
        let missing_sections = super::lifecycle::plan_missing_sections(&plan_contents);
        log_plan_stall(
            agent.session_store.session_id(),
            args.last_iter,
            "empty_reply",
            current_stage,
            &next_sections,
            &missing_sections,
            *args.plan_progress_retries,
        );
        agent.push_system_note(recovery::plan_progress_recovery_note(
            current_stage,
            &next_sections,
            &missing_sections,
            *args.plan_progress_retries,
        ));
        return ActorLoopNoToolReplyOutcome::Continue;
    }

    *args.empty_retries += 1;
    if *args.empty_retries >= 3 {
        return ActorLoopNoToolReplyOutcome::Exit {
            reason: ExitReason::EmptyResponses,
            error_text: ExitReason::EmptyResponses.default_error_text().to_string(),
        };
    }
    super::turn::write_stdout_rendered(
        &format_iteration_status(
            args.last_iter,
            agent.config.max_iterations,
            "Retry requested",
            "The model returned an empty reply. Asked it to continue with concrete tool actions.",
            agent.footer.current_cols(),
        ),
        true,
    );
    agent.push_system_note(recovery::empty_response_recovery_note(
        *args.empty_retries,
        args.requires_action,
    ));
    ActorLoopNoToolReplyOutcome::Continue
}

pub(super) fn handle_actor_loop_prose_only_reply(
    agent: &mut Agent,
    mut args: ActorLoopProseOnlyReplyArgs<'_, '_>,
) -> ActorLoopNoToolReplyOutcome {
    if args.tool_calls_made_this_turn != 0 {
        return ActorLoopNoToolReplyOutcome::NotHandled;
    }
    if args.action_expectation == recovery::ActionExpectation::RepoChange
        && args
            .recovery_dispatch_gate
            .allows_generic_repo_change_recovery()
    {
        return handle_actor_loop_missing_repo_change_reply(
            agent,
            ActorLoopMissingRepoChangeReplyArgs {
                kind: ActorLoopMissingRepoChangeReplyKind::ProseOnly,
                last_iter: args.last_iter,
                repo_change_retries: args.repo_change_retries,
                framework_app_fallback_materialized: args.framework_app_fallback_materialized,
            },
        );
    }
    if args.action_expectation == recovery::ActionExpectation::PlanProgress {
        return handle_plan_progress_prose_only_reply(agent, &mut args);
    }
    handle_generic_prose_only_retry(agent, args.last_iter, args.no_tool_retries)
}

fn handle_plan_progress_prose_only_reply(
    agent: &mut Agent,
    args: &mut ActorLoopProseOnlyReplyArgs<'_, '_>,
) -> ActorLoopNoToolReplyOutcome {
    let plan_contents = agent
        .current_plan_contents()
        .ok()
        .flatten()
        .unwrap_or_default();
    if agent.plan_is_substantive_with_fallback(&plan_contents) {
        return ActorLoopNoToolReplyOutcome::Done {
            final_prose: args.final_reply.to_string(),
        };
    }
    let current_stage = super::lifecycle::current_plan_stage(&plan_contents);
    let next_sections = super::lifecycle::plan_next_stage_sections(&plan_contents);
    *args.plan_progress_retries += 1;
    if *args.plan_progress_retries >= 2 {
        return handle_plan_progress_prose_only_fallback(agent);
    }
    super::turn::write_stdout_rendered(
        &format_iteration_status(
            args.last_iter,
            agent.config.max_iterations,
            "Retry requested",
            &format!(
                "The model answered without tool calls. Asked it to update {} with Write or Edit.",
                join_sections_for_progress(&next_sections)
            ),
            agent.footer.current_cols(),
        ),
        true,
    );
    let missing_sections = super::lifecycle::plan_missing_sections(&plan_contents);
    log_plan_stall(
        agent.session_store.session_id(),
        args.last_iter,
        "no_tool_reply",
        current_stage,
        &next_sections,
        &missing_sections,
        *args.plan_progress_retries,
    );
    agent.push_system_note(recovery::plan_no_tool_recovery_note(
        current_stage,
        &next_sections,
        *args.plan_progress_retries,
    ));
    ActorLoopNoToolReplyOutcome::Continue
}

fn handle_generic_prose_only_retry(
    agent: &mut Agent,
    last_iter: usize,
    no_tool_retries: &mut usize,
) -> ActorLoopNoToolReplyOutcome {
    *no_tool_retries += 1;
    if *no_tool_retries >= 3 {
        agent
            .session
            .record_feedback_if_unset(build_feedback_for_no_tool_call(
                "no_tool_retries_exhausted",
                &agent.work_root,
            ));
        return ActorLoopNoToolReplyOutcome::Exit {
            reason: ExitReason::NoToolCalls,
            error_text: ExitReason::NoToolCalls.default_error_text().to_string(),
        };
    }
    super::turn::write_stdout_rendered(
        &format_iteration_status(
            last_iter,
            agent.config.max_iterations,
            "Retry requested",
            "The model answered without tool calls. Asked it to continue with concrete actions.",
            agent.footer.current_cols(),
        ),
        true,
    );
    agent.push_system_note(recovery::no_tool_recovery_note(*no_tool_retries));
    ActorLoopNoToolReplyOutcome::Continue
}

fn handle_actor_loop_missing_repo_change_reply(
    agent: &mut Agent,
    args: ActorLoopMissingRepoChangeReplyArgs<'_>,
) -> ActorLoopNoToolReplyOutcome {
    *args.repo_change_retries += 1;
    if *args.repo_change_retries >= 2 {
        return handle_actor_loop_missing_repo_change_retry_exhausted(
            agent,
            ActorLoopMissingRepoChangeRetryExhaustedArgs {
                last_iter: args.last_iter,
                repo_change_retries: *args.repo_change_retries,
                framework_app_fallback_materialized: args.framework_app_fallback_materialized,
            },
        );
    }
    handle_actor_loop_missing_repo_change_retry_prompt(
        agent,
        ActorLoopMissingRepoChangeRetryPromptArgs {
            kind: args.kind,
            last_iter: args.last_iter,
            repo_change_retries: *args.repo_change_retries,
        },
    )
}

fn handle_actor_loop_missing_repo_change_retry_prompt(
    agent: &mut Agent,
    args: ActorLoopMissingRepoChangeRetryPromptArgs,
) -> ActorLoopNoToolReplyOutcome {
    super::turn::write_stdout_rendered(
        &format_iteration_status(
            args.last_iter,
            agent.config.max_iterations,
            "Retry requested",
            missing_repo_change_retry_status_note(args.kind),
            agent.footer.current_cols(),
        ),
        true,
    );

    match args.kind {
        ActorLoopMissingRepoChangeReplyKind::Empty => {
            handle_empty_missing_repo_change_retry(agent, args.repo_change_retries)
        }
        ActorLoopMissingRepoChangeReplyKind::ProseOnly => {
            handle_prose_only_missing_repo_change_retry(agent, args.repo_change_retries)
        }
    }
}

fn handle_empty_missing_repo_change_retry(
    agent: &mut Agent,
    repo_change_retries: usize,
) -> ActorLoopNoToolReplyOutcome {
    if !agent.push_artifact_directed_recovery_note(repo_change_retries)
        && !agent.push_repo_change_no_edit_recovery_note(repo_change_retries)
    {
        agent.push_system_note(recovery::repo_change_recovery_note(repo_change_retries));
    }
    if agent.record_artifact_completion_attempt(
        super::artifact_completion_job::ArtifactAttemptOutcomeKind::NoTool,
        Vec::new(),
    ) {
        return missing_repo_change_budget_exhausted_outcome();
    }
    ActorLoopNoToolReplyOutcome::Continue
}

fn handle_prose_only_missing_repo_change_retry(
    agent: &mut Agent,
    repo_change_retries: usize,
) -> ActorLoopNoToolReplyOutcome {
    if let Some(target) = agent.focused_edit_recovery_target() {
        let target_already_read =
            focused_edit_target_already_read(&agent.session.messages, &target, &agent.work_root);
        let note = agent.focused_edit_no_tool_note_for_target(
            &target,
            target_already_read,
            repo_change_retries,
        );
        agent.push_system_note(note);
    } else if !agent.push_artifact_directed_recovery_note(repo_change_retries)
        && !agent.push_repo_change_no_edit_recovery_note(repo_change_retries)
    {
        agent.push_system_note(recovery::repo_change_no_tool_recovery_note(
            repo_change_retries,
        ));
    }
    if agent.record_artifact_completion_attempt(
        super::artifact_completion_job::ArtifactAttemptOutcomeKind::ProseOnly,
        Vec::new(),
    ) {
        return missing_repo_change_budget_exhausted_outcome();
    }
    ActorLoopNoToolReplyOutcome::Continue
}

fn handle_actor_loop_missing_repo_change_retry_exhausted(
    agent: &mut Agent,
    args: ActorLoopMissingRepoChangeRetryExhaustedArgs<'_>,
) -> ActorLoopNoToolReplyOutcome {
    if args.repo_change_retries == 2
        && (agent.push_artifact_directed_recovery_note(args.repo_change_retries)
            || agent.push_repo_change_no_edit_recovery_note(args.repo_change_retries))
    {
        super::turn::write_stdout_rendered(
            &format_iteration_status(
                args.last_iter,
                agent.config.max_iterations,
                "Retry requested",
                "Asked the model to continue with one allowed repository edit on the target artifact.",
                agent.footer.current_cols(),
            ),
            true,
        );
        return ActorLoopNoToolReplyOutcome::Continue;
    }
    let request = agent.active_request_text().unwrap_or_default();
    let fallback = match agent.maybe_apply_local_llm_small_edit_fallback(&request) {
        Ok(fallback) => fallback,
        Err(err) => {
            return ActorLoopNoToolReplyOutcome::Exit {
                reason: ExitReason::TransportError,
                error_text: err,
            };
        }
    };
    if let Some(relative) = fallback {
        return ActorLoopNoToolReplyOutcome::Done {
            final_prose: format!(
                "Applied a verified small edit fallback after the local model stopped before editing {relative}."
            ),
        };
    }
    if should_try_framework_app_fallback(args.last_iter, *args.framework_app_fallback_materialized)
        && agent.maybe_materialize_framework_game_fallback(args.last_iter)
    {
        *args.framework_app_fallback_materialized = true;
        agent.push_system_note(framework_app_fallback_continuation_note().to_string());
        return ActorLoopNoToolReplyOutcome::Continue;
    }
    ActorLoopNoToolReplyOutcome::Exit {
        reason: ExitReason::MissingRepoEdits,
        error_text: ExitReason::MissingRepoEdits
            .default_error_text()
            .to_string(),
    }
}

pub(super) fn handle_actor_loop_post_tool_cleanup(
    agent: &mut Agent,
    args: ActorLoopPostToolCleanupArgs<'_>,
) -> ActorLoopPostToolCleanupOutcome {
    sync_post_tool_contract_recovery_target(agent, &args);
    agent.maybe_invoke_reminder(args.interrupt_flag);
    run_post_tool_cleanup_compaction(
        agent,
        args.tool_calls_made_this_turn,
        args.repo_edit_calls_made_this_turn,
    );
    if let Some(outcome) = post_tool_cleanup_interrupt_outcome(args.interrupt_flag) {
        return outcome;
    }
    ActorLoopPostToolCleanupOutcome::Continue
}

fn sync_post_tool_contract_recovery_target(
    agent: &mut Agent,
    args: &ActorLoopPostToolCleanupArgs<'_>,
) {
    if agent.session.mode_state.mode == super::ExecutionMode::Plan {
        return;
    }
    let Some(contract) = args.task_contract else {
        return;
    };
    let action = agent.task_contract_recovery_action(
        contract,
        args.contract_verifier_repair_edit_count,
        args.repo_edit_calls_made_this_turn,
    );
    match action {
        super::task_contract::ArtifactRecoveryAction::Continue { .. }
        | super::task_contract::ArtifactRecoveryAction::RepairArtifact { .. } => {
            agent.set_artifact_recovery_target_for_action(
                &action,
                args.contract_completion_retries.saturating_add(1),
            );
        }
        super::task_contract::ArtifactRecoveryAction::RunVerifier
        | super::task_contract::ArtifactRecoveryAction::Done => {
            agent.clear_artifact_recovery_target("contract_artifacts_satisfied");
        }
        super::task_contract::ArtifactRecoveryAction::SafeStop { reason } => {
            agent.clear_artifact_recovery_target(task_contract_safe_stop_clear_tag(reason));
        }
    }
}

fn run_post_tool_cleanup_compaction(
    agent: &mut Agent,
    tool_calls_made_this_turn: usize,
    repo_edit_calls_made_this_turn: usize,
) {
    if agent.session.mode_state.mode == super::ExecutionMode::Plan {
        return;
    }
    let compacted = agent
        .maybe_compact_late_turn_session(tool_calls_made_this_turn, repo_edit_calls_made_this_turn);
    if !compacted {
        agent.maybe_compact_session(super::DEFAULT_KEEP_TAIL);
    }
}

fn post_tool_cleanup_interrupt_outcome(
    interrupt_flag: &InterruptFlag,
) -> Option<ActorLoopPostToolCleanupOutcome> {
    if interrupt_flag.is_set() {
        return Some(ActorLoopPostToolCleanupOutcome::Exit {
            reason: ExitReason::Interrupted,
            error_text: String::new(),
        });
    }
    None
}

pub(super) fn handle_actor_loop_completion(
    agent: &mut Agent,
    args: ActorLoopCompletionArgs<'_, '_>,
) -> ActorLoopCompletionOutcome {
    if agent.session.mode_state.mode == super::ExecutionMode::Plan {
        let plan_contents = match agent.current_plan_contents() {
            Ok(contents) => contents.unwrap_or_default(),
            Err(err) => {
                return ActorLoopCompletionOutcome::Exit {
                    reason: ExitReason::TransportError,
                    error_text: err,
                };
            }
        };
        agent.session.mode_state.plan_stage = super::lifecycle::current_plan_stage(&plan_contents);
        if !agent.plan_is_substantive_with_fallback(&plan_contents) {
            let next_sections = super::lifecycle::plan_next_stage_sections(&plan_contents);
            let missing_sections = super::lifecycle::plan_missing_sections(&plan_contents);
            let current_stage = super::lifecycle::current_plan_stage(&plan_contents);
            *args.plan_progress_retries += 1;
            if *args.plan_progress_retries >= 2 {
                return match agent.materialize_deterministic_fallback_plan(
                    "agent.plan.progress_fallback_materialized",
                ) {
                    Ok(true) => ActorLoopCompletionOutcome::Done {
                        final_prose:
                            "Plan complete. Reply yes to execute, no to revise, or provide feedback."
                                .to_string(),
                    },
                    Ok(false) => ActorLoopCompletionOutcome::Exit {
                        reason: ExitReason::PlanIncomplete,
                        error_text: ExitReason::PlanIncomplete.default_error_text().to_string(),
                    },
                    Err(err) => ActorLoopCompletionOutcome::Exit {
                        reason: ExitReason::TransportError,
                        error_text: err,
                    },
                };
            }
            super::turn::write_stdout_rendered(
                &format_iteration_status(
                    args.last_iter,
                    agent.config.max_iterations,
                    "Plan still incomplete",
                    &format!(
                        "Asked the model to finish {} before approval.",
                        join_sections_for_progress(&missing_sections)
                    ),
                    agent.footer.current_cols(),
                ),
                true,
            );
            log_plan_stall(
                agent.session_store.session_id(),
                args.last_iter,
                "plan_incomplete_after_reply",
                current_stage,
                &next_sections,
                &missing_sections,
                *args.plan_progress_retries,
            );
            agent.push_system_note(recovery::plan_progress_recovery_note(
                current_stage,
                &next_sections,
                &missing_sections,
                *args.plan_progress_retries,
            ));
            return ActorLoopCompletionOutcome::Continue;
        }
    }
    ActorLoopCompletionOutcome::Done {
        final_prose: args.final_reply.to_string(),
    }
}

pub(super) fn handle_actor_loop_post_tool_fallbacks(
    agent: &mut Agent,
    args: ActorLoopPostToolFallbackArgs<'_>,
) -> ActorLoopPostToolFallbackOutcome {
    record_actor_loop_post_tool_notes(
        agent,
        args.action_expectation,
        args.emitted_bash_loop_note,
        args.bash_only_tool_turn,
        args.repo_edit_calls_made_this_turn,
        args.logged_act_first_repo_edit,
    );
    match handle_actor_loop_post_tool_no_edit_fallbacks(
        agent,
        args.last_iter,
        args.recovery_dispatch_gate,
        args.repo_edit_calls_made_this_turn,
        args.tool_calls_made_this_turn,
        args.repo_change_retries,
    ) {
        ActorLoopPostToolFallbackOutcome::Proceed => {}
        outcome => return outcome,
    }
    match handle_actor_loop_post_tool_repo_edit_quality_gate(
        agent,
        args.last_iter,
        args.action_expectation,
        args.recovery_dispatch_gate,
        args.repo_edit_calls_made_this_turn,
        args.repo_change_retries,
    ) {
        ActorLoopPostToolFallbackOutcome::Proceed => {}
        outcome => return outcome,
    }
    ActorLoopPostToolFallbackOutcome::Proceed
}

fn record_actor_loop_post_tool_notes(
    agent: &mut Agent,
    action_expectation: recovery::ActionExpectation,
    emitted_bash_loop_note: bool,
    bash_only_tool_turn: bool,
    repo_edit_calls_made_this_turn: usize,
    logged_act_first_repo_edit: bool,
) {
    if emitted_bash_loop_note {
        agent.push_system_note(recovery::install_loop_recovery_note());
    } else if agent.session.mode_state.mode == super::ExecutionMode::Act
        && action_expectation == recovery::ActionExpectation::RepoChange
        && bash_only_tool_turn
        && repo_edit_calls_made_this_turn == 0
        && !logged_act_first_repo_edit
    {
        agent.push_system_note(recovery::repo_change_after_setup_note());
    }
}

fn handle_actor_loop_post_tool_no_edit_fallbacks(
    agent: &mut Agent,
    last_iter: usize,
    recovery_dispatch_gate: RecoveryDispatchGate,
    repo_edit_calls_made_this_turn: usize,
    tool_calls_made_this_turn: usize,
    repo_change_retries: &mut usize,
) -> ActorLoopPostToolFallbackOutcome {
    if repo_edit_calls_made_this_turn != 0
        || tool_calls_made_this_turn == 0
        || !recovery_dispatch_gate.allows_deterministic_fallback()
        || !agent.current_request_needs_playable_ui_quality_gate()
    {
        return ActorLoopPostToolFallbackOutcome::Proceed;
    }
    if let Some((request, target_path)) = agent.accepted_repo_change_polish_target() {
        return handle_actor_loop_post_tool_polish_fallback(
            agent,
            last_iter,
            &request,
            &target_path,
            repo_change_retries,
        );
    }
    if let Some((request, target_path, _issue)) = agent.accepted_repo_change_quality_issue() {
        return handle_actor_loop_post_tool_quality_fallback(
            agent,
            last_iter,
            &request,
            &target_path,
            repo_change_retries,
        );
    }
    ActorLoopPostToolFallbackOutcome::Proceed
}

pub(super) fn handle_actor_loop_post_tool_polish_fallback(
    agent: &mut Agent,
    last_iter: usize,
    request: &str,
    target_path: &str,
    repo_change_retries: &mut usize,
) -> ActorLoopPostToolFallbackOutcome {
    match agent.maybe_apply_deterministic_polish_fallback(request, target_path) {
        Ok(true) => {
            super::turn::write_stdout_rendered(
                &format_iteration_status(
                    last_iter,
                    agent.config.max_iterations,
                    "Polish fallback",
                    &format!("Applied deterministic visual polish to {target_path}."),
                    agent.footer.current_cols(),
                ),
                true,
            );
            agent.session.record_feedback_if_unset(
                build_feedback_for_deterministic_content_fallback(&agent.work_root),
            );
            agent.push_deterministic_ui_recovery_continuation_note(
                target_path,
                (*repo_change_retries).saturating_add(1),
            );
            ActorLoopPostToolFallbackOutcome::Continue
        }
        Ok(false) => ActorLoopPostToolFallbackOutcome::Proceed,
        Err(err) => ActorLoopPostToolFallbackOutcome::Exit {
            reason: ExitReason::TransportError,
            error_text: err,
        },
    }
}

fn handle_actor_loop_post_tool_quality_fallback(
    agent: &mut Agent,
    last_iter: usize,
    request: &str,
    target_path: &str,
    repo_change_retries: &mut usize,
) -> ActorLoopPostToolFallbackOutcome {
    match agent.maybe_apply_deterministic_quality_fallback(request, target_path) {
        Ok(true) => {
            super::turn::write_stdout_rendered(
                &format_iteration_status(
                    last_iter,
                    agent.config.max_iterations,
                    "Quality fallback",
                    &format!("Replaced scaffold placeholder output in {target_path}."),
                    agent.footer.current_cols(),
                ),
                true,
            );
            agent.session.record_feedback_if_unset(
                build_feedback_for_deterministic_content_fallback(&agent.work_root),
            );
            agent.push_deterministic_ui_recovery_continuation_note(
                target_path,
                (*repo_change_retries).saturating_add(1),
            );
            ActorLoopPostToolFallbackOutcome::Continue
        }
        Ok(false) => ActorLoopPostToolFallbackOutcome::Proceed,
        Err(err) => ActorLoopPostToolFallbackOutcome::Exit {
            reason: ExitReason::TransportError,
            error_text: err,
        },
    }
}

fn handle_actor_loop_post_tool_repo_edit_quality_gate(
    agent: &mut Agent,
    last_iter: usize,
    action_expectation: recovery::ActionExpectation,
    recovery_dispatch_gate: RecoveryDispatchGate,
    repo_edit_calls_made_this_turn: usize,
    repo_change_retries: &mut usize,
) -> ActorLoopPostToolFallbackOutcome {
    if repo_edit_calls_made_this_turn == 0
        || !recovery_dispatch_gate.allows_deterministic_fallback()
        || !(should_apply_repo_change_quality_gate(
            action_expectation,
            agent.active_task_expects_repo_change(),
            agent.session.mode_state.mode,
        ) || agent.current_request_needs_playable_ui_quality_gate())
    {
        return ActorLoopPostToolFallbackOutcome::Proceed;
    }
    let Some((request, target_path, issue)) = agent.accepted_repo_change_quality_issue() else {
        return ActorLoopPostToolFallbackOutcome::Proceed;
    };
    match agent.maybe_apply_deterministic_quality_fallback(&request, &target_path) {
        Ok(true) => {
            super::turn::write_stdout_rendered(
                &format_iteration_status(
                    last_iter,
                    agent.config.max_iterations,
                    "Quality fallback",
                    &format!("Replaced scaffold placeholder output in {target_path}."),
                    agent.footer.current_cols(),
                ),
                true,
            );
            agent.session.record_feedback_if_unset(
                build_feedback_for_deterministic_content_fallback(&agent.work_root),
            );
            agent.push_deterministic_ui_recovery_continuation_note(
                &target_path,
                (*repo_change_retries).saturating_add(1),
            );
            ActorLoopPostToolFallbackOutcome::Continue
        }
        Ok(false) => {
            *repo_change_retries += 1;
            if *repo_change_retries >= 3 {
                ActorLoopPostToolFallbackOutcome::Exit {
                    reason: ExitReason::MissingRepoEdits,
                    error_text: issue,
                }
            } else {
                super::turn::write_stdout_rendered(
                    &format_iteration_status(
                        last_iter,
                        agent.config.max_iterations,
                        "Quality gate",
                        &format!("Asked the model to replace placeholder output in {target_path}."),
                        agent.footer.current_cols(),
                    ),
                    true,
                );
                agent.push_system_note(recovery::repo_change_quality_gate_note(
                    &request,
                    &target_path,
                    &issue,
                    *repo_change_retries,
                ));
                ActorLoopPostToolFallbackOutcome::Proceed
            }
        }
        Err(err) => ActorLoopPostToolFallbackOutcome::Exit {
            reason: ExitReason::TransportError,
            error_text: err,
        },
    }
}

pub(super) fn handle_actor_loop_plan_tool_followup(
    agent: &mut Agent,
    mut args: ActorLoopPlanToolFollowupArgs<'_>,
) -> ActorLoopPlanToolFollowupOutcome {
    if agent.session.mode_state.mode != super::ExecutionMode::Plan {
        return ActorLoopPlanToolFollowupOutcome::Proceed;
    }
    if args.plan_ready_after_tool {
        return ActorLoopPlanToolFollowupOutcome::Done {
            final_prose: plan_tool_followup_done_message(),
        };
    }
    if let Some(outcome) = handle_plan_file_edit_followup(agent, &mut args) {
        return outcome;
    }
    if let Some(outcome) = handle_plan_exploration_followup(agent, &mut args) {
        return outcome;
    }
    ActorLoopPlanToolFollowupOutcome::Proceed
}

fn handle_plan_file_edit_followup(
    agent: &mut Agent,
    args: &mut ActorLoopPlanToolFollowupArgs<'_>,
) -> Option<ActorLoopPlanToolFollowupOutcome> {
    if args.plan_file_edit_calls_this_turn == 0 {
        return None;
    }
    let plan_contents = agent
        .current_plan_contents()
        .ok()
        .flatten()
        .unwrap_or_default();
    agent.session.mode_state.plan_stage = super::lifecycle::current_plan_stage(&plan_contents);
    let missing_after = super::lifecycle::plan_missing_sections(&plan_contents);
    let made_section_progress = args
        .plan_missing_before_turn
        .is_none_or(|before| missing_after.len() < before);
    if made_section_progress {
        *args.plan_progress_retries = 0;
        *args.plan_exploration_only_turns = 0;
        return Some(ActorLoopPlanToolFollowupOutcome::Proceed);
    }
    *args.plan_progress_retries += 1;
    if *args.plan_progress_retries >= 2 {
        return Some(handle_non_progress_plan_edit_fallback(agent));
    }
    agent.push_system_note(recovery::plan_progress_recovery_note(
        agent.session.mode_state.plan_stage,
        &super::lifecycle::plan_next_stage_sections(&plan_contents),
        &missing_after,
        *args.plan_progress_retries,
    ));
    Some(ActorLoopPlanToolFollowupOutcome::Proceed)
}

fn handle_plan_exploration_followup(
    agent: &mut Agent,
    args: &mut ActorLoopPlanToolFollowupArgs<'_>,
) -> Option<ActorLoopPlanToolFollowupOutcome> {
    if args.plan_exploration_calls_this_turn >= 2 {
        return Some(
            handle_actor_loop_plan_exploration_only_turn(
                agent,
                args.last_iter,
                "exploration_only_turn",
                args.plan_progress_retries,
            )
            .unwrap_or(ActorLoopPlanToolFollowupOutcome::Proceed),
        );
    }
    if args.plan_exploration_calls_this_turn == 0 {
        return None;
    }
    *args.plan_exploration_only_turns += 1;
    if *args.plan_exploration_only_turns >= 1 {
        let outcome = handle_actor_loop_plan_exploration_only_turn(
            agent,
            args.last_iter,
            "repeated_exploration_only_turns",
            args.plan_progress_retries,
        );
        *args.plan_exploration_only_turns = 0;
        return Some(outcome.unwrap_or(ActorLoopPlanToolFollowupOutcome::Proceed));
    }
    Some(ActorLoopPlanToolFollowupOutcome::Proceed)
}

fn handle_actor_loop_plan_exploration_only_turn(
    agent: &mut Agent,
    last_iter: usize,
    stall_reason: &'static str,
    plan_progress_retries: &mut usize,
) -> Option<ActorLoopPlanToolFollowupOutcome> {
    match agent.current_plan_contents() {
        Ok(Some(contents)) => {
            let current_stage = super::lifecycle::current_plan_stage(&contents);
            let next_sections = super::lifecycle::plan_next_stage_sections(&contents);
            let missing_sections = super::lifecycle::plan_missing_sections(&contents);
            if !missing_sections.is_empty() {
                *plan_progress_retries += 1;
                log_plan_stall(
                    agent.session_store.session_id(),
                    last_iter,
                    stall_reason,
                    current_stage,
                    &next_sections,
                    &missing_sections,
                    *plan_progress_retries,
                );
                agent.push_system_note(recovery::plan_progress_recovery_note(
                    current_stage,
                    &next_sections,
                    &missing_sections,
                    *plan_progress_retries,
                ));
            }
            Some(ActorLoopPlanToolFollowupOutcome::Proceed)
        }
        Ok(None) => Some(ActorLoopPlanToolFollowupOutcome::Proceed),
        Err(err) => Some(ActorLoopPlanToolFollowupOutcome::Exit {
            reason: ExitReason::TransportError,
            error_text: err,
        }),
    }
}

pub(super) fn drive_actor_loop_tool_preparation_phase(
    agent: &mut Agent,
    args: ActorLoopToolPreparationArgs<'_, '_>,
) -> ActorLoopToolPreparationOutcome {
    let current_reply_tool_call_count = args.reply_tool_calls.len();
    let mut prepared_tool_calls = args
        .reply_tool_calls
        .into_iter()
        .map(|tool_call| agent.prepare_tool_call(tool_call))
        .collect::<Vec<_>>();
    record_actor_loop_tool_call_summaries(&prepared_tool_calls, args.tool_call_summaries);

    let effective_tool_policy = agent.effective_tool_policy();
    if effective_tool_policy
        .allowed_tool_names_for_prompt()
        .is_some()
    {
        let batch_scope = if agent.missing_verifier_job.is_some() {
            Some(agent.current_workspace_scope())
        } else {
            None
        };
        match super::tool_policy::effective_tool_batch_action_with_scope(
            &prepared_tool_calls,
            &effective_tool_policy,
            &agent.work_root,
            batch_scope.as_ref(),
        ) {
            super::tool_policy::FocusedEditBatchAction::Accept => {}
            super::tool_policy::FocusedEditBatchAction::TruncateToFirst => {
                prepared_tool_calls.truncate(1);
                super::turn::write_stdout_rendered(
                    &format_iteration_status(
                        args.last_iter,
                        agent.config.max_iterations,
                        "Tool policy narrowed",
                        "Ignored extra tool calls and kept only the first allowed action on the target file.",
                        agent.footer.current_cols(),
                    ),
                    true,
                );
            }
            super::tool_policy::FocusedEditBatchAction::Reject(err) => {
                return handle_actor_loop_rejected_tool_batch(
                    agent,
                    ActorLoopRejectedToolBatchArgs {
                        err,
                        effective_tool_policy: &effective_tool_policy,
                        task_contract: args.task_contract,
                        contract_completion_role_retries: args.contract_completion_role_retries,
                        focused_policy_retries: args.focused_policy_retries,
                        missing_verifier_setup_turn: args.missing_verifier_setup_turn,
                        recovery_dispatch_gate: args.recovery_dispatch_gate,
                        recovery_owner: args.recovery_owner,
                        last_iter: args.last_iter,
                    },
                );
            }
        }
    }

    ActorLoopToolPreparationOutcome::Prepared {
        current_reply_tool_call_count,
        prepared_tool_calls,
        effective_tool_policy,
    }
}

fn record_actor_loop_tool_call_summaries(
    prepared_tool_calls: &[ToolCall],
    tool_call_summaries: &mut Vec<crate::session::eval_log::ToolCallSummary>,
) {
    use crate::session::eval_log::ToolCallSummary;
    use crate::session::feedback::mask_secrets;

    for tc in prepared_tool_calls {
        let raw_args = tc.arguments.to_string();
        let args_summary = {
            let masked = mask_secrets(&raw_args);
            if masked.len() > crate::session::eval_log::MAX_EVAL_TOOL_ARG_BYTES {
                let mut end = crate::session::eval_log::MAX_EVAL_TOOL_ARG_BYTES;
                while !masked.is_char_boundary(end) {
                    end -= 1;
                }
                format!("{}…", &masked[..end])
            } else {
                masked
            }
        };
        tool_call_summaries.push(ToolCallSummary {
            name: tc.name.clone(),
            args_summary,
        });
    }
}

pub(super) fn drive_actor_loop_pre_reply_phase(
    agent: &mut Agent,
    mut args: ActorLoopPreReplyArgs<'_, '_>,
) -> ActorLoopPreReplyOutcome {
    if args.interrupt_flag.is_set() {
        return ActorLoopPreReplyOutcome::Exit {
            reason: ExitReason::Interrupted,
            error_text: String::new(),
        };
    }
    let control_state = build_actor_loop_pre_reply_control_state(agent, &args);
    if let Some(outcome) =
        handle_actor_loop_pre_reply_control_action(agent, &mut args, &control_state)
    {
        return outcome;
    }
    if let Some(outcome) = handle_actor_loop_pre_reply_fallbacks(
        agent,
        &mut args,
        control_state.recovery_dispatch_gate,
    ) {
        return outcome;
    }
    request_actor_loop_pre_reply_model_turn(agent, &args, control_state)
}

pub(super) fn build_actor_loop_pre_reply_control_state(
    agent: &mut Agent,
    args: &ActorLoopPreReplyArgs<'_, '_>,
) -> ActorLoopPreReplyControlState {
    let pre_model_task_contract_action = if agent.session.mode_state.mode
        == super::ExecutionMode::Plan
        || (agent.task_contract_verifier_repair_pending && agent.repair_job.is_some())
    {
        None
    } else {
        args.task_contract.map(|contract| {
            agent.task_contract_recovery_action(
                contract,
                *args.contract_verifier_repair_edit_count,
                *args.repo_edit_calls_made_this_turn,
            )
        })
    };
    let loop_control_action = super::active_job_arbiter::determine_loop_control_action(
        super::active_job_arbiter::LoopControlInputs {
            mode: agent.session.mode_state.mode,
            task_contract_verifier_repair_pending: agent.task_contract_verifier_repair_pending,
            repair_next_action: agent.repair_job.as_ref().map(|job| job.next_action()),
            missing_verifier_next_action: agent
                .missing_verifier_job
                .as_ref()
                .map(|job| job.next_action()),
            task_contract_action: pre_model_task_contract_action.clone(),
        },
    );
    let recovery_owner = RecoveryOwner::from_control_action(
        &loop_control_action,
        pre_model_task_contract_action.as_ref(),
    );
    ActorLoopPreReplyControlState {
        missing_verifier_setup_turn:
            super::active_job_arbiter::loop_control_action_requires_missing_verifier_setup(
                &loop_control_action,
            ),
        recovery_dispatch_gate: RecoveryDispatchGate::from_owner(recovery_owner),
        recovery_owner,
        loop_control_action,
    }
}

pub(super) fn handle_actor_loop_pre_reply_control_action(
    agent: &mut Agent,
    args: &mut ActorLoopPreReplyArgs<'_, '_>,
    control_state: &ActorLoopPreReplyControlState,
) -> Option<ActorLoopPreReplyOutcome> {
    let flow_args = super::verifier_orchestration::TaskContractVerifierFlowArgs {
        before_snapshot: args.before_snapshot,
        accumulated: args.accumulated,
        repo_edit_calls_made_this_turn: *args.repo_edit_calls_made_this_turn,
        task_contract: args.task_contract,
        contract_verification_retries: args.contract_verification_retries,
        contract_verifier_repair_edit_count: args.contract_verifier_repair_edit_count,
        repo_change_retries: args.repo_change_retries,
        verifier_repair_retries: args.verifier_repair_retries,
        task_contract_verify_commands_collected: args.task_contract_verify_commands_collected,
        task_contract_verifier_passed_in_loop: args.task_contract_verifier_passed_in_loop,
        last_iter: args.last_iter,
    };
    match control_state.loop_control_action.clone() {
        LoopControlAction::ContinueRepairJob { .. } => {
            let outcome =
                agent.dispatch_repair_job_step(flow_args, args.repo_edit_calls_made_this_turn);
            Some(actor_loop_pre_reply_flow_outcome(outcome))
        }
        LoopControlAction::ContinueMissingVerifierJob { next_action } => agent
            .dispatch_missing_verifier_job_step(flow_args, next_action)
            .map(actor_loop_pre_reply_flow_outcome),
        LoopControlAction::RunVerifier => Some(actor_loop_pre_reply_flow_outcome(
            super::verifier_orchestration::drive_task_contract_verifier(agent, flow_args),
        )),
        LoopControlAction::RequestModelTurn => None,
    }
}

pub(super) fn handle_actor_loop_pre_reply_fallbacks(
    agent: &mut Agent,
    args: &mut ActorLoopPreReplyArgs<'_, '_>,
    recovery_dispatch_gate: RecoveryDispatchGate,
) -> Option<ActorLoopPreReplyOutcome> {
    if maybe_continue_actor_loop_mode_deterministic_fallback(agent, args, recovery_dispatch_gate) {
        return Some(ActorLoopPreReplyOutcome::Continue);
    }
    if maybe_continue_actor_loop_framework_fallback(agent, args, recovery_dispatch_gate) {
        return Some(ActorLoopPreReplyOutcome::Continue);
    }
    if let Some(outcome) =
        maybe_handle_actor_loop_playable_ui_fallback(agent, args, recovery_dispatch_gate)
    {
        return Some(outcome);
    }
    None
}

pub(super) fn maybe_continue_actor_loop_mode_deterministic_fallback(
    agent: &mut Agent,
    args: &mut ActorLoopPreReplyArgs<'_, '_>,
    recovery_dispatch_gate: RecoveryDispatchGate,
) -> bool {
    actor_loop_pre_reply_repo_change_fallback_allowed(
        args.action_expectation,
        *args.repo_edit_calls_made_this_turn,
        recovery_dispatch_gate,
    ) && agent.maybe_materialize_mode_deterministic_fallback(args.last_iter)
}

pub(super) fn maybe_continue_actor_loop_framework_fallback(
    agent: &mut Agent,
    args: &mut ActorLoopPreReplyArgs<'_, '_>,
    recovery_dispatch_gate: RecoveryDispatchGate,
) -> bool {
    if !actor_loop_pre_reply_repo_change_fallback_allowed(
        args.action_expectation,
        *args.repo_edit_calls_made_this_turn,
        recovery_dispatch_gate,
    ) || !should_try_framework_app_fallback(
        args.last_iter,
        *args.framework_app_fallback_materialized,
    ) || !agent.maybe_materialize_framework_game_fallback(args.last_iter)
    {
        return false;
    }
    *args.framework_app_fallback_materialized = true;
    agent.push_system_note(framework_app_fallback_continuation_note().to_string());
    true
}

pub(super) fn maybe_handle_actor_loop_playable_ui_fallback(
    agent: &mut Agent,
    args: &mut ActorLoopPreReplyArgs<'_, '_>,
    recovery_dispatch_gate: RecoveryDispatchGate,
) -> Option<ActorLoopPreReplyOutcome> {
    if !actor_loop_pre_reply_deterministic_fallback_allowed(
        *args.repo_edit_calls_made_this_turn,
        recovery_dispatch_gate,
    ) || !agent.current_request_needs_playable_ui_quality_gate()
    {
        return None;
    }
    let (request, target_path) = agent.accepted_repo_change_polish_target()?;
    match handle_actor_loop_post_tool_polish_fallback(
        agent,
        args.last_iter,
        &request,
        &target_path,
        args.repo_change_retries,
    ) {
        ActorLoopPostToolFallbackOutcome::Continue => Some(ActorLoopPreReplyOutcome::Continue),
        ActorLoopPostToolFallbackOutcome::Exit { reason, error_text } => {
            Some(ActorLoopPreReplyOutcome::Exit { reason, error_text })
        }
        ActorLoopPostToolFallbackOutcome::Proceed => None,
    }
}

pub(super) fn request_actor_loop_pre_reply_model_turn(
    agent: &mut Agent,
    args: &ActorLoopPreReplyArgs<'_, '_>,
    control_state: ActorLoopPreReplyControlState,
) -> ActorLoopPreReplyOutcome {
    match agent.request_assistant_reply_with_retry(
        args.stream_output,
        args.interrupt_flag,
        control_state.recovery_dispatch_gate,
    ) {
        Ok(reply) => ActorLoopPreReplyOutcome::ReplyPrepared {
            reply,
            recovery_dispatch_gate: control_state.recovery_dispatch_gate,
            missing_verifier_setup_turn: control_state.missing_verifier_setup_turn,
            recovery_owner: control_state.recovery_owner,
        },
        Err(err) => actor_loop_pre_reply_request_error(agent, err),
    }
}

pub(super) fn handle_actor_loop_task_contract_reply(
    agent: &mut Agent,
    args: ActorLoopTaskContractReplyArgs<'_, '_>,
) -> ActorLoopTaskContractReplyOutcome {
    let (Some(contract), Some(action)) = (args.task_contract, args.task_contract_action) else {
        return ActorLoopTaskContractReplyOutcome::Proceed;
    };

    match action {
        super::task_contract::ArtifactRecoveryAction::Continue {
            missing,
            target_hint,
        } => handle_actor_loop_task_contract_continue_action(
            agent,
            ActorLoopTaskContractContinueArgs {
                contract,
                action,
                missing,
                target_hint,
                final_reply: args.final_reply,
                current_reply_tool_call_count: args.current_reply_tool_call_count,
                last_iter: args.last_iter,
                contract_completion_retries: args.contract_completion_retries,
                contract_completion_role_retries: args.contract_completion_role_retries,
                contract_deterministic_fallback_materialized: args
                    .contract_deterministic_fallback_materialized,
            },
        ),
        super::task_contract::ArtifactRecoveryAction::RepairArtifact { .. } => {
            handle_actor_loop_task_contract_repair_artifact(
                agent,
                args.last_iter,
                args.verifier_repair_retries,
            )
        }
        super::task_contract::ArtifactRecoveryAction::RunVerifier => {
            handle_actor_loop_task_contract_run_verifier(agent, args)
        }
        super::task_contract::ArtifactRecoveryAction::Done => {
            ActorLoopTaskContractReplyOutcome::Proceed
        }
        super::task_contract::ArtifactRecoveryAction::SafeStop { reason } => {
            handle_actor_loop_task_contract_safe_stop(agent, *reason, args.last_iter)
        }
    }
}

pub(super) fn handle_actor_loop_task_contract_continue_action(
    agent: &mut Agent,
    args: ActorLoopTaskContractContinueArgs<'_, '_>,
) -> ActorLoopTaskContractReplyOutcome {
    let decision = super::task_contract::CompletionDecision::Continue {
        missing: args.missing.to_vec(),
    };
    let target_hint = args
        .target_hint
        .clone()
        .or_else(|| agent.task_contract_recovery_target(&decision))
        .and_then(|hint| {
            agent.set_artifact_recovery_target_from_hint(
                hint,
                (*args.contract_completion_retries).saturating_add(1),
            )
        });
    if task_contract_continue_requires_tool_recovery(
        Some(args.action),
        args.current_reply_tool_call_count,
    ) {
        return handle_actor_loop_task_contract_tool_recovery(
            agent,
            ActorLoopTaskContractToolRecoveryArgs {
                contract: args.contract,
                decision: &decision,
                target_hint,
                missing: args.missing,
                final_reply: args.final_reply,
                last_iter: args.last_iter,
                contract_completion_retries: args.contract_completion_retries,
                contract_completion_role_retries: args.contract_completion_role_retries,
            },
        );
    }
    if !*args.contract_deterministic_fallback_materialized
        && agent.maybe_materialize_task_contract_fallback(&decision, args.last_iter)
    {
        *args.contract_deterministic_fallback_materialized = true;
        agent.set_artifact_recovery_target_for_decision(
            &decision,
            (*args.contract_completion_retries).saturating_add(1),
        );
        let scaffold_note = "[Task Contract] Deterministic fallback created framework scaffold files only. Treat them as bootstrap, edit them to satisfy the user's specific request, then update tests and docs before final response.";
        agent.push_system_note(scaffold_note.to_string());
        return ActorLoopTaskContractReplyOutcome::Continue;
    }
    handle_actor_loop_task_contract_incomplete_artifacts(
        agent,
        ActorLoopTaskContractIncompleteArgs {
            contract: args.contract,
            decision,
            target_hint,
            missing: args.missing,
            last_iter: args.last_iter,
            contract_completion_retries: args.contract_completion_retries,
            contract_completion_role_retries: args.contract_completion_role_retries,
        },
    )
}

pub(super) fn handle_actor_loop_task_contract_tool_recovery(
    agent: &mut Agent,
    args: ActorLoopTaskContractToolRecoveryArgs<'_, '_>,
) -> ActorLoopTaskContractReplyOutcome {
    *args.contract_completion_retries = (*args.contract_completion_retries).saturating_add(1);
    let role = args
        .missing
        .first()
        .copied()
        .unwrap_or(super::task_contract::ArtifactRole::Implementation);
    let kind = if args.final_reply.is_empty() {
        super::artifact_completion_job::ArtifactAttemptOutcomeKind::NoTool
    } else {
        super::artifact_completion_job::ArtifactAttemptOutcomeKind::ProseOnly
    };
    if agent.record_artifact_completion_attempt(kind, Vec::new()) {
        return ActorLoopTaskContractReplyOutcome::Exit {
            reason: ExitReason::MissingRepoEdits,
            error_text: ARTIFACT_COMPLETION_BUDGET_EXHAUSTED_TEXT.to_string(),
        };
    }
    let artifact_attempt =
        increment_artifact_completion_role_attempt(args.contract_completion_role_retries, role);
    let attempt_limit = args.contract.artifact_completion_attempt_limit();
    if artifact_attempt >= attempt_limit {
        let expected_target = args
            .target_hint
            .as_ref()
            .map(|hint| hint.path.clone())
            .or_else(|| {
                agent
                    .current_artifact_recovery_target
                    .as_ref()
                    .map(|target| target.path.clone())
            });
        agent.emit_safe_stop_report_for_artifact_completion_failed(role, expected_target);
        return ActorLoopTaskContractReplyOutcome::Exit {
            reason: ExitReason::MissingRepoEdits,
            error_text: format!(
                "assistant stopped before editing required artifact role {}",
                role.label()
            ),
        };
    }
    super::turn::write_stdout_rendered(
        &format_iteration_status(
            args.last_iter,
            agent.config.max_iterations,
            "Retry requested",
            "Task contract requires a repository edit on the current artifact target.",
            agent.footer.current_cols(),
        ),
        true,
    );
    if !agent.push_artifact_directed_recovery_note(artifact_attempt) {
        let note = super::task_contract::render_contract_recovery_note_with_hint(
            args.decision,
            agent.active_request_text().as_deref().unwrap_or_default(),
            artifact_attempt,
            attempt_limit,
            args.target_hint.as_ref(),
        );
        agent.push_system_note(note);
    }
    ActorLoopTaskContractReplyOutcome::Continue
}

pub(super) fn handle_actor_loop_task_contract_incomplete_artifacts(
    agent: &mut Agent,
    args: ActorLoopTaskContractIncompleteArgs<'_, '_>,
) -> ActorLoopTaskContractReplyOutcome {
    *args.contract_completion_retries += 1;
    let missing_labels = args
        .missing
        .iter()
        .map(|role| role.label())
        .collect::<Vec<_>>();
    let role = args
        .missing
        .first()
        .copied()
        .unwrap_or(super::task_contract::ArtifactRole::Implementation);
    let artifact_attempt =
        increment_artifact_completion_role_attempt(args.contract_completion_role_retries, role);
    let attempt_limit = args.contract.artifact_completion_attempt_limit();
    if artifact_attempt >= attempt_limit {
        let expected_target = args
            .target_hint
            .as_ref()
            .map(|hint| hint.path.clone())
            .or_else(|| {
                agent
                    .current_artifact_recovery_target
                    .as_ref()
                    .map(|target| target.path.clone())
            });
        agent.emit_safe_stop_report_for_artifact_completion_failed(role, expected_target);
        return ActorLoopTaskContractReplyOutcome::Exit {
            reason: ExitReason::MissingRepoEdits,
            error_text: format!(
                "task contract incomplete; missing required artifact(s): {}",
                missing_labels.join(", ")
            ),
        };
    }
    super::turn::write_stdout_rendered(
        &format_iteration_status(
            args.last_iter,
            agent.config.max_iterations,
            "Task contract",
            &format!(
                "Asked the model to complete missing artifact(s): {}.",
                missing_labels.join(", ")
            ),
            agent.footer.current_cols(),
        ),
        true,
    );
    crate::logging::log_llm_event(
        "agent.task_contract.incomplete",
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "turn_index": agent.current_turn_index,
            "iter": args.last_iter,
            "missing": missing_labels,
        }),
    );
    let note = super::task_contract::render_contract_recovery_note_with_hint(
        &args.decision,
        agent.active_request_text().as_deref().unwrap_or_default(),
        artifact_attempt,
        attempt_limit,
        args.target_hint.as_ref(),
    );
    agent.push_system_note(note);
    ActorLoopTaskContractReplyOutcome::Continue
}

pub(super) fn handle_actor_loop_task_contract_repair_artifact(
    agent: &mut Agent,
    last_iter: usize,
    verifier_repair_retries: &mut usize,
) -> ActorLoopTaskContractReplyOutcome {
    agent.repair_job_artifact_attempts = agent.repair_job_artifact_attempts.saturating_add(1);
    *verifier_repair_retries = agent.repair_job_artifact_attempts;
    if agent.repair_job_artifact_attempts >= super::turn::TASK_CONTRACT_VERIFIER_ATTEMPT_LIMIT {
        let role = agent
            .current_artifact_recovery_target
            .as_ref()
            .map(|target| target.role)
            .unwrap_or(super::task_contract::ArtifactRole::Implementation);
        let expected_target = agent
            .current_artifact_recovery_target
            .as_ref()
            .map(|target| target.path.clone())
            .or_else(|| {
                agent
                    .repair_job
                    .as_ref()
                    .and_then(|job| job.target_hint.as_ref())
                    .map(|hint| hint.path.clone())
            });
        agent.emit_safe_stop_report_for_artifact_completion_failed(role, expected_target);
        return ActorLoopTaskContractReplyOutcome::Exit {
            reason: ExitReason::MissingRepoEdits,
            error_text: "assistant stopped before repairing the verifier failure".to_string(),
        };
    }
    super::turn::write_stdout_rendered(
        &format_iteration_status(
            last_iter,
            agent.config.max_iterations,
            "Retry requested",
            "Verifier repair requires a repository edit before verification is retried.",
            agent.footer.current_cols(),
        ),
        true,
    );
    if !agent.push_verifier_repair_recovery_note(*verifier_repair_retries)
        && !agent.push_artifact_directed_recovery_note(*verifier_repair_retries)
    {
        agent.push_system_note(task_contract_verifier_edit_required_note(
            *verifier_repair_retries,
            super::turn::TASK_CONTRACT_VERIFIER_ATTEMPT_LIMIT,
        ));
    }
    ActorLoopTaskContractReplyOutcome::Continue
}

pub(super) fn handle_actor_loop_task_contract_run_verifier(
    agent: &mut Agent,
    args: ActorLoopTaskContractReplyArgs<'_, '_>,
) -> ActorLoopTaskContractReplyOutcome {
    match super::verifier_orchestration::drive_task_contract_verifier(
        agent,
        super::verifier_orchestration::TaskContractVerifierFlowArgs {
            before_snapshot: args.before_snapshot,
            accumulated: args.accumulated,
            repo_edit_calls_made_this_turn: args.repo_edit_calls_made_this_turn,
            task_contract: args.task_contract,
            contract_verification_retries: args.contract_verification_retries,
            contract_verifier_repair_edit_count: args.contract_verifier_repair_edit_count,
            repo_change_retries: args.repo_change_retries,
            verifier_repair_retries: args.verifier_repair_retries,
            task_contract_verify_commands_collected: args.task_contract_verify_commands_collected,
            task_contract_verifier_passed_in_loop: args.task_contract_verifier_passed_in_loop,
            last_iter: args.last_iter,
        },
    ) {
        TaskContractVerifierFlowOutcome::Continue => {
            *args.no_tool_retries = 0;
            ActorLoopTaskContractReplyOutcome::Continue
        }
        TaskContractVerifierFlowOutcome::Done { final_prose } => {
            ActorLoopTaskContractReplyOutcome::Done { final_prose }
        }
        TaskContractVerifierFlowOutcome::Exit { reason, error_text } => {
            ActorLoopTaskContractReplyOutcome::Exit { reason, error_text }
        }
    }
}

pub(super) fn handle_actor_loop_task_contract_safe_stop(
    agent: &mut Agent,
    reason: super::task_contract::SafeStopReason,
    last_iter: usize,
) -> ActorLoopTaskContractReplyOutcome {
    let (mapped_reason, log_outcome) = task_contract_verifier_safe_stop_mapping(reason);
    crate::logging::log_llm_event(
        "agent.task_contract.safe_stop",
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "turn_index": agent.current_turn_index,
            "iter": last_iter,
            "outcome": log_outcome,
        }),
    );
    ActorLoopTaskContractReplyOutcome::Exit {
        reason: mapped_reason,
        error_text: mapped_reason.default_error_text().to_string(),
    }
}

pub(super) fn handle_actor_loop_rejected_tool_batch(
    agent: &mut Agent,
    args: ActorLoopRejectedToolBatchArgs<'_, '_>,
) -> ActorLoopToolPreparationOutcome {
    let focused_retry = args.effective_tool_policy.focused_edit_policy().cloned();
    let artifact_retry = args
        .effective_tool_policy
        .artifact_directed_policy()
        .cloned();
    agent.session.working_memory.note_error(args.err);
    if let Some(outcome) = maybe_handle_rejected_tool_batch_missing_verifier(
        agent,
        args.missing_verifier_setup_turn,
        args.last_iter,
    ) {
        return outcome;
    }
    if let Some(outcome) = maybe_handle_rejected_tool_batch_artifact(
        agent,
        args.last_iter,
        artifact_retry.is_some(),
        args.task_contract,
        args.contract_completion_role_retries,
    ) {
        return outcome;
    }

    *args.focused_policy_retries += 1;
    if let Some(outcome) = maybe_handle_rejected_tool_batch_focused_retry_exhausted(
        agent,
        focused_retry.is_some(),
        *args.focused_policy_retries,
        args.recovery_dispatch_gate,
        args.recovery_owner,
    ) {
        return outcome;
    }
    if unrestricted_policy_retry_exhausted(focused_retry.is_some(), *args.focused_policy_retries) {
        return ActorLoopToolPreparationOutcome::Exit {
            reason: ExitReason::ToolCallFormatError,
            error_text: "assistant kept calling tools outside the current tool policy".to_string(),
        };
    }
    let retry_status_note = rejected_tool_batch_retry_status_note(focused_retry.is_some());
    super::turn::write_stdout_rendered(
        &format_iteration_status(
            args.last_iter,
            agent.config.max_iterations,
            "Retry requested",
            retry_status_note,
            agent.footer.current_cols(),
        ),
        true,
    );
    if let Some(policy) = focused_retry {
        let note = agent.focused_edit_no_tool_note_for_policy(
            &policy,
            args.effective_tool_policy,
            *args.focused_policy_retries,
        );
        agent.push_system_note(note);
    } else {
        agent.push_system_note(format!(
            "The previous tool call violated the current tool policy and was not executed. Emit exactly one allowed tool call now. tool_policy_retry_attempt={}",
            *args.focused_policy_retries
        ));
    }
    ActorLoopToolPreparationOutcome::Continue
}

pub(super) fn maybe_handle_rejected_tool_batch_missing_verifier(
    agent: &mut Agent,
    missing_verifier_setup_turn: bool,
    last_iter: usize,
) -> Option<ActorLoopToolPreparationOutcome> {
    if !missing_verifier_setup_turn {
        return None;
    }
    if agent.record_missing_verifier_setup_failure(last_iter, "tool policy violation") {
        return Some(ActorLoopToolPreparationOutcome::Exit {
            reason: ExitReason::MissingVerification,
            error_text:
                "task contract requires verification, but the MissingVerifierJob setup budget is exhausted"
                    .to_string(),
        });
    }
    Some(ActorLoopToolPreparationOutcome::Continue)
}

pub(super) fn maybe_handle_rejected_tool_batch_artifact(
    agent: &mut Agent,
    last_iter: usize,
    artifact_retry_present: bool,
    task_contract: Option<&super::task_contract::TaskContract>,
    contract_completion_role_retries: &mut HashMap<super::task_contract::ArtifactRole, usize>,
) -> Option<ActorLoopToolPreparationOutcome> {
    if !artifact_retry_present {
        return None;
    }
    let role = agent
        .current_artifact_recovery_target
        .as_ref()
        .map(|target| target.role)
        .unwrap_or(super::task_contract::ArtifactRole::Implementation);
    if agent.record_artifact_completion_attempt(
        super::artifact_completion_job::ArtifactAttemptOutcomeKind::RolePolicyViolation,
        vec!["focused_edit_batch_reject".to_string()],
    ) {
        return Some(ActorLoopToolPreparationOutcome::Exit {
            reason: ExitReason::MissingRepoEdits,
            error_text: format!(
                "artifact completion role-policy violation budget exhausted for role {}",
                role.label()
            ),
        });
    }
    let artifact_attempt =
        increment_artifact_completion_role_attempt(contract_completion_role_retries, role);
    let attempt_limit = task_contract
        .as_ref()
        .map(|contract| contract.artifact_completion_attempt_limit())
        .unwrap_or(4);
    if artifact_attempt >= attempt_limit {
        let expected_target = agent
            .current_artifact_recovery_target
            .as_ref()
            .map(|target| target.path.clone());
        agent.emit_safe_stop_report_for_artifact_completion_failed(role, expected_target);
        return Some(ActorLoopToolPreparationOutcome::Exit {
            reason: ExitReason::MissingRepoEdits,
            error_text: format!(
                "artifact edit rejected repeatedly for required role {}",
                role.label()
            ),
        });
    }
    super::turn::write_stdout_rendered(
        &format_iteration_status(
            last_iter,
            agent.config.max_iterations,
            "Retry requested",
            "Artifact completion rejected an invalid tool call before execution; asked for one allowed edit on the target.",
            agent.footer.current_cols(),
        ),
        true,
    );
    if !agent.push_artifact_directed_recovery_note(artifact_attempt) {
        agent.push_system_note(format!(
            "[Artifact Completion] Previous tool call was rejected and was not executed. Missing role: {}. Emit exactly one allowed tool call on the current target path now. artifact_completion_attempt={artifact_attempt}/{attempt_limit}",
            role.label()
        ));
    }
    Some(ActorLoopToolPreparationOutcome::Continue)
}

pub(super) fn maybe_handle_rejected_tool_batch_focused_retry_exhausted(
    agent: &mut Agent,
    focused_retry_present: bool,
    focused_policy_retries: usize,
    recovery_dispatch_gate: RecoveryDispatchGate,
    recovery_owner: RecoveryOwner,
) -> Option<ActorLoopToolPreparationOutcome> {
    if !focused_policy_retry_exhausted(focused_retry_present, focused_policy_retries) {
        return None;
    }
    if !recovery_dispatch_gate.allows_focused_edit_recovery() {
        return Some(ActorLoopToolPreparationOutcome::Exit {
            reason: Agent::tool_policy_violation_exit_reason(recovery_owner),
            error_text:
                "verifier-owned recovery rejected invalid tool calls repeatedly before an allowed repair edit"
                    .to_string(),
        });
    }
    let request = agent.active_request_text().unwrap_or_default();
    let fallback = match agent.maybe_apply_local_llm_small_edit_fallback(&request) {
        Ok(fallback) => fallback,
        Err(err) => {
            return Some(ActorLoopToolPreparationOutcome::Exit {
                reason: ExitReason::TransportError,
                error_text: err,
            });
        }
    };
    if let Some(relative) = fallback {
        return Some(ActorLoopToolPreparationOutcome::Done {
            final_prose: format!(
                "Applied a verified small edit fallback after the local model could not produce a compact edit for {relative}."
            ),
        });
    }
    Some(ActorLoopToolPreparationOutcome::Exit {
        reason: ExitReason::MissingRepoEdits,
        error_text: ExitReason::MissingRepoEdits
            .default_error_text()
            .to_string(),
    })
}

pub(super) fn handle_post_reply_recovery(
    agent: &mut Agent,
    mut args: PostReplyRecoveryArgs<'_, '_>,
) -> Option<PostReplyRecoveryOutcome> {
    if let Some(outcome) = maybe_handle_answer_only_future_work_recovery(agent, &mut args) {
        return Some(outcome);
    }
    if let Some(outcome) = maybe_handle_missing_repo_edit_recovery(agent, &mut args) {
        return Some(outcome);
    }
    if let Some(outcome) = maybe_handle_python_test_artifact_recovery(agent, &mut args) {
        return Some(outcome);
    }
    if let Some(outcome) = maybe_handle_answer_only_inadequate_recovery(agent, &mut args) {
        return Some(outcome);
    }
    if let Some(outcome) = maybe_handle_repo_change_partial_progress_recovery(agent, &mut args) {
        return Some(outcome);
    }
    if let Some(outcome) = maybe_handle_repo_change_quality_gate_recovery(agent, &mut args) {
        return Some(outcome);
    }

    None
}

pub(super) fn maybe_handle_answer_only_future_work_recovery(
    agent: &mut Agent,
    args: &mut PostReplyRecoveryArgs<'_, '_>,
) -> Option<PostReplyRecoveryOutcome> {
    if !args.requires_action
        && agent.answer_only_mode_active()
        && reply_looks_like_future_work(args.final_reply)
    {
        *args.no_tool_retries += 1;
        if *args.no_tool_retries >= 1 {
            return Some(PostReplyRecoveryOutcome::Finalize {
                final_prose: agent.answer_only_fallback_response(),
                exit_reason: ExitReason::Done,
                error_text: String::new(),
            });
        }
        super::turn::write_stdout_rendered(
            &format_iteration_status(
                args.last_iter,
                agent.config.max_iterations,
                "Retry requested",
                "The model answered with next-step prose in answer-only mode. Asked it to answer directly without more tools.",
                agent.footer.current_cols(),
            ),
            true,
        );
        agent.push_system_note(
            "[Answer-only Recovery] Answer the user's request now using only the context already inspected. Do not announce the next action, do not use tools, do not edit files, and do not ask the user to run anything."
                .to_string(),
        );
        return Some(PostReplyRecoveryOutcome::Continue);
    }
    None
}

pub(super) fn handle_plan_progress_prose_only_fallback(
    agent: &mut Agent,
) -> ActorLoopNoToolReplyOutcome {
    match agent.materialize_deterministic_fallback_plan("agent.plan.progress_fallback_materialized")
    {
        Ok(true) => ActorLoopNoToolReplyOutcome::Done {
            final_prose: plan_tool_followup_done_message(),
        },
        Ok(false) => ActorLoopNoToolReplyOutcome::Exit {
            reason: ExitReason::PlanIncomplete,
            error_text: ExitReason::PlanIncomplete.default_error_text().to_string(),
        },
        Err(err) => ActorLoopNoToolReplyOutcome::Exit {
            reason: ExitReason::TransportError,
            error_text: err,
        },
    }
}

pub(super) fn handle_non_progress_plan_edit_fallback(
    agent: &mut Agent,
) -> ActorLoopPlanToolFollowupOutcome {
    match agent.materialize_deterministic_fallback_plan(
        "agent.plan.non_progress_edit_fallback_materialized",
    ) {
        Ok(true) => ActorLoopPlanToolFollowupOutcome::Done {
            final_prose: plan_tool_followup_done_message(),
        },
        Ok(false) => ActorLoopPlanToolFollowupOutcome::Exit {
            reason: ExitReason::PlanIncomplete,
            error_text: ExitReason::PlanIncomplete.default_error_text().to_string(),
        },
        Err(err) => ActorLoopPlanToolFollowupOutcome::Exit {
            reason: ExitReason::TransportError,
            error_text: err,
        },
    }
}

pub(super) fn missing_repo_change_budget_exhausted_outcome() -> ActorLoopNoToolReplyOutcome {
    ActorLoopNoToolReplyOutcome::Exit {
        reason: ExitReason::MissingRepoEdits,
        error_text: ARTIFACT_COMPLETION_BUDGET_EXHAUSTED_TEXT.to_string(),
    }
}

pub(super) fn missing_repo_edit_recovery_allowed(args: &PostReplyRecoveryArgs<'_, '_>) -> bool {
    args.action_expectation == recovery::ActionExpectation::RepoChange
        && args.repo_edit_calls_made_this_turn == 0
        && args
            .recovery_dispatch_gate
            .allows_generic_repo_change_recovery()
}

pub(super) fn repair_job_done_outcome() -> TaskContractVerifierFlowOutcome {
    TaskContractVerifierFlowOutcome::Done {
        final_prose:
            "Completed requested repository changes and verified them with the required verifier."
                .to_string(),
    }
}

pub(super) fn finalize_missing_repo_edit_retry_exhausted(
    agent: &mut Agent,
) -> PostReplyRecoveryOutcome {
    let request = agent.active_request_text().unwrap_or_default();
    match agent.maybe_apply_local_llm_small_edit_fallback(&request) {
        Ok(Some(relative)) => PostReplyRecoveryOutcome::Finalize {
            final_prose: format!(
                "Applied a verified small edit fallback after the local model stopped before editing {relative}."
            ),
            exit_reason: ExitReason::Done,
            error_text: String::new(),
        },
        Ok(None) => missing_repo_edits_finalize_outcome(),
        Err(err) => PostReplyRecoveryOutcome::Finalize {
            final_prose: String::new(),
            exit_reason: ExitReason::TransportError,
            error_text: err,
        },
    }
}

pub(super) fn missing_repo_edits_finalize_outcome() -> PostReplyRecoveryOutcome {
    PostReplyRecoveryOutcome::Finalize {
        final_prose: String::new(),
        exit_reason: ExitReason::MissingRepoEdits,
        error_text: ExitReason::MissingRepoEdits
            .default_error_text()
            .to_string(),
    }
}

pub(super) fn run_actor_loop(
    agent: &mut Agent,
    action_expectation: recovery::ActionExpectation,
    requires_action: bool,
    stream_output: bool,
    restart_convergence_mode: bool,
    monitor: &mut InterruptMonitor,
) -> LoopResult {
    let use_color = io::stdout().is_terminal() && !no_color_requested();
    let use_unicode = unicode_supported();
    let start = Instant::now();
    let mut before_snapshot = capture_repo_snapshot(&agent.work_root);
    let mut accumulated: Vec<RepoVerification> = Vec::new();
    let mut last_known_root = agent.work_root.clone();
    let task_contract = agent.prepare_actor_loop_turn_state();

    let mut tool_calls_made_this_turn = 0usize;
    let mut repo_edit_calls_made_this_turn = 0usize;
    let mut empty_retries = 0usize;
    let mut no_tool_retries = 0usize;
    let mut repo_change_retries = 0usize;
    let mut verifier_repair_retries = 0usize;
    let mut focused_policy_retries = 0usize;
    let mut contract_completion_retries = 0usize;
    let mut contract_completion_role_retries =
        HashMap::<super::task_contract::ArtifactRole, usize>::new();
    let mut contract_verification_retries = 0usize;
    let mut contract_verifier_repair_edit_count: Option<usize> = None;
    let mut task_contract_verifier_passed_in_loop = false;
    let mut task_contract_verify_commands_collected = Vec::<String>::new();
    let mut python_test_retries = 0usize;
    let mut plan_progress_retries = 0usize;
    let mut plan_exploration_only_turns = 0usize;
    let mut plan_exploration_counts = HashMap::<PlanExplorationKey, usize>::new();
    let mut plan_write_signature_counts = HashMap::<String, usize>::new();
    let mut recent_bash_commands = Vec::<String>::new();
    let mut install_commands_seen = 0usize;
    let mut logged_plan_first_write = false;
    let mut logged_act_first_repo_edit = false;
    let mut framework_app_fallback_materialized = false;
    let mut contract_deterministic_fallback_materialized = false;

    let mut exit_reason = ExitReason::MaxIterations;
    let mut error_text = String::new();
    let mut last_iter = 0usize;
    let mut final_prose = String::new();
    // Issue #471: collect all LLM-requested tool calls BEFORE any
    // focused-edit truncation so the eval log records the full intent
    // (DR3-003).
    let mut tool_call_summaries: Vec<crate::session::eval_log::ToolCallSummary> = Vec::new();

    let interrupt_flag = monitor.flag();

    'outer: for iter_count in 0..agent.config.max_iterations {
        last_iter = iter_count + 1;
        let approx_tokens = approximate_token_count(&agent.session.messages);
        tracing::debug!(iter = iter_count, tokens = approx_tokens, "iter");
        // Publish per-turn token count to the footer (issue #430, AC12).
        // Reuses the value we just computed — O(1), no second walk over
        // `messages`. No-op when the footer handle is disabled.
        agent.footer.publish_tokens(approx_tokens);

        // Issue #660 (Phase C / DD-4): emit `agent.active_job.selected`
        // at the head of every iteration when the selection differs from
        // the previous emission. Per-turn diff-based dedup state lives
        // on `agent.last_active_job_selection`, reset at
        // `handle_user_message` entry adjacent to
        // `safe_stop_report_emitted`. The helper itself is the only emit
        // site; raw verifier commands / raw paths are redacted by the
        // pure `build_active_job_selected_payload` builder (DR4-001/002).
        //
        // Codex CB-002: pass the actor-loop `iter_count` as the
        // payload's `iteration_seq` so the field name and the value
        // semantics agree (previously the per-turn index was passed,
        // which collapsed all same-turn re-emits to a single value).
        agent.emit_active_job_selected_if_changed(iter_count as u32);

        let (reply, recovery_dispatch_gate, missing_verifier_setup_turn, recovery_owner) =
            match drive_actor_loop_pre_reply_phase(
                agent,
                ActorLoopPreReplyArgs {
                    before_snapshot: &before_snapshot,
                    accumulated: &accumulated,
                    task_contract: task_contract.as_ref(),
                    repo_edit_calls_made_this_turn: &mut repo_edit_calls_made_this_turn,
                    contract_verification_retries: &mut contract_verification_retries,
                    contract_verifier_repair_edit_count: &mut contract_verifier_repair_edit_count,
                    repo_change_retries: &mut repo_change_retries,
                    verifier_repair_retries: &mut verifier_repair_retries,
                    task_contract_verify_commands_collected:
                        &mut task_contract_verify_commands_collected,
                    task_contract_verifier_passed_in_loop:
                        &mut task_contract_verifier_passed_in_loop,
                    framework_app_fallback_materialized: &mut framework_app_fallback_materialized,
                    action_expectation,
                    stream_output,
                    last_iter,
                    interrupt_flag: &interrupt_flag,
                },
            ) {
                ActorLoopPreReplyOutcome::Continue => continue,
                ActorLoopPreReplyOutcome::Done { final_prose: prose } => {
                    final_prose = prose;
                    exit_reason = ExitReason::Done;
                    break 'outer;
                }
                ActorLoopPreReplyOutcome::Exit {
                    reason,
                    error_text: loop_error,
                } => {
                    exit_reason = reason;
                    error_text = loop_error;
                    break 'outer;
                }
                ActorLoopPreReplyOutcome::ReplyPrepared {
                    reply,
                    recovery_dispatch_gate,
                    missing_verifier_setup_turn,
                    recovery_owner,
                } => (
                    reply,
                    recovery_dispatch_gate,
                    missing_verifier_setup_turn,
                    recovery_owner,
                ),
            };

        // Boundary 2: right after the Ollama response completes. This is
        // the AC-10 checkpoint — mid-flight cancel is out of scope.
        if interrupt_flag.is_set() {
            exit_reason = ExitReason::Interrupted;
            break 'outer;
        }

        let AssistantReply {
            content: reply_content,
            tool_calls: reply_tool_calls,
            ..
        } = reply;

        let (current_reply_tool_call_count, prepared_tool_calls, effective_tool_policy) =
            match drive_actor_loop_tool_preparation_phase(
                agent,
                ActorLoopToolPreparationArgs {
                    reply_tool_calls,
                    task_contract: task_contract.as_ref(),
                    tool_call_summaries: &mut tool_call_summaries,
                    focused_policy_retries: &mut focused_policy_retries,
                    contract_completion_role_retries: &mut contract_completion_role_retries,
                    missing_verifier_setup_turn,
                    recovery_dispatch_gate,
                    recovery_owner,
                    last_iter,
                },
            ) {
                ActorLoopToolPreparationOutcome::Continue => continue,
                ActorLoopToolPreparationOutcome::Done { final_prose: prose } => {
                    final_prose = prose;
                    exit_reason = ExitReason::Done;
                    break 'outer;
                }
                ActorLoopToolPreparationOutcome::Exit {
                    reason,
                    error_text: tool_error,
                } => {
                    exit_reason = reason;
                    error_text = tool_error;
                    break 'outer;
                }
                ActorLoopToolPreparationOutcome::Prepared {
                    current_reply_tool_call_count,
                    prepared_tool_calls,
                    effective_tool_policy,
                } => (
                    current_reply_tool_call_count,
                    prepared_tool_calls,
                    effective_tool_policy,
                ),
            };

        if !prepared_tool_calls.is_empty() {
            let mut plan_file_edit_calls_this_turn = 0usize;
            let mut plan_exploration_calls_this_turn = 0usize;
            let mut plan_ready_after_tool = false;
            let mut bash_only_tool_turn = true;
            let current_plan_stage = agent.session.mode_state.plan_stage;
            let plan_missing_before_turn = if agent.session.mode_state.mode == ExecutionMode::Plan {
                agent
                    .current_plan_contents()
                    .ok()
                    .flatten()
                    .map(|contents| lifecycle::plan_missing_sections(&contents).len())
            } else {
                None
            };
            let plan_exploration_budget =
                lifecycle::plan_stage_exploration_budget(current_plan_stage);
            tool_calls_made_this_turn += prepared_tool_calls.len();
            repo_edit_calls_made_this_turn += prepared_tool_calls
                .iter()
                .filter(|tool_call| recovery::tool_call_counts_as_repo_edit(&tool_call.name))
                .count();
            empty_retries = 0;
            no_tool_retries = 0;
            focused_policy_retries = 0;
            if repo_edit_calls_made_this_turn > 0 {
                repo_change_retries = 0;
            }

            agent.session.messages.push(ConversationMessage::assistant(
                reply_content,
                prepared_tool_calls.clone(),
            ));
            let mut emitted_bash_loop_note = false;
            for tool_call in prepared_tool_calls {
                let tool_name = tool_call.name.clone();
                if tool_name != "Bash" {
                    bash_only_tool_turn = false;
                }
                let args_str = tool_call.arguments.to_string();
                let bash_command = if tool_name == "Bash" {
                    tool_call
                        .arguments
                        .get("command")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                } else {
                    String::new()
                };
                if agent.session.mode_state.mode == ExecutionMode::Plan {
                    if is_plan_file_tool_call(
                        &tool_name,
                        &tool_call.arguments,
                        &agent.work_root,
                        agent.session.mode_state.active_plan_path.as_deref(),
                    ) {
                        plan_file_edit_calls_this_turn += 1;
                        if !logged_plan_first_write {
                            logged_plan_first_write = true;
                            log_llm_event(
                                "agent.milestone.plan_first_write",
                                serde_json::json!({
                                    "session_id": agent.session_store.session_id(),
                                    "iter": last_iter,
                                    "tool": tool_name,
                                    "path": tool_call.arguments.get("path").and_then(serde_json::Value::as_str),
                                }),
                            );
                        }
                    } else if matches!(tool_name.as_str(), "Read" | "Glob" | "Grep") {
                        plan_exploration_calls_this_turn += 1;
                    }
                }
                if agent.session.mode_state.mode == ExecutionMode::Act
                    && recovery::tool_call_counts_as_repo_edit(&tool_name)
                    && !logged_act_first_repo_edit
                {
                    logged_act_first_repo_edit = true;
                    log_llm_event(
                        "agent.milestone.act_first_repo_edit",
                        serde_json::json!({
                            "session_id": agent.session_store.session_id(),
                            "iter": last_iter,
                            "task_profile": agent.session.mode_state.task_profile.as_str(),
                            "tool": tool_name,
                            "path": tool_call.arguments.get("path").and_then(serde_json::Value::as_str),
                        }),
                    );
                }
                let block_restart_discovery = recovery::should_block_restart_discovery(
                    &tool_name,
                    restart_convergence_mode && repo_edit_calls_made_this_turn == 0,
                );
                let repeated_plan_exploration =
                    if agent.session.mode_state.mode == ExecutionMode::Plan {
                        normalize_plan_exploration_key(
                            &tool_name,
                            &tool_call.arguments,
                            &agent.work_root,
                            current_plan_stage.as_str(),
                        )
                        .map(|key| {
                            let count = plan_exploration_counts.entry(key.clone()).or_insert(0);
                            *count += 1;
                            if *count == PLAN_REPEATED_EXPLORATION_BLOCK_THRESHOLD {
                                log_llm_event(
                                    "agent.plan.repeated_exploration_detected",
                                    serde_json::json!({
                                        "session_id": agent.session_store.session_id(),
                                        "iter": last_iter,
                                        "stage": key.stage,
                                        "tool": key.tool_name,
                                        "normalized_args": key.normalized_args,
                                        "count": *count,
                                    }),
                                );
                            }
                            *count >= PLAN_REPEATED_EXPLORATION_BLOCK_THRESHOLD
                        })
                        .unwrap_or(false)
                    } else {
                        false
                    };
                // Issue #664: `recovery::should_block_bash_command` is the
                // legacy recovery-side semantics (DR1-001 案 B). SetupBootstrap
                // policy projection uses `bash::is_setup_command` instead.
                #[allow(deprecated)]
                let block_bash_loop = tool_name == "Bash"
                    && recovery::should_block_bash_command(
                        &bash_command,
                        &recent_bash_commands,
                        install_commands_seen,
                    );
                let block_repeated_plan_exploration = agent.session.mode_state.mode
                    == ExecutionMode::Plan
                    && matches!(tool_name.as_str(), "Read" | "Glob" | "Grep")
                    && repeated_plan_exploration;
                let block_plan_exploration = agent.session.mode_state.mode == ExecutionMode::Plan
                    && matches!(tool_name.as_str(), "Read" | "Glob" | "Grep")
                    && plan_exploration_budget > 0
                    && plan_exploration_calls_this_turn > plan_exploration_budget;
                tracing::debug!(
                    tool = %tool_name,
                    args = %truncate(&args_str, LOG_ARGS_MAX_CHARS),
                    "tool call"
                );
                // Issue #430 Phase D: pause footer redraw for the whole
                // tool dispatch (progress println, spinner, child-process
                // fd-inheriting exec, optional approve prompt). The guard
                // drops at the end of this iteration so the worker resumes
                // before the next loop tick.
                let _footer_freeze = agent.footer.freeze_for_inference();
                let progress = if block_restart_discovery {
                    format_blocked_progress_line(
                        &tool_name,
                        &tool_call.arguments,
                        iter_count + 1,
                        agent.config.max_iterations,
                        &agent.work_root,
                        use_color,
                        use_unicode,
                        agent.footer.current_cols(),
                        "Restart discovery blocked",
                        "Resume from the current repo state instead of restarting broad discovery.",
                        agent.session.mode_state.active_plan_path.as_deref(),
                        current_plan_stage,
                    )
                } else if block_bash_loop {
                    format_blocked_progress_line(
                        &tool_name,
                        &tool_call.arguments,
                        iter_count + 1,
                        agent.config.max_iterations,
                        &agent.work_root,
                        use_color,
                        use_unicode,
                        agent.footer.current_cols(),
                        "Bash loop blocked",
                        "Repeated shell command detected; choose a different next step.",
                        agent.session.mode_state.active_plan_path.as_deref(),
                        current_plan_stage,
                    )
                } else if block_plan_exploration {
                    format_blocked_progress_line(
                        &tool_name,
                        &tool_call.arguments,
                        iter_count + 1,
                        agent.config.max_iterations,
                        &agent.work_root,
                        use_color,
                        use_unicode,
                        agent.footer.current_cols(),
                        "Plan exploration blocked",
                        "Exploration budget reached for this stage; write the next missing plan section.",
                        agent.session.mode_state.active_plan_path.as_deref(),
                        current_plan_stage,
                    )
                } else if block_repeated_plan_exploration {
                    format_blocked_progress_line(
                        &tool_name,
                        &tool_call.arguments,
                        iter_count + 1,
                        agent.config.max_iterations,
                        &agent.work_root,
                        use_color,
                        use_unicode,
                        agent.footer.current_cols(),
                        "Plan exploration blocked",
                        "Repeated exploration detected; move the plan forward instead of rereading.",
                        agent.session.mode_state.active_plan_path.as_deref(),
                        current_plan_stage,
                    )
                } else {
                    let live_plan_stage = if agent.session.mode_state.mode == ExecutionMode::Plan {
                        agent
                            .current_plan_contents()
                            .ok()
                            .flatten()
                            .map(|contents| lifecycle::current_plan_stage(&contents))
                            .unwrap_or(current_plan_stage)
                    } else {
                        current_plan_stage
                    };
                    let stage_label = progress_stage_label(
                        agent.session.mode_state.mode,
                        live_plan_stage,
                        &tool_name,
                        &tool_call.arguments,
                        &agent.work_root,
                        agent.session.mode_state.active_plan_path.as_deref(),
                    );
                    let write_retry_label = if agent.session.mode_state.mode == ExecutionMode::Plan
                        && is_plan_file_tool_call(
                            &tool_name,
                            &tool_call.arguments,
                            &agent.work_root,
                            agent.session.mode_state.active_plan_path.as_deref(),
                        ) {
                        let raw_path = tool_call
                            .arguments
                            .get("path")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default();
                        let source_text = tool_call
                            .arguments
                            .get("content")
                            .or_else(|| tool_call.arguments.get("new_string"))
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default();
                        let summary = summarize_plan_write(
                            &tool_name,
                            raw_path,
                            source_text,
                            &agent.work_root,
                            agent.session.mode_state.active_plan_path.as_deref(),
                            live_plan_stage,
                        );
                        let count = plan_write_signature_counts
                            .entry(summary.signature)
                            .and_modify(|value| *value += 1)
                            .or_insert(1);
                        (*count > 1).then(|| format!("Model rewrite #{}", *count))
                    } else {
                        None
                    };
                    format_progress_line(
                        &tool_name,
                        &tool_call.arguments,
                        iter_count + 1,
                        agent.config.max_iterations,
                        &agent.work_root,
                        use_color,
                        use_unicode,
                        agent.footer.current_cols(),
                        agent.session.mode_state.active_plan_path.as_deref(),
                        live_plan_stage,
                        write_retry_label.as_deref(),
                        stage_label.as_deref(),
                    )
                };
                write_stdout_rendered(&progress, true);
                // approve-guard: tools Bash/Write/Edit may invoke an
                // interactive approve prompt in `tools/registry.rs`. We
                // must not let the spinner write to stderr while stdin is
                // being read. Skip spinner in that narrow case; RAII
                // scope ends when execute_tool_call returns for all
                // other branches.
                let needs_approve_prompt = matches!(tool_name.as_str(), "Bash" | "Write" | "Edit")
                    && !agent.config.yes_mode
                    && io::stdin().is_terminal();
                let start_spinner_for_exec = !needs_approve_prompt;
                // Yield raw mode to the approve `stdin().read_line` and park
                // the daemon thread until `resume()` is called. Idempotent,
                // so a tool that never triggers the prompt is unaffected.
                if needs_approve_prompt {
                    monitor.pause();
                }
                let raw_result = if block_restart_discovery {
                    recovery::broad_restart_discovery_error(&tool_name)
                } else if block_repeated_plan_exploration {
                    let next_sections = agent
                        .current_plan_contents()
                        .ok()
                        .flatten()
                        .map(|contents| lifecycle::plan_next_stage_sections(&contents))
                        .unwrap_or_default();
                    log_llm_event(
                        "agent.plan.guard_blocked",
                        serde_json::json!({
                            "session_id": agent.session_store.session_id(),
                            "iter": last_iter,
                            "tool": tool_name,
                            "reason": "repeated_exploration",
                            "stage": current_plan_stage.as_str(),
                        }),
                    );
                    recovery::repeated_plan_exploration_error(
                        current_plan_stage,
                        &next_sections,
                        &tool_name,
                    )
                } else if block_plan_exploration {
                    let next_sections = agent
                        .current_plan_contents()
                        .ok()
                        .flatten()
                        .map(|contents| lifecycle::plan_next_stage_sections(&contents))
                        .unwrap_or_default();
                    log_llm_event(
                        "agent.plan.guard_blocked",
                        serde_json::json!({
                            "session_id": agent.session_store.session_id(),
                            "iter": last_iter,
                            "tool": tool_name,
                            "reason": "exploration_budget",
                            "stage": current_plan_stage.as_str(),
                            "budget": plan_exploration_budget,
                        }),
                    );
                    recovery::plan_stage_budget_error(
                        current_plan_stage,
                        &next_sections,
                        plan_exploration_budget,
                    )
                } else if tool_name == "Bash" {
                    recent_bash_commands.push(bash_command.clone());
                    // Issue #664: legacy recovery-side classifier; retains
                    // `cargo install` semantics for the install-loop counter.
                    // SetupBootstrap policy projection uses
                    // `bash::is_setup_command` instead.
                    #[allow(deprecated)]
                    let is_install = recovery::is_dependency_install_command(&bash_command);
                    if is_install {
                        install_commands_seen += 1;
                    }
                    if block_bash_loop {
                        emitted_bash_loop_note = true;
                        // CB-001: pre-dispatch unsafe/repeated-block path.
                        // Record an UnsafeCommandBlocked frame so the
                        // session reflects the gate decision.
                        let frame =
                            build_feedback_for_unsafe_block(&bash_command, &agent.work_root);
                        agent.session.record_feedback(frame);
                        // Issue #456: count this unsafe block toward the
                        // turn-local AnvilScore counter.
                        agent.session.unsafe_blocks_this_turn =
                            agent.session.unsafe_blocks_this_turn.saturating_add(1);
                        recovery::repeated_bash_error(&bash_command)
                    } else if start_spinner_for_exec {
                        let _sp = Spinner::start(format!("running {tool_name}..."));
                        agent.execute_tool_call(
                            &tool_name,
                            &tool_call.arguments,
                            Some(&effective_tool_policy),
                            Some(interrupt_flag.flag.clone()),
                        )
                    } else {
                        agent.execute_tool_call(
                            &tool_name,
                            &tool_call.arguments,
                            Some(&effective_tool_policy),
                            Some(interrupt_flag.flag.clone()),
                        )
                    }
                } else if start_spinner_for_exec {
                    let _sp = Spinner::start(format!("running {tool_name}..."));
                    agent.execute_tool_call(
                        &tool_name,
                        &tool_call.arguments,
                        Some(&effective_tool_policy),
                        Some(interrupt_flag.flag.clone()),
                    )
                } else {
                    agent.execute_tool_call(
                        &tool_name,
                        &tool_call.arguments,
                        Some(&effective_tool_policy),
                        Some(interrupt_flag.flag.clone()),
                    )
                };
                if needs_approve_prompt {
                    monitor.resume();
                }

                // detect work_root change after each tool execution
                if agent.work_root != last_known_root {
                    let verif = verify_repo_progress(&before_snapshot, &last_known_root);
                    accumulated.push(verif);
                    before_snapshot = capture_repo_snapshot(&agent.work_root);
                    last_known_root = agent.work_root.clone();
                }

                let compact_result = prompting::compact_tool_result(&tool_name, raw_result);
                agent
                    .session
                    .messages
                    .push(ConversationMessage::tool(tool_name.clone(), compact_result));

                // CB-002: if an artifact-directed WrongTarget rejection
                // just exhausted the role-specific retry budget inside
                // `execute_tool_call`, the actor loop must terminate
                // with `MissingRepoEdits` mirror to the NoTool /
                // ProseOnly exhaustion exits above (lines 6566 /
                // 6741). The flag was flipped in
                // `record_artifact_completion_attempt` and is reset
                // at `handle_user_message` head.
                if agent.artifact_completion_exhausted_this_turn {
                    exit_reason = ExitReason::MissingRepoEdits;
                    error_text = ARTIFACT_COMPLETION_BUDGET_EXHAUSTED_TEXT.to_string();
                    break 'outer;
                }

                if agent.session.mode_state.mode == ExecutionMode::Plan
                    && is_plan_file_tool_call(
                        &tool_name,
                        &tool_call.arguments,
                        &agent.work_root,
                        agent.session.mode_state.active_plan_path.as_deref(),
                    )
                    && agent
                        .current_plan_contents()
                        .ok()
                        .flatten()
                        .is_some_and(|contents| {
                            agent.plan_is_approval_ready_with_fallback(&contents)
                        })
                {
                    plan_ready_after_tool = true;
                    break;
                }
            }
            match handle_actor_loop_plan_tool_followup(
                agent,
                ActorLoopPlanToolFollowupArgs {
                    last_iter,
                    plan_ready_after_tool,
                    plan_file_edit_calls_this_turn,
                    plan_exploration_calls_this_turn,
                    plan_missing_before_turn,
                    plan_progress_retries: &mut plan_progress_retries,
                    plan_exploration_only_turns: &mut plan_exploration_only_turns,
                },
            ) {
                ActorLoopPlanToolFollowupOutcome::Proceed => {}
                ActorLoopPlanToolFollowupOutcome::Done { final_prose: prose } => {
                    final_prose = prose;
                    exit_reason = ExitReason::Done;
                    break 'outer;
                }
                ActorLoopPlanToolFollowupOutcome::Exit {
                    reason,
                    error_text: followup_error,
                } => {
                    exit_reason = reason;
                    error_text = followup_error;
                    break 'outer;
                }
            }
            match handle_actor_loop_post_tool_fallbacks(
                agent,
                ActorLoopPostToolFallbackArgs {
                    last_iter,
                    action_expectation,
                    recovery_dispatch_gate,
                    emitted_bash_loop_note,
                    bash_only_tool_turn,
                    repo_edit_calls_made_this_turn,
                    tool_calls_made_this_turn,
                    logged_act_first_repo_edit,
                    repo_change_retries: &mut repo_change_retries,
                },
            ) {
                ActorLoopPostToolFallbackOutcome::Proceed => {}
                ActorLoopPostToolFallbackOutcome::Continue => continue,
                ActorLoopPostToolFallbackOutcome::Exit {
                    reason,
                    error_text: followup_error,
                } => {
                    exit_reason = reason;
                    error_text = followup_error;
                    break 'outer;
                }
            }
            match handle_actor_loop_post_tool_cleanup(
                agent,
                ActorLoopPostToolCleanupArgs {
                    task_contract: task_contract.as_ref(),
                    contract_verifier_repair_edit_count,
                    repo_edit_calls_made_this_turn,
                    contract_completion_retries,
                    tool_calls_made_this_turn,
                    interrupt_flag: &interrupt_flag,
                },
            ) {
                ActorLoopPostToolCleanupOutcome::Continue => continue,
                ActorLoopPostToolCleanupOutcome::Exit {
                    reason,
                    error_text: cleanup_error,
                } => {
                    exit_reason = reason;
                    error_text = cleanup_error;
                    break 'outer;
                }
            }
        }

        let final_reply = reply_content.trim().to_string();
        if missing_verifier_setup_turn {
            if agent.record_missing_verifier_setup_failure(last_iter, "no setup edit emitted") {
                exit_reason = ExitReason::MissingVerification;
                error_text =
                    "task contract requires verification, but the MissingVerifierJob setup budget is exhausted"
                        .to_string();
                break 'outer;
            }
            continue;
        }
        let task_contract_action = if agent.session.mode_state.mode == ExecutionMode::Plan {
            None
        } else {
            task_contract.as_ref().map(|contract| {
                agent.task_contract_recovery_action(
                    contract,
                    contract_verifier_repair_edit_count,
                    repo_edit_calls_made_this_turn,
                )
            })
        };
        match handle_actor_loop_task_contract_reply(
            agent,
            ActorLoopTaskContractReplyArgs {
                before_snapshot: &before_snapshot,
                accumulated: &accumulated,
                task_contract: task_contract.as_ref(),
                task_contract_action: task_contract_action.as_ref(),
                final_reply: &final_reply,
                current_reply_tool_call_count,
                repo_edit_calls_made_this_turn,
                last_iter,
                contract_completion_retries: &mut contract_completion_retries,
                contract_completion_role_retries: &mut contract_completion_role_retries,
                contract_verification_retries: &mut contract_verification_retries,
                contract_verifier_repair_edit_count: &mut contract_verifier_repair_edit_count,
                repo_change_retries: &mut repo_change_retries,
                verifier_repair_retries: &mut verifier_repair_retries,
                task_contract_verify_commands_collected:
                    &mut task_contract_verify_commands_collected,
                task_contract_verifier_passed_in_loop: &mut task_contract_verifier_passed_in_loop,
                no_tool_retries: &mut no_tool_retries,
                contract_deterministic_fallback_materialized:
                    &mut contract_deterministic_fallback_materialized,
            },
        ) {
            ActorLoopTaskContractReplyOutcome::Proceed => {}
            ActorLoopTaskContractReplyOutcome::Continue => continue,
            ActorLoopTaskContractReplyOutcome::Done {
                final_prose: next_final_prose,
            } => {
                final_prose = next_final_prose;
                exit_reason = ExitReason::Done;
                break 'outer;
            }
            ActorLoopTaskContractReplyOutcome::Exit {
                reason,
                error_text: next_error_text,
            } => {
                exit_reason = reason;
                error_text = next_error_text;
                break 'outer;
            }
        }
        match handle_actor_loop_no_tool_reply(
            agent,
            ActorLoopNoToolReplyArgs {
                last_iter,
                action_expectation,
                requires_action,
                recovery_dispatch_gate,
                final_reply: &final_reply,
                tool_calls_made_this_turn,
                repo_change_retries: &mut repo_change_retries,
                plan_progress_retries: &mut plan_progress_retries,
                empty_retries: &mut empty_retries,
                no_tool_retries: &mut no_tool_retries,
                framework_app_fallback_materialized: &mut framework_app_fallback_materialized,
            },
        ) {
            ActorLoopNoToolReplyOutcome::NotHandled => {}
            ActorLoopNoToolReplyOutcome::Continue => continue,
            ActorLoopNoToolReplyOutcome::Done {
                final_prose: next_final_prose,
            } => {
                final_prose = next_final_prose;
                exit_reason = ExitReason::Done;
                break 'outer;
            }
            ActorLoopNoToolReplyOutcome::Exit {
                reason,
                error_text: next_error_text,
            } => {
                exit_reason = reason;
                error_text = next_error_text;
                break 'outer;
            }
        }

        if let Some(outcome) = handle_post_reply_recovery(
            agent,
            PostReplyRecoveryArgs {
                last_iter,
                action_expectation,
                requires_action,
                recovery_dispatch_gate,
                repo_edit_calls_made_this_turn,
                final_reply: &final_reply,
                task_contract_action: task_contract_action.as_ref(),
                interrupt_flag: &interrupt_flag,
                repo_change_retries: &mut repo_change_retries,
                python_test_retries: &mut python_test_retries,
                no_tool_retries: &mut no_tool_retries,
                framework_app_fallback_materialized: &mut framework_app_fallback_materialized,
            },
        ) {
            match outcome {
                PostReplyRecoveryOutcome::Continue => continue,
                PostReplyRecoveryOutcome::Finalize {
                    final_prose: next_final_prose,
                    exit_reason: next_exit_reason,
                    error_text: next_error_text,
                } => {
                    final_prose = next_final_prose;
                    exit_reason = next_exit_reason;
                    error_text = next_error_text;
                    break 'outer;
                }
            }
        }

        match handle_actor_loop_completion(
            agent,
            ActorLoopCompletionArgs {
                last_iter,
                final_reply: &final_reply,
                plan_progress_retries: &mut plan_progress_retries,
            },
        ) {
            ActorLoopCompletionOutcome::Continue => continue,
            ActorLoopCompletionOutcome::Done {
                final_prose: next_final_prose,
            } => {
                final_prose = next_final_prose;
                exit_reason = ExitReason::Done;
                break 'outer;
            }
            ActorLoopCompletionOutcome::Exit {
                reason,
                error_text: next_error_text,
            } => {
                exit_reason = reason;
                error_text = next_error_text;
                break 'outer;
            }
        }
    }

    // Single exit point: compute stats and return LoopResult
    let duration_secs = start.elapsed().as_secs();
    let final_verif = verify_repo_progress(&before_snapshot, &agent.work_root);
    // CB-001 / CB2-002: NoRepoProgress is only recorded when *this turn*
    // attempted at least one repo-mutating tool call (Write / Edit) but
    // produced no measurable repo diff, AND no other FeedbackFrame has
    // already been recorded this turn. Read-only / answer-only turns
    // (no Write/Edit attempted) intentionally leave `last_feedback`
    // untouched so consumers do not mistake a successful investigation
    // for a "no progress" failure (design 5.5 last-write-wins).
    if should_record_no_repo_progress(
        repo_edit_calls_made_this_turn,
        final_verif.made_any_progress(),
        agent.session.eligible_feedback_recorded_this_turn,
    ) {
        // Issue #455 / D4: switch to first-eligible-failure-wins so a
        // deterministic content fallback / NoToolCall frame recorded
        // earlier in this turn is preserved over the post-loop
        // NoRepoProgress signal.
        let frame = build_feedback_for_no_repo_progress(&agent.work_root);
        agent.session.record_feedback_if_unset(frame);
    }
    // Issue #456: maintain `consecutive_no_progress_turns` baseline. A
    // turn that produced verifiable progress resets the counter; a turn
    // that recorded NoRepoProgress increments it. Other failure shapes
    // (build/test failure with diff, parser failure, etc.) leave the
    // counter unchanged.
    if final_verif.made_any_progress() {
        agent.session.consecutive_no_progress_turns = 0;
    } else if matches!(
        agent.session.last_feedback.as_ref().map(|f| f.kind.clone()),
        Some(FeedbackKind::NoRepoProgress)
    ) {
        agent.session.consecutive_no_progress_turns = agent
            .session
            .consecutive_no_progress_turns
            .saturating_add(1);
    }
    // Issue #601: populate per-turn counters that Case F no-progress
    // detection consumes. The SSOT for `iter_count_this_turn` is the
    // local `last_iter.min(agent.config.max_iterations)` expression below
    // (S5-002 — `agent.last_iter` field does NOT exist; only the local
    // mutable `last_iter` in the actor loop exists). For
    // `tool_calls_this_turn` the SSOT is the local
    // `tool_calls_made_this_turn` counter. Populate happens here, after
    // the loop exits but before any post-loop hook reads the values
    // (Reminder / CaseRecord / AntiPattern / photon evaluate all run
    // below this line).
    agent.session.iter_count_this_turn = last_iter.min(agent.config.max_iterations);
    agent.session.tool_calls_this_turn = tool_calls_made_this_turn;
    let stats = build_stats(
        accumulated,
        final_verif.clone(),
        last_iter.min(agent.config.max_iterations),
        agent.config.max_iterations,
        duration_secs,
    );
    if exit_reason == ExitReason::ToolCallFormatError
        && model_capabilities(&agent.current_assistant_model()).finish_after_edit_format_error
        && stats.total_changed > 0
        && agent.session.mode_state.mode == ExecutionMode::Act
        && (!agent.active_python_request_requires_tests() || agent.python_test_artifact_exists())
    {
        // Issue #634: 旧文言は qwen3.5 を名指ししていたが、capability ベース
        // (`finish_after_edit_format_error`) に統一されたためモデル非依存の文言に変更。
        final_prose =
            "Applied repository edits before a malformed follow-up tool call.".to_string();
        exit_reason = ExitReason::Done;
        error_text.clear();
    }
    let mut verify_commands_collected = task_contract_verify_commands_collected;
    verify_commands_collected.extend(agent.run_post_loop_success_verifier(
        &final_verif,
        &stats,
        repo_edit_calls_made_this_turn,
        task_contract_verifier_passed_in_loop,
        &mut exit_reason,
        &mut error_text,
    ));
    // Issue #452: Reminder Sidecar (post-loop hook). Picks up
    // NoRepoProgress / auto_test / NoVerifierAvailable frames recorded
    // after the actor loop exited. Per-turn cap means this no-ops if the
    // iteration-internal hook already ran.
    agent.maybe_invoke_reminder(&interrupt_flag);
    // Issue #462: CaseRecord extraction (post-loop, after Reminder, before
    // turn_completed event). Pure success-condition + scrub + persist; no
    // sidecar / LLM calls. Failures are logged and never propagate.
    //
    // DR2-008 (Issue #604): now returns `Option<CaseRecord>` so the
    // post-loop auto-promote hook can consume the freshly-extracted record
    // without re-reading from disk.
    let extracted_case = agent.maybe_extract_case_record(&stats, &verify_commands_collected);
    // Issue #464: AntiPatternRecord extraction (post-loop, after CaseRecord).
    // Triggered by the latest eligible failure feedback. Pure upsert; no
    // sidecar / LLM calls.
    agent.maybe_extract_anti_pattern();

    // [Issue #556] post-loop photon evaluate hook — must run before
    // build_eval_record so last_photon_eval_summary is populated.
    // Clear per-turn context_pack_response here (no longer needed).
    agent.photon_context_pack_response = None;
    if agent.session.mode_state.mode != ExecutionMode::Plan {
        agent.invoke_photon_evaluate();
    } else if agent.photon.is_some() {
        log_llm_event(
            "agent.photon_evaluate.skipped",
            serde_json::json!({
                "session_id": agent.session_store.session_id(),
                "turn_index": agent.current_turn_index,
                "reason": "plan_mode",
            }),
        );
    }

    // Issue #604 (Task 5.2): post-loop auto-promote hook. Order B —
    // runs *after* invoke_photon_evaluate, before build_eval_record so
    // `last_auto_promote_outcome` is populated for `EvalRecord.auto_promote`.
    // Fail-open: the hook never panics or interrupts the agent loop.
    {
        use crate::agent::loop_run::auto_promote::{AutoPromoteConfig, invoke_photon_auto_promote};
        use crate::session::auto_promote_scrub::ScrubMode;
        use std::time::{SystemTime, UNIX_EPOCH};

        let now_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let interrupted = interrupt_flag.is_set();
        let session_id = agent.session_store.session_id().to_string();
        let turn_idx = agent.current_turn_index as u64;
        let state_root = agent.session_store.state_root().to_path_buf();
        let cfg = AutoPromoteConfig {
            enabled: agent.config.photon_auto_promote,
            force_disabled: agent.config.photon_no_auto_promote,
            dry_run: agent.config.photon_auto_promote_dry_run,
            scrub_mode: ScrubMode::from_env_str_or_default(
                &agent.config.photon_auto_promote_scrub_mode,
            ),
        };
        // Per-turn cap: set BEFORE invoking on non-Interrupted/non-Disabled
        // paths. DR2-010 — Interrupted intentionally leaves the flag false
        // so the next turn can re-try. The hook itself never flips it
        // (the flag is the caller's responsibility).
        let will_invoke = !interrupted
            && cfg.enabled
            && !cfg.force_disabled
            && agent.photon.is_some()
            && !agent.session.auto_promote_called_this_turn;
        if will_invoke {
            agent.session.auto_promote_called_this_turn = true;
        }
        // Borrow split: read all `&self`-only fields first, then re-borrow
        // `agent.photon` and call the free function.
        let plan_mode = agent.session.mode_state.mode == ExecutionMode::Plan;
        let auto_called = agent.session.auto_promote_called_this_turn;
        // NB: `should_auto_promote` re-checks `auto_called` and routes to
        // `PerTurnCapConsumed` only if the flag was *already* true on
        // entry. Because we just flipped it ABOVE (on the will_invoke path),
        // we pass the pre-flip value here.
        let auto_called_for_gate = if will_invoke { false } else { auto_called };
        let extracted_this_turn = agent.session.case_record_extracted_this_turn;
        let photon_ref = agent.photon.as_ref();
        let outcome = invoke_photon_auto_promote(
            &session_id,
            turn_idx,
            plan_mode,
            auto_called_for_gate,
            extracted_this_turn,
            extracted_case.as_ref(),
            &state_root,
            now_unix,
            interrupted,
            photon_ref,
            &cfg,
        );
        agent.last_auto_promote_outcome = Some(outcome);
    }

    // Issue #471: write structured eval log record (turn-level snapshot).
    {
        use crate::session::eval_log::{
            AnvilScoreSummary, ChangedFileClasses, EvalPrecautionSnapshot, FeedbackFrameSummary,
            build_eval_record, write_eval_record,
        };
        use crate::session::precaution::PrecautionStatus;
        use std::time::{SystemTime, UNIX_EPOCH};

        let ts_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let active_task = agent
            .session
            .working_memory
            .active_task
            .as_deref()
            .unwrap_or("");
        let session_id = agent.session_store.session_id().to_string();
        let model = agent.models.main.clone();
        let mode_str = format!("{:?}", agent.session.mode_state.mode);
        let tool_protocol = if agent.native_tools_enabled {
            "native"
        } else {
            "xml"
        };
        let feedback_summary =
            agent
                .session
                .last_feedback
                .as_ref()
                .map(|ff| FeedbackFrameSummary {
                    kind: format!("{:?}", ff.kind),
                    excerpt: {
                        let raw = format!("{}{}", ff.stdout_excerpt(), ff.stderr_excerpt());
                        let masked = crate::session::feedback::mask_secrets(&raw);
                        if masked.len() > crate::session::eval_log::MAX_EVAL_FEEDBACK_EXCERPT_BYTES
                        {
                            let mut end = crate::session::eval_log::MAX_EVAL_FEEDBACK_EXCERPT_BYTES;
                            while !masked.is_char_boundary(end) {
                                end -= 1;
                            }
                            format!("{}…", &masked[..end])
                        } else {
                            masked
                        }
                    },
                });
        let precaution_snapshots: Vec<EvalPrecautionSnapshot> = agent
            .session
            .working_memory
            .active_precautions
            .iter()
            .filter(|p| p.status == PrecautionStatus::Active)
            .map(EvalPrecautionSnapshot::from)
            .collect();
        let anvil_summary = agent
            .session
            .last_anvil_score
            .as_ref()
            .map(AnvilScoreSummary::from);
        let changed_classes = ChangedFileClasses {
            test: stats.changed_test_count,
            impl_files: stats.changed_impl_count,
            setup: stats.changed_setup_count,
        };
        let mut record = build_eval_record(
            &session_id,
            ts_ms,
            active_task,
            &model,
            &mode_str,
            tool_protocol,
            &tool_call_summaries,
            feedback_summary,
            &precaution_snapshots,
            anvil_summary,
            changed_classes,
            &verify_commands_collected,
            agent.last_case_retrieval_summary.take(),
            agent.last_photon_eval_summary.take(),
            agent.last_auto_promote_outcome.clone(),
            exit_reason.label(),
        );
        record.photon_canary = agent.config.photon_canary;
        write_eval_record(&record);
    }

    // Issue #659 (Task 2.2 / Task 2.7): end-of-turn ArtifactLedger
    // observability. The turn_summary event fires once per termination
    // path (both SafeStop and Done); the dual-source divergence
    // assertion runs adjacent so the adapter-period contract
    // (`turn_edited_relative_paths` == ledger RepoEdit projection) is
    // pinned at the same boundary that the summary publishes.
    agent.assert_dual_source_alignment_at_turn_end();
    agent.record_turn_end_artifact_ledger_summary();

    log_llm_event(
        "agent.milestone.turn_completed",
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "mode": format!("{:?}", agent.session.mode_state.mode),
            "task_profile": agent.session.mode_state.task_profile.as_str(),
            "exit_reason": exit_reason.label(),
            "iter_used": stats.iter_used,
            "iter_max": stats.iter_max,
            "duration_secs": stats.duration_secs,
            "total_changed": stats.total_changed,
            "changed_files": stats.changed_files.clone(),
        }),
    );

    if exit_reason.is_success() {
        agent.session.messages.push(ConversationMessage::assistant(
            final_prose.clone(),
            Vec::new(),
        ));
        Ok((final_prose, stats))
    } else {
        if error_text.is_empty() {
            error_text = exit_reason.default_error_text().to_string();
        }
        Err((exit_reason, error_text, stats))
    }
}

pub(super) fn build_stats(
    accumulated: Vec<RepoVerification>,
    final_verif: RepoVerification,
    iter_used: usize,
    iter_max: usize,
    duration_secs: u64,
) -> LoopStats {
    let mut all_changed: HashSet<String> = HashSet::new();
    let mut all_changed_full: HashSet<String> = HashSet::new();
    let mut impl_changed = 0usize;
    let mut test_changed = 0usize;
    let mut setup_changed = 0usize;
    let mut other_changed = 0usize;
    let mut deleted_changed = 0usize;

    for verif in accumulated.iter().chain(std::iter::once(&final_verif)) {
        for f in &verif.changed_files {
            if is_ignored_workspace_display_path(f) {
                continue;
            }
            all_changed.insert(f.clone());
        }
        for f in &verif.all_changed_files {
            if is_ignored_workspace_display_path(f) {
                continue;
            }
            all_changed_full.insert(f.clone());
        }
        if verif
            .all_changed_files
            .iter()
            .all(|f| !is_ignored_workspace_display_path(f))
        {
            impl_changed += verif.implementation_files_changed;
            test_changed += verif.test_files_changed;
            setup_changed += verif.setup_files_changed;
            other_changed += verif.other_files_changed;
            deleted_changed += verif.deleted_files_changed;
        } else {
            for f in &verif.all_changed_files {
                if is_ignored_workspace_display_path(f) {
                    continue;
                }
                let (path, deleted) = f
                    .strip_suffix(" (deleted)")
                    .map(|path| (path, true))
                    .unwrap_or((f.as_str(), false));
                if deleted {
                    deleted_changed += 1;
                } else if crate::util::file_classify::is_test_file(Path::new(path)) {
                    test_changed += 1;
                } else if crate::util::file_classify::is_setup_file(Path::new(path)) {
                    setup_changed += 1;
                } else if crate::util::file_classify::is_implementation_file(Path::new(path)) {
                    impl_changed += 1;
                } else {
                    other_changed += 1;
                }
            }
        }
    }

    let total_changed =
        impl_changed + test_changed + setup_changed + other_changed + deleted_changed;
    let mut changed_files: Vec<String> = all_changed.into_iter().collect();
    changed_files.sort();
    changed_files.truncate(16);
    let mut all_changed_files: Vec<String> = all_changed_full.into_iter().collect();
    all_changed_files.sort();

    LoopStats {
        iter_used,
        iter_max,
        duration_secs,
        changed_files: changed_files.into_boxed_slice(),
        all_changed_files: all_changed_files.into_boxed_slice(),
        total_changed,
        changed_impl_count: impl_changed,
        changed_test_count: test_changed,
        changed_setup_count: setup_changed,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn format_progress_line(
    tool_name: &str,
    arguments: &serde_json::Value,
    iter_human: usize,
    max_iterations: usize,
    work_root: &std::path::Path,
    use_color: bool,
    use_unicode: bool,
    cols: Option<u16>,
    plan_path: Option<&Path>,
    current_stage: PlanStage,
    status_prefix: Option<&str>,
    stage_label: Option<&str>,
) -> String {
    let arg_budget =
        progress_available_width(cols, tool_name, iter_human, max_iterations, use_unicode);
    let display = tool_display(
        tool_name,
        arguments,
        work_root,
        plan_path,
        current_stage,
        arg_budget,
    );
    // Sanitize before painting so an adversarial tool_name cannot inject escapes.
    // emoji は &'static str ハードコードなので再 sanitize は不要。
    let safe_tool_name = sanitize_for_progress(tool_name);
    let label = if use_unicode {
        format!("{} {}", tool_emoji(tool_name), safe_tool_name)
    } else {
        safe_tool_name
    };
    let painted = paint(&label, tool_color(tool_name), use_color);
    if matches!(tool_name, "Read" | "Write" | "Edit") {
        let mut lines = Vec::new();
        let stage = stage_label.unwrap_or("Working");
        lines.push(format!("[iter {iter_human}/{max_iterations}] {stage}"));
        lines.push(format!("  tool:   {painted}"));
        lines.push(format_progress_field("  action: ", &display.action, cols));
        if let Some(path) = display.path {
            lines.push(format_progress_field("  file:   ", &path, cols));
        }
        if let Some(note) = display.note {
            lines.push(format_progress_field("  note:   ", &note, cols));
        }
        if let Some(status) = display.status {
            let combined_status = status_prefix
                .map(|prefix| format!("{prefix} | {status}"))
                .unwrap_or(status);
            lines.push(format_progress_field("  status: ", &combined_status, cols));
        } else if let Some(prefix) = status_prefix {
            lines.push(format_progress_field("  status: ", prefix, cols));
        }
        lines.push(String::new());
        lines.join("\n")
    } else {
        format!(
            "[iter {iter_human}/{max_iterations}]  {painted}  {}",
            display.action
        )
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn format_blocked_progress_line(
    tool_name: &str,
    arguments: &serde_json::Value,
    iter_human: usize,
    max_iterations: usize,
    work_root: &std::path::Path,
    use_color: bool,
    use_unicode: bool,
    cols: Option<u16>,
    headline: &str,
    note: &str,
    plan_path: Option<&Path>,
    current_stage: PlanStage,
) -> String {
    let arg_budget =
        progress_available_width(cols, headline, iter_human, max_iterations, use_unicode);
    let display = tool_display(
        tool_name,
        arguments,
        work_root,
        plan_path,
        current_stage,
        arg_budget,
    );
    let label = if use_unicode {
        format!("⛔ {headline}")
    } else {
        headline.to_string()
    };
    let painted = paint(&label, "\x1b[38;5;196m", use_color);
    if matches!(tool_name, "Read" | "Write" | "Edit") {
        let mut lines = Vec::new();
        lines.push(format!("[iter {iter_human}/{max_iterations}] {headline}"));
        lines.push(format!("  tool:   {painted}"));
        lines.push(format_progress_field("  action: ", &display.action, cols));
        if let Some(path) = display.path {
            lines.push(format_progress_field("  file:   ", &path, cols));
        }
        lines.push(format_progress_field("  status: ", note, cols));
        lines.push(String::new());
        lines.join("\n")
    } else {
        format!(
            "[iter {iter_human}/{max_iterations}]  {painted}  {}",
            display.action
        )
    }
}

pub(super) fn summarize_plan_write(
    tool_name: &str,
    raw_path: &str,
    new_text: &str,
    work_root: &Path,
    plan_path: Option<&Path>,
    current_stage: PlanStage,
) -> PlanWriteSummary {
    let previous = plan_write_previous_contents(raw_path, work_root, plan_path);
    let delta_sections = build_plan_write_section_delta(&previous, new_text);
    let verb = if !delta_sections.removed_sections.is_empty() {
        "Rewrite"
    } else if delta_sections.previous_sections.is_empty() {
        "Draft"
    } else if !delta_sections.added_sections.is_empty() {
        "Add"
    } else if tool_name == "Edit" {
        "Revise"
    } else {
        "Update"
    };
    let action = if delta_sections.focus_sections.is_empty() {
        "Update plan draft".to_string()
    } else {
        format!(
            "{verb} {}",
            join_sections_for_progress(&delta_sections.focus_sections)
        )
    };
    let note = plan_section_excerpt(new_text, &delta_sections.focus_sections).or_else(|| {
        let fallback = delta_sections.current_sections.clone();
        plan_section_excerpt(new_text, &fallback)
    });
    let delta = new_text.len() as isize - previous.len() as isize;
    let approval_ready = lifecycle::plan_missing_sections(new_text).is_empty();
    let status = plan_write_status(new_text, delta);
    let phase = plan_phase_from_sections(
        &delta_sections.focus_sections,
        current_stage,
        approval_ready,
    )
    .to_string();
    let signature = format!("{}|{}|{}", phase, action, note.clone().unwrap_or_default());
    PlanWriteSummary {
        action,
        note,
        status,
        phase,
        signature,
    }
}

pub(super) fn progress_stage_label(
    mode: ExecutionMode,
    plan_stage: PlanStage,
    tool_name: &str,
    arguments: &serde_json::Value,
    work_root: &Path,
    plan_path: Option<&Path>,
) -> Option<String> {
    if mode != ExecutionMode::Plan {
        return Some("Implementation".to_string());
    }
    let raw_path = arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if plan_path_matches(raw_path, work_root, plan_path) {
        if tool_name == "Read" {
            return Some(if plan_stage == PlanStage::Ready {
                "Approval review".to_string()
            } else {
                "Plan review".to_string()
            });
        }
        let source_text = arguments
            .get("content")
            .or_else(|| arguments.get("new_string"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let summary = summarize_plan_write(
            tool_name,
            raw_path,
            source_text,
            work_root,
            plan_path,
            plan_stage,
        );
        return Some(summary.phase);
    }
    Some("Repo exploration".to_string())
}

pub(super) fn log_plan_stall(
    session_id: &str,
    iter: usize,
    reason: &str,
    stage: PlanStage,
    next_sections: &[&str],
    missing_sections: &[&str],
    attempt: usize,
) {
    log_llm_event(
        "agent.plan.stalled",
        serde_json::json!({
            "session_id": session_id,
            "iter": iter,
            "reason": reason,
            "stage": stage.as_str(),
            "next_sections": next_sections,
            "missing_sections": missing_sections,
            "attempt": attempt,
        }),
    );
}

pub(super) fn normalize_plan_exploration_key(
    tool_name: &str,
    arguments: &serde_json::Value,
    work_root: &Path,
    stage: &str,
) -> Option<PlanExplorationKey> {
    let normalized_args = match tool_name {
        "Read" => {
            let path = arguments.get("path").and_then(serde_json::Value::as_str)?;
            let path = normalize_exploration_path(path, work_root);
            let start_line = arguments
                .get("start_line")
                .and_then(serde_json::Value::as_u64);
            let end_line = arguments
                .get("end_line")
                .and_then(serde_json::Value::as_u64);
            serde_json::json!({
                "path": path,
                "start_line": start_line,
                "end_line": end_line,
            })
            .to_string()
        }
        "Glob" => serde_json::json!({
            "pattern": arguments
                .get("pattern")
                .and_then(serde_json::Value::as_str)?
                .trim(),
        })
        .to_string(),
        "Grep" => serde_json::json!({
            "pattern": arguments
                .get("pattern")
                .and_then(serde_json::Value::as_str)?
                .trim(),
            "glob": arguments.get("glob").and_then(serde_json::Value::as_str),
            "case_sensitive": arguments
                .get("case_sensitive")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        })
        .to_string(),
        _ => return None,
    };

    Some(PlanExplorationKey {
        stage: stage.to_string(),
        tool_name: tool_name.to_string(),
        normalized_args,
    })
}

pub(super) fn build_feedback_for_unsafe_block(
    command: &str,
    workspace_root: &Path,
) -> FeedbackFrame {
    let draft = FeedbackFrameDraft {
        kind: FeedbackKind::UnsafeCommandBlocked,
        command: Some(command.to_string()),
        primary_error: Some(format!("unsafe command blocked: {command}")),
        ..Default::default()
    };
    build_feedback_frame(draft, workspace_root)
}

pub(super) fn build_feedback_for_no_repo_progress(workspace_root: &Path) -> FeedbackFrame {
    let draft = FeedbackFrameDraft {
        kind: FeedbackKind::NoRepoProgress,
        primary_error: Some("turn ended without modifying repository files".to_string()),
        ..Default::default()
    };
    build_feedback_frame(draft, workspace_root)
}

pub(super) fn build_feedback_for_no_tool_call(
    reason: &'static str,
    workspace_root: &Path,
) -> FeedbackFrame {
    let draft = FeedbackFrameDraft {
        kind: FeedbackKind::NoToolCall,
        primary_error: Some(reason.to_string()),
        ..Default::default()
    };
    build_feedback_frame(draft, workspace_root)
}

pub(super) fn should_record_no_repo_progress(
    repo_edit_calls_made_this_turn: usize,
    final_made_any_progress: bool,
    last_feedback_changed_this_turn: bool,
) -> bool {
    repo_edit_calls_made_this_turn > 0
        && !final_made_any_progress
        && !last_feedback_changed_this_turn
}

pub(super) fn task_contract_verifier_edit_required_note(
    attempt: usize,
    attempt_limit: usize,
) -> String {
    format!(
        "[Task Contract Verification] The verifier already failed and no repository edit has been made since that diagnostic. Do not rerun verification and do not answer in prose. Inspect project files if needed, then emit a Write or Edit tool call that repairs the failing implementation, tests, or setup. task_contract_verify_edit_attempt={attempt}/{attempt_limit}"
    )
}

pub(super) fn missing_repo_change_retry_status_note(
    kind: ActorLoopMissingRepoChangeReplyKind,
) -> &'static str {
    match kind {
        ActorLoopMissingRepoChangeReplyKind::Empty => {
            "The model replied without edits. Asked it to make the required repository changes."
        }
        ActorLoopMissingRepoChangeReplyKind::ProseOnly => {
            "The model answered with prose only. Asked it to emit exactly one tool call now and resume concrete repo work."
        }
    }
}

pub(super) fn task_contract_safe_stop_clear_tag(
    reason: super::task_contract::SafeStopReason,
) -> &'static str {
    match reason {
        super::task_contract::SafeStopReason::VerifierWeak => {
            "task_contract_safe_stop_verifier_weak"
        }
        super::task_contract::SafeStopReason::VerifierMissing => {
            "task_contract_safe_stop_verifier_missing"
        }
    }
}

pub(super) fn focused_policy_retry_exhausted(focused_retry_present: bool, retries: usize) -> bool {
    focused_retry_present && retries >= 3
}

pub(super) fn unrestricted_policy_retry_exhausted(
    focused_retry_present: bool,
    retries: usize,
) -> bool {
    !focused_retry_present && retries >= 3
}

pub(super) fn rejected_tool_batch_retry_status_note(focused_retry_present: bool) -> &'static str {
    if focused_retry_present {
        "Focused edit recovery requires exactly one compact tool call on the target file. Asked the model to retry with a single action."
    } else {
        "The tool call violated the current tool policy. Asked the model to retry with an allowed tool."
    }
}

pub(super) fn should_try_framework_app_fallback(
    last_iter: usize,
    already_materialized: bool,
) -> bool {
    last_iter > 1 && !already_materialized
}

pub(super) fn framework_app_fallback_continuation_note() -> &'static str {
    "[Deterministic App Fallback] Treat the materialized framework files as a recovery scaffold only, not as task completion. Continue by reading and editing the real UI entry file with task-specific implementation details, then verify the app before final response."
}

pub(super) fn build_feedback_for_tool_protocol_failure(
    err: &str,
    workspace_root: &Path,
) -> FeedbackFrame {
    let draft = FeedbackFrameDraft {
        kind: FeedbackKind::ToolProtocolFailure,
        primary_error: Some(err.to_string()),
        ..Default::default()
    };
    build_feedback_frame(draft, workspace_root)
}

pub(super) fn build_feedback_for_deterministic_content_fallback(
    workspace_root: &Path,
) -> FeedbackFrame {
    let draft = FeedbackFrameDraft {
        kind: FeedbackKind::ToolProtocolFailure,
        primary_error: Some(DETERMINISTIC_CONTENT_FALLBACK_TAG.to_string()),
        ..Default::default()
    };
    build_feedback_frame(draft, workspace_root)
}

pub(super) fn reply_looks_like_future_work(reply: &str) -> bool {
    let normalized = reply.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return false;
    }
    let completion_markers = [
        "done",
        "completed",
        "implemented",
        "finished",
        "ready",
        "作成しました",
        "実装しました",
        "完了",
        "できました",
    ];
    if completion_markers
        .iter()
        .any(|marker| normalized.contains(marker))
    {
        return false;
    }
    let future_markers = [
        "now i'll",
        "now i will",
        "i'll ",
        "i will ",
        "let me ",
        "you can run",
        "please run",
        "run this yourself",
        "run it yourself",
        "next,",
        "next i",
        "次に",
        "これから",
        "今から",
        "次は",
        "探してみます",
        "確認します",
        "調べます",
        "見てみます",
        "してみます",
        "実行してください",
        "確認してください",
    ];
    future_markers
        .iter()
        .any(|marker| normalized.contains(marker))
}

pub(super) fn task_contract_verifier_safe_stop_mapping(
    reason: super::task_contract::SafeStopReason,
) -> (ExitReason, &'static str) {
    match reason {
        super::task_contract::SafeStopReason::VerifierWeak => {
            (ExitReason::SafeStopVerifierWeak, "safe_stop_verifier_weak")
        }
        super::task_contract::SafeStopReason::VerifierMissing => (
            ExitReason::SafeStopVerifierMissing,
            "safe_stop_verifier_missing",
        ),
    }
}

pub(super) fn task_contract_continue_requires_tool_recovery(
    action: Option<&super::task_contract::ArtifactRecoveryAction>,
    current_reply_tool_calls: usize,
) -> bool {
    matches!(
        action,
        Some(super::task_contract::ArtifactRecoveryAction::Continue { .. })
    ) && current_reply_tool_calls == 0
}

pub(super) fn increment_artifact_completion_role_attempt(
    attempts: &mut HashMap<super::task_contract::ArtifactRole, usize>,
    role: super::task_contract::ArtifactRole,
) -> usize {
    let entry = attempts.entry(role).or_insert(0);
    *entry = entry.saturating_add(1);
    *entry
}

pub(super) fn should_apply_repo_change_partial_progress_recovery(
    action_expectation: recovery::ActionExpectation,
    repo_edit_calls_made_this_turn: usize,
    final_reply: &str,
    task_contract_action: Option<&super::task_contract::ArtifactRecoveryAction>,
) -> bool {
    let contract_allows_generic_recovery = match task_contract_action {
        None | Some(super::task_contract::ArtifactRecoveryAction::Done) => true,
        Some(
            super::task_contract::ArtifactRecoveryAction::Continue { .. }
            | super::task_contract::ArtifactRecoveryAction::RunVerifier
            | super::task_contract::ArtifactRecoveryAction::RepairArtifact { .. },
        ) => false,
        // Issue #651 Phase 4.2: SafeStop says the agent must stop without
        // claiming completion. Generic repo-change partial-progress
        // recovery (which would prompt the model to keep editing) is
        // never appropriate in that mode — we are about to surface the
        // safe stop to the user. `_ =>` fallback stays forbidden per
        // design judgement #2 so a future SafeStopReason variant lights
        // up this match site.
        Some(super::task_contract::ArtifactRecoveryAction::SafeStop { .. }) => false,
    };

    action_expectation == recovery::ActionExpectation::RepoChange
        && repo_edit_calls_made_this_turn > 0
        && contract_allows_generic_recovery
        && reply_looks_like_future_work(final_reply)
}

pub(super) fn answer_only_reply_is_inadequate(reply: &str) -> bool {
    let trimmed = reply.trim();
    if trimmed.is_empty() {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "read('readme.md')" | "read(\"readme.md\")" | "glob('**/*.md')" | "grep"
    ) {
        return true;
    }
    if (lower.starts_with("read(")
        || lower.starts_with("glob(")
        || lower.starts_with("grep(")
        || lower.starts_with("bash("))
        && trimmed.chars().count() < 120
    {
        return true;
    }
    // Issue #574: do not use length as a proxy for adequacy. Short factual
    // answers (codename, single value, Yes/No, especially in Japanese) were
    // being discarded and replaced with a canned fallback. Only empty and
    // tool-call-like replies are inadequate.
    false
}

pub(super) fn format_iteration_status(
    iter_human: usize,
    max_iterations: usize,
    headline: &str,
    note: &str,
    cols: Option<u16>,
) -> String {
    let mut lines = vec![format!("[iter {iter_human}/{max_iterations}] {headline}")];
    lines.push(format_progress_field("  note:   ", note, cols));
    lines.push(String::new());
    lines.join("\n")
}

pub(super) fn should_apply_repo_change_quality_gate(
    action_expectation: recovery::ActionExpectation,
    active_task_expects_repo_change: bool,
    mode: ExecutionMode,
) -> bool {
    mode == ExecutionMode::Act
        && (action_expectation == recovery::ActionExpectation::RepoChange
            || active_task_expects_repo_change)
}

pub(super) struct PlanWriteSummary {
    pub(super) action: String,
    pub(super) note: Option<String>,
    pub(super) status: Option<String>,
    pub(super) phase: String,
    pub(super) signature: String,
}

pub(super) fn plan_path_matches(
    raw_path: &str,
    work_root: &Path,
    plan_path: Option<&Path>,
) -> bool {
    resolve_plan_mode_write_target(work_root, raw_path, plan_path)
        .ok()
        .flatten()
        .is_some()
}

pub(super) fn plan_section_excerpt(contents: &str, sections: &[&str]) -> Option<String> {
    for section in sections {
        let Some(body) = plan_section_body_for_progress(contents, section) else {
            continue;
        };
        for line in body.lines().map(str::trim) {
            if line.is_empty()
                || line == "-"
                || matches!(
                    line,
                    "1." | "2."
                        | "3."
                        | "1. First slice:"
                        | "2. Next phases:"
                        | "3. Review checkpoint:"
                )
            {
                continue;
            }
            let cleaned = line.trim_start_matches("- ").trim();
            return Some(format!(
                "{section}: {}",
                truncate(&sanitize_for_progress(cleaned), 72)
            ));
        }
    }
    None
}

pub(super) fn plan_write_previous_contents(
    raw_path: &str,
    work_root: &Path,
    plan_path: Option<&Path>,
) -> String {
    if raw_path.is_empty() {
        String::new()
    } else if plan_path_matches(raw_path, work_root, plan_path) {
        plan_path
            .and_then(|path| std::fs::read_to_string(path).ok())
            .unwrap_or_default()
    } else {
        resolve_user_path(work_root, raw_path)
            .ok()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .unwrap_or_default()
    }
}

pub(super) struct PlanWriteSectionDelta {
    pub(super) previous_sections: Vec<&'static str>,
    pub(super) current_sections: Vec<&'static str>,
    pub(super) added_sections: Vec<&'static str>,
    pub(super) removed_sections: Vec<&'static str>,
    pub(super) focus_sections: Vec<&'static str>,
}

pub(super) fn build_plan_write_section_delta(
    previous: &str,
    new_text: &str,
) -> PlanWriteSectionDelta {
    let previous_sections = plan_sections_with_content(previous);
    let current_sections = plan_sections_with_content(new_text);
    let changed_sections = current_sections
        .iter()
        .copied()
        .filter(|section| {
            let old_body = plan_section_body_for_progress(previous, section).unwrap_or_default();
            let new_body = plan_section_body_for_progress(new_text, section).unwrap_or_default();
            sanitize_for_progress(old_body) != sanitize_for_progress(new_body)
        })
        .collect::<Vec<_>>();
    let added_sections = current_sections
        .iter()
        .copied()
        .filter(|section| !previous_sections.contains(section))
        .collect::<Vec<_>>();
    let removed_sections = previous_sections
        .iter()
        .copied()
        .filter(|section| !current_sections.contains(section))
        .collect::<Vec<_>>();
    let focus_sections = if !changed_sections.is_empty() {
        changed_sections
    } else if !current_sections.is_empty() {
        current_sections.clone()
    } else {
        Vec::new()
    };
    PlanWriteSectionDelta {
        previous_sections,
        current_sections,
        added_sections,
        removed_sections,
        focus_sections,
    }
}

pub(super) fn plan_write_status(new_text: &str, delta: isize) -> Option<String> {
    let next = lifecycle::plan_next_stage_sections(new_text);
    if lifecycle::plan_missing_sections(new_text).is_empty() {
        Some(format!("Approval ready | delta {delta:+}B"))
    } else if !next.is_empty() {
        Some(format!(
            "Next: {} | delta {delta:+}B",
            join_sections_for_progress(&next)
        ))
    } else {
        Some(format!("delta {delta:+}B"))
    }
}

pub(super) fn plan_phase_from_sections(
    sections: &[&str],
    current_stage: PlanStage,
    approval_ready: bool,
) -> &'static str {
    if approval_ready {
        "Approval review"
    } else if sections.iter().any(|section| {
        matches!(
            *section,
            "First Action"
                | "Verification"
                | "Execution Plan"
                | "Verification Plan"
                | "Risks / Fallbacks"
        )
    }) {
        "Define next action"
    } else if sections
        .iter()
        .any(|section| matches!(*section, "Acceptance Criteria" | "Quality Bar"))
    {
        "Define quality bar"
    } else if sections
        .iter()
        .any(|section| matches!(*section, "Goal" | "Constraints" | "Deliverables"))
    {
        "Draft foundation"
    } else {
        match current_stage {
            PlanStage::Stage1 => "Draft foundation",
            PlanStage::Stage2 => "Define next action",
            PlanStage::Stage3 => "Approval review",
            PlanStage::Ready => "Approval review",
        }
    }
}
