use std::collections::BTreeMap;
use std::path::Path;

use serde_json::json;

use crate::eval_events;
use crate::tools::path_guard::{
    normalize_workspace_path, relative_display, resolve_optional_existing,
};

pub(crate) const EDIT_ANCHOR_FULL_FILE_WRITE_THRESHOLD: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditAnchorRecoveryStage {
    Reanchor,
    FullFileWrite,
}

impl EditAnchorRecoveryStage {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Reanchor => "reanchor",
            Self::FullFileWrite => "full_file_write",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditAnchorRecovery {
    pub(crate) path: String,
    pub(crate) failure_count: usize,
    pub(crate) stage: EditAnchorRecoveryStage,
}

impl EditAnchorRecovery {
    pub(crate) fn should_force_full_file_write(&self) -> bool {
        self.stage == EditAnchorRecoveryStage::FullFileWrite
    }
}

#[derive(Debug, Default)]
pub(crate) struct EditAnchorRecoveryState {
    failures_by_path: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditAnchorFailureSummary {
    pub(crate) path: String,
    pub(crate) failure_count: usize,
}

impl EditAnchorRecoveryState {
    pub(crate) fn record_failure(
        &mut self,
        root: &Path,
        arguments: &serde_json::Value,
    ) -> Option<EditAnchorRecovery> {
        let path = edit_anchor_tracking_path(root, arguments)?;
        let count = self.failures_by_path.entry(path.clone()).or_default();
        *count += 1;
        Some(EditAnchorRecovery {
            path,
            failure_count: *count,
            stage: if *count >= EDIT_ANCHOR_FULL_FILE_WRITE_THRESHOLD {
                EditAnchorRecoveryStage::FullFileWrite
            } else {
                EditAnchorRecoveryStage::Reanchor
            },
        })
    }

    pub(crate) fn note_successful_write(&mut self, root: &Path, arguments: &serde_json::Value) {
        let Some(path) = edit_anchor_tracking_path(root, arguments) else {
            return;
        };
        self.failures_by_path.remove(&path);
    }

    pub(crate) fn strongest_failure(&self) -> Option<EditAnchorFailureSummary> {
        self.failures_by_path
            .iter()
            .max_by(|(left_path, left_count), (right_path, right_count)| {
                left_count
                    .cmp(right_count)
                    .then_with(|| right_path.cmp(left_path))
            })
            .map(|(path, failure_count)| EditAnchorFailureSummary {
                path: path.clone(),
                failure_count: *failure_count,
            })
    }
}

pub(crate) fn emit_recovery_event(eval_events_path: Option<&Path>, recovery: &EditAnchorRecovery) {
    eval_events::emit(
        eval_events_path,
        json!({
            "event": "edit_anchor_recovery",
            "stage": recovery.stage.as_str(),
            "path": recovery.path,
            "failure_count": recovery.failure_count,
        }),
    );
}

pub(crate) fn feedback(name: &str, err_text: &str, recovery: &EditAnchorRecovery) -> String {
    if recovery.should_force_full_file_write() {
        format!(
            "Tool call `{name}` was rejected with a recoverable validation error: {err_text}. This is edit anchor failure #{} for `{}` in this step; switch to the Write tool and write the complete corrected file content for that one file.",
            recovery.failure_count, recovery.path
        )
    } else {
        format!(
            "Tool call `{name}` was rejected with a recoverable validation error: {err_text}. This is edit anchor failure #{} for `{}` in this step; retry with the Edit tool using the deterministic best-match excerpt and re-anchor mandate from the error.",
            recovery.failure_count, recovery.path
        )
    }
}

fn edit_anchor_tracking_path(root: &Path, arguments: &serde_json::Value) -> Option<String> {
    let normalized = normalized_tool_path_arg(root, arguments)?;
    let path = resolve_optional_existing(root, &normalized).ok()?;
    let root = root.canonicalize().ok()?;
    Some(relative_display(&root, &path))
}

fn normalized_tool_path_arg(root: &Path, arguments: &serde_json::Value) -> Option<String> {
    let raw = arguments.get("path")?.as_str()?;
    match normalize_workspace_path(root, raw).ok()? {
        Some(normalization) => Some(normalization.relative),
        None => Some(raw.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Action, Config, ConfigFieldSources, NarrationMode, PlanPreset, Provider};
    use crate::minimal_loop::loop_run::{RunSessionOptions, run_session_with_outcome_with_options};
    use crate::providers::{AssistantReply, ChatClient};
    use crate::state::{ConversationMessage, SessionSnapshot, ToolCall};
    use crate::tools::registry::ToolSpec;
    use crate::tui::NOOP_UI;
    use serde_json::Value;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct Fake {
        replies: Arc<Mutex<Vec<anyhow::Result<AssistantReply>>>>,
    }

    impl Fake {
        fn new(replies: Vec<anyhow::Result<AssistantReply>>) -> Self {
            Self {
                replies: Arc::new(Mutex::new(replies)),
            }
        }
    }

    impl ChatClient for Fake {
        fn label(&self) -> &str {
            "fake"
        }

        fn boxed_clone(&self) -> Box<dyn ChatClient> {
            Box::new(self.clone())
        }

        fn chat(
            &mut self,
            _model: &str,
            _messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _native_tools_enabled: bool,
        ) -> anyhow::Result<AssistantReply> {
            self.replies.lock().unwrap().remove(0)
        }
    }

    fn config(root: &Path) -> Config {
        Config {
            workspace_root: root.to_path_buf(),
            state_dir: root.join("state"),
            yes: true,
            offline: false,
            context_budget: 1000,
            model: "m".to_string(),
            provider: Provider::Ollama,
            prompt_layout: crate::config::PromptLayout::Stable,
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
            eval_events_path: Some(root.join(".anvil/runs/edit-anchor/events.jsonl")),
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

    fn reply_with_tool(name: &str, arguments: Value) -> anyhow::Result<AssistantReply> {
        Ok(AssistantReply {
            content: String::new(),
            tool_calls: vec![ToolCall::new(name, arguments)],
            prompt_tokens: None,
            completion_tokens: None,
        })
    }

    fn event_values(path: &Path) -> Vec<Value> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect()
    }

    #[test]
    fn third_anchor_failure_escalates_to_full_file_write() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "actual").unwrap();
        let args = serde_json::json!({"path":"a.txt"});
        let mut state = EditAnchorRecoveryState::default();

        let first = state.record_failure(dir.path(), &args).unwrap();
        let second = state.record_failure(dir.path(), &args).unwrap();
        let third = state.record_failure(dir.path(), &args).unwrap();

        assert_eq!(first.stage, EditAnchorRecoveryStage::Reanchor);
        assert_eq!(second.stage, EditAnchorRecoveryStage::Reanchor);
        assert_eq!(third.stage, EditAnchorRecoveryStage::FullFileWrite);
        assert_eq!(third.failure_count, EDIT_ANCHOR_FULL_FILE_WRITE_THRESHOLD);
    }

    #[test]
    fn successful_write_resets_failure_count_for_that_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "actual").unwrap();
        let args = serde_json::json!({"path":"a.txt"});
        let mut state = EditAnchorRecoveryState::default();

        state.record_failure(dir.path(), &args).unwrap();
        state.record_failure(dir.path(), &args).unwrap();
        state.note_successful_write(dir.path(), &args);
        let next = state.record_failure(dir.path(), &args).unwrap();

        assert_eq!(next.failure_count, 1);
        assert_eq!(next.stage, EditAnchorRecoveryStage::Reanchor);
    }

    #[test]
    fn strongest_failure_reports_anchor_interlock_target() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "actual").unwrap();
        std::fs::write(dir.path().join("b.txt"), "actual").unwrap();
        let mut state = EditAnchorRecoveryState::default();

        state
            .record_failure(dir.path(), &serde_json::json!({"path":"a.txt"}))
            .unwrap();
        state
            .record_failure(dir.path(), &serde_json::json!({"path":"b.txt"}))
            .unwrap();
        state
            .record_failure(dir.path(), &serde_json::json!({"path":"b.txt"}))
            .unwrap();

        assert_eq!(
            state.strongest_failure(),
            Some(EditAnchorFailureSummary {
                path: "b.txt".to_string(),
                failure_count: 2,
            })
        );
    }

    #[test]
    fn recovery_event_records_stage_path_and_failure_count() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join(".anvil/runs/edit-anchor/events.jsonl");
        let recovery = EditAnchorRecovery {
            path: "src/app/page.tsx".to_string(),
            failure_count: 3,
            stage: EditAnchorRecoveryStage::FullFileWrite,
        };

        emit_recovery_event(Some(&events), &recovery);

        let events = event_values(&events);
        assert_eq!(events[0]["event"], "edit_anchor_recovery");
        assert_eq!(events[0]["stage"], "full_file_write");
        assert_eq!(events[0]["path"], "src/app/page.tsx");
        assert_eq!(events[0]["failure_count"], 3);
    }

    #[test]
    fn final_acceptance_repair_accepts_write_after_anchor_escalation() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(dir.path().join("src/app/page.tsx"), "actual content\n").unwrap();
        let cfg = config(dir.path());
        let mut fake = Fake::new(vec![
            reply_with_tool(
                "Edit",
                json!({"path":"src/app/page.tsx","old_string":"missing anchor","new_string":"replacement"}),
            ),
            reply_with_tool(
                "Edit",
                json!({"path":"src/app/page.tsx","old_string":"other missing anchor","new_string":"replacement"}),
            ),
            reply_with_tool(
                "Edit",
                json!({"path":"src/app/page.tsx","old_string":"third missing anchor","new_string":"replacement"}),
            ),
            reply_with_tool(
                "Write",
                json!({"path":"src/app/page.tsx","content":"corrected content\n"}),
            ),
        ]);
        let mut session = SessionSnapshot::new();

        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Repair final acceptance.\n\nRequired final artifacts:\n- src/app/page.tsx",
            &["src/app/page.tsx".to_string()],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::final_acceptance_repair(),
        )
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/app/page.tsx")).unwrap(),
            "corrected content\n"
        );
        assert!(
            outcome
                .changed_paths
                .contains(&"src/app/page.tsx".to_string())
        );
        assert!(session.messages.iter().any(|message| message.role == "tool"
            && message.content.contains("anchor failure #3")
            && message.content.contains("Write tool")));
    }
}
