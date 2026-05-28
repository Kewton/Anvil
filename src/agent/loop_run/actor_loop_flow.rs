//! Issue #681 (parent #680, Phase 1): actor loop control-flow data types
//! extracted from `turn.rs`.
//!
//! Hosts `PostReplyRecovery*` / `ActorLoop*Args` / `ActorLoop*Outcome`
//! struct/enum definitions consumed by the actor loop sub-flows
//! (pre-reply / tool-preparation / no-tool / task-contract-reply /
//! completion / post-tool / missing-repo-change / empty-reply /
//! prose-only / plan-tool-followup).
//!
//! DR3-001: `pub(super)` limited. `loop_run.rs` MUST NOT re-export via
//! `pub use`. `turn.rs` is the only in-crate consumer.

use std::collections::HashMap;

use crate::agent::orchestration::{RepoSnapshot, RepoVerification};
use crate::agent::recovery;
use crate::ollama::client::AssistantReply;
use crate::ollama::xml_fallback::ToolCall;

use super::active_job_arbiter::{LoopControlAction, RecoveryDispatchGate, RecoveryOwner};
use super::interrupt::InterruptFlag;
use super::summary::ExitReason;
use super::tool_policy::EffectiveToolPolicy;

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

pub(super) fn repair_job_done_outcome() -> TaskContractVerifierFlowOutcome {
    TaskContractVerifierFlowOutcome::Done {
        final_prose:
            "Completed requested repository changes and verified them with the required verifier."
                .to_string(),
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
