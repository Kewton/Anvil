use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, bail};
use serde_json::Value;

use crate::planner::ultra_plan::{UltraPlan, parse_ultra_plan};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunInventoryItem {
    pub id: String,
    pub short_id: String,
    pub started_at: String,
    pub status: String,
    pub assurance: String,
    pub completed_phases: Option<usize>,
    pub total_phases: Option<usize>,
    pub stop_reason: String,
    pub recovery: String,
    pub recovery_ultra_plan_path: String,
    pub events_path: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveryMetadata {
    pub schema_version: Option<String>,
    pub original_goal: Option<String>,
    pub failure_kind: Option<String>,
    pub profile: Option<String>,
    pub failed_phase: Option<String>,
    pub failed_step: Option<String>,
    pub recovered_from_run_id: Option<String>,
    pub expected_completed_artifacts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumePlan {
    pub resumed_from: String,
    pub yaml_path: PathBuf,
    pub yaml_display_path: String,
    pub plan: UltraPlan,
    pub metadata: RecoveryMetadata,
    pub original_goal: String,
    pub completed_phase_ids: Vec<String>,
    pub phases_to_run: Vec<String>,
    pub failure_kind: String,
    pub effective_profile: String,
    pub requested_port: String,
    pub workspace_gaps: Vec<String>,
    pub compatibility_notes: Vec<String>,
}

pub fn recent_runs(root: &Path, cap: usize) -> Vec<RunInventoryItem> {
    let runs_dir = root.join(".anvil").join("runs");
    let Ok(entries) = std::fs::read_dir(&runs_dir) else {
        return Vec::new();
    };
    let mut candidates = entries
        .filter_map(Result::ok)
        .filter_map(|entry| run_inventory_item(root, &entry.path()))
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        run_sort_key(&b.events_path)
            .cmp(&run_sort_key(&a.events_path))
            .then_with(|| b.id.cmp(&a.id))
    });
    candidates.truncate(cap);
    candidates
}

pub fn render_runs_table(root: &Path) -> String {
    let runs = recent_runs(root, 10);
    if runs.is_empty() {
        return "No runs found for this workspace.".to_string();
    }
    let mut lines = vec![format!(
        "{:<10} {:<16} {:<24} {:<8} {:<9} {}",
        "RUN", "STARTED", "STATUS/ASSURANCE", "PHASES", "RECOVERY", "STOP"
    )];
    for run in runs {
        lines.push(format!(
            "{:<10} {:<16} {:<24} {:<8} {:<9} {}",
            run.short_id,
            run.started_at,
            format!("{}/{}", run.status, run.assurance),
            phase_display(run.completed_phases, run.total_phases),
            run.recovery,
            fit_one_line(&run.stop_reason, 96),
        ));
    }
    lines.join("\n")
}

pub fn newest_run_with_recovery(root: &Path) -> Option<RunInventoryItem> {
    recent_runs(root, 50)
        .into_iter()
        .find(|run| !run.recovery_ultra_plan_path.is_empty())
}

pub fn find_run(root: &Path, id_or_prefix: &str) -> Option<RunInventoryItem> {
    let needle = id_or_prefix.trim();
    if needle.is_empty() {
        return None;
    }
    recent_runs(root, 100)
        .into_iter()
        .find(|run| run.id == needle || run.short_id == needle || run.id.starts_with(needle))
}

pub fn prepare_resume(root: &Path, target: &str) -> anyhow::Result<ResumePlan> {
    let target = target.trim();
    let source = if target.is_empty() {
        let run = newest_run_with_recovery(root)
            .ok_or_else(|| anyhow::anyhow!("no recovery UltraPlan found; run /runs"))?;
        ResumeSource::Run(Box::new(run))
    } else if resume_target_looks_like_path(target) {
        ResumeSource::Yaml(
            resolve_resume_yaml_path(root, Path::new(target)).with_context(|| {
                format!("failed to resolve recovery UltraPlan path `{target}`; run /runs")
            })?,
        )
    } else if let Some(run) = find_run(root, target) {
        ResumeSource::Run(Box::new(run))
    } else {
        bail!("run `{target}` not found; run /runs");
    };
    resume_plan_from_source(root, source)
}

pub fn emit_resume_start(config: &crate::config::Config, resume: &ResumePlan) {
    crate::eval_events::emit(
        config.eval_events_path.as_deref(),
        serde_json::json!({
            "event": "resume_start",
            "lifecycle_stage": "tui_command",
            "resumed_from": &resume.resumed_from,
            "resume_recovery_ultra_plan_path": &resume.yaml_display_path,
            "resume_original_goal": crate::eval_events::body_snippet(&resume.original_goal),
            "resume_failure_kind": &resume.failure_kind,
            "resume_completed_phase_ids": &resume.completed_phase_ids,
            "resume_phase_ids": &resume.phases_to_run,
            "resume_effective_profile": &resume.effective_profile,
            "resume_requested_port": &resume.requested_port,
        }),
    );
}

pub fn latest_resumed_from(events_path: Option<&Path>) -> Option<String> {
    let path = events_path?;
    read_events(path)
        .into_iter()
        .rev()
        .find(|event| event.get("event").and_then(Value::as_str) == Some("resume_start"))
        .and_then(|event| latest_text(&event, &["resumed_from"]))
}

impl ResumePlan {
    pub fn confirmation_card(&self) -> String {
        let mut lines = vec![
            "### Resume recovery run".to_string(),
            format!("- resumed from: {}", self.resumed_from),
            format!("- recovery yaml: {}", self.yaml_display_path),
            format!("- original goal: {}", self.original_goal),
            format!(
                "- completed phases skipped: {}",
                display_list_or(&self.completed_phase_ids, "none recorded")
            ),
            format!(
                "- phases to run: {}",
                display_list_or(&self.phases_to_run, "none")
            ),
            format!("- recovering failure: {}", self.failure_kind),
            format!("- effective profile: {}", self.effective_profile),
            format!("- port: {}", self.requested_port),
        ];
        if self.workspace_gaps.is_empty() {
            lines.push("- workspace drift: none detected".to_string());
        } else {
            lines.push(format!(
                "- workspace drift: missing {}",
                display_list_or(&self.workspace_gaps, "none")
            ));
            lines.push(format!(
                "- suggestion: re-run from phase {} instead of blind resume",
                self.phases_to_run
                    .first()
                    .map(String::as_str)
                    .unwrap_or("the first remaining phase")
            ));
        }
        for note in &self.compatibility_notes {
            lines.push(format!("- note: {note}"));
        }
        lines.push("- confirmation: y/N".to_string());
        lines.join("\n")
    }

    pub fn workspace_drift_error(&self) -> Option<String> {
        (!self.workspace_gaps.is_empty()).then(|| {
            format!(
                "{}\n\nworkspace drift detected; refusing blind resume",
                self.confirmation_card()
            )
        })
    }
}

enum ResumeSource {
    Run(Box<RunInventoryItem>),
    Yaml(PathBuf),
}

fn resume_plan_from_source(root: &Path, source: ResumeSource) -> anyhow::Result<ResumePlan> {
    let (source_run, yaml_path) = match source {
        ResumeSource::Run(run) => {
            let yaml_path =
                resolve_resume_yaml_path(root, Path::new(&run.recovery_ultra_plan_path))
                    .with_context(|| {
                        format!(
                            "failed to resolve recovery UltraPlan for run `{}`; run /runs",
                            run.short_id
                        )
                    })?;
            (Some(*run), yaml_path)
        }
        ResumeSource::Yaml(path) => {
            let source_run = find_run_by_recovery_yaml(root, &path);
            (source_run, path)
        }
    };
    let text = std::fs::read_to_string(&yaml_path).with_context(|| {
        format!(
            "failed to read recovery UltraPlan YAML `{}`; run /runs",
            yaml_path.display()
        )
    })?;
    let plan = parse_ultra_plan(&text).with_context(|| {
        format!(
            "failed to parse recovery UltraPlan YAML `{}`; run /runs",
            yaml_path.display()
        )
    })?;
    let metadata = parse_recovery_metadata(&text);
    let yaml_display_path = workspace_relative_display(root, &yaml_path);
    let run_events = source_run.as_ref().map(|run| read_events(&run.events_path));
    let completed_phase_ids = run_events
        .as_deref()
        .and_then(completed_phase_ids)
        .unwrap_or_default();
    let failure_kind = metadata
        .failure_kind
        .clone()
        .or_else(|| run_events.as_deref().and_then(latest_failure_kind))
        .unwrap_or_else(|| "not recorded".to_string());
    let effective_profile = run_events
        .as_deref()
        .and_then(|events| latest_events_text(events, &["effective_profile", "profile"]))
        .or_else(|| metadata.profile.clone())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| plan.profile.clone());
    let requested_port = run_events
        .as_deref()
        .and_then(|events| latest_events_text(events, &["requested_port"]))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "not recorded".to_string());
    let original_goal = metadata
        .original_goal
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| plan.goal.clone());
    let phases_to_run = plan
        .phases
        .iter()
        .map(|phase| phase.id.clone())
        .collect::<Vec<_>>();
    let workspace_gaps = metadata
        .expected_completed_artifacts
        .iter()
        .filter(|path| !artifact_exists(root, path))
        .cloned()
        .collect::<Vec<_>>();
    let mut compatibility_notes = Vec::new();
    if metadata.schema_version.is_none() {
        compatibility_notes.push(
            "old recovery YAML has no metadata; profile/failure context is derived from the plan or run events"
                .to_string(),
        );
    }
    if completed_phase_ids.is_empty() {
        compatibility_notes.push(
            "completed phase list is not recorded; only recovery-plan phases will run".to_string(),
        );
    }
    let resumed_from = source_run
        .as_ref()
        .map(|run| run.short_id.clone())
        .or_else(|| metadata.recovered_from_run_id.clone())
        .unwrap_or_else(|| format!("yaml:{yaml_display_path}"));
    Ok(ResumePlan {
        resumed_from,
        yaml_path,
        yaml_display_path,
        plan,
        metadata,
        original_goal,
        completed_phase_ids,
        phases_to_run,
        failure_kind,
        effective_profile,
        requested_port,
        workspace_gaps,
        compatibility_notes,
    })
}

fn run_inventory_item(root: &Path, dir: &Path) -> Option<RunInventoryItem> {
    if !dir.is_dir() {
        return None;
    }
    let id = dir.file_name()?.to_string_lossy().to_string();
    let events_path = dir.join("events.jsonl");
    if !events_path.is_file() {
        return None;
    }
    let events = read_events(&events_path);
    let stop = latest_terminal_event(&events);
    let snapshot = crate::eval_events::latest_completion_snapshot(Some(&events_path));
    let ok = stop
        .and_then(|event| event.get("ok").and_then(Value::as_bool))
        .unwrap_or(false);
    let projection = crate::eval_events::project_completion(ok, &snapshot);
    let status = stop
        .and_then(|event| event.get("status").and_then(Value::as_str))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&projection.status)
        .to_string();
    let assurance = if projection.assurance_level.trim().is_empty() {
        "missing".to_string()
    } else {
        projection.assurance_level.clone()
    };
    let (completed_phases, total_phases) = phase_counts(&events);
    let stop_reason = stop
        .and_then(|event| latest_text(event, &["stop_reason", "primary_reason", "reason"]))
        .or_else(|| latest_events_text(&events, &["stop_reason", "primary_reason", "reason"]))
        .unwrap_or_else(|| "unknown".to_string());
    let recovery_ultra_plan_path = projection.recovery_ultra_plan_path.clone();
    let recovery_prompt_path = projection.recovery_prompt_path.clone();
    let recovery = recovery_display(root, &recovery_prompt_path, &recovery_ultra_plan_path);
    Some(RunInventoryItem {
        short_id: crate::util::truncate_at_char_boundary(&id, 8).to_string(),
        id,
        started_at: started_at_display(&events_path),
        status,
        assurance,
        completed_phases,
        total_phases,
        stop_reason,
        recovery,
        recovery_ultra_plan_path,
        events_path,
    })
}

fn find_run_by_recovery_yaml(root: &Path, yaml_path: &Path) -> Option<RunInventoryItem> {
    let canonical = yaml_path.canonicalize().ok()?;
    recent_runs(root, 100).into_iter().find(|run| {
        resolve_resume_yaml_path(root, Path::new(&run.recovery_ultra_plan_path))
            .ok()
            .and_then(|path| path.canonicalize().ok())
            .is_some_and(|path| path == canonical)
    })
}

fn read_events(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

fn latest_terminal_event(events: &[Value]) -> Option<&Value> {
    events.iter().rev().find(|event| {
        matches!(
            event.get("event").and_then(Value::as_str),
            Some("tui_command_stop" | "run_stop")
        )
    })
}

fn latest_events_text(events: &[Value], keys: &[&str]) -> Option<String> {
    events
        .iter()
        .rev()
        .find_map(|event| latest_text(event, keys))
}

fn latest_text(event: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        event
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn phase_counts(events: &[Value]) -> (Option<usize>, Option<usize>) {
    for event in events.iter().rev() {
        let completed = event
            .get("completed_phase_ids")
            .and_then(Value::as_array)
            .map(Vec::len)
            .or_else(|| {
                event
                    .get("completed_phase_count")
                    .and_then(Value::as_u64)
                    .map(|value| value as usize)
            });
        let pending = event
            .get("pending_phase_ids")
            .and_then(Value::as_array)
            .map(Vec::len);
        if completed.is_some() || pending.is_some() {
            let total = completed.zip(pending).map(|(done, todo)| {
                done + todo
                    + usize::from(
                        event
                            .get("failed_phase_id")
                            .and_then(Value::as_str)
                            .is_some_and(|value| !value.is_empty()),
                    )
            });
            return (completed, total);
        }
        if let Some(total) = event.get("total_phases").and_then(Value::as_u64) {
            let index = event
                .get("phase_index")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let event_name = event.get("event").and_then(Value::as_str).unwrap_or("");
            let completed = if matches!(
                event_name,
                "ultra_phase_complete" | "ultra_phase_execute_complete" | "ultra_plan_complete"
            ) {
                Some(index)
            } else if event_name == "ultra_phase_failed" {
                Some(index.saturating_sub(1))
            } else {
                None
            };
            return (completed, Some(total as usize));
        }
    }
    (None, None)
}

fn completed_phase_ids(events: &[Value]) -> Option<Vec<String>> {
    events.iter().rev().find_map(|event| {
        event
            .get("completed_phase_ids")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .filter(|values| !values.is_empty())
    })
}

fn latest_failure_kind(events: &[Value]) -> Option<String> {
    latest_events_text(
        events,
        &["failure_kind", "stop_reason", "primary_reason", "reason"],
    )
}

fn phase_display(completed: Option<usize>, total: Option<usize>) -> String {
    match (completed, total) {
        (Some(done), Some(total)) => format!("{done}/{total}"),
        (Some(done), None) => format!("{done}/?"),
        (None, Some(total)) => format!("?/{total}"),
        (None, None) => "?/?".to_string(),
    }
}

fn recovery_display(root: &Path, prompt: &str, yaml: &str) -> String {
    let prompt_exists = artifact_exists(root, prompt);
    let yaml_exists = artifact_exists(root, yaml);
    match (prompt_exists, yaml_exists) {
        (_, true) => "yaml".to_string(),
        (true, false) => "prompt".to_string(),
        (false, false) if !yaml.trim().is_empty() => "yaml-missing".to_string(),
        (false, false) if !prompt.trim().is_empty() => "prompt-missing".to_string(),
        _ => "none".to_string(),
    }
}

fn artifact_exists(root: &Path, value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return false;
    }
    let path = Path::new(value);
    if path.is_absolute() {
        path.is_file()
    } else {
        root.join(path).is_file()
    }
}

fn resolve_resume_yaml_path(root: &Path, path: &Path) -> anyhow::Result<PathBuf> {
    let root = root.canonicalize()?;
    let canonical = if path.is_absolute() {
        path.canonicalize()?
    } else {
        let raw = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("recovery UltraPlan path is not valid UTF-8"))?;
        crate::tools::path_guard::resolve_existing(&root, raw)?
    };
    if !canonical.starts_with(&root) {
        bail!("recovery UltraPlan path escapes workspace");
    }
    if !canonical.is_file() {
        bail!("recovery UltraPlan path is not a file");
    }
    Ok(canonical)
}

fn resume_target_looks_like_path(target: &str) -> bool {
    target.contains('/')
        || target.contains('\\')
        || target.ends_with(".yaml")
        || target.ends_with(".yml")
        || Path::new(target).exists()
}

fn started_at_display(path: &Path) -> String {
    let secs = run_sort_key(path);
    if secs == 0 {
        "unknown".to_string()
    } else {
        format!("unix:{secs}")
    }
}

fn run_sort_key(path: &Path) -> u64 {
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn fit_one_line(value: &str, max: usize) -> String {
    let one_line = value.replace(['\n', '\r'], " ");
    crate::util::excerpt_with_marker(&one_line, max, "...")
}

fn parse_recovery_metadata(text: &str) -> RecoveryMetadata {
    let mut metadata = RecoveryMetadata::default();
    let mut in_expected_artifacts = false;
    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(value) = metadata_scalar(trimmed, "recovery_schema_version") {
            metadata.schema_version = Some(value);
            in_expected_artifacts = false;
        } else if let Some(value) = metadata_scalar(trimmed, "recovery_original_goal") {
            metadata.original_goal = Some(value);
            in_expected_artifacts = false;
        } else if let Some(value) = metadata_scalar(trimmed, "recovery_failure_kind") {
            metadata.failure_kind = Some(value);
            in_expected_artifacts = false;
        } else if let Some(value) = metadata_scalar(trimmed, "recovery_profile") {
            metadata.profile = Some(value);
            in_expected_artifacts = false;
        } else if let Some(value) = metadata_scalar(trimmed, "recovery_failed_phase") {
            metadata.failed_phase = Some(value);
            in_expected_artifacts = false;
        } else if let Some(value) = metadata_scalar(trimmed, "recovery_failed_step") {
            metadata.failed_step = Some(value);
            in_expected_artifacts = false;
        } else if let Some(value) = metadata_scalar(trimmed, "recovered_from_run_id") {
            metadata.recovered_from_run_id = Some(value);
            in_expected_artifacts = false;
        } else if trimmed == "recovery_expected_completed_artifacts:" {
            in_expected_artifacts = true;
        } else if in_expected_artifacts {
            if let Some(value) = trimmed.strip_prefix("- ") {
                metadata
                    .expected_completed_artifacts
                    .push(unquote_yaml_scalar(value.trim()));
            } else if !raw.starts_with(' ') {
                in_expected_artifacts = false;
            }
        }
    }
    metadata
}

fn metadata_scalar(line: &str, key: &str) -> Option<String> {
    line.strip_prefix(key)
        .and_then(|rest| rest.strip_prefix(':'))
        .map(str::trim)
        .map(unquote_yaml_scalar)
        .filter(|value| !value.trim().is_empty())
}

fn unquote_yaml_scalar(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        serde_json::from_str(value).unwrap_or_else(|_| value.trim_matches('"').to_string())
    } else {
        value.trim_matches('"').trim_matches('\'').to_string()
    }
}

fn workspace_relative_display(root: &Path, path: &Path) -> String {
    if let Ok(root) = root.canonicalize()
        && let Ok(path) = path.canonicalize()
        && let Ok(relative) = path.strip_prefix(root)
    {
        return relative.display().to_string();
    }
    path.display().to_string()
}

fn display_list_or(values: &[String], fallback: &str) -> String {
    if values.is_empty() {
        fallback.to_string()
    } else {
        values.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn runs_inventory_lists_newest_recoverable_run() {
        let dir = tempfile::tempdir().unwrap();
        let plan = dir
            .path()
            .join(".anvil/plans/recovery-ultra-plan-phase-a.yaml");
        std::fs::create_dir_all(plan.parent().unwrap()).unwrap();
        std::fs::write(
            &plan,
            "goal: \"g\"\nphases:\n  - id: \"p\"\n    prompt: \"p\"\n",
        )
        .unwrap();
        let run_dir = dir.path().join(".anvil/runs/018f1111-aaaa");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(
            run_dir.join("events.jsonl"),
            format!(
                "{}\n{}\n",
                json!({
                    "event": "ultra_partial_artifact_summary",
                    "completed_phase_ids": ["setup"],
                    "failed_phase_id": "implement",
                    "pending_phase_ids": ["verify"],
                    "recovery_ultra_plan_path": ".anvil/plans/recovery-ultra-plan-phase-a.yaml"
                }),
                json!({
                    "event": "tui_command_stop",
                    "ok": false,
                    "status": "interrupted",
                    "task_status": "interrupted",
                    "assurance_level": "full",
                    "runtime_acceptance_status": "pass",
                    "final_acceptance_status": "failed",
                    "release_gate_status": "failed",
                    "stop_reason": "interrupted by user while 日本語 path was active",
                    "recovery_ultra_plan_path": ".anvil/plans/recovery-ultra-plan-phase-a.yaml"
                })
            ),
        )
        .unwrap();

        let rendered = render_runs_table(dir.path());

        assert!(rendered.contains("018f1111"), "{rendered}");
        assert!(rendered.contains("interrupted/partial"), "{rendered}");
        assert!(rendered.contains("1/3"), "{rendered}");
        assert!(rendered.contains("yaml"), "{rendered}");
        assert!(rendered.contains("日本語"), "{rendered}");
        assert_eq!(
            newest_run_with_recovery(dir.path()).unwrap().short_id,
            "018f1111"
        );
    }

    #[test]
    fn empty_inventory_is_honest() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            render_runs_table(dir.path()),
            "No runs found for this workspace."
        );
    }

    #[test]
    fn resume_resolves_run_recovery_yaml_and_confirmation_card() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(dir.path().join("src/app/page.tsx"), "ok").unwrap();
        write_recovery_yaml(
            dir.path(),
            "recover.yaml",
            r#"
recovery_schema_version: "1"
recovery_original_goal: "日本語 goal"
recovery_failure_kind: "interrupted"
recovery_profile: "nextjs"
recovery_expected_completed_artifacts:
  - "src/app/page.tsx"
goal: "日本語 goal"
profile: "nextjs"
style: "recovery"
intent: "recover"
phases:
  - id: "repair"
    prompt: "repair remaining work"
"#,
        );
        write_run_events(
            dir.path(),
            "018f3333-run",
            ".anvil/plans/recover.yaml",
            json!({
                "event": "ultra_partial_artifact_summary",
                "completed_phase_ids": ["setup"],
                "failed_phase_id": "repair",
                "pending_phase_ids": ["verify"]
            }),
            json!({
                "event": "tui_command_stop",
                "ok": false,
                "status": "interrupted",
                "assurance_level": "partial",
                "failure_kind": "interrupted",
                "effective_profile": "nextjs",
                "requested_port": "3000",
                "recovery_ultra_plan_path": ".anvil/plans/recover.yaml"
            }),
        );

        let resume = prepare_resume(dir.path(), "018f3333").unwrap();
        let card = resume.confirmation_card();

        assert_eq!(resume.resumed_from, "018f3333");
        assert_eq!(resume.phases_to_run, vec!["repair"]);
        assert!(resume.workspace_gaps.is_empty());
        assert!(card.contains("original goal: 日本語 goal"), "{card}");
        assert!(card.contains("completed phases skipped: setup"), "{card}");
        assert!(card.contains("phases to run: repair"), "{card}");
        assert!(card.contains("recovering failure: interrupted"), "{card}");
        assert!(card.contains("effective profile: nextjs"), "{card}");
        assert!(card.contains("port: 3000"), "{card}");
    }

    #[test]
    fn resume_old_yaml_degrades_with_compatibility_note() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_recovery_yaml(
            dir.path(),
            "old.yaml",
            r#"
goal: "old goal"
profile: "generic"
phases:
  - id: "verify-recovery"
    prompt: "verify"
"#,
        );

        let resume = prepare_resume(dir.path(), path.to_str().unwrap()).unwrap();

        assert_eq!(resume.resumed_from, "yaml:.anvil/plans/old.yaml");
        assert!(
            resume
                .compatibility_notes
                .iter()
                .any(|note| note.contains("old recovery YAML"))
        );
        assert!(resume.confirmation_card().contains("old recovery YAML"));
    }

    #[test]
    fn corrupt_recovery_yaml_reports_runs_pointer() {
        let dir = tempfile::tempdir().unwrap();
        write_recovery_yaml(dir.path(), "bad.yaml", "profile: \"generic\"\n");
        write_run_events(
            dir.path(),
            "018f4444-run",
            ".anvil/plans/bad.yaml",
            json!({"event": "tui_command_stop", "ok": false, "status": "failed", "recovery_ultra_plan_path": ".anvil/plans/bad.yaml"}),
            json!({"event": "run_stop", "ok": false, "status": "failed", "recovery_ultra_plan_path": ".anvil/plans/bad.yaml"}),
        );

        let err = prepare_resume(dir.path(), "018f4444")
            .unwrap_err()
            .to_string();

        assert!(
            err.contains("failed to parse recovery UltraPlan YAML"),
            "{err}"
        );
        assert!(err.contains("/runs"), "{err}");
    }

    #[test]
    fn drifted_workspace_resume_reports_gaps() {
        let dir = tempfile::tempdir().unwrap();
        write_recovery_yaml(
            dir.path(),
            "drift.yaml",
            r#"
recovery_schema_version: "1"
recovery_original_goal: "goal"
recovery_failure_kind: "failed"
recovery_profile: "generic"
recovery_expected_completed_artifacts:
  - "dist/app.js"
goal: "goal"
profile: "generic"
phases:
  - id: "repair"
    prompt: "repair"
"#,
        );
        write_run_events(
            dir.path(),
            "018f5555-run",
            ".anvil/plans/drift.yaml",
            json!({"event": "ultra_partial_artifact_summary", "completed_phase_ids": ["setup"], "pending_phase_ids": ["repair"]}),
            json!({"event": "tui_command_stop", "ok": false, "status": "failed", "recovery_ultra_plan_path": ".anvil/plans/drift.yaml"}),
        );

        let resume = prepare_resume(dir.path(), "018f5555").unwrap();
        let err = resume.workspace_drift_error().unwrap();

        assert_eq!(resume.workspace_gaps, vec!["dist/app.js"]);
        assert!(
            err.contains("workspace drift: missing dist/app.js"),
            "{err}"
        );
        assert!(err.contains("re-run from phase repair"), "{err}");
    }

    fn write_recovery_yaml(root: &Path, file_name: &str, text: &str) -> PathBuf {
        let path = root.join(".anvil/plans").join(file_name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text.trim_start()).unwrap();
        path
    }

    fn write_run_events(
        root: &Path,
        run_id: &str,
        recovery_path: &str,
        first: serde_json::Value,
        second: serde_json::Value,
    ) {
        let run_dir = root.join(".anvil/runs").join(run_id);
        std::fs::create_dir_all(&run_dir).unwrap();
        let mut first = first;
        let mut second = second;
        first["recovery_ultra_plan_path"] = json!(recovery_path);
        second["recovery_ultra_plan_path"] = json!(recovery_path);
        std::fs::write(run_dir.join("events.jsonl"), format!("{first}\n{second}\n")).unwrap();
    }
}
