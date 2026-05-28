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

use std::collections::HashMap;

use crate::agent::orchestration::{RepoSnapshot, RepoVerification};
use crate::agent::recovery;
use crate::ollama::client::AssistantReply;
use crate::ollama::xml_fallback::ToolCall;

use super::Agent;
use super::active_job_arbiter::{LoopControlAction, RecoveryDispatchGate, RecoveryOwner};
use super::interrupt::InterruptFlag;
use super::summary::ExitReason;
use super::tool_history::focused_edit_target_already_read;
use super::tool_policy::EffectiveToolPolicy;

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
        && super::turn::answer_only_reply_is_inadequate(args.final_reply)
    {
        *args.no_tool_retries += 1;
        if *args.no_tool_retries >= 2 {
            agent
                .session
                .record_feedback_if_unset(super::turn::build_feedback_for_no_tool_call(
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
            &super::turn::format_iteration_status(
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
    if !(super::turn::should_apply_repo_change_quality_gate(
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
                &super::turn::format_iteration_status(
                    args.last_iter,
                    agent.config.max_iterations,
                    "Quality fallback",
                    &format!("Replaced scaffold placeholder output in {target_path}."),
                    agent.footer.current_cols(),
                ),
                true,
            );
            agent.session.record_feedback_if_unset(
                super::turn::build_feedback_for_deterministic_content_fallback(&agent.work_root),
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
        &super::turn::format_iteration_status(
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
    if !super::turn::should_apply_repo_change_partial_progress_recovery(
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
        &super::turn::format_iteration_status(
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
        &super::turn::format_iteration_status(
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
        &super::turn::format_iteration_status(
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
    if !super::turn::should_try_framework_app_fallback(
        args.last_iter,
        *args.framework_app_fallback_materialized,
    ) || !agent.maybe_materialize_framework_game_fallback(args.last_iter)
    {
        return false;
    }
    *args.framework_app_fallback_materialized = true;
    agent.push_system_note(super::turn::framework_app_fallback_continuation_note().to_string());
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
        let frame = super::turn::build_feedback_for_tool_protocol_failure(&err, &agent.work_root);
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
    let flow_args = super::turn::TaskContractVerifierFlowArgs {
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
            agent.drive_task_contract_verifier(flow_args),
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
    ) || !super::turn::should_try_framework_app_fallback(
        args.last_iter,
        *args.framework_app_fallback_materialized,
    ) || !agent.maybe_materialize_framework_game_fallback(args.last_iter)
    {
        return false;
    }
    *args.framework_app_fallback_materialized = true;
    agent.push_system_note(super::turn::framework_app_fallback_continuation_note().to_string());
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
    match agent.handle_actor_loop_post_tool_polish_fallback(
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
    if super::turn::unrestricted_policy_retry_exhausted(
        focused_retry.is_some(),
        *args.focused_policy_retries,
    ) {
        return ActorLoopToolPreparationOutcome::Exit {
            reason: ExitReason::ToolCallFormatError,
            error_text: "assistant kept calling tools outside the current tool policy".to_string(),
        };
    }
    let retry_status_note =
        super::turn::rejected_tool_batch_retry_status_note(focused_retry.is_some());
    super::turn::write_stdout_rendered(
        &super::turn::format_iteration_status(
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
    let artifact_attempt = super::turn::increment_artifact_completion_role_attempt(
        contract_completion_role_retries,
        role,
    );
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
        &super::turn::format_iteration_status(
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
    if !super::turn::focused_policy_retry_exhausted(focused_retry_present, focused_policy_retries) {
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
        && super::turn::reply_looks_like_future_work(args.final_reply)
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
            &super::turn::format_iteration_status(
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
