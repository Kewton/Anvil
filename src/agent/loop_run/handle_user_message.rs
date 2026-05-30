//! Per-turn entry point extracted from `turn.rs` (parent #680).
//!
//! Hosts `handle_user_message` (pub(super)) — the entry point invoked
//! from `commands.rs` for every user input. Resets the ~30 per-turn
//! state fields (CLAUDE.md per-turn rule), increments
//! `current_turn_index`, hands off to `Agent::run_turn`, then runs the
//! post-turn ledger refresh + per-turn job report emit chokepoint.
//!
//! Originally an `impl Agent` method; converted to a free function
//! taking `&mut Agent`, matching `actor_loop_flow` / `reply_retry` /
//! earlier vertical-slice patterns. `pub(super)` limited / no facade
//! re-export (DR3-001).

use super::Agent;
use super::interrupt::{InterruptEnv, InterruptMonitor};
use super::photon_feedback_derive::build_rerun_prompt_hint_if_eligible;
use super::summary::LoopResult;
use crate::session::store::ConversationMessage;

pub(super) fn handle_user_message(
    agent: &mut Agent,
    input: &str,
    stream_output: bool,
) -> LoopResult {
    // Start the ESC interrupt monitor for the duration of this turn only —
    // rustyline owns raw mode during the REPL line-edit, so the monitor
    // must live strictly inside `handle_user_message`. Drop at function
    // exit disables raw mode deterministically (AC-2 / AC-3 / R1 / R2).
    let env = InterruptEnv::detect();
    let mut monitor = InterruptMonitor::start(&env);
    // Issue #452: Reminder Sidecar per-turn cap counter. "Turn" is one
    // user message — reset here so a fresh handle_user_message can fire
    // the Reminder once even if the previous turn already did.
    agent.reminder_called_this_turn = false;
    // Issue #646 (C1): per-turn ownership reset. Each user turn starts a
    // fresh task — prior-turn edits do NOT auto-confer ownership on the
    // new task. Clearing here also wipes the MissingVerifierJob so
    // verifier retry budgets restart per task, and the
    // `turn_pre_tool_file_hashes` capture cache so stale baselines from
    // a prior turn cannot mask the next turn's first write.
    agent.turn_edited_relative_paths.clear();
    agent.turn_pre_tool_file_hashes.clear();
    agent.missing_verifier_job = None;
    // Issue #654: per-turn dedup marker reset (DR1-006 / DR2-005). The
    // `agent.safe_stop.report` event is emitted at most once per
    // StopReason per turn; clearing here lets a new user turn re-emit
    // the same StopReason if the stop condition recurs.
    agent.safe_stop_report_emitted.clear();
    // Issue #660 (Phase C / DD-4 / DR1-007): per-turn diff-based dedup
    // state for the `agent.active_job.selected` event. Reset adjacent to
    // `safe_stop_report_emitted.clear()` so all per-turn dedup state
    // restarts together at turn boundary (locality eases review when
    // adding new per-turn caps). Forces the first selection of the new
    // turn to emit (None → Some triggers emit), so each turn starts the
    // observation series fresh.
    agent.last_active_job_selection = None;
    // Issue #666: per-turn fire-once dedup for the four new
    // `agent.{artifact_completion,verification,repair,memory}.report`
    // events. Reset adjacent to `safe_stop_report_emitted.clear()` and
    // `last_active_job_selection = None` so all per-turn dedup state
    // restarts together at turn boundary (locality, CLAUDE.md per-turn
    // rule). NOT serialized.
    agent.job_report_dedup_keys.clear();
    // Issue #665 (Phase 6 / S5-006 / S7-002): per-turn diff-based dedup
    // state for `agent.behavior_contract.projected` event. Reset adjacent
    // to `last_active_job_selection = None` so all per-turn dedup state
    // restarts together at turn boundary. Forces the first consumed
    // projection in the new turn to emit.
    agent.last_behavior_contract_projection_event = None;
    // Issue #667 (DR1-004 / per-turn rule): clear the PAM advisory
    // decision carrier. `is_some()` is the "decided this turn" predicate;
    // the 2 production chokepoints set this exactly once (DR1-005).
    agent.last_pam_decision_this_turn = None;
    // Issue #661 Task 2.6 (DR1-004 / DR1-010): per-turn dedup state for
    // `agent.verifier.invoked` (digest of canonical-JSON payload) and
    // per-turn cap for `agent.verifier.external_import_rejected`. Reset
    // adjacent to `last_active_job_selection = None` so the per-turn
    // reset group stays co-located. Producers land in iteration-3.
    agent.last_verifier_invoked_payload_digest = None;
    agent.external_import_rejected_emitted_this_turn = false;
    // Issue #459: Tester Skill per-turn cap counter (DR1-004). Mirror of
    // the reminder cap above; reset so a fresh user turn can fire the
    // Tester once even if the previous turn already did.
    agent.tester_called_this_turn = false;
    // Issue #652 CB-002: per-turn `ArtifactCompletionJob` exhaustion flag.
    // Reset here so a fresh turn can re-arm the budget once the previous
    // turn's job was either completed or exhausted.
    agent.artifact_completion_exhausted_this_turn = false;
    // Issue #652 CB2-003: turn-local dedup flag for
    // `maybe_emit_artifact_completion_failed_diagnostic`. The previous
    // dedup looked at `working_memory.unresolved_errors`, which is
    // *session* state that handle_user_message does NOT clear — so a
    // residual `artifact_completion_failed role=<role>` from the prior
    // turn suppressed the very first emission of the current turn.
    // Resetting a turn-local bool here is the CLAUDE.md per-turn cap
    // pattern; combined with `artifact_completion_exhausted_this_turn`
    // it gives a within-turn single-emit guarantee that does NOT bleed
    // across turn boundaries.
    agent.artifact_completion_failed_diagnostic_emitted_this_turn = false;
    // Issue #456: AnvilScore compute happens once per turn, post-loop.
    // The flag flips after the compute so the post-loop Reminder hook
    // sees `CurrentTurn` while the iteration-internal hook sees
    // `PreviousTurn`.
    agent.anvil_score_computed_this_turn = false;
    // Issue #473: increment monotonic per-session turn counter so the
    // dataset export can join `agent.reminder.completed` with
    // `agent.anvil_score.computed` events by `(session_id, turn_index)`.
    // Saturating add defends against pathological session lengths.
    //
    // Issue #659 PR-002 (Option B): perform the increment **before** the
    // per-turn ArtifactLedger reset so the upcoming turn index is the
    // post-increment value. Decoupling the increment from the stamp
    // timing prevents an off-by-one where `event_recorded` /
    // `turn_summary` carry `N-1` while every other observability event
    // emitted during the same user turn carries `N`.
    agent.current_turn_index = agent.current_turn_index.saturating_add(1);
    // Issue #659 (Task 2.2 / PR-002): per-turn ArtifactLedger reset.
    // Lives at the same per-turn boundary as
    // `turn_edited_relative_paths.clear()` / `turn_pre_tool_file_hashes.clear()`
    // above so all artifact-observation state restarts together on a
    // fresh user turn (CLAUDE.md per-turn rule). The upcoming turn
    // index is passed explicitly so the ledger log context stamp
    // shares `(session_id, turn_index)` join keys with sibling
    // observability events.
    let upcoming_turn_index = u32::try_from(agent.current_turn_index).unwrap_or(u32::MAX);
    super::artifact_ledger_state::clear_per_turn_ledger_state_for_turn(agent, upcoming_turn_index);
    // Issue #556: clear per-turn photon context_pack response.
    agent.photon_context_pack_response = None;
    // Issue #558: clear context_pack_id (turn boundary).
    agent.last_context_pack_id = None;
    // Live injection: reset adopted item count.
    agent.last_photon_adopted_items = 0;
    // Issue #601: reset per-turn counters consumed by Case F no-progress
    // detection. Reset HERE (handle_user_message head) — NOT in
    // `run_actor_loop` head — because the `#[serde(skip, default)]`
    // counters need to be cleared even for entry points that bypass the
    // actor loop (Plan-mode turns skip `invoke_photon_evaluate`, but a
    // subsequent Act turn must still see a clean baseline). Populated at
    // `run_actor_loop` tail; see §5.5 of design v2.
    agent.session.iter_count_this_turn = 0;
    agent.session.tool_calls_this_turn = 0;
    // Issue #591 (AS-01 / 設計判断 #2): reset the per-turn adopted summary
    // ids HERE — NOT at `run_actor_loop` head. `invoke_photon_evaluate`
    // runs post-loop and reads this field; resetting at the loop entry
    // would wipe the ids before the evaluate hook can consume them.
    agent.last_adopted_summary_ids.clear();
    // Issue #594: clear per-turn provenance summary cache. NOTE we do NOT
    // reset `last_photon_context_pack_status` here — `/photon-why` must
    // remain able to surface the previous Act turn's lineage even after a
    // `/plan` mode change (S7-002).
    agent.last_injected_seed_provenance.clear();
    // LI-2: reset the one-shot flag here (before invoke_photon_context_pack
    // in run_turn) so the flag set by path (a) is still true when path (b)
    // in build_request_messages runs. Previously this reset lived in
    // run_actor_loop which wiped it before path (b) could check it.
    agent.session.context_pack_sent_this_turn = false;
    // Issue #608 Phase α-2 (AP-09): detect rerun-trigger keyword in the
    // user message and re-present the previous turn's verifier command
    // as a model prompt hint. The runnable eligibility guard
    // (`runnable_rerun_hint_for_session`) re-validates the persisted
    // command against the same DR4-002 / DR4-003 invariants that gated
    // its original observation (BuildTest class, no shell control,
    // non-empty / no control chars / no 4096-byte cap hit). Tampered or
    // ineligible commands are silently skipped — the prompt hint is
    // never emitted as a direct Bash dispatch (DR4-001).
    if let Some(hint) = build_rerun_prompt_hint_if_eligible(input, &agent.session) {
        agent
            .session
            .messages
            .push(ConversationMessage::system(hint));
    }
    let result = super::run_turn::run_turn(agent, input, stream_output, &mut monitor);
    // Issue #663 (Phase B / AD2 / DR1-001): refresh the active
    // `ArtifactCompletionJob` Satisfied state from the ledger
    // projection. This is the SSOT chokepoint — no status guard
    // here, the guard lives inside
    // `ArtifactCompletionJob::record_satisfied_from_ledger`
    // (DR1-001 SSOT集約). Runs BEFORE `maybe_emit_job_reports` so
    // the job state emitted in `ArtifactCompletionReport` reflects
    // ledger-driven Satisfied transitions for the turn.
    agent.refresh_artifact_completion_satisfied();
    // Issue #666: emit per-turn structured job reports just before
    // returning. Wrapping the result guarantees emit fires once per
    // turn regardless of how `run_turn` exited (Ok / Err / early
    // return inside the loop). Order: 4 Report → SafeStopReport is
    // preserved because `record_safe_stop_report` has already run by
    // the time `run_turn` returns, so the SafeStopLinkage snapshot
    // here reflects the final state of the turn (DR3-003).
    agent.maybe_emit_job_reports();
    result
}
