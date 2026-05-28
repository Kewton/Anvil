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
