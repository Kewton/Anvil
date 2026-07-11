use std::path::Path;

use serde_json::json;

use crate::eval_events;

use super::edit_anchor_recovery::EditAnchorFailureSummary;
use super::stagnation_escalation::{
    READ_ONLY_STAGNATION_WRITE_REQUIRED_THRESHOLD, ReadOnlyStagnationStage,
    WriteRequiredSelectionReason, WriteRequiredTargetSelection,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AnchorStagnationDecision {
    pub(crate) stage: ReadOnlyStagnationStage,
    pub(crate) read_only_streak: usize,
    pub(crate) effective_streak: usize,
    pub(crate) anchor_failure: Option<EditAnchorFailureSummary>,
}

impl AnchorStagnationDecision {
    pub(crate) fn anchor_interlocked(&self) -> bool {
        self.anchor_failure.is_some()
    }

    pub(crate) fn write_required_selection(&self) -> Option<WriteRequiredTargetSelection> {
        if self.stage != ReadOnlyStagnationStage::WriteRequired {
            return None;
        }
        let anchor = self.anchor_failure.as_ref()?;
        Some(WriteRequiredTargetSelection {
            selected_targets: vec![anchor.path.clone()],
            selection_reason: WriteRequiredSelectionReason::AnchorFailure,
        })
    }

    pub(crate) fn full_file_write_feedback(&self, objective: &str) -> Option<String> {
        let anchor = self.anchor_failure.as_ref()?;
        Some(format!(
            "Edit anchors already failed for `{}` (anchor_failures={}). Do not keep inspecting or retrying anchored Edit. Update `{}` with a full-file Write now using the complete corrected file content. Objective: {objective}. read_only_streak={}; effective_read_only_streak={}",
            anchor.path,
            anchor.failure_count,
            anchor.path,
            self.read_only_streak,
            self.effective_streak
        ))
    }

    pub(crate) fn write_required_feedback(&self, attempt_limit: usize) -> Option<String> {
        let anchor = self.anchor_failure.as_ref()?;
        Some(format!(
            "Read-only stagnation is interlocked with edit anchor failures. Use a full-file Write for `{}` now; do not use another anchored Edit because anchors already failed. Read, Grep, Glob, Bash, and prose-only responses are suspended until `{}` is written. read_only_streak={}; anchor_failures={}; write_required_no_write_limit={attempt_limit}",
            anchor.path, anchor.path, self.read_only_streak, anchor.failure_count
        ))
    }

    pub(crate) fn diagnostic_feedback(&self) -> String {
        self.anchor_failure
            .as_ref()
            .map(|anchor| {
                format!(
                    "Anchor interlock: `{}` has {} edit_anchor_not_found failure(s); update it with full-file Write content instead of another anchored Edit.",
                    anchor.path, anchor.failure_count
                )
            })
            .unwrap_or_default()
    }
}

pub(crate) fn read_only_stagnation_decision(
    read_only_streak: usize,
    anchor_failure: Option<EditAnchorFailureSummary>,
) -> Option<AnchorStagnationDecision> {
    let anchor_failure = anchor_failure
        .filter(|failure| failure.failure_count > 0 && !failure.path.trim().is_empty());
    if let Some(anchor_failure) = anchor_failure {
        let effective_streak = read_only_streak.saturating_add(anchor_failure.failure_count);
        let stage = if effective_streak >= READ_ONLY_STAGNATION_WRITE_REQUIRED_THRESHOLD {
            ReadOnlyStagnationStage::WriteRequired
        } else {
            ReadOnlyStagnationStage::for_streak(effective_streak)
                .or_else(|| ReadOnlyStagnationStage::for_streak(read_only_streak))?
        };
        return Some(AnchorStagnationDecision {
            stage,
            read_only_streak,
            effective_streak,
            anchor_failure: Some(anchor_failure),
        });
    }
    let stage = ReadOnlyStagnationStage::for_streak(read_only_streak)?;
    Some(AnchorStagnationDecision {
        stage,
        read_only_streak,
        effective_streak: read_only_streak,
        anchor_failure: None,
    })
}

pub(crate) fn emit_interlock_event(
    eval_events_path: Option<&Path>,
    decision: &AnchorStagnationDecision,
    session_scope: &str,
    step_kind: &str,
    phase_scope: Option<&str>,
) {
    let Some(anchor) = decision.anchor_failure.as_ref() else {
        return;
    };
    eval_events::emit(
        eval_events_path,
        json!({
            "event": "anchor_stagnation_interlock",
            "stage": decision.stage.as_str(),
            "path": anchor.path,
            "anchor_failures": anchor.failure_count,
            "streak": decision.read_only_streak,
            "effective_streak": decision.effective_streak,
            "session_scope": session_scope,
            "step_kind": step_kind,
            "phase_scope": phase_scope.unwrap_or(""),
        }),
    );
}

#[cfg(test)]
mod tests {
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
    use serde_json::Value;
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
