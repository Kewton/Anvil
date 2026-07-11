pub(crate) const READ_ONLY_STAGNATION_INTERVENTION_THRESHOLD: usize = 3;
pub(crate) const READ_ONLY_STAGNATION_COMPACT_THRESHOLD: usize = 5;
pub(crate) const READ_ONLY_STAGNATION_WRITE_REQUIRED_THRESHOLD: usize = 7;
pub(crate) const WRITE_REQUIRED_NO_WRITE_LIMIT: usize = 2;
pub(crate) const NO_PROGRESS_FEEDBACK_LIMIT: usize = 3;
pub(crate) const READ_ONLY_STAGNATION_REASON: &str = "model_stagnation:read_only_loop";
pub(crate) const NO_PROGRESS_STAGNATION_REASON: &str = "model_stagnation:no_progress_recorded";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PressureLevel {
    Normal,
    Intervention,
    CompactRestatement,
    WriteRequired,
    FullFileWrite,
    Exhausted,
}

impl Default for PressureLevel {
    fn default() -> Self {
        Self::Normal
    }
}

impl PressureLevel {
    #[allow(dead_code)]
    pub(crate) fn feedback_stage(self) -> Option<&'static str> {
        match self {
            Self::Intervention => Some("intervention"),
            Self::CompactRestatement => Some("compact_restatement"),
            Self::WriteRequired => Some("write_required"),
            Self::Normal | Self::FullFileWrite | Self::Exhausted => None,
        }
    }
}

impl PressureState {
    pub(crate) fn read_only_streak(&self) -> usize {
        self.counters.read_only_streak
    }

    pub(crate) fn no_progress_feedback_available(&self, limit: usize) -> bool {
        self.counters.no_progress_streak < limit
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PressureTerminalReason {
    ReadOnlyLoop,
    NoProgressRecorded,
    WriteRequiredExhausted,
}

impl PressureTerminalReason {
    #[allow(dead_code)]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnlyLoop | Self::WriteRequiredExhausted => READ_ONLY_STAGNATION_REASON,
            Self::NoProgressRecorded => NO_PROGRESS_STAGNATION_REASON,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct CarriedPressure {
    pub(crate) read_only_streak: usize,
    pub(crate) anchor_failures: usize,
    pub(crate) write_required_exhausted: bool,
    pub(crate) anchor_target: Option<String>,
    pub(crate) pending_evidence: Vec<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct PressureInputs {
    pub(crate) read_only_streak: usize,
    pub(crate) no_progress_streak: usize,
    pub(crate) anchor_failures: usize,
    pub(crate) remaining_budget: usize,
    pub(crate) carried: Option<CarriedPressure>,
    pub(crate) missing_evidence_present: bool,
    pub(crate) missing_paths_present: bool,
    pub(crate) blocking_reason_present: bool,
    pub(crate) provider_error_present: bool,
    pub(crate) write_required_active: bool,
    pub(crate) write_required_no_write_attempts: usize,
    pub(crate) anchor_target: Option<String>,
    pub(crate) selected_targets: Vec<String>,
    pub(crate) selection_reason: Option<String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PressureCounters {
    pub(crate) read_only_streak: usize,
    pub(crate) no_progress_streak: usize,
    pub(crate) anchor_failures: usize,
    pub(crate) write_required_no_write_attempts: usize,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PressureSeed {
    pub(crate) initial_read_only_streak: usize,
    pub(crate) carried_streak: usize,
    pub(crate) carried_anchor_failures: usize,
    pub(crate) pre_advanced: bool,
    pub(crate) carried_write_required_exhausted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PressureState {
    pub(crate) level: PressureLevel,
    pub(crate) feedback_level: Option<PressureLevel>,
    pub(crate) counters: PressureCounters,
    pub(crate) effective_read_only_streak: usize,
    pub(crate) remaining_budget: usize,
    pub(crate) seed: PressureSeed,
    pub(crate) missing_evidence_present: bool,
    pub(crate) selected_targets: Vec<String>,
    pub(crate) selection_reason: Option<String>,
    pub(crate) terminal_reason: Option<PressureTerminalReason>,
}

impl Default for PressureState {
    fn default() -> Self {
        transition(PressureInputs::default())
    }
}

pub(crate) fn pressure_seed(
    carried: Option<&CarriedPressure>,
    remaining_budget: usize,
) -> PressureSeed {
    let Some(carried) = carried else {
        return PressureSeed::default();
    };
    let distance = READ_ONLY_STAGNATION_WRITE_REQUIRED_THRESHOLD.saturating_sub(
        carried
            .read_only_streak
            .saturating_add(carried.anchor_failures),
    );
    let pre_advanced =
        carried.write_required_exhausted || (remaining_budget > 0 && remaining_budget < distance);
    PressureSeed {
        initial_read_only_streak: if pre_advanced {
            READ_ONLY_STAGNATION_WRITE_REQUIRED_THRESHOLD.saturating_sub(1)
        } else {
            carried.read_only_streak
        },
        carried_streak: carried.read_only_streak,
        carried_anchor_failures: carried.anchor_failures,
        pre_advanced,
        carried_write_required_exhausted: carried.write_required_exhausted,
    }
}

pub(crate) fn transition(inputs: PressureInputs) -> PressureState {
    let seed = pressure_seed(inputs.carried.as_ref(), inputs.remaining_budget);
    let read_only_streak = if inputs.carried.is_some() && inputs.read_only_streak == 0 {
        seed.initial_read_only_streak
    } else {
        inputs.read_only_streak
    };
    let anchor_failures = inputs.anchor_failures.max(seed.carried_anchor_failures);
    let effective_read_only_streak = read_only_streak.saturating_add(anchor_failures);
    let counters = PressureCounters {
        read_only_streak,
        no_progress_streak: inputs.no_progress_streak,
        anchor_failures,
        write_required_no_write_attempts: inputs.write_required_no_write_attempts,
    };
    let missing_evidence_present = inputs.missing_evidence_present
        || inputs
            .carried
            .as_ref()
            .is_some_and(|carried| !carried.pending_evidence.is_empty());

    if inputs.write_required_active
        && inputs.write_required_no_write_attempts >= WRITE_REQUIRED_NO_WRITE_LIMIT
    {
        return PressureState {
            level: PressureLevel::Exhausted,
            feedback_level: Some(PressureLevel::WriteRequired),
            counters,
            effective_read_only_streak,
            remaining_budget: inputs.remaining_budget,
            seed,
            missing_evidence_present,
            selected_targets: inputs.selected_targets,
            selection_reason: inputs.selection_reason,
            terminal_reason: Some(PressureTerminalReason::WriteRequiredExhausted),
        };
    }

    let feedback_level = if inputs.write_required_active {
        Some(PressureLevel::WriteRequired)
    } else {
        read_only_feedback_level(
            read_only_streak,
            effective_read_only_streak,
            anchor_failures,
        )
    };
    let anchor_interlocked = anchor_failures > 0 && feedback_level.is_some();
    let level = match feedback_level {
        Some(PressureLevel::Intervention | PressureLevel::CompactRestatement)
            if anchor_interlocked =>
        {
            PressureLevel::FullFileWrite
        }
        Some(level) => level,
        None => PressureLevel::Normal,
    };
    let anchor_target = inputs.anchor_target.or_else(|| {
        inputs
            .carried
            .as_ref()
            .and_then(|carried| carried.anchor_target.clone())
    });
    let (selected_targets, selection_reason) = if anchor_interlocked {
        anchor_target
            .map(|target| (vec![target], Some("anchor_failure".to_string())))
            .unwrap_or((inputs.selected_targets, inputs.selection_reason))
    } else {
        (inputs.selected_targets, inputs.selection_reason)
    };

    // NOTE: The current no-progress path does not raise write pressure. See
    // docs/integration-notes.md; this table intentionally preserves that gap.
    PressureState {
        level,
        feedback_level,
        counters,
        effective_read_only_streak,
        remaining_budget: inputs.remaining_budget,
        seed,
        missing_evidence_present,
        selected_targets,
        selection_reason,
        terminal_reason: None,
    }
}

pub(crate) fn exhaustion_reason(inputs: &PressureInputs) -> Option<PressureTerminalReason> {
    if inputs.missing_paths_present || inputs.missing_evidence_present {
        return None;
    }
    if inputs.read_only_streak >= READ_ONLY_STAGNATION_INTERVENTION_THRESHOLD {
        return Some(PressureTerminalReason::ReadOnlyLoop);
    }
    if !inputs.blocking_reason_present && !inputs.provider_error_present {
        return Some(PressureTerminalReason::NoProgressRecorded);
    }
    None
}

fn read_only_feedback_level(
    read_only_streak: usize,
    effective_read_only_streak: usize,
    anchor_failures: usize,
) -> Option<PressureLevel> {
    if anchor_failures > 0 {
        if effective_read_only_streak >= READ_ONLY_STAGNATION_WRITE_REQUIRED_THRESHOLD {
            return Some(PressureLevel::WriteRequired);
        }
        return exact_feedback_level(effective_read_only_streak)
            .or_else(|| exact_feedback_level(read_only_streak));
    }
    exact_feedback_level(read_only_streak)
}

fn exact_feedback_level(streak: usize) -> Option<PressureLevel> {
    match streak {
        READ_ONLY_STAGNATION_INTERVENTION_THRESHOLD => Some(PressureLevel::Intervention),
        READ_ONLY_STAGNATION_COMPACT_THRESHOLD => Some(PressureLevel::CompactRestatement),
        READ_ONLY_STAGNATION_WRITE_REQUIRED_THRESHOLD => Some(PressureLevel::WriteRequired),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(read_only_streak: usize) -> PressureInputs {
        PressureInputs {
            read_only_streak,
            remaining_budget: 8,
            ..PressureInputs::default()
        }
    }

    #[test]
    fn read_only_transition_table_preserves_exact_thresholds() {
        let cases = [
            (0, PressureLevel::Normal),
            (2, PressureLevel::Normal),
            (3, PressureLevel::Intervention),
            (4, PressureLevel::Normal),
            (5, PressureLevel::CompactRestatement),
            (6, PressureLevel::Normal),
            (7, PressureLevel::WriteRequired),
            (8, PressureLevel::Normal),
        ];
        for (streak, expected) in cases {
            assert_eq!(
                transition(inputs(streak)).level,
                expected,
                "streak={streak}"
            );
        }
    }

    #[test]
    fn anchor_failures_advance_existing_rungs_without_changing_event_stage() {
        let cases = [
            (
                2,
                1,
                PressureLevel::FullFileWrite,
                PressureLevel::Intervention,
            ),
            (
                4,
                1,
                PressureLevel::FullFileWrite,
                PressureLevel::CompactRestatement,
            ),
            (
                3,
                1,
                PressureLevel::FullFileWrite,
                PressureLevel::Intervention,
            ),
            (
                6,
                1,
                PressureLevel::WriteRequired,
                PressureLevel::WriteRequired,
            ),
        ];
        for (streak, failures, expected, feedback) in cases {
            let state = transition(PressureInputs {
                anchor_failures: failures,
                anchor_target: Some("src/app/page.tsx".to_string()),
                ..inputs(streak)
            });
            assert_eq!(state.level, expected, "streak={streak}");
            assert_eq!(state.feedback_level, Some(feedback), "streak={streak}");
            assert_eq!(state.selected_targets, ["src/app/page.tsx"]);
            assert_eq!(state.selection_reason.as_deref(), Some("anchor_failure"));
        }
    }

    #[test]
    fn carryover_preserves_long_budget_and_preadvances_short_budget() {
        let carried = CarriedPressure {
            read_only_streak: 2,
            ..CarriedPressure::default()
        };
        let long = pressure_seed(Some(&carried), 8);
        assert_eq!(long.initial_read_only_streak, 2);
        assert!(!long.pre_advanced);

        let short = pressure_seed(Some(&CarriedPressure::default()), 2);
        assert_eq!(short.initial_read_only_streak, 6);
        assert!(short.pre_advanced);
    }

    #[test]
    fn carried_anchor_failures_and_exhaustion_preserve_preadvance_rules() {
        let anchored = pressure_seed(
            Some(&CarriedPressure {
                anchor_failures: 2,
                ..CarriedPressure::default()
            }),
            4,
        );
        assert!(anchored.pre_advanced);
        assert_eq!(anchored.carried_anchor_failures, 2);

        let exhausted = pressure_seed(
            Some(&CarriedPressure {
                write_required_exhausted: true,
                ..CarriedPressure::default()
            }),
            12,
        );
        assert!(exhausted.pre_advanced);
        assert!(exhausted.carried_write_required_exhausted);
        assert_eq!(exhausted.initial_read_only_streak, 6);
    }

    #[test]
    fn short_budget_seed_reaches_write_required_after_next_read_only_turn() {
        let seeded = transition(PressureInputs {
            remaining_budget: 2,
            carried: Some(CarriedPressure::default()),
            ..PressureInputs::default()
        });
        assert_eq!(seeded.counters.read_only_streak, 6);
        assert_eq!(seeded.level, PressureLevel::Normal);

        let next = transition(inputs(seeded.counters.read_only_streak + 1));
        assert_eq!(next.level, PressureLevel::WriteRequired);
    }

    #[test]
    fn active_write_required_exhausts_on_second_rejected_read_only_tool() {
        let active = transition(PressureInputs {
            write_required_active: true,
            write_required_no_write_attempts: 1,
            selected_targets: vec!["target.txt".to_string()],
            selection_reason: Some("required_path".to_string()),
            ..inputs(7)
        });
        assert_eq!(active.level, PressureLevel::WriteRequired);

        let exhausted = transition(PressureInputs {
            write_required_active: true,
            write_required_no_write_attempts: WRITE_REQUIRED_NO_WRITE_LIMIT,
            selected_targets: vec!["target.txt".to_string()],
            selection_reason: Some("required_path".to_string()),
            ..inputs(7)
        });
        assert_eq!(exhausted.level, PressureLevel::Exhausted);
        assert_eq!(
            exhausted.terminal_reason,
            Some(PressureTerminalReason::WriteRequiredExhausted)
        );
        assert_eq!(
            exhausted.terminal_reason.unwrap().as_str(),
            "model_stagnation:read_only_loop"
        );
    }

    #[test]
    fn no_progress_is_recorded_but_does_not_raise_write_pressure() {
        let state = transition(PressureInputs {
            no_progress_streak: 4,
            remaining_budget: 0,
            ..PressureInputs::default()
        });
        assert_eq!(state.level, PressureLevel::Normal);
        assert_eq!(state.counters.no_progress_streak, 4);
        assert_eq!(state.terminal_reason, None);

        let terminal = exhaustion_reason(&PressureInputs {
            no_progress_streak: 4,
            ..PressureInputs::default()
        });
        assert_eq!(terminal, Some(PressureTerminalReason::NoProgressRecorded));
        assert_eq!(
            terminal.unwrap().as_str(),
            "model_stagnation:no_progress_recorded"
        );
    }

    #[test]
    fn exhaustion_priority_preserves_read_only_and_evidence_blockers() {
        assert_eq!(
            exhaustion_reason(&inputs(3)),
            Some(PressureTerminalReason::ReadOnlyLoop)
        );
        assert_eq!(
            exhaustion_reason(&PressureInputs {
                read_only_streak: 3,
                missing_evidence_present: true,
                ..PressureInputs::default()
            }),
            None
        );
        assert_eq!(
            exhaustion_reason(&PressureInputs {
                blocking_reason_present: true,
                ..PressureInputs::default()
            }),
            None
        );
        assert_eq!(
            exhaustion_reason(&PressureInputs {
                missing_paths_present: true,
                ..PressureInputs::default()
            }),
            None
        );
    }
}

#[cfg(test)]
mod anchor_tests {
    use super::super::anchor_stagnation_interlock::*;
    use super::super::edit_anchor_recovery::EditAnchorFailureSummary;
    use super::super::stagnation_escalation::{
        ReadOnlyStagnationStage, WriteRequiredSelectionReason,
    };
    use super::*;
    use crate::config::{
        Action, Config, ConfigFieldSources, NarrationMode, PlanPreset, PromptLayout, Provider,
    };
    use crate::minimal_loop::loop_run::{
        RunSessionOptions, RunSessionStepKind, run_session_with_outcome_with_options,
    };
    use crate::providers::{AssistantReply, ChatClient};
    use crate::state::{ConversationMessage, SessionSnapshot, ToolCall};
    use crate::tools::registry::ToolSpec;
    use crate::tui::NOOP_UI;
    use serde_json::{Value, json};
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct RecordingFake {
        replies: Arc<Mutex<Vec<anyhow::Result<AssistantReply>>>>,
        requests: Arc<Mutex<Vec<Vec<ConversationMessage>>>>,
    }

    impl RecordingFake {
        fn new(replies: Vec<anyhow::Result<AssistantReply>>) -> Self {
            Self {
                replies: Arc::new(Mutex::new(replies)),
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn requests(&self) -> Vec<Vec<ConversationMessage>> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl ChatClient for RecordingFake {
        fn label(&self) -> &str {
            "fake"
        }

        fn boxed_clone(&self) -> Box<dyn ChatClient> {
            Box::new(self.clone())
        }

        fn chat(
            &mut self,
            _model: &str,
            messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _native_tools_enabled: bool,
        ) -> anyhow::Result<AssistantReply> {
            self.requests.lock().unwrap().push(messages.to_vec());
            self.replies.lock().unwrap().remove(0)
        }
    }

    fn read_reply(path: &str) -> AssistantReply {
        AssistantReply {
            content: String::new(),
            tool_calls: vec![ToolCall::new("Read", json!({"path": path}))],
            prompt_tokens: None,
            completion_tokens: None,
        }
    }

    fn config(root: std::path::PathBuf) -> Config {
        Config {
            workspace_root: root,
            state_dir: std::path::PathBuf::from("state"),
            yes: true,
            offline: false,
            context_budget: 1000,
            model: "m".to_string(),
            provider: Provider::Ollama,
            prompt_layout: PromptLayout::Stable,
            plan_preset: PlanPreset::None,
            planner_model: "m".to_string(),
            planner_provider: Provider::Ollama,
            ollama_host: "http://localhost:11434".to_string(),
            num_predict: 100,
            max_iterations: 6,
            chat_timeout_secs: 1,
            chat_timeout_source: "override:test".to_string(),
            field_sources: ConfigFieldSources::default(),
            chat_retries: 1,
            eval_events_path: None,
            completion_contract_path: None,
            resume: None,
            fresh_session: false,
            no_footer: false,
            narration: NarrationMode::Normal,
            profile: "generic".to_string(),
            profile_explicit: false,
            profile_inference: None,
            style: "default".to_string(),
            action: Action::Repl,
        }
    }

    fn event_values(path: &Path) -> Vec<Value> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect()
    }

    #[test]
    fn no_anchor_failure_keeps_legacy_stagnation_path() {
        let decision = read_only_stagnation_decision(
            super::super::stagnation_escalation::READ_ONLY_STAGNATION_INTERVENTION_THRESHOLD,
            None,
        )
        .unwrap();

        assert_eq!(decision.stage, ReadOnlyStagnationStage::Intervention);
        assert!(!decision.anchor_interlocked());
        assert!(decision.full_file_write_feedback("fix").is_none());
    }

    #[test]
    fn anchor_failure_replaces_first_stagnation_feedback_with_full_file_write() {
        let decision = read_only_stagnation_decision(
            super::super::stagnation_escalation::READ_ONLY_STAGNATION_INTERVENTION_THRESHOLD,
            Some(EditAnchorFailureSummary {
                path: "src/app/page.tsx".to_string(),
                failure_count: 1,
            }),
        )
        .unwrap();

        assert_eq!(decision.stage, ReadOnlyStagnationStage::Intervention);
        let feedback = decision.full_file_write_feedback("fix the page").unwrap();
        assert!(feedback.contains("full-file Write now"), "{feedback}");
        assert!(feedback.contains("src/app/page.tsx"), "{feedback}");
        assert!(!feedback.contains("Inspection is sufficient"), "{feedback}");
    }

    #[test]
    fn anchor_failures_advance_write_required_threshold() {
        let decision = read_only_stagnation_decision(
            READ_ONLY_STAGNATION_WRITE_REQUIRED_THRESHOLD - 1,
            Some(EditAnchorFailureSummary {
                path: "src/app/page.tsx".to_string(),
                failure_count: 1,
            }),
        )
        .unwrap();

        assert_eq!(decision.stage, ReadOnlyStagnationStage::WriteRequired);
        let selection = decision.write_required_selection().unwrap();
        assert_eq!(selection.selected_targets, vec!["src/app/page.tsx"]);
        assert_eq!(
            selection.selection_reason,
            WriteRequiredSelectionReason::AnchorFailure
        );
        assert!(
            read_only_stagnation_decision(READ_ONLY_STAGNATION_WRITE_REQUIRED_THRESHOLD - 1, None)
                .is_none()
        );
    }

    #[test]
    fn interlock_event_records_anchor_failures_and_streak() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let decision = read_only_stagnation_decision(
            3,
            Some(EditAnchorFailureSummary {
                path: "target.txt".to_string(),
                failure_count: 2,
            }),
        )
        .unwrap();

        emit_interlock_event(
            Some(&events),
            &decision,
            "plan-run-step",
            "implement",
            Some("final_acceptance_repair"),
        );

        let events = event_values(&events);
        assert_eq!(events[0]["event"], "anchor_stagnation_interlock");
        assert_eq!(events[0]["anchor_failures"], 2);
        assert_eq!(events[0]["streak"], 3);
        assert_eq!(events[0]["path"], "target.txt");
    }

    #[test]
    fn anchor_failure_then_read_only_feedback_uses_full_file_write_guidance() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("target.txt"), "old\n").unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        let mut fake = RecordingFake::new(vec![
            Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Edit",
                    json!({"path":"target.txt","old_string":"missing anchor","new_string":"new\n"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            }),
            Ok(read_reply("target.txt")),
            Ok(read_reply("target.txt")),
            Ok(read_reply("target.txt")),
            Ok(AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"target.txt","content":"new\n"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            }),
            Ok(AssistantReply::text("done")),
        ]);
        let mut session = SessionSnapshot::new();

        run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Implement the requested change in target.txt.",
            &[],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Implement),
        )
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.path().join("target.txt")).unwrap(),
            "new\n"
        );
        assert!(fake.requests().iter().any(|request| {
            request.iter().any(|message| {
                message.content.contains("full-file Write now")
                    && message.content.contains("target.txt")
                    && !message.content.contains("Inspection is sufficient")
            })
        }));
        let events = event_values(&events);
        assert!(events.iter().any(|event| {
            event.get("event").and_then(Value::as_str) == Some("anchor_stagnation_interlock")
                && event.get("anchor_failures").and_then(Value::as_u64) == Some(1)
                && event.get("streak").and_then(Value::as_u64) == Some(3)
        }));
    }
}
