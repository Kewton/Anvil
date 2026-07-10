use std::path::Path;

use serde_json::json;

use crate::config::Config;
use crate::eval_events;
use crate::minimal_loop::evidence::required_evidence_for_capability;
use crate::planner::profile::profile_evidence_repair_target_paths;
use crate::state::ToolCall;
use crate::tools::path_guard::normalize_workspace_path;

pub(crate) const READ_ONLY_STAGNATION_INTERVENTION_THRESHOLD: usize = 3;
pub(crate) const READ_ONLY_STAGNATION_COMPACT_THRESHOLD: usize = 5;
pub(crate) const READ_ONLY_STAGNATION_WRITE_REQUIRED_THRESHOLD: usize = 7;
pub(crate) const WRITE_REQUIRED_NO_WRITE_LIMIT: usize = 2;
pub(crate) const READ_ONLY_STAGNATION_REASON: &str = "model_stagnation:read_only_loop";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadOnlyStagnationStage {
    Intervention,
    CompactRestatement,
    WriteRequired,
}

impl ReadOnlyStagnationStage {
    pub(crate) fn for_streak(streak: usize) -> Option<Self> {
        match streak {
            READ_ONLY_STAGNATION_INTERVENTION_THRESHOLD => Some(Self::Intervention),
            READ_ONLY_STAGNATION_COMPACT_THRESHOLD => Some(Self::CompactRestatement),
            READ_ONLY_STAGNATION_WRITE_REQUIRED_THRESHOLD => Some(Self::WriteRequired),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Intervention => "intervention",
            Self::CompactRestatement => "compact_restatement",
            Self::WriteRequired => "write_required",
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct WriteRequiredState {
    selected_targets: Vec<String>,
    selection_reason: Option<WriteRequiredSelectionReason>,
    no_write_attempts: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriteRequiredSelectionReason {
    EvidenceMapped,
    RepairChanged,
    RequiredPath,
    Fallback,
}

impl WriteRequiredSelectionReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::EvidenceMapped => "evidence_mapped",
            Self::RepairChanged => "repair_changed",
            Self::RequiredPath => "required_path",
            Self::Fallback => "fallback",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WriteRequiredTargetSelection {
    pub(crate) selected_targets: Vec<String>,
    pub(crate) selection_reason: WriteRequiredSelectionReason,
}

impl WriteRequiredTargetSelection {
    pub(crate) fn primary_target(&self) -> Option<&str> {
        self.selected_targets.first().map(String::as_str)
    }
}

impl WriteRequiredState {
    pub(crate) fn activate(&mut self, selection: WriteRequiredTargetSelection) {
        self.selected_targets = selection.selected_targets;
        self.selection_reason = Some(selection.selection_reason);
        self.no_write_attempts = 0;
    }

    pub(crate) fn reset(&mut self) {
        self.selected_targets.clear();
        self.selection_reason = None;
        self.no_write_attempts = 0;
    }

    pub(crate) fn target_path(&self) -> Option<&str> {
        self.selected_targets.first().map(String::as_str)
    }

    pub(crate) fn selected_targets(&self) -> &[String] {
        &self.selected_targets
    }

    pub(crate) fn selection_reason(&self) -> Option<WriteRequiredSelectionReason> {
        self.selection_reason
    }

    pub(crate) fn reject_if_read_only_or_wrong_target(
        &mut self,
        root: &Path,
        eval_events_path: Option<&Path>,
        call: &ToolCall,
        event_context: ReadOnlyToolRejectionContext<'_>,
    ) -> Option<ReadOnlyToolRejection> {
        if self.selected_targets.is_empty() {
            return None;
        }
        if tool_call_writes_any_target_path(root, call, &self.selected_targets) {
            return None;
        }
        if matches!(call.name.as_str(), "Write" | "Edit") {
            return None;
        }
        self.no_write_attempts = self.no_write_attempts.saturating_add(1);
        let target_path = self.target_path().unwrap_or("");
        let selection_reason = self
            .selection_reason
            .map(|reason| reason.as_str())
            .unwrap_or("");
        eval_events::emit(
            eval_events_path,
            json!({
                "event": "read_only_tool_rejected",
                "stage": "write_required",
                "tool_name": call.name.as_str(),
                "target_path": target_path,
                "selected_targets": self.selected_targets.clone(),
                "selection_reason": selection_reason,
                "read_only_streak": event_context.read_only_streak,
                "write_required_no_write_attempts": self.no_write_attempts,
                "write_required_no_write_limit": WRITE_REQUIRED_NO_WRITE_LIMIT,
                "session_scope": event_context.session_scope,
                "step_kind": event_context.step_kind,
                "phase_scope": event_context.phase_scope.unwrap_or(""),
            }),
        );
        let feedback = super::feedback::read_only_write_required_tool_rejected(
            &self.selected_targets,
            self.no_write_attempts,
            WRITE_REQUIRED_NO_WRITE_LIMIT,
        );
        Some(ReadOnlyToolRejection {
            feedback,
            exhausted: self.no_write_attempts >= WRITE_REQUIRED_NO_WRITE_LIMIT,
            no_write_attempts: self.no_write_attempts,
        })
    }

    pub(crate) fn off_target_write_warning(
        &self,
        root: &Path,
        eval_events_path: Option<&Path>,
        call: &ToolCall,
        event_context: ReadOnlyToolRejectionContext<'_>,
    ) -> Option<String> {
        if self.selected_targets.is_empty()
            || !matches!(call.name.as_str(), "Write" | "Edit")
            || tool_call_writes_any_target_path(root, call, &self.selected_targets)
        {
            return None;
        }
        let actual_path = normalized_tool_path_arg(root, &call.arguments).unwrap_or_default();
        let selection_reason = self
            .selection_reason
            .map(|reason| reason.as_str())
            .unwrap_or("");
        let feedback = super::feedback::read_only_write_required_off_target_write_allowed(
            &actual_path,
            &self.selected_targets,
        );
        eval_events::emit(
            eval_events_path,
            json!({
                "event": "write_required_off_target_write_allowed",
                "stage": "write_required",
                "tool_name": call.name.as_str(),
                "target_path": self.target_path().unwrap_or(""),
                "actual_path": actual_path,
                "selected_targets": self.selected_targets.clone(),
                "selection_reason": selection_reason,
                "read_only_streak": event_context.read_only_streak,
                "session_scope": event_context.session_scope,
                "step_kind": event_context.step_kind,
                "phase_scope": event_context.phase_scope.unwrap_or(""),
                "feedback": eval_events::body_snippet(&feedback),
            }),
        );
        Some(feedback)
    }

    pub(crate) fn note_successful_write(
        &mut self,
        root: &Path,
        arguments: &serde_json::Value,
    ) -> bool {
        if self.selected_targets.is_empty() {
            return false;
        }
        if tool_arguments_path_matches_any(root, arguments, &self.selected_targets) {
            self.reset();
            return true;
        }
        false
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ReadOnlyToolRejectionContext<'a> {
    pub(crate) read_only_streak: usize,
    pub(crate) session_scope: &'a str,
    pub(crate) step_kind: &'a str,
    pub(crate) phase_scope: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReadOnlyToolRejection {
    pub(crate) feedback: String,
    pub(crate) exhausted: bool,
    pub(crate) no_write_attempts: usize,
}

pub(crate) fn write_required_target_selection(
    evidence_mapped_paths: &[String],
    repair_changed_paths: &[String],
    required_paths: &[String],
    changed_paths: &[String],
    path_fallback_candidates: &[String],
) -> Option<WriteRequiredTargetSelection> {
    for (paths, reason) in [
        (
            evidence_mapped_paths,
            WriteRequiredSelectionReason::EvidenceMapped,
        ),
        (
            repair_changed_paths,
            WriteRequiredSelectionReason::RepairChanged,
        ),
        (required_paths, WriteRequiredSelectionReason::RequiredPath),
        (changed_paths, WriteRequiredSelectionReason::Fallback),
        (
            path_fallback_candidates,
            WriteRequiredSelectionReason::Fallback,
        ),
    ] {
        let selected_targets = ordered_non_empty_paths(paths);
        if !selected_targets.is_empty() {
            return Some(WriteRequiredTargetSelection {
                selected_targets,
                selection_reason: reason,
            });
        }
    }
    None
}

pub(crate) fn write_required_evidence_targets(
    root: &Path,
    profile: &str,
    missing_evidence: &[String],
    missing_capabilities: &[String],
) -> Vec<String> {
    let evidence_keys = write_required_evidence_keys(missing_evidence, missing_capabilities);
    if evidence_keys.is_empty() {
        return Vec::new();
    }
    profile_evidence_repair_target_paths(root, profile, &evidence_keys)
}

fn write_required_evidence_keys(
    missing_evidence: &[String],
    missing_capabilities: &[String],
) -> Vec<String> {
    let mut out = Vec::new();
    for key in missing_evidence {
        push_unique_evidence_key(&mut out, key);
    }
    for capability in missing_capabilities {
        for evidence in required_evidence_for_capability(capability) {
            push_unique_evidence_key(&mut out, &evidence);
        }
    }
    out
}

fn push_unique_evidence_key(out: &mut Vec<String>, key: &str) {
    let trimmed = key.trim();
    if !trimmed.is_empty() && !out.iter().any(|existing| existing == trimmed) {
        out.push(trimmed.to_string());
    }
}

#[cfg(test)]
pub(crate) fn write_required_target_path(
    repair_changed_paths: &[String],
    required_paths: &[String],
    changed_paths: &[String],
    path_fallback_candidates: &[String],
) -> Option<String> {
    write_required_target_selection(
        &[],
        repair_changed_paths,
        required_paths,
        changed_paths,
        path_fallback_candidates,
    )
    .and_then(|selection| selection.primary_target().map(str::to_string))
}

pub(crate) fn read_only_write_required_feedback(
    selected_targets: &[String],
    streak: usize,
) -> String {
    super::feedback::read_only_stagnation_write_required(
        selected_targets,
        streak,
        WRITE_REQUIRED_NO_WRITE_LIMIT,
    )
}

#[cfg(test)]
pub(crate) fn tool_call_writes_target_path(
    root: &Path,
    call: &ToolCall,
    target_path: &str,
) -> bool {
    matches!(call.name.as_str(), "Write" | "Edit")
        && tool_arguments_path_matches(root, &call.arguments, target_path)
}

pub(crate) fn tool_call_writes_any_target_path(
    root: &Path,
    call: &ToolCall,
    target_paths: &[String],
) -> bool {
    matches!(call.name.as_str(), "Write" | "Edit")
        && tool_arguments_path_matches_any(root, &call.arguments, target_paths)
}

fn tool_arguments_path_matches_any(
    root: &Path,
    arguments: &serde_json::Value,
    target_paths: &[String],
) -> bool {
    let Some(actual) = normalized_tool_path_arg(root, arguments) else {
        return false;
    };
    let actual = normalized_relative_for_compare(&actual);
    target_paths
        .iter()
        .any(|target_path| actual == normalized_relative_for_compare(target_path))
}

#[cfg(test)]
fn tool_arguments_path_matches(
    root: &Path,
    arguments: &serde_json::Value,
    target_path: &str,
) -> bool {
    let Some(actual) = normalized_tool_path_arg(root, arguments) else {
        return false;
    };
    normalized_relative_for_compare(&actual) == normalized_relative_for_compare(target_path)
}

fn normalized_tool_path_arg(root: &Path, arguments: &serde_json::Value) -> Option<String> {
    let raw = arguments.get("path")?.as_str()?;
    match normalize_workspace_path(root, raw).ok()? {
        Some(normalization) => Some(normalization.relative),
        None => Some(raw.to_string()),
    }
}

fn normalized_relative_for_compare(path: &str) -> String {
    path.trim().trim_start_matches("./").replace('\\', "/")
}

fn ordered_non_empty_paths(paths: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for path in paths {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            continue;
        }
        let normalized = normalized_relative_for_compare(trimmed);
        if !out
            .iter()
            .any(|existing| normalized_relative_for_compare(existing.as_str()) == normalized)
        {
            out.push(trimmed.to_string());
        }
    }
    out
}

#[derive(Debug, Clone)]
pub(crate) struct ReadOnlyRecoveryPaths {
    pub(crate) prompt_path: String,
    pub(crate) yaml_path: String,
    pub(crate) suggested_prompt_command: String,
    pub(crate) suggested_yaml_command: String,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn save_read_only_write_required_handoff(
    config: &Config,
    objective: &str,
    session_scope: &str,
    step_kind: &str,
    phase_scope: Option<&str>,
    selected_targets: &[String],
    selection_reason: &str,
    changed_paths: &[String],
    read_only_streak: usize,
    no_write_attempts: usize,
) -> Option<ReadOnlyRecoveryPaths> {
    let failure_kind = READ_ONLY_STAGNATION_REASON;
    let target_path = selected_targets
        .first()
        .map(String::as_str)
        .unwrap_or_default();
    let profile = if config.profile.trim().is_empty() {
        "generic"
    } else {
        config.profile.as_str()
    };
    let handoff = crate::planner::repair::RecoveryHandoff {
        profile: profile.to_string(),
        original_goal: objective.to_string(),
        failed_phase: phase_scope
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .or_else(|| Some(session_scope.to_string())),
        failed_step: (!step_kind.trim().is_empty()).then(|| step_kind.to_string()),
        failure_kind: failure_kind.to_string(),
        failure_evidence: vec![
            format!(
                "read_only_stagnation: write_required reached after read_only_streak={read_only_streak}"
            ),
            format!(
                "write_required exhausted without Write/Edit to {target_path}: attempts={no_write_attempts}/{WRITE_REQUIRED_NO_WRITE_LIMIT}"
            ),
            format!(
                "write_required selected_targets={}; selection_reason={selection_reason}",
                selected_targets.join(",")
            ),
        ],
        missing_paths: Vec::new(),
        missing_capabilities: Vec::new(),
        verify_commands: Vec::new(),
        changed_paths: changed_paths.to_vec(),
        repair_targets: selected_targets.to_vec(),
    };
    let prompt_path = match crate::planner::repair::save_ultra_recovery_prompt(
        &config.workspace_root,
        "read-only-stagnation",
        &handoff,
    ) {
        Ok(path) => path,
        Err(err) => {
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "recovery_prompt_save_failed",
                    "recovery_handoff_kind": failure_kind,
                    "reason": eval_events::body_snippet(&err.to_string()),
                    "status": "incomplete",
                }),
            );
            return None;
        }
    };
    let yaml_path = match crate::planner::repair::save_recovery_ultra_plan(
        &config.workspace_root,
        "read-only-stagnation",
        &handoff,
    ) {
        Ok(path) => path,
        Err(err) => {
            let prompt_display =
                crate::planner::repair::workspace_relative_handoff_path(&prompt_path);
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "recovery_ultra_plan_save_failed",
                    "recovery_handoff_kind": failure_kind,
                    "recovery_prompt_path": prompt_display,
                    "reason": eval_events::body_snippet(&err.to_string()),
                    "recovery_yaml_missing": true,
                    "status": "incomplete",
                }),
            );
            return None;
        }
    };
    let suggested_prompt_command =
        crate::planner::repair::suggested_ultra_recovery_command(&prompt_path, profile);
    let suggested_yaml_command =
        crate::planner::repair::suggested_recovery_ultra_plan_command(&yaml_path);
    let prompt_display = crate::planner::repair::workspace_relative_handoff_path(&prompt_path);
    let yaml_display = crate::planner::repair::workspace_relative_handoff_path(&yaml_path);
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "recovery_prompt_saved",
            "recovery_handoff_kind": failure_kind,
            "recovery_prompt_path": &prompt_display,
            "recovery_ultra_plan_path": &yaml_display,
            "recovery_yaml_missing": false,
            "recovery_yaml_roundtrip_ok": true,
            "suggested_recovery_command": suggested_prompt_command,
            "suggested_recovery_yaml_command": suggested_yaml_command,
            "recovery_profile": profile,
            "local_repair_exhausted": true,
            "failure_kind": failure_kind,
            "status": "incomplete",
        }),
    );
    Some(ReadOnlyRecoveryPaths {
        prompt_path: prompt_display,
        yaml_path: yaml_display,
        suggested_prompt_command,
        suggested_yaml_command,
    })
}

pub(crate) fn render_read_only_recovery_stop_reason(
    free_text: impl Into<String>,
    recovery_paths: Option<&ReadOnlyRecoveryPaths>,
) -> String {
    let mut parts = eval_events::StopReasonParts::free_text(free_text);
    if let Some(paths) = recovery_paths {
        parts
            .paths
            .push(format!("recovery prompt saved: {}", paths.prompt_path));
        parts
            .paths
            .push(format!("recovery YAML saved: {}", paths.yaml_path));
        parts.commands.push(format!(
            "suggested command: {}",
            paths.suggested_prompt_command
        ));
        parts.commands.push(format!(
            "suggested YAML command: {}",
            paths.suggested_yaml_command
        ));
    }
    eval_events::render_stop_reason(&parts)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_write_required_exhaustion_and_render_stop(
    config: &Config,
    objective: &str,
    session_scope: &str,
    step_kind: &str,
    phase_scope: Option<&str>,
    selected_targets: &[String],
    selection_reason: &str,
    changed_paths: &[String],
    read_only_streak: usize,
    no_write_attempts: usize,
    tool_calls: usize,
    verify_attempts: usize,
    last_blocking_reason: Option<&str>,
) -> String {
    let target_path = selected_targets
        .first()
        .map(String::as_str)
        .unwrap_or_default();
    let recovery_paths = save_read_only_write_required_handoff(
        config,
        objective,
        session_scope,
        step_kind,
        phase_scope,
        selected_targets,
        selection_reason,
        changed_paths,
        read_only_streak,
        no_write_attempts,
    );
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "loop_stop",
            "reason": READ_ONLY_STAGNATION_REASON,
            "read_only_streak": read_only_streak,
            "write_required_no_write_attempts": no_write_attempts,
            "write_required_no_write_limit": WRITE_REQUIRED_NO_WRITE_LIMIT,
            "write_required_target_path": target_path,
            "selected_targets": selected_targets,
            "selection_reason": selection_reason,
            "tool_calls": tool_calls,
            "verify_attempts": verify_attempts,
            "last_blocking_reason": last_blocking_reason,
            "last_provider_error": null,
            "session_scope": session_scope,
            "step_kind": step_kind,
            "phase_scope": phase_scope.unwrap_or(""),
            "recovery_prompt_path": recovery_paths
                .as_ref()
                .map(|paths| paths.prompt_path.as_str())
                .unwrap_or(""),
            "recovery_ultra_plan_path": recovery_paths
                .as_ref()
                .map(|paths| paths.yaml_path.as_str())
                .unwrap_or(""),
            "recovery_yaml_missing": recovery_paths.is_none(),
        }),
    );
    render_read_only_recovery_stop_reason(
        format!(
            "{READ_ONLY_STAGNATION_REASON}: write_required exhausted for {target_path}; objective: {}",
            eval_events::body_snippet(objective)
        ),
        recovery_paths.as_ref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Action, ConfigFieldSources, NarrationMode, PromptLayout, Provider};
    use crate::minimal_loop::loop_run::{
        RunSessionOptions, RunSessionStepKind, RunStopReason, run_session_with_outcome_with_options,
    };
    use crate::providers::{AssistantReply, ChatClient};
    use crate::state::ToolCall;
    use crate::state::{ConversationMessage, SessionSnapshot};
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

        fn supports_native_tools(&self, _model: &str) -> bool {
            true
        }

        fn chat(
            &mut self,
            _model: &str,
            messages: &[ConversationMessage],
            _tools: &[crate::tools::registry::ToolSpec],
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

    fn event_values(path: &Path) -> Vec<Value> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect()
    }

    #[test]
    fn write_required_stage_is_third_read_only_threshold() {
        assert_eq!(
            ReadOnlyStagnationStage::for_streak(READ_ONLY_STAGNATION_INTERVENTION_THRESHOLD)
                .map(ReadOnlyStagnationStage::as_str),
            Some("intervention")
        );
        assert_eq!(
            ReadOnlyStagnationStage::for_streak(READ_ONLY_STAGNATION_COMPACT_THRESHOLD)
                .map(ReadOnlyStagnationStage::as_str),
            Some("compact_restatement")
        );
        assert_eq!(
            ReadOnlyStagnationStage::for_streak(READ_ONLY_STAGNATION_WRITE_REQUIRED_THRESHOLD)
                .map(ReadOnlyStagnationStage::as_str),
            Some("write_required")
        );
    }

    #[test]
    fn write_required_target_prefers_repair_then_required_path() {
        assert_eq!(
            write_required_target_path(
                &["src/app/page.tsx".to_string()],
                &["fallback.tsx".to_string()],
                &[],
                &[]
            )
            .as_deref(),
            Some("src/app/page.tsx")
        );
        assert_eq!(
            write_required_target_path(&[], &["fallback.tsx".to_string()], &[], &[]).as_deref(),
            Some("fallback.tsx")
        );
    }

    #[test]
    fn write_required_selection_prefers_evidence_mapped_targets_over_required_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(dir.path().join("package.json"), "{}\n").unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page(){ return <button data-anvil-action=\"restart\">Restart</button>; }\n",
        )
        .unwrap();
        let evidence_targets = write_required_evidence_targets(
            dir.path(),
            "nextjs",
            &["restart_or_recoverable_state_evidence".to_string()],
            &[],
        );

        let selection = write_required_target_selection(
            &evidence_targets,
            &[],
            &["package.json".to_string(), "src/app/page.tsx".to_string()],
            &[],
            &[],
        )
        .unwrap();

        assert_eq!(
            selection.selection_reason,
            WriteRequiredSelectionReason::EvidenceMapped
        );
        assert_eq!(selection.selected_targets, vec!["src/app/page.tsx"]);
    }

    #[test]
    fn write_required_selection_keeps_legacy_order_without_evidence() {
        let selection = write_required_target_selection(
            &[],
            &["repair.ts".to_string()],
            &["required.ts".to_string()],
            &["changed.ts".to_string()],
            &["fallback.ts".to_string()],
        )
        .unwrap();
        assert_eq!(
            selection.selection_reason,
            WriteRequiredSelectionReason::RepairChanged
        );
        assert_eq!(selection.selected_targets, vec!["repair.ts"]);

        let selection = write_required_target_selection(
            &[],
            &[],
            &["required.ts".to_string()],
            &["changed.ts".to_string()],
            &["fallback.ts".to_string()],
        )
        .unwrap();
        assert_eq!(
            selection.selection_reason,
            WriteRequiredSelectionReason::RequiredPath
        );
        assert_eq!(selection.selected_targets, vec!["required.ts"]);
    }

    #[test]
    fn write_required_accepts_write_or_edit_to_target_only() {
        let root = tempfile::tempdir().unwrap();
        let absolute = root.path().join("src/app/page.tsx");
        std::fs::create_dir_all(absolute.parent().unwrap()).unwrap();
        std::fs::write(&absolute, "x").unwrap();

        assert!(tool_call_writes_target_path(
            root.path(),
            &ToolCall::new(
                "Write",
                json!({"path": absolute.display().to_string(), "content": "x"})
            ),
            "src/app/page.tsx"
        ));
        assert!(tool_call_writes_target_path(
            root.path(),
            &ToolCall::new(
                "Edit",
                json!({"path": "./src/app/page.tsx", "old_string": "x", "new_string": "y"})
            ),
            "src/app/page.tsx"
        ));
        assert!(!tool_call_writes_target_path(
            root.path(),
            &ToolCall::new("Read", json!({"path": "src/app/page.tsx"})),
            "src/app/page.tsx"
        ));
        assert!(!tool_call_writes_target_path(
            root.path(),
            &ToolCall::new(
                "Write",
                json!({"path": "src/app/other.tsx", "content": "x"})
            ),
            "src/app/page.tsx"
        ));
    }

    #[test]
    fn write_required_candidate_edit_clears_mode() {
        let root = tempfile::tempdir().unwrap();
        let mut state = WriteRequiredState::default();
        state.activate(WriteRequiredTargetSelection {
            selected_targets: vec![
                "src/app/page.tsx".to_string(),
                "src/app/game.tsx".to_string(),
            ],
            selection_reason: WriteRequiredSelectionReason::EvidenceMapped,
        });

        assert!(state.note_successful_write(
            root.path(),
            &json!({"path":"src/app/game.tsx","old_string":"x","new_string":"y"})
        ));
        assert!(state.selected_targets().is_empty());
        assert!(state.selection_reason().is_none());
    }

    #[test]
    fn write_required_allows_off_target_write_with_warning() {
        let root = tempfile::tempdir().unwrap();
        let mut state = WriteRequiredState::default();
        state.activate(WriteRequiredTargetSelection {
            selected_targets: vec!["src/app/page.tsx".to_string()],
            selection_reason: WriteRequiredSelectionReason::EvidenceMapped,
        });
        let call = ToolCall::new(
            "Write",
            json!({"path":"package.json","content":"{\"scripts\":{}}\n"}),
        );

        let rejection = state.reject_if_read_only_or_wrong_target(
            root.path(),
            None,
            &call,
            ReadOnlyToolRejectionContext {
                read_only_streak: 7,
                session_scope: "plan-run-step",
                step_kind: "implement",
                phase_scope: Some("final_acceptance"),
            },
        );
        assert!(rejection.is_none());
        let warning = state
            .off_target_write_warning(
                root.path(),
                None,
                &call,
                ReadOnlyToolRejectionContext {
                    read_only_streak: 7,
                    session_scope: "plan-run-step",
                    step_kind: "implement",
                    phase_scope: Some("final_acceptance"),
                },
            )
            .unwrap();
        assert!(warning.contains("was allowed"), "{warning}");
        assert_eq!(state.selected_targets(), &["src/app/page.tsx".to_string()]);
    }

    #[test]
    fn write_required_rejects_read_only_then_target_write_completes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("target.txt"), "old\n").unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 8;
        let mut replies = (0..7)
            .map(|_| Ok(read_reply("target.txt")))
            .collect::<Vec<_>>();
        replies.push(Ok(read_reply("target.txt")));
        replies.push(Ok(AssistantReply {
            content: String::new(),
            tool_calls: vec![ToolCall::new(
                "Write",
                json!({"path":"target.txt","content":"new\n"}),
            )],
            prompt_tokens: None,
            completion_tokens: None,
        }));
        let mut fake = RecordingFake::new(replies);
        let mut session = SessionSnapshot::new();
        let outcome = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Implement the requested change in target.txt.",
            &["target.txt".to_string()],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Implement),
        )
        .unwrap();

        assert_eq!(
            outcome.stop_reason,
            RunStopReason::RequiredArtifactsSatisfiedAfterTool
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("target.txt")).unwrap(),
            "new\n"
        );
        let events = event_values(&events);
        let stages = events
            .iter()
            .filter(|event| {
                event.get("event").and_then(Value::as_str) == Some("read_only_stagnation_feedback")
            })
            .map(|event| {
                event
                    .get("stage")
                    .and_then(Value::as_str)
                    .unwrap()
                    .to_string()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            stages,
            vec!["intervention", "compact_restatement", "write_required"]
        );
        assert!(events.iter().any(|event| {
            event.get("event").and_then(Value::as_str) == Some("read_only_tool_rejected")
                && event.get("target_path").and_then(Value::as_str) == Some("target.txt")
                && event
                    .get("selected_targets")
                    .and_then(Value::as_array)
                    .is_some_and(|targets| targets.iter().any(|value| value == "target.txt"))
                && event.get("selection_reason").and_then(Value::as_str) == Some("required_path")
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
    fn write_required_exhausts_and_saves_recovery() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("target.txt"), "old\n").unwrap();
        let events = dir.path().join("events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());
        cfg.max_iterations = 8;
        let mut replies = (0..7)
            .map(|_| Ok(read_reply("target.txt")))
            .collect::<Vec<_>>();
        replies.push(Ok(read_reply("target.txt")));
        replies.push(Ok(AssistantReply {
            content: String::new(),
            tool_calls: vec![ToolCall::new("Glob", json!({"pattern":"**/*"}))],
            prompt_tokens: None,
            completion_tokens: None,
        }));
        let mut fake = RecordingFake::new(replies);
        let mut session = SessionSnapshot::new();
        let err = run_session_with_outcome_with_options(
            &mut fake,
            &mut session,
            "Implement the requested change in target.txt.",
            &["target.txt".to_string()],
            &cfg,
            &NOOP_UI,
            RunSessionOptions::plan_step(RunSessionStepKind::Implement),
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("model_stagnation:read_only_loop"), "{err}");
        assert!(err.contains("recovery prompt saved:"), "{err}");
        assert!(dir.path().join(".anvil/repairs").is_dir());
        assert!(dir.path().join(".anvil/plans").is_dir());
        let events = event_values(&events);
        assert_eq!(
            events
                .iter()
                .filter(|event| {
                    event.get("event").and_then(Value::as_str) == Some("read_only_tool_rejected")
                })
                .count(),
            2
        );
        assert!(events.iter().any(|event| {
            event.get("event").and_then(Value::as_str) == Some("loop_stop")
                && event.get("reason").and_then(Value::as_str)
                    == Some("model_stagnation:read_only_loop")
                && event.get("recovery_yaml_missing").and_then(Value::as_bool) == Some(false)
        }));
    }
}
