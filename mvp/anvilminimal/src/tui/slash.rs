use std::any::Any;
use std::panic::{AssertUnwindSafe, PanicHookInfo};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::bail;
use serde_json::json;

use crate::config::Config;
use crate::providers::ChatClient;

static PANIC_HOOK_LOCK: Mutex<()> = Mutex::new(());
type PanicHook = Box<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TuiTerminalStatus {
    Completed,
    Partial,
    Failed,
    Aborted,
    Interrupted,
}

impl TuiTerminalStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Partial => "partial",
            Self::Failed => "failed",
            Self::Aborted => "aborted",
            Self::Interrupted => "interrupted",
        }
    }

    fn ok(self) -> bool {
        matches!(self, Self::Completed | Self::Partial)
    }

    fn failure_kind(self) -> &'static str {
        match self {
            Self::Completed | Self::Partial => "",
            Self::Failed => "tui_command_failed",
            Self::Aborted => "tui_command_aborted",
            Self::Interrupted => "tui_command_interrupted",
        }
    }
}

struct TuiCommandCompletionGuard {
    config: Config,
    command: String,
    finalized: bool,
}

impl TuiCommandCompletionGuard {
    fn new(config: &Config, command: &str) -> Self {
        Self {
            config: config.clone(),
            command: command.to_string(),
            finalized: false,
        }
    }

    fn finalize(
        mut self,
        result: &anyhow::Result<String>,
        status: TuiTerminalStatus,
    ) -> crate::eval_events::CompletionProjection {
        let completion =
            emit_tui_command_stop_with_status(&self.config, &self.command, result, status);
        self.finalized = true;
        completion
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PanicDiagnostic {
    message: String,
    location: String,
}

struct PanicHookGuard {
    previous: Option<PanicHook>,
    _lock: MutexGuard<'static, ()>,
}

impl PanicHookGuard {
    fn install(diagnostic: Arc<Mutex<Option<PanicDiagnostic>>>) -> Self {
        let lock = PANIC_HOOK_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let mut diagnostic = diagnostic
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *diagnostic = Some(PanicDiagnostic::from_hook(info));
        }));
        Self {
            previous: Some(previous),
            _lock: lock,
        }
    }
}

impl Drop for PanicHookGuard {
    fn drop(&mut self) {
        let _ = std::panic::take_hook();
        if let Some(previous) = self.previous.take() {
            std::panic::set_hook(previous);
        }
    }
}

impl PanicDiagnostic {
    fn from_hook(info: &PanicHookInfo<'_>) -> Self {
        let location = info
            .location()
            .map(|location| {
                format!(
                    "{}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                )
            })
            .unwrap_or_else(|| "unknown".to_string());
        Self {
            message: panic_payload_message(info.payload()),
            location,
        }
    }

    fn from_payload(payload: &(dyn Any + Send)) -> Self {
        Self {
            message: panic_payload_message(payload),
            location: "unknown".to_string(),
        }
    }

    fn stop_reason(&self) -> String {
        format!("panic caught: {} at {}", self.message, self.location)
    }
}

impl Drop for TuiCommandCompletionGuard {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        let result: anyhow::Result<String> =
            Err(anyhow::anyhow!("command aborted before completion"));
        let _ = emit_tui_command_stop_with_status(
            &self.config,
            &self.command,
            &result,
            TuiTerminalStatus::Aborted,
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSlash {
    pub command: String,
    pub profile: String,
    pub profile_explicit: bool,
    pub profile_inference: Option<crate::planner::profile::ProfileInference>,
    pub style: String,
    pub goal: String,
}

pub fn parse_slash(line: &str, config: &Config) -> anyhow::Result<ParsedSlash> {
    let args = parse_words(line);
    let Some(command) = args.first() else {
        bail!("empty command");
    };
    let parsed_profile = parse_profile_style_details(&args[1..], config);
    let goal = expand_goal_references(&parsed_profile.rest.join(" "), config)?;
    let mut profile = parsed_profile.profile;
    let mut profile_inference = None;
    if !parsed_profile.profile_explicit
        && let Some(inference) =
            crate::planner::profile::infer_profile(Some(&goal), &config.workspace_root)
    {
        profile = inference.profile.to_string();
        profile_inference = Some(inference);
    }
    Ok(ParsedSlash {
        command: command.clone(),
        profile,
        profile_explicit: parsed_profile.profile_explicit,
        profile_inference,
        style: parsed_profile.style,
        goal,
    })
}

pub fn handle_command(
    line: &str,
    config: &Config,
    planner: &mut dyn ChatClient,
    execution: &mut dyn ChatClient,
    ui: &dyn crate::tui::InteractionUi,
) -> anyhow::Result<String> {
    let parsed = parse_slash(line, config)?;
    let mut config = config.clone();
    config.profile = parsed.profile;
    config.profile_explicit = parsed.profile_explicit;
    config.profile_inference = parsed.profile_inference;
    config.style = parsed.style;
    if let Some(inference) = config.profile_inference {
        crate::eval_events::emit(
            config.eval_events_path.as_deref(),
            json!({
                "event": "profile_inferred",
                "profile": inference.profile,
                "from": inference.source.as_str(),
                "lifecycle_stage": "tui_command",
                "command": &parsed.command,
            }),
        );
    }
    crate::eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "tui_command_start",
            "lifecycle_stage": "tui_command",
            "command": &parsed.command,
            "profile": &config.profile,
            "profile_inferred": config
                .profile_inference
                .map(|inference| inference.profile)
                .unwrap_or(""),
            "profile_inference_source": config
                .profile_inference
                .map(|inference| inference.source.as_str())
                .unwrap_or(""),
            "style": &config.style,
            "goal": crate::eval_events::body_snippet(&parsed.goal),
        }),
    );
    let completion_guard = TuiCommandCompletionGuard::new(&config, &parsed.command);
    let panic_diagnostic = Arc::new(Mutex::new(None));
    let panic_hook_guard = PanicHookGuard::install(Arc::clone(&panic_diagnostic));
    let command_result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        match parsed.command.as_str() {
            "/plan-steps" => {
                let plan =
                    crate::planner::generate_step_plan_with_ui(planner, &parsed.goal, &config, ui)?;
                let path = crate::planner::save_step_plan(&config.workspace_root, &plan)?;
                Ok(path.display().to_string())
            }
            "/plan-run" => crate::planner::generate_and_run_step_plan_with_ui(
                planner,
                execution,
                &parsed.goal,
                &config,
                ui,
            ),
            "/run-plan" => crate::planner::run_plan_file_with_ui(
                execution,
                Path::new(&parsed.goal),
                &config,
                ui,
            ),
            "/ultra-plan" => {
                let plan = crate::planner::generate_ultra_plan_with_ui(
                    planner,
                    &parsed.goal,
                    &config,
                    ui,
                )?;
                let path = crate::planner::save_ultra_plan(&config.workspace_root, &plan)?;
                Ok(path.display().to_string())
            }
            "/ultra-plan-run" => crate::planner::generate_and_run_ultra_plan_with_ui(
                planner,
                execution,
                &parsed.goal,
                &config,
                ui,
            ),
            "/run-ultra-plan" => crate::planner::run_ultra_plan_file_with_ui(
                planner,
                execution,
                Path::new(&parsed.goal),
                &config,
                ui,
            ),
            "/setup-interaction-probe" => {
                let report =
                crate::minimal_loop::interaction_probe::setup_interaction_probe_with_stdout_progress()?;
                Ok(report.summary_lines().join("\n"))
            }
            other => bail!("unknown slash command: {other}"),
        }
    }));
    drop(panic_hook_guard);
    let result = match command_result {
        Ok(result) => result,
        Err(payload) => {
            let diagnostic = panic_diagnostic
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
                .unwrap_or_else(|| PanicDiagnostic::from_payload(payload.as_ref()));
            emit_panic_caught(&config, &parsed.command, &diagnostic);
            let result = Err(anyhow::anyhow!(diagnostic.stop_reason()));
            let _ = completion_guard.finalize(&result, TuiTerminalStatus::Aborted);
            std::panic::resume_unwind(payload);
        }
    };
    let terminal_status = terminal_status_for_result(&result, ui);
    let completion = completion_guard.finalize(&result, terminal_status);
    if let Err(err) = &result {
        eprintln!(
            "{}",
            crate::eval_events::render_tui_command_failure_block(
                config.eval_events_path.as_deref(),
                &err.to_string(),
                &completion,
            )
        );
    } else if let Some(notice) = crate::eval_events::render_tui_command_incomplete_notice(
        config.eval_events_path.as_deref(),
        &completion,
    ) {
        eprintln!("{notice}");
    }
    result.map(|output| crate::eval_events::render_tui_completion_output(&output, &completion))
}

fn emit_panic_caught(config: &Config, command: &str, diagnostic: &PanicDiagnostic) {
    crate::eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "panic_caught",
            "lifecycle_stage": "tui_command",
            "command": command,
            "status": "aborted",
            "failure_kind": "panic",
            "message": crate::eval_events::body_snippet(&diagnostic.message),
            "location": diagnostic.location,
            "panic_message": crate::eval_events::body_snippet(&diagnostic.message),
            "panic_location": diagnostic.location,
        }),
    );
}

fn panic_payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

#[cfg(test)]
fn emit_tui_command_stop(
    config: &Config,
    command: &str,
    result: &anyhow::Result<String>,
) -> crate::eval_events::CompletionProjection {
    emit_tui_command_stop_with_status(config, command, result, status_without_ui(result))
}

fn emit_tui_command_stop_with_status(
    config: &Config,
    command: &str,
    result: &anyhow::Result<String>,
    terminal_status: TuiTerminalStatus,
) -> crate::eval_events::CompletionProjection {
    let requested_ok = terminal_status.ok();
    let mut completion_snapshot =
        crate::eval_events::latest_completion_snapshot(config.eval_events_path.as_deref());
    apply_config_completion_metadata(config, &mut completion_snapshot);
    let completion = crate::eval_events::project_completion(requested_ok, &completion_snapshot);
    let terminal_status = effective_terminal_status(terminal_status, &completion);
    let ok = terminal_status.ok();
    let failure_kind = terminal_status.failure_kind();
    let stop_reason = stop_reason_for_result(result, terminal_status);
    let event_projection = event_projection_for_terminal_status(&completion, terminal_status);
    crate::eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "tui_command_stop",
            "lifecycle_stage": "tui_command",
            "command": command,
            "ok": ok,
            "status": terminal_status.as_str(),
            "build_commit": crate::build_info::COMMIT,
            "build_dirty": crate::build_info::dirty(),
            "build_timestamp": crate::build_info::TIMESTAMP,
            "failure_kind": failure_kind,
            "stop_reason": stop_reason,
            "completion_status": &completion.status,
            "task_status": &event_projection.task_status,
            "profile": &completion.profile,
            "effective_profile": &completion.effective_profile,
            "contract_origin": &completion.contract_origin,
            "assurance_level": &completion.assurance_level,
            "assurance_reason": &completion.assurance_reason,
            "profile_inferred": &completion.profile_inferred,
            "profile_inference_source": &completion.profile_inference_source,
            "requested_port": &completion.requested_port,
            "session_status": "repl_ready",
            "repl_status": "ready",
            "command_completion_state": &event_projection.command_completion,
            "runtime_acceptance_status": &completion.runtime_acceptance,
            "final_acceptance_status": &completion.final_acceptance,
            "release_gate_status": &completion.release_gate,
            "completion_contract_verification_enabled": completion.completion_contract_verification_enabled,
            "completion_contract_path_merge_enabled": completion.completion_contract_path_merge_enabled,
            "completion_contract_path": &completion.completion_contract_path,
            "completion_contract_generated": completion.completion_contract_generated,
            "external_contract_checked": completion.external_contract_checked,
            "external_contract_ok": completion.external_contract_ok,
            "release_gate_reasons": &completion.release_gate_reasons,
            "browser_readiness_applicable": completion.browser_readiness_applicable,
            "browser_readiness_execution_status": &completion.browser_readiness_execution_status,
            "browser_readiness_status": &completion.browser_readiness,
            "browser_readiness_evidence_path": &completion.browser_readiness_evidence_path,
            "interaction_evidence_applicable": completion.interaction_evidence_applicable,
            "interaction_evidence_execution_status": &completion.interaction_evidence_execution_status,
            "interaction_evidence_status": &completion.interaction_evidence,
            "interaction_evidence_path": &completion.interaction_evidence_path,
            "state_dimensions_changed": &completion.state_dimensions_changed,
            "action_hooks": &completion.action_hooks,
            "text_entry_target": &completion.text_entry_target,
            "typed_token": &completion.typed_token,
            "token_echoed": &completion.token_echoed,
            "text_input_state_change": &completion.text_input_state_change,
            "release_quality_completion": &completion.release_quality_completion,
            "next_action": &event_projection.next_action,
            "recovery_next_action": &event_projection.next_action,
            "recovery_prompt_path": &completion.recovery_prompt_path,
            "recovery_ultra_plan_path": &completion.recovery_ultra_plan_path,
            "suggested_recovery_command": &completion.suggested_recovery_command,
            "suggested_recovery_yaml_command": &completion.suggested_recovery_yaml_command,
            "planner_verify_normalization_count": completion.planner_verify_normalization_count,
            "planner_retry_count": completion.planner_retry_count,
            "planner_quality_warning_count": completion.planner_quality_warning_count,
            "planner_quality_issue_count": completion.planner_quality_issue_count,
            "planner_repaired": completion.planner_repaired,
            "planner_release_risk": completion.planner_release_risk,
        }),
    );
    crate::eval_events::write_tui_command_completion_summary(
        config.eval_events_path.as_deref(),
        command,
        &stop_reason,
        failure_kind,
        terminal_status.as_str(),
        &completion,
    );
    if !ok {
        crate::eval_events::emit(
            config.eval_events_path.as_deref(),
            json!({
                "event": "loop_stop",
                "lifecycle_stage": "tui_command",
                "reason": failure_kind,
                "primary_reason": stop_reason,
            }),
        );
    }
    if ok { completion } else { event_projection }
}

fn apply_config_completion_metadata(
    config: &Config,
    snapshot: &mut crate::eval_events::CompletionSnapshot,
) {
    if let Some(inference) = config.profile_inference {
        snapshot.profile_inferred = inference.profile.to_string();
        snapshot.profile_inference_source = inference.source.as_str().to_string();
    }
    if snapshot.profile.trim().is_empty() {
        snapshot.profile = config.profile.clone();
    }
    if crate::planner::profile::canonical_profile_name(&snapshot.profile) == "generic" {
        if snapshot.assurance_level == "static" {
            snapshot.assurance_reason =
                crate::eval_events::GENERIC_STATIC_ASSURANCE_REASON.to_string();
        } else {
            snapshot.assurance_level = "reduced".to_string();
            snapshot.assurance_reason =
                crate::eval_events::GENERIC_REDUCED_ASSURANCE_REASON.to_string();
        }
    } else {
        snapshot.assurance_level = "full".to_string();
        snapshot.assurance_reason.clear();
    }
}

fn terminal_status_for_result(
    result: &anyhow::Result<String>,
    ui: &dyn crate::tui::InteractionUi,
) -> TuiTerminalStatus {
    match result {
        Ok(_) => TuiTerminalStatus::Completed,
        Err(err) if ui.interrupted() || error_is_interrupted(err) => TuiTerminalStatus::Interrupted,
        Err(_) => TuiTerminalStatus::Failed,
    }
}

#[cfg(test)]
fn status_without_ui(result: &anyhow::Result<String>) -> TuiTerminalStatus {
    match result {
        Ok(_) => TuiTerminalStatus::Completed,
        Err(err) if error_is_interrupted(err) => TuiTerminalStatus::Interrupted,
        Err(_) => TuiTerminalStatus::Failed,
    }
}

fn stop_reason_for_result(
    result: &anyhow::Result<String>,
    terminal_status: TuiTerminalStatus,
) -> String {
    match result {
        Ok(_) => "completed".to_string(),
        Err(err) => {
            let reason = crate::eval_events::render_stop_reason_text(&err.to_string());
            if terminal_status == TuiTerminalStatus::Interrupted && reason.trim().is_empty() {
                "interrupted by user".to_string()
            } else {
                reason
            }
        }
    }
}

fn effective_terminal_status(
    requested: TuiTerminalStatus,
    completion: &crate::eval_events::CompletionProjection,
) -> TuiTerminalStatus {
    if requested == TuiTerminalStatus::Completed
        && completion.release_gate == "partial"
        && completion
            .release_gate_reasons
            .iter()
            .any(|reason| reason.contains("interaction_unverified:probe_unavailable"))
    {
        TuiTerminalStatus::Partial
    } else {
        requested
    }
}

fn error_is_interrupted(err: &anyhow::Error) -> bool {
    err.to_string().contains("interrupted by user")
}

fn event_projection_for_terminal_status(
    completion: &crate::eval_events::CompletionProjection,
    terminal_status: TuiTerminalStatus,
) -> crate::eval_events::CompletionProjection {
    let mut projection = completion.clone();
    if matches!(
        terminal_status,
        TuiTerminalStatus::Completed | TuiTerminalStatus::Partial
    ) {
        return projection;
    }
    projection.status = terminal_status.as_str().to_string();
    projection.command_completion = terminal_status.as_str().to_string();
    projection.task_status = terminal_status.as_str().to_string();
    projection.next_action = match terminal_status {
        TuiTerminalStatus::Interrupted => "resume_or_rerun_command".to_string(),
        TuiTerminalStatus::Aborted => "inspect_summary_and_resume_or_rerun".to_string(),
        TuiTerminalStatus::Failed => "fix_command_failure".to_string(),
        TuiTerminalStatus::Completed | TuiTerminalStatus::Partial => projection.next_action,
    };
    projection
}

pub fn parse_profile_style(args: &[String], config: &Config) -> (String, String, Vec<String>) {
    let parsed = parse_profile_style_details(args, config);
    (parsed.profile, parsed.style, parsed.rest)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedProfileStyle {
    profile: String,
    profile_explicit: bool,
    style: String,
    rest: Vec<String>,
}

fn parse_profile_style_details(args: &[String], config: &Config) -> ParsedProfileStyle {
    let mut profile = config.profile.clone();
    let mut profile_explicit = config.profile_explicit;
    let mut style = config.style.clone();
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--profile" if i + 1 < args.len() => {
                profile = args[i + 1].clone();
                profile_explicit = true;
                i += 2;
            }
            "--style" if i + 1 < args.len() => {
                style = args[i + 1].clone();
                i += 2;
            }
            _ => {
                rest.push(args[i].clone());
                i += 1;
            }
        }
    }
    ParsedProfileStyle {
        profile,
        profile_explicit,
        style,
        rest,
    }
}

pub fn expand_goal_references(goal: &str, config: &Config) -> anyhow::Result<String> {
    let mut out = goal.to_string();
    while let Some(start) = out.find("$(cat ") {
        let Some(end_rel) = out[start..].find(')') else {
            break;
        };
        let end = start + end_rel;
        let raw = out[start + "$(cat ".len()..end].trim();
        let path = crate::tools::path_guard::resolve_existing(&config.workspace_root, raw)?;
        let content = std::fs::read_to_string(path)?;
        out.replace_range(start..=end, &content);
    }
    Ok(out)
}

pub fn parse_words(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in input.chars() {
        match ch {
            '"' => quoted = !quoted,
            ' ' | '\t' if !quoted => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            workspace_root: std::path::PathBuf::from("."),
            state_dir: std::path::PathBuf::from("state"),
            eval_events_path: None,
            completion_contract_path: None,
            yes: true,
            offline: false,
            context_budget: 1000,
            model: "m".to_string(),
            provider: crate::config::Provider::Ollama,
            planner_model: "m".to_string(),
            planner_provider: crate::config::Provider::Ollama,
            ollama_host: "http://localhost:11434".to_string(),
            num_predict: 100,
            max_iterations: 4,
            chat_timeout_secs: 1,
            chat_retries: 1,
            resume: None,
            fresh_session: false,
            no_footer: false,
            profile: "generic".to_string(),
            profile_explicit: false,
            profile_inference: None,
            style: "default".to_string(),
            action: crate::config::Action::Repl,
        }
    }

    #[test]
    fn slash_command_parse_quoted_prompt() {
        let words = parse_words(r#"/ultra-plan-run --profile nextjs "3011 port app""#);
        assert_eq!(words[0], "/ultra-plan-run");
        assert_eq!(words[2], "nextjs");
        assert_eq!(words[3], "3011 port app");
    }

    #[test]
    fn slash_command_parse_profile_style_and_goal() {
        let words = parse_words(r#"/plan-run --profile nextjs --style compact "3011 port app""#);
        let (profile, style, rest) = parse_profile_style(&words[1..], &config());
        assert_eq!(profile, "nextjs");
        assert_eq!(style, "compact");
        assert_eq!(rest, vec!["3011 port app"]);
    }

    #[test]
    fn parse_slash_uses_config_defaults() {
        let parsed = parse_slash("/plan-run goal", &config()).unwrap();
        assert_eq!(parsed.profile, "generic");
        assert_eq!(parsed.style, "default");
        assert_eq!(parsed.goal, "goal");
    }

    #[test]
    fn parse_slash_infers_profile_from_goal_when_unset() {
        let parsed = parse_slash("/ultra-plan-run Web アプリを作って", &config()).unwrap();
        assert_eq!(parsed.profile, "nextjs");
        assert_eq!(
            parsed
                .profile_inference
                .expect("profile inference")
                .source
                .as_str(),
            "goal"
        );
    }

    #[test]
    fn parse_slash_explicit_generic_wins_over_inference() {
        let parsed = parse_slash(
            "/ultra-plan-run --profile generic Web アプリを作って",
            &config(),
        )
        .unwrap();
        assert_eq!(parsed.profile, "generic");
        assert!(parsed.profile_explicit);
        assert!(parsed.profile_inference.is_none());
    }

    #[test]
    fn tui_command_stop_marks_generic_completion_reduced_assurance() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        let events = workspace.join(".anvil/runs/test/events.jsonl");
        std::fs::create_dir_all(events.parent().unwrap()).unwrap();
        let mut cfg = config();
        cfg.workspace_root = workspace;
        cfg.eval_events_path = Some(events.clone());

        let result: anyhow::Result<String> = Ok("done".to_string());
        let projection = emit_tui_command_stop(&cfg, "/ultra-plan-run", &result);

        assert_eq!(projection.assurance_level, "reduced");
        assert_eq!(projection.task_status, "completed (reduced assurance)");
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(event_text.contains(r#""assurance_level":"reduced""#));
        assert!(event_text.contains(r#""task_status":"completed (reduced assurance)""#));
        let summary = std::fs::read_to_string(events.parent().unwrap().join("summary.md")).unwrap();
        assert!(summary.contains(
            "Assurance: reduced (generic profile — no capability contract, no behavioral verification)"
        ));
    }

    #[test]
    fn tui_command_stop_preserves_generic_static_assurance() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        let events = workspace.join(".anvil/runs/test/events.jsonl");
        std::fs::create_dir_all(events.parent().unwrap()).unwrap();
        let mut cfg = config();
        cfg.workspace_root = workspace;
        cfg.eval_events_path = Some(events.clone());
        crate::eval_events::emit(
            cfg.eval_events_path.as_deref(),
            serde_json::json!({
                "event": "ultra_final_acceptance",
                "runtime_acceptance_status": "pass",
                "final_acceptance_status": "full_success",
                "release_gate_status": "pass",
                "assurance_level": "static",
                "assurance_reason": crate::eval_events::GENERIC_STATIC_ASSURANCE_REASON,
            }),
        );

        let result: anyhow::Result<String> = Ok("done".to_string());
        let projection = emit_tui_command_stop(&cfg, "/ultra-plan-run", &result);

        assert_eq!(projection.assurance_level, "static");
        assert_eq!(projection.task_status, "completed (static assurance)");
        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(event_text.contains(r#""assurance_level":"static""#));
        assert!(event_text.contains(r#""task_status":"completed (static assurance)""#));
        let summary = std::fs::read_to_string(events.parent().unwrap().join("summary.md")).unwrap();
        assert!(summary.contains(
            "Assurance: static (generic profile — minimal interactive contract verified statically; no behavioral verification)"
        ));
    }

    #[test]
    fn tui_command_stop_uses_phase_scoped_recovery_paths() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        let events = workspace.join(".anvil/runs/test/events.jsonl");
        let prompt_path = workspace.join(".anvil/repairs/repair-phase.md");
        let yaml_path = workspace.join(".anvil/plans/recovery-phase.yaml");
        std::fs::create_dir_all(events.parent().unwrap()).unwrap();
        let mut cfg = config();
        cfg.workspace_root = workspace.clone();
        cfg.eval_events_path = Some(events.clone());
        crate::eval_events::emit(
            cfg.eval_events_path.as_deref(),
            serde_json::json!({
                "event": "recovery_prompt_saved",
                "recovery_prompt_path": prompt_path.display().to_string(),
                "recovery_ultra_plan_path": yaml_path.display().to_string(),
                "suggested_recovery_command": format!("/ultra-plan-run --profile nextjs \"$(cat {})\"", prompt_path.display()),
                "suggested_recovery_yaml_command": format!("/run-ultra-plan {}", yaml_path.display()),
            }),
        );

        let result: anyhow::Result<String> = Err(anyhow::anyhow!("phase invariant failed"));
        let projection = emit_tui_command_stop(&cfg, "/ultra-plan-run", &result);

        assert_eq!(
            projection.recovery_prompt_path,
            ".anvil/repairs/repair-phase.md"
        );
        assert_eq!(
            projection.recovery_ultra_plan_path,
            ".anvil/plans/recovery-phase.yaml"
        );
        let event_text = std::fs::read_to_string(&events).unwrap();
        let tui_stop = event_text
            .lines()
            .rfind(|line| line.contains(r#""event":"tui_command_stop""#))
            .unwrap();
        assert!(
            tui_stop.contains(r#""recovery_prompt_path":".anvil/repairs/repair-phase.md""#),
            "{tui_stop}"
        );
        assert!(
            tui_stop.contains(r#""recovery_ultra_plan_path":".anvil/plans/recovery-phase.yaml""#),
            "{tui_stop}"
        );
        assert!(
            tui_stop.contains(&format!(
                r#""build_commit":"{}""#,
                crate::build_info::COMMIT
            )),
            "{tui_stop}"
        );
        assert!(tui_stop.contains(r#""suggested_recovery_command":"/ultra-plan-run"#));
        assert!(tui_stop.contains(r#""suggested_recovery_yaml_command":"/run-ultra-plan"#));
    }

    #[test]
    fn tui_command_stop_reports_interaction_unverified_as_partial() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("workspace");
        let events = workspace.join(".anvil/runs/test/events.jsonl");
        std::fs::create_dir_all(events.parent().unwrap()).unwrap();
        let mut cfg = config();
        cfg.workspace_root = workspace;
        cfg.eval_events_path = Some(events.clone());
        crate::eval_events::emit(
            cfg.eval_events_path.as_deref(),
            serde_json::json!({
                "event": "ultra_final_acceptance",
                "profile": "nextjs",
                "effective_profile": "nextjs",
                "contract_origin": "promoted_union",
                "runtime_acceptance_status": "pass",
                "final_acceptance_status": "partial",
                "release_gate_status": "partial",
                "release_gate_reasons": [
                    "interaction_unverified:probe_unavailable",
                    crate::minimal_loop::interaction_probe::INTERACTION_PROBE_SETUP_REMEDIATION
                ],
                "browser_readiness_applicable": true,
                "browser_readiness_execution_status": "performed",
                "browser_readiness_status": "passed",
                "browser_readiness_evidence_path": "browser-readiness.json",
                "interaction_evidence_applicable": true,
                "interaction_evidence_execution_status": "unavailable",
                "interaction_evidence_status": "unavailable:playwright_not_installed",
                "interaction_evidence_path": "",
                "completion_contract_verification_enabled": true,
                "completion_contract_path_merge_enabled": true,
                "external_contract_checked": true,
                "external_contract_ok": true,
            }),
        );

        let result: anyhow::Result<String> = Ok("ultra-plan-run complete".to_string());
        let projection = emit_tui_command_stop(&cfg, "/ultra-plan-run", &result);

        assert_eq!(projection.release_gate, "partial");
        assert_eq!(projection.task_status, "partial (interaction unverified)");
        assert_eq!(
            projection.next_action,
            "run_setup_interaction_probe_to_enable_interaction_release_checks"
        );
        let event_text = std::fs::read_to_string(&events).unwrap();
        let tui_stop = event_text
            .lines()
            .rfind(|line| line.contains(r#""event":"tui_command_stop""#))
            .unwrap();
        assert!(tui_stop.contains(r#""status":"partial""#), "{tui_stop}");
        assert!(
            tui_stop.contains(r#""task_status":"partial (interaction unverified)""#),
            "{tui_stop}"
        );
        assert!(
            tui_stop.contains(r#""effective_profile":"nextjs""#),
            "{tui_stop}"
        );
        assert!(
            tui_stop.contains(r#""contract_origin":"promoted_union""#),
            "{tui_stop}"
        );
        assert!(
            tui_stop.contains(r#""browser_readiness_applicable":true"#),
            "{tui_stop}"
        );
        assert!(
            tui_stop.contains(r#""browser_readiness_execution_status":"performed""#),
            "{tui_stop}"
        );
        assert!(
            tui_stop.contains(r#""interaction_evidence_applicable":true"#),
            "{tui_stop}"
        );
        assert!(
            tui_stop.contains(r#""interaction_evidence_execution_status":"unavailable""#),
            "{tui_stop}"
        );
        let summary = std::fs::read_to_string(events.with_file_name("summary.md")).unwrap();
        assert!(summary.contains("Status: partial"), "{summary}");
        assert!(
            summary.contains("interaction_unverified:probe_unavailable"),
            "{summary}"
        );
        assert!(
            summary.contains(
                crate::minimal_loop::interaction_probe::INTERACTION_PROBE_SETUP_REMEDIATION
            ),
            "{summary}"
        );
    }

    #[test]
    fn parses_user_ultra_plan_run_nextjs_command() {
        let parsed = parse_slash(
            "/ultra-plan-run --profile nextjs あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。",
            &config(),
        )
        .unwrap();
        assert_eq!(parsed.command, "/ultra-plan-run");
        assert_eq!(parsed.profile, "nextjs");
        assert_eq!(
            parsed.goal,
            "あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。"
        );
    }
}
