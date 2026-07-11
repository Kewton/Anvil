use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::config::Config;
use crate::eval_events;
use crate::providers::ChatClient;
use crate::state::SessionSnapshot;
use crate::tui::InteractionUi;

use super::edit_anchor_recovery::{EditAnchorFailureSummary, EditAnchorRecovery};
use super::loop_run::{
    RunSessionOptions, RunSessionOutcome, run_session_with_outcome_with_options,
};
use super::stagnation_escalation::READ_ONLY_STAGNATION_WRITE_REQUIRED_THRESHOLD;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct EscalationCarryoverState {
    read_only_streak: usize,
    anchor_failure: Option<EditAnchorFailureSummary>,
    write_required_exhausted: bool,
    pending_evidence: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct EscalationCarryoverHandle {
    state: Arc<Mutex<EscalationCarryoverState>>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EscalationCarryoverSeed {
    pub(crate) initial_read_only_streak: usize,
    pub(crate) carried_streak: usize,
    pub(crate) carried_anchor_failures: usize,
    pub(crate) pre_advanced: bool,
    pub(crate) carried_write_required_exhausted: bool,
}

impl EscalationCarryoverHandle {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(EscalationCarryoverState::default())),
        }
    }

    pub(crate) fn seed_for_session(&self, repair_turn_budget: usize) -> EscalationCarryoverSeed {
        let state = self.state.lock().unwrap().clone();
        let carried_anchor_failures = state
            .anchor_failure
            .as_ref()
            .map(|failure| failure.failure_count)
            .unwrap_or_default();
        let distance = distance_to_write_required(state.read_only_streak, carried_anchor_failures);
        let pre_advanced = state.write_required_exhausted
            || (repair_turn_budget > 0 && repair_turn_budget < distance);
        EscalationCarryoverSeed {
            initial_read_only_streak: if pre_advanced {
                READ_ONLY_STAGNATION_WRITE_REQUIRED_THRESHOLD.saturating_sub(1)
            } else {
                state.read_only_streak
            },
            carried_streak: state.read_only_streak,
            carried_anchor_failures,
            pre_advanced,
            carried_write_required_exhausted: state.write_required_exhausted,
        }
    }

    pub(crate) fn record_read_only_streak(&self, read_only_streak: usize) {
        self.state.lock().unwrap().read_only_streak = read_only_streak;
    }

    pub(crate) fn record_anchor_failure(&self, failure: EditAnchorFailureSummary) {
        if failure.failure_count == 0 || failure.path.trim().is_empty() {
            return;
        }
        let mut state = self.state.lock().unwrap();
        let replace = state
            .anchor_failure
            .as_ref()
            .map(|current| {
                failure.failure_count > current.failure_count
                    || (failure.failure_count == current.failure_count
                        && failure.path < current.path)
            })
            .unwrap_or(true);
        if replace {
            state.anchor_failure = Some(failure);
        }
    }

    pub(crate) fn note_successful_write_path(&self, path: &str) {
        let mut state = self.state.lock().unwrap();
        state.read_only_streak = 0;
        state.write_required_exhausted = false;
        if state
            .anchor_failure
            .as_ref()
            .is_some_and(|failure| normalized_path(&failure.path) == normalized_path(path))
        {
            state.anchor_failure = None;
        }
    }

    pub(crate) fn note_write_required_exhausted(&self) {
        self.state.lock().unwrap().write_required_exhausted = true;
    }

    pub(crate) fn set_pending_evidence(&self, pending_evidence: &[String]) {
        let mut normalized = Vec::new();
        for evidence in pending_evidence {
            let evidence = evidence.trim();
            if !evidence.is_empty() && !normalized.iter().any(|item| item == evidence) {
                normalized.push(evidence.to_string());
            }
        }
        self.state.lock().unwrap().pending_evidence = normalized;
    }

    pub(crate) fn carry_pending_evidence(
        &self,
        mut pending_evidence: Vec<String>,
        additional_evidence: &[String],
    ) -> Vec<String> {
        for evidence in additional_evidence {
            if !pending_evidence.iter().any(|existing| existing == evidence) {
                pending_evidence.push(evidence.clone());
            }
        }
        self.set_pending_evidence(&pending_evidence);
        pending_evidence
    }

    pub(crate) fn pending_evidence(&self) -> Vec<String> {
        self.state.lock().unwrap().pending_evidence.clone()
    }

    pub(crate) fn strongest_anchor_failure(&self) -> Option<EditAnchorFailureSummary> {
        self.state.lock().unwrap().anchor_failure.clone()
    }
}

pub(crate) fn strongest_anchor_failure(
    local: Option<EditAnchorFailureSummary>,
    carryover: Option<&EscalationCarryoverHandle>,
) -> Option<EditAnchorFailureSummary> {
    let carried = carryover.and_then(EscalationCarryoverHandle::strongest_anchor_failure);
    match (local, carried) {
        (Some(local), Some(carried)) => Some(strongest(local, carried)),
        (Some(local), None) => Some(local),
        (None, Some(carried)) => Some(carried),
        (None, None) => None,
    }
}

pub(crate) fn attach_to_options(
    mut options: RunSessionOptions,
    carryover: EscalationCarryoverHandle,
) -> RunSessionOptions {
    options.escalation_carryover = Some(carryover);
    options
}

pub(crate) fn seed_from_options(
    options: &RunSessionOptions,
    eval_events_path: Option<&Path>,
    repair_turn_budget: usize,
) -> usize {
    seed_read_only_streak(
        options.escalation_carryover.as_ref(),
        eval_events_path,
        repair_turn_budget,
        options.scope.as_str(),
        options
            .step_kind
            .map(super::loop_run::RunSessionStepKind::as_str)
            .unwrap_or(""),
        options.phase_scope.as_deref(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn seed_read_only_streak(
    carryover: Option<&EscalationCarryoverHandle>,
    eval_events_path: Option<&Path>,
    repair_turn_budget: usize,
    session_scope: &str,
    step_kind: &str,
    phase_scope: Option<&str>,
) -> usize {
    let Some(carryover) = carryover else {
        return 0;
    };
    let seed = carryover.seed_for_session(repair_turn_budget);
    emit_escalation_carryover_event(
        eval_events_path,
        seed,
        repair_turn_budget,
        session_scope,
        step_kind,
        phase_scope,
    );
    seed.initial_read_only_streak
}

pub(crate) fn record_streak(carryover: Option<&EscalationCarryoverHandle>, streak: usize) {
    if let Some(carryover) = carryover {
        carryover.record_read_only_streak(streak);
    }
}

pub(crate) fn record_successful_write_path(
    carryover: Option<&EscalationCarryoverHandle>,
    path: &str,
) {
    if let Some(carryover) = carryover {
        carryover.note_successful_write_path(path);
    }
}

pub(crate) fn record_anchor_recovery(
    carryover: Option<&EscalationCarryoverHandle>,
    recovery: &EditAnchorRecovery,
) {
    if let Some(carryover) = carryover {
        carryover.record_anchor_failure(EditAnchorFailureSummary {
            path: recovery.path.clone(),
            failure_count: recovery.failure_count,
        });
    }
}

pub(crate) fn record_write_required_exhaustion(carryover: Option<&EscalationCarryoverHandle>) {
    if let Some(carryover) = carryover {
        carryover.note_write_required_exhausted();
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_escalation_carryover_event(
    eval_events_path: Option<&Path>,
    seed: EscalationCarryoverSeed,
    repair_turn_budget: usize,
    session_scope: &str,
    step_kind: &str,
    phase_scope: Option<&str>,
) {
    if seed.carried_streak == 0
        && seed.carried_anchor_failures == 0
        && !seed.pre_advanced
        && !seed.carried_write_required_exhausted
    {
        return;
    }
    eval_events::emit(
        eval_events_path,
        json!({
            "event": "escalation_carryover",
            "carried_streak": seed.carried_streak,
            "carried_anchor_failures": seed.carried_anchor_failures,
            "carried_write_required_exhausted": seed.carried_write_required_exhausted,
            "pre_advanced": seed.pre_advanced,
            "initial_read_only_streak": seed.initial_read_only_streak,
            "repair_turn_budget": repair_turn_budget,
            "session_scope": session_scope,
            "step_kind": step_kind,
            "phase_scope": phase_scope.unwrap_or(""),
        }),
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run_final_acceptance_repair_with_carryover(
    execution: &mut dyn ChatClient,
    ultra_session: &mut SessionSnapshot,
    repair_prompt: &str,
    expected_paths: &[String],
    config: &Config,
    ui: &dyn InteractionUi,
    carryover: EscalationCarryoverHandle,
) -> anyhow::Result<RunSessionOutcome> {
    run_session_with_outcome_with_options(
        execution,
        ultra_session,
        repair_prompt,
        expected_paths,
        config,
        ui,
        attach_to_options(RunSessionOptions::final_acceptance_repair(), carryover),
    )
}

fn distance_to_write_required(read_only_streak: usize, anchor_failures: usize) -> usize {
    READ_ONLY_STAGNATION_WRITE_REQUIRED_THRESHOLD
        .saturating_sub(read_only_streak.saturating_add(anchor_failures))
}

fn strongest(
    left: EditAnchorFailureSummary,
    right: EditAnchorFailureSummary,
) -> EditAnchorFailureSummary {
    if left.failure_count > right.failure_count
        || (left.failure_count == right.failure_count && left.path <= right.path)
    {
        left
    } else {
        right
    }
}

fn normalized_path(path: &str) -> String {
    path.trim().trim_start_matches("./").replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        Action, Config, ConfigFieldSources, NarrationMode, PlanPreset, PromptLayout, Provider,
    };
    use crate::minimal_loop::loop_run::{
        RunSessionOptions, RunSessionStepKind, RunStopReason, run_session_with_outcome_with_options,
    };
    use crate::providers::{AssistantReply, ChatClient};
    use crate::state::{ConversationMessage, SessionSnapshot, ToolCall};
    use crate::tools::registry::ToolSpec;
    use crate::tui::NOOP_UI;
    use serde_json::{Value, json};
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
            "recording-fake"
        }

        fn boxed_clone(&self) -> Box<dyn ChatClient> {
            Box::new(self.clone())
        }

        fn supports_native_tools(&self, _model: &str) -> bool {
            true
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
            max_iterations: 4,
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

    fn read_reply(path: &str) -> AssistantReply {
        AssistantReply {
            content: String::new(),
            tool_calls: vec![ToolCall::new("Read", json!({"path": path}))],
            prompt_tokens: None,
            completion_tokens: None,
        }
    }

    fn write_reply(path: &str, content: &str) -> AssistantReply {
        AssistantReply {
            content: String::new(),
            tool_calls: vec![ToolCall::new(
                "Write",
                json!({"path": path, "content": content}),
            )],
            prompt_tokens: None,
            completion_tokens: None,
        }
    }

    fn missing_anchor_edit_reply(path: &str) -> AssistantReply {
        AssistantReply {
            content: String::new(),
            tool_calls: vec![ToolCall::new(
                "Edit",
                json!({"path": path, "old_string": "missing anchor", "new_string": "new\n"}),
            )],
            prompt_tokens: None,
            completion_tokens: None,
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
    fn short_budget_pre_advance_seeds_write_required_rung() {
        let carryover = EscalationCarryoverHandle::new();

        let seed = carryover.seed_for_session(2);

        assert!(seed.pre_advanced);
        assert_eq!(
            seed.initial_read_only_streak,
            READ_ONLY_STAGNATION_WRITE_REQUIRED_THRESHOLD - 1
        );
        assert_eq!(seed.carried_streak, 0);
        assert_eq!(seed.carried_anchor_failures, 0);
    }

    #[test]
    fn long_budget_keeps_carried_rung() {
        let carryover = EscalationCarryoverHandle::new();
        carryover.record_read_only_streak(2);

        let seed = carryover.seed_for_session(8);

        assert!(!seed.pre_advanced);
        assert_eq!(seed.initial_read_only_streak, 2);
        assert_eq!(seed.carried_streak, 2);
    }

    #[test]
    fn carried_anchor_failures_reduce_distance_to_write_required() {
        let carryover = EscalationCarryoverHandle::new();
        carryover.record_anchor_failure(EditAnchorFailureSummary {
            path: "src/app/page.tsx".to_string(),
            failure_count: 2,
        });

        let seed = carryover.seed_for_session(4);

        assert!(seed.pre_advanced);
        assert_eq!(seed.carried_anchor_failures, 2);
    }

    #[test]
    fn write_required_exhaustion_forces_next_attempt_to_write_required() {
        let carryover = EscalationCarryoverHandle::new();
        carryover.note_write_required_exhausted();

        let seed = carryover.seed_for_session(12);

        assert!(seed.pre_advanced);
        assert!(seed.carried_write_required_exhausted);
    }

    #[test]
    fn short_repair_budget_starts_with_write_required_feedback() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("target.txt"), "old\n").unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 2;
        let carryover = EscalationCarryoverHandle::new();
        let mut fake = RecordingFake::new(vec![
            Ok(read_reply("target.txt")),
            Ok(write_reply("target.txt", "new\n")),
        ]);
        let mut session = SessionSnapshot::new();

        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Implement the requested change in target.txt.",
            &["target.txt".to_string()],
            &cfg,
            &NOOP_UI,
            attach_to_options(RunSessionOptions::final_acceptance_repair(), carryover),
        )
        .unwrap();

        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
        let events = event_values(&events);
        let stages = events
            .iter()
            .filter(|event| {
                event.get("event").and_then(Value::as_str) == Some("read_only_stagnation_feedback")
            })
            .map(|event| event.get("stage").and_then(Value::as_str).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(stages, vec!["write_required"]);
        assert!(events.iter().any(|event| {
            event.get("event").and_then(Value::as_str) == Some("escalation_carryover")
                && event.get("pre_advanced").and_then(Value::as_bool) == Some(true)
                && event.get("carried_streak").and_then(Value::as_u64) == Some(0)
        }));
        assert!(fake.requests().iter().any(|request| {
            request.iter().any(|message| {
                message
                    .content
                    .contains("Use a full-file Write or Edit for `target.txt` now")
            })
        }));
    }

    #[test]
    fn final_acceptance_carryover_maps_prefixed_restart_evidence_for_write_required() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(dir.path().join("package.json"), "{}\n").unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page(){ return <button>Restart</button>; }\n",
        )
        .unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 2;
        cfg.profile = "nextjs".to_string();
        let carryover = EscalationCarryoverHandle::new();
        let pending_evidence = carryover.carry_pending_evidence(
            Vec::new(),
            &[
                "weak_source_evidence:restart_or_recoverable_state_evidence:restart handler does not reset entities"
                    .to_string(),
            ],
        );
        assert_eq!(carryover.pending_evidence(), pending_evidence);
        let mut fake = RecordingFake::new(vec![
            Ok(read_reply("src/app/page.tsx")),
            Ok(write_reply(
                "src/app/page.tsx",
                "export default function Page(){ return <button data-anvil-action=\"restart\">Restart</button>; }\n",
            )),
        ]);
        let mut session = SessionSnapshot::new();

        run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Implement restart behavior in src/app/page.tsx.",
            &["package.json".to_string(), "src/app/page.tsx".to_string()],
            &cfg,
            &NOOP_UI,
            attach_to_options(RunSessionOptions::final_acceptance_repair(), carryover),
        )
        .unwrap();

        let events = event_values(&events);
        let write_required = events
            .iter()
            .find(|event| {
                event.get("event").and_then(Value::as_str) == Some("read_only_stagnation_feedback")
                    && event.get("stage").and_then(Value::as_str) == Some("write_required")
            })
            .unwrap();
        assert_eq!(
            write_required
                .get("selection_reason")
                .and_then(Value::as_str),
            Some("evidence_mapped")
        );
        assert_eq!(
            write_required.get("selected_targets"),
            Some(&json!(["src/app/page.tsx"]))
        );
    }

    #[test]
    fn long_repair_budget_keeps_staged_read_only_escalation() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("target.txt"), "old\n").unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 8;
        let carryover = EscalationCarryoverHandle::new();
        let mut replies = (0..7)
            .map(|_| Ok(read_reply("target.txt")))
            .collect::<Vec<_>>();
        replies.push(Ok(write_reply("target.txt", "new\n")));
        let mut fake = RecordingFake::new(replies);
        let mut session = SessionSnapshot::new();

        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Implement the requested change in target.txt.",
            &["target.txt".to_string()],
            &cfg,
            &NOOP_UI,
            attach_to_options(
                RunSessionOptions::plan_step(RunSessionStepKind::Implement),
                carryover,
            ),
        )
        .unwrap();

        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
        let events = event_values(&events);
        let stages = events
            .iter()
            .filter(|event| {
                event.get("event").and_then(Value::as_str) == Some("read_only_stagnation_feedback")
            })
            .map(|event| event.get("stage").and_then(Value::as_str).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            stages,
            vec!["intervention", "compact_restatement", "write_required"]
        );
        assert!(!events.iter().any(|event| {
            event.get("event").and_then(Value::as_str) == Some("escalation_carryover")
                && event.get("pre_advanced").and_then(Value::as_bool) == Some(true)
        }));
    }

    #[test]
    fn carried_anchor_failures_start_next_attempt_on_advanced_rung() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("target.txt"), "old\n").unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 8;
        let carryover = EscalationCarryoverHandle::new();
        let mut first_attempt = RecordingFake::new(vec![
            Ok(missing_anchor_edit_reply("target.txt")),
            Ok(missing_anchor_edit_reply("target.txt")),
            Ok(AssistantReply::text("done")),
        ]);
        let mut first_session = SessionSnapshot::new();
        run_session_with_outcome_with_options(
            &mut first_attempt,
            &mut first_session,
            "Implement the requested change in target.txt.",
            &[],
            &cfg,
            &NOOP_UI,
            attach_to_options(
                RunSessionOptions::plan_step(RunSessionStepKind::Implement),
                carryover.clone(),
            ),
        )
        .unwrap();

        let mut second_attempt = RecordingFake::new(vec![
            Ok(read_reply("target.txt")),
            Ok(write_reply("target.txt", "new\n")),
        ]);
        let mut second_session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_options(
            &mut second_attempt,
            &mut second_session,
            "Implement the requested change in target.txt.",
            &["target.txt".to_string()],
            &cfg,
            &NOOP_UI,
            attach_to_options(
                RunSessionOptions::plan_step(RunSessionStepKind::Implement),
                carryover,
            ),
        )
        .unwrap();

        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
        let events = event_values(&events);
        assert!(events.iter().any(|event| {
            event.get("event").and_then(Value::as_str) == Some("escalation_carryover")
                && event.get("carried_anchor_failures").and_then(Value::as_u64) == Some(2)
                && event.get("pre_advanced").and_then(Value::as_bool) == Some(false)
        }));
        assert!(events.iter().any(|event| {
            event.get("event").and_then(Value::as_str) == Some("anchor_stagnation_interlock")
                && event.get("anchor_failures").and_then(Value::as_u64) == Some(2)
                && event.get("streak").and_then(Value::as_u64) == Some(1)
                && event.get("effective_streak").and_then(Value::as_u64) == Some(3)
                && event.get("stage").and_then(Value::as_str) == Some("intervention")
        }));
        assert!(second_attempt.requests().iter().any(|request| {
            request.iter().any(|message| {
                message.content.contains("full-file Write now")
                    && message.content.contains("target.txt")
            })
        }));
    }
}
